//! A D-Bus connection: authenticate, dispatch, serve.
//!
//! # Authentication
//!
//! Before any message, the client sends a single NUL byte — a relic of the
//! credential-passing handshake — then a line-based SASL exchange:
//!
//! ```text
//!   AUTH EXTERNAL <uid, in ASCII, hex-encoded>
//!   OK <server guid>
//!   BEGIN
//! ```
//!
//! `EXTERNAL` means "the kernel already told you who I am", which is true of a
//! Unix socket. `ANONYMOUS` is tried as a fallback for buses configured to
//! allow it, which is how some test harnesses are set up.
//!
//! # Dispatch
//!
//! One reader thread owns the read half and routes by message type: replies to
//! the caller waiting on that serial, signals to match handlers, and inbound
//! method calls to registered objects — the last being what the peripheral role
//! needs, since BlueZ calls *back* into the application to read a
//! characteristic.
//!
//! Calls are callback-based rather than blocking, so the async layer above can
//! wrap them without a thread per request. [`Connection::call_blocking`] is
//! provided for setup paths and tests.

use super::codec::CodecError;
use super::message::{Message, MessageType};
use super::value::Value;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

unsafe extern "C" {
    /// From libc, which is linked into every Unix process anyway — no crate.
    fn getuid() -> u32;
}

/// Something went wrong on the bus.
#[derive(Debug, Clone)]
pub enum Error {
    /// The socket could not be reached, or died.
    Io(String),
    /// SASL did not complete.
    Auth(String),
    /// A malformed message.
    Codec(CodecError),
    /// The peer replied with an error.
    Call { name: String, message: String },
    /// No reply within the deadline.
    Timeout,
    /// The connection is shutting down.
    Disconnected,
    /// `DBUS_SYSTEM_BUS_ADDRESS` was unparseable, or names no socket.
    BadAddress(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "D-Bus I/O error: {m}"),
            Self::Auth(m) => write!(f, "D-Bus authentication failed: {m}"),
            Self::Codec(e) => write!(f, "D-Bus protocol error: {e}"),
            Self::Call { name, message } => write!(f, "{name}: {message}"),
            Self::Timeout => f.write_str("D-Bus call timed out"),
            Self::Disconnected => f.write_str("D-Bus connection closed"),
            Self::BadAddress(a) => write!(f, "unusable D-Bus address: {a}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
impl From<CodecError> for Error {
    fn from(e: CodecError) -> Self {
        Self::Codec(e)
    }
}

/// A D-Bus result: this module's [`Error`], not the crate's.
pub type Result<T> = std::result::Result<T, Error>;

/// Handles inbound method calls on an exported object.
///
/// Returning `Err((name, text))` sends a D-Bus error reply.
pub trait ObjectHandler: Send + Sync {
    fn handle(&self, call: &Message) -> std::result::Result<Vec<Value>, (String, String)>;
}

impl<F> ObjectHandler for F
where
    F: Fn(&Message) -> std::result::Result<Vec<Value>, (String, String)> + Send + Sync,
{
    fn handle(&self, call: &Message) -> std::result::Result<Vec<Value>, (String, String)> {
        self(call)
    }
}

/// Called for every signal that arrives.
pub type SignalHandler = Arc<dyn Fn(&Message) + Send + Sync>;

type ReplyHandler = Box<dyn FnOnce(Result<Message>) + Send>;

struct Shared {
    write: Mutex<UnixStream>,
    serial: AtomicU32,
    pending: Mutex<HashMap<u32, ReplyHandler>>,
    signals: Mutex<Vec<SignalHandler>>,
    /// Exported objects, matched by exact path then by `path/` prefix.
    objects: Mutex<Vec<(String, Arc<dyn ObjectHandler>)>>,
    unique_name: Mutex<String>,
    stopped: AtomicBool,
    /// Descriptors that arrived with a reply, by the serial it answers.
    ///
    /// Claimed by [`Connection::take_fds`] after the call returns. Anything
    /// nobody claims is closed when the connection is dropped — an unclaimed
    /// descriptor is an open file the process cannot name.
    received_fds: Mutex<HashMap<u32, Vec<std::os::fd::RawFd>>>,
}

impl Shared {
    fn next_serial(&self) -> u32 {
        // Serial 0 is reserved.
        let mut s = self.serial.fetch_add(1, Ordering::Relaxed);
        if s == 0 {
            s = self.serial.fetch_add(1, Ordering::Relaxed);
        }
        s
    }

    fn send(&self, message: &Message, serial: u32) -> Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(Error::Disconnected);
        }
        let bytes = message.serialize(serial);
        let mut w = self.write.lock().map_err(|_| Error::Disconnected)?;
        w.write_all(&bytes)?;
        w.flush()?;
        Ok(())
    }

    /// Fail every outstanding call — called when the reader thread exits, so a
    /// dropped bus never leaves a caller waiting forever.
    fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, handler) in pending {
            handler(Err(Error::Disconnected));
        }
    }
}

/// A connection to a message bus.
pub struct Connection {
    shared: Arc<Shared>,
}

impl Connection {
    /// Connect to the system bus, where BlueZ lives.
    ///
    /// `WEBBLUETOOTH_DBUS_ADDRESS` overrides the address, which is how the test
    /// harness points at a private bus carrying a mocked `org.bluez` instead of
    /// the real one.
    pub fn system() -> Result<Self> {
        if let Ok(address) = std::env::var("WEBBLUETOOTH_DBUS_ADDRESS") {
            return Self::connect(&address);
        }
        let address = std::env::var("DBUS_SYSTEM_BUS_ADDRESS")
            .unwrap_or_else(|_| "unix:path=/var/run/dbus/system_bus_socket".into());
        Self::connect(&address)
    }

    /// Connect to the session bus.
    pub fn session() -> Result<Self> {
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .map_err(|_| Error::BadAddress("DBUS_SESSION_BUS_ADDRESS is unset".into()))?;
        Self::connect(&address)
    }

    /// Connect to a bus at a D-Bus address.
    ///
    /// Only `unix:path=` and `unix:abstract=` transports are supported; TCP is
    /// not, because BlueZ is never on one.
    pub fn connect(address: &str) -> Result<Self> {
        let socket = Self::open(address)?;
        Self::authenticate(&socket)?;

        let write = socket.try_clone()?;
        let shared = Arc::new(Shared {
            write: Mutex::new(write),
            serial: AtomicU32::new(1),
            pending: Mutex::new(HashMap::new()),
            signals: Mutex::new(Vec::new()),
            objects: Mutex::new(Vec::new()),
            unique_name: Mutex::new(String::new()),
            stopped: AtomicBool::new(false),
            received_fds: Mutex::new(HashMap::new()),
        });

        spawn_reader(socket, shared.clone());

        let connection = Self { shared };
        // `Hello` is mandatory: the bus assigns a unique name and will reject
        // everything else until it has been called.
        let reply = connection.call_blocking(
            Message::call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "Hello",
            ),
            Duration::from_secs(10),
        )?;
        if let Some(name) = reply.arg(0).and_then(Value::as_str) {
            *connection.shared.unique_name.lock().unwrap() = name.to_owned();
        }
        Ok(connection)
    }

    fn open(address: &str) -> Result<UnixStream> {
        // An address is a semicolon-separated list of transports to try.
        for candidate in address.split(';') {
            let Some(rest) = candidate.trim().strip_prefix("unix:") else {
                continue;
            };
            for part in rest.split(',') {
                if let Some(path) = part.strip_prefix("path=") {
                    if let Ok(s) = UnixStream::connect(path) {
                        return Ok(s);
                    }
                }
                if let Some(name) = part.strip_prefix("abstract=") {
                    // Linux abstract sockets are a NUL followed by the name.
                    #[cfg(target_os = "linux")]
                    {
                        use std::os::linux::net::SocketAddrExt;
                        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
                        if let Ok(s) = UnixStream::connect_addr(&addr) {
                            return Ok(s);
                        }
                    }
                    #[cfg(not(target_os = "linux"))]
                    let _ = name;
                }
            }
        }
        Err(Error::BadAddress(address.to_owned()))
    }

    /// The SASL handshake.
    fn authenticate(socket: &UnixStream) -> Result<()> {
        let mut w = socket.try_clone()?;
        let mut r = std::io::BufReader::new(socket.try_clone()?);

        // The leading NUL must be its own write before any SASL line.
        w.write_all(&[0])?;
        w.flush()?;

        let uid = unsafe { getuid() };
        let hex_uid: String = uid
            .to_string()
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();

        for attempt in [
            format!("AUTH EXTERNAL {hex_uid}\r\n"),
            "AUTH ANONYMOUS\r\n".to_string(),
        ] {
            w.write_all(attempt.as_bytes())?;
            w.flush()?;
            let line = read_line(&mut r)?;
            if line.starts_with("OK") {
                w.write_all(b"BEGIN\r\n")?;
                w.flush()?;
                return Ok(());
            }
            if !line.starts_with("REJECTED") {
                return Err(Error::Auth(line));
            }
        }
        Err(Error::Auth(
            "no supported authentication mechanism was accepted".into(),
        ))
    }

    /// This connection's unique bus name, e.g. `:1.42`.
    pub fn unique_name(&self) -> String {
        self.shared.unique_name.lock().unwrap().clone()
    }

    /// Send a method call; `on_reply` runs on the reader thread when the answer
    /// arrives. This is the primitive the async layer wraps.
    pub fn call_async(&self, message: Message, on_reply: ReplyHandler) {
        let serial = self.shared.next_serial();
        self.shared.pending.lock().unwrap().insert(serial, on_reply);
        if let Err(e) = self.shared.send(&message, serial) {
            if let Some(handler) = self.shared.pending.lock().unwrap().remove(&serial) {
                handler(Err(e));
            }
        }
    }

    /// Send a method call and wait for the reply, claiming any descriptors.
    ///
    /// For methods like `AcquireWrite` whose answer is a file descriptor: the
    /// body carries only an index, and the descriptor itself arrived beside
    /// the message. The caller owns what comes back and must close it.
    pub fn call_blocking_with_fds(
        &self,
        message: Message,
        timeout: Duration,
    ) -> Result<(Message, Vec<std::os::fd::RawFd>)> {
        let reply = self.call_blocking(message, timeout)?;
        let fds = reply
            .reply_serial
            .map(|s| self.take_fds(s))
            .unwrap_or_default();
        Ok((reply, fds))
    }

    /// Send a method call and wait for the reply.
    pub fn call_blocking(&self, message: Message, timeout: Duration) -> Result<Message> {
        let slot: Arc<(Mutex<Option<Result<Message>>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        let writer = slot.clone();
        self.call_async(
            message,
            Box::new(move |reply| {
                *writer.0.lock().unwrap() = Some(reply);
                writer.1.notify_all();
            }),
        );

        let (lock, cv) = &*slot;
        let mut guard = lock.lock().map_err(|_| Error::Disconnected)?;
        let deadline = std::time::Instant::now() + timeout;
        while guard.is_none() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout);
            }
            let (g, _) = cv
                .wait_timeout(guard, remaining)
                .map_err(|_| Error::Disconnected)?;
            guard = g;
        }
        guard.take().unwrap_or(Err(Error::Disconnected))
    }

    /// Send a signal or a reply, expecting nothing back.
    pub fn send(&self, message: Message) -> Result<()> {
        let serial = self.shared.next_serial();
        self.shared.send(&message, serial)
    }

    /// Subscribe to signals. The rule is a D-Bus match expression, e.g.
    /// `type='signal',interface='org.freedesktop.DBus.Properties'`.
    ///
    /// The handler sees *every* signal, because the bus may deliver more than
    /// the rule asks for; filter inside it.
    pub fn add_match(&self, rule: &str, handler: SignalHandler) -> Result<()> {
        self.shared.signals.lock().unwrap().push(handler);
        self.call_blocking(
            Message::call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "AddMatch",
            )
            .with_body(vec![Value::Str(rule.into())]),
            Duration::from_secs(10),
        )?;
        Ok(())
    }

    /// Export an object. `path` matches itself and everything beneath it, which
    /// is how one handler can serve a whole GATT application tree.
    pub fn export(&self, path: &str, handler: Arc<dyn ObjectHandler>) {
        self.shared
            .objects
            .lock()
            .unwrap()
            .push((path.to_owned(), handler));
    }

    /// Claim a well-known bus name.
    pub fn request_name(&self, name: &str) -> Result<()> {
        self.call_blocking(
            Message::call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "RequestName",
            )
            .with_body(vec![Value::Str(name.into()), Value::Uint32(0)]),
            Duration::from_secs(10),
        )?;
        Ok(())
    }

    /// Whether the reader thread is still running.
    pub fn is_connected(&self) -> bool {
        !self.shared.stopped.load(Ordering::Acquire)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shared.shutdown();
        // Closing the socket wakes the reader out of its blocking read.
        if let Ok(w) = self.shared.write.lock() {
            let _ = w.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn read_line(r: &mut impl Read) -> Result<String> {
    // SASL is line-based and pre-dates the binary protocol, so read one byte at
    // a time rather than buffering past the BEGIN.
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = r.read(&mut byte)?;
        if n == 0 {
            return Err(Error::Auth(
                "connection closed during authentication".into(),
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            line.push(byte[0]);
        }
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

fn spawn_reader(socket: UnixStream, shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("webbluetooth-dbus".into())
        .spawn(move || {
            let reader = socket;
            // Read with `recvmsg` from the first byte, not only once something
            // expects a descriptor: a plain `read` does not ignore ancillary
            // data, it makes the kernel discard it and close the descriptors
            // it carried. By the time anything noticed, they would be gone.
            let fd = std::os::fd::AsRawFd::as_raw_fd(&reader);
            let mut pending = super::fds::Pending::default();
            let mut buf: Vec<u8> = Vec::with_capacity(8192);
            let mut chunk = [0u8; 4096];

            loop {
                // Drain every complete message already buffered before reading.
                loop {
                    let Some(len) = Message::framed_length(&buf) else {
                        break;
                    };
                    if buf.len() < len {
                        break;
                    }
                    let frame: Vec<u8> = buf.drain(..len).collect();
                    match Message::parse(&frame) {
                        Ok(mut message) => {
                            // Each message claims exactly what its header
                            // declared, so one that carries none cannot take
                            // a descriptor meant for the message behind it.
                            let claimed = pending.take(message.unix_fds as usize);
                            attach(&shared, &mut message, claimed);
                            dispatch(&shared, message)
                        }
                        // A message we cannot parse is skipped; the stream stays
                        // framed because the length came from the fixed header.
                        Err(_) => continue,
                    }
                }

                match super::fds::recv_with_fds(fd, &mut chunk) {
                    Ok(None) => continue, // Interrupted; try again.
                    Ok(Some(received)) if received.bytes == 0 && received.fds.is_empty() => break,
                    Ok(Some(received)) => {
                        pending.push(received.fds);
                        buf.extend_from_slice(&chunk[..received.bytes]);
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            shared.shutdown();
        })
        .expect("could not start the D-Bus reader thread");
}

impl Connection {
    /// Take the descriptors that came back with the reply to `serial`.
    ///
    /// Empty unless the method actually returned one. The caller owns them and
    /// must close them; nothing else will.
    pub fn take_fds(&self, serial: u32) -> Vec<std::os::fd::RawFd> {
        self.shared
            .received_fds
            .lock()
            .unwrap()
            .remove(&serial)
            .unwrap_or_default()
    }
}

/// Record the descriptors that arrived with a reply.
///
/// They are keyed by the serial of the call they answer, because that is what
/// the waiting side knows; a `h` in the body is an index into this list.
fn attach(shared: &Arc<Shared>, message: &mut Message, fds: Vec<std::os::fd::RawFd>) {
    if fds.is_empty() {
        return;
    }
    if let Some(serial) = message.reply_serial {
        shared.received_fds.lock().unwrap().insert(serial, fds);
    } else {
        // Nothing to hand them to. Closing beats leaking an open file that
        // nothing in the process can name.
        for fd in fds {
            unsafe { super::fds::close_raw(fd) };
        }
    }
}

/// Run the handler for one inbound method call and send its reply.
fn answer(shared: &Arc<Shared>, message: Message) {
    let path = message.path.clone().unwrap_or_default();
    let handler = {
        let objects = shared.objects.lock().unwrap();
        objects
            .iter()
            .find(|(p, _)| *p == path || path.starts_with(&format!("{p}/")))
            .map(|(_, h)| h.clone())
    };

    let reply = match handler {
        Some(h) => match h.handle(&message) {
            Ok(body) => Message::reply_to(&message).with_body(body),
            Err((name, text)) => Message::error_to(&message, &name, &text),
        },
        None => Message::error_to(
            &message,
            "org.freedesktop.DBus.Error.UnknownObject",
            &format!("no object at {path}"),
        ),
    };
    if message.flags & super::message::FLAG_NO_REPLY_EXPECTED == 0 {
        let serial = shared.next_serial();
        let _ = shared.send(&reply, serial);
    }
}

fn dispatch(shared: &Arc<Shared>, message: Message) {
    match message.message_type {
        MessageType::MethodReturn | MessageType::Error => {
            let Some(serial) = message.reply_serial else {
                return;
            };
            let handler = shared.pending.lock().unwrap().remove(&serial);
            if let Some(handler) = handler {
                if message.message_type == MessageType::Error {
                    handler(Err(Error::Call {
                        name: message.error_name.clone().unwrap_or_default(),
                        message: message.error_text().unwrap_or_default(),
                    }));
                } else {
                    handler(Ok(message));
                }
            }
        }

        MessageType::Signal => {
            let handlers = shared.signals.lock().unwrap().clone();
            for h in handlers {
                h(&message);
            }
        }

        // Answered on its own thread, never on the reader's.
        //
        // A handler may take arbitrarily long — a GATT read is answered by
        // application code, not from a table — and this is the only thread
        // reading the socket. Running handlers here would stall every inbound
        // message behind them, including the replies to our own outstanding
        // calls, which deadlocks the obvious case: `RegisterApplication` does
        // not return until BlueZ has walked our object tree, so we have to be
        // able to answer `GetManagedObjects` while blocked waiting for it.
        //
        // One thread per call is more than a busy bus would want, but method
        // calls arrive at the rate a GATT client generates them, and the peer
        // here is `bluetoothd`.
        MessageType::MethodCall => {
            let shared = shared.clone();
            let spawned = std::thread::Builder::new()
                .name("webbluetooth-dbus-call".into())
                .spawn(move || answer(&shared, message));
            // If a thread cannot be spawned the call simply goes unanswered,
            // which the caller sees as a timeout — the same as a handler that
            // never returns, and better than killing the connection.
            drop(spawned);
        }
    }
}
