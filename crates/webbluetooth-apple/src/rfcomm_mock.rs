//! A mock IOBluetooth device and RFCOMM channel — the far end, without a radio.
//!
//! The pump is the part of this backend most likely to be wrong and least
//! likely to be tested: it needs a paired Classic device to talk to. So the two
//! IOBluetooth classes it messages are synthesised here, the way the delegate
//! is, and play the remote side the way IOBluetooth presents it:
//!
//! * `openRFCOMMChannelAsync:withChannelID:delegate:` returns at once and
//!   reports completion later, through the run loop — scheduled with
//!   `performSelector:withObject:afterDelay:` on the thread that called it.
//!   **That is not where IOBluetooth delivers.** Against a real peer every
//!   callback arrives on the main thread's run loop; a test process has none
//!   turning, so the mock uses the pump's instead. What this checks is how the
//!   pump handles the callbacks, not where they come from — the second was
//!   found, and is handled, by running against real hardware.
//! * Data arrives through `rfcommChannelData:data:length:` in whatever chunks
//!   the [`Server`] script says, and the peer can close with
//!   `rfcommChannelClosed:`.
//! * Writes are recorded, checked against the MTU, and optionally echoed.
//!
//! What this cannot stand in for is the radio, device lookup by name or
//! address, and macOS's permission checks. Everything from the device object
//! inward is the code that ships.

use super::*;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;

/// What the far end does, and what it saw.
pub struct Server {
    /// Returned by `openRFCOMMChannelAsync:…` itself. Non-zero fails at once.
    pub refuse_with: i32,
    /// Whether `rfcommChannelOpenComplete:status:` is ever called.
    pub answer_open: bool,
    /// Report the channel open through `isOpen` without ever calling back —
    /// what a process whose main run loop is not turning sees.
    pub open_silently: bool,
    /// The status it is called with.
    pub open_status: i32,
    pub mtu: u16,
    /// Delivered after the open, one `rfcommChannelData:` call per entry.
    pub to_send: Mutex<VecDeque<Vec<u8>>>,
    /// Send `rfcommChannelClosed:` once `to_send` is empty.
    pub close_when_done: bool,
    /// Deliver the open, all of `to_send` and the close in one run-loop turn,
    /// as a peer that answers once and hangs up can.
    pub all_at_once: bool,
    /// Send everything written straight back.
    pub echo: bool,
    /// Keep `isOpen` answering true after closing, so the only sign of the
    /// close is the delegate callback.
    pub stale_is_open: bool,

    /// Each `writeSync:length:`, in order.
    pub writes: Mutex<Vec<Vec<u8>>>,
    /// The largest write that exceeded the MTU, if any did.
    pub oversized_write: AtomicUsize,
    pub closed_by_client: AtomicBool,
    pub delegate_cleared: AtomicBool,
    /// The thread the open was requested on, and the one callbacks fired on.
    pub open_thread: Mutex<Option<String>>,
    pub callback_threads: Mutex<Vec<String>>,

    open: AtomicBool,
    delegate: AtomicUsize,
    device: AtomicUsize,
    channel: Mutex<Option<Retained>>,
    address: Mutex<Option<Retained>>,
}

impl Server {
    pub fn new() -> Self {
        Self {
            refuse_with: 0,
            answer_open: true,
            open_silently: false,
            open_status: 0,
            mtu: 127,
            to_send: Mutex::new(VecDeque::new()),
            close_when_done: false,
            all_at_once: false,
            echo: false,
            stale_is_open: false,
            writes: Mutex::new(Vec::new()),
            oversized_write: AtomicUsize::new(0),
            closed_by_client: AtomicBool::new(false),
            delegate_cleared: AtomicBool::new(false),
            open_thread: Mutex::new(None),
            callback_threads: Mutex::new(Vec::new()),
            open: AtomicBool::new(false),
            delegate: AtomicUsize::new(0),
            device: AtomicUsize::new(0),
            channel: Mutex::new(None),
            address: Mutex::new(None),
        }
    }

    pub fn sending(self, chunks: impl IntoIterator<Item = Vec<u8>>) -> Self {
        lock(&self.to_send).extend(chunks);
        self
    }
}

/// Mock objects to the script they follow, by object address.
/// Lock without caring whether a failed assertion elsewhere poisoned it. The
/// callbacks below run inside `extern "C"` functions on the pump thread, where
/// a panic aborts the whole test process instead of failing one test.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn servers() -> &'static Mutex<HashMap<usize, Arc<Server>>> {
    static SERVERS: OnceLock<Mutex<HashMap<usize, Arc<Server>>>> = OnceLock::new();
    SERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn server_for(object: Id) -> Option<Arc<Server>> {
    lock(servers()).get(&(object as usize)).cloned()
}

fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("<unnamed>")
        .to_owned()
}

/// `BOOL` is `bool` on arm64 and `signed char` on x86_64, and the encoding
/// has to say which.
#[cfg(target_arch = "aarch64")]
const BOOL_GETTER: &CStr = c"B@:";
#[cfg(not(target_arch = "aarch64"))]
const BOOL_GETTER: &CStr = c"c@:";

use core::ffi::CStr;

fn register(name: &str, methods: &[(&CStr, *const c_void, &CStr)]) -> Id {
    for attempt in 0..64u32 {
        let class_name = std::ffi::CString::new(format!("{name}{attempt}")).unwrap();
        let Some(mut builder) = (unsafe { ClassBuilder::new(c"NSObject", &class_name) }) else {
            continue;
        };
        for (selector, imp, types) in methods {
            builder = unsafe { builder.method(selector, *imp, types) };
        }
        return builder.register();
    }
    panic!("could not register {name}");
}

fn device_class() -> Id {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| {
        register(
            "WBMockIOBluetoothDevice",
            &[
                (
                    c"isConnected",
                    device_is_connected as *const c_void,
                    BOOL_GETTER,
                ),
                (
                    c"openConnection",
                    device_open_connection as *const c_void,
                    c"i@:",
                ),
                (
                    c"addressString",
                    device_address_string as *const c_void,
                    c"@@:",
                ),
                (
                    c"openRFCOMMChannelAsync:withChannelID:delegate:",
                    device_open_channel as *const c_void,
                    c"i@:^@C@",
                ),
                (
                    c"deliverOpen:",
                    device_deliver_open as *const c_void,
                    c"v@:@",
                ),
                (
                    c"deliverNext:",
                    device_deliver_next as *const c_void,
                    c"v@:@",
                ),
            ],
        ) as usize
    }) as Id
}

fn channel_class() -> Id {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| {
        register(
            "WBMockIOBluetoothRFCOMMChannel",
            &[
                (c"isOpen", channel_is_open as *const c_void, BOOL_GETTER),
                (c"getMTU", channel_get_mtu as *const c_void, c"S@:"),
                (
                    c"writeSync:length:",
                    channel_write as *const c_void,
                    c"i@:^vS",
                ),
                (c"closeChannel", channel_close as *const c_void, c"i@:"),
                (
                    c"setDelegate:",
                    channel_set_delegate as *const c_void,
                    c"v@:@",
                ),
            ],
        ) as usize
    }) as Id
}

/// Build a mock device that follows `server`.
pub fn device(server: Arc<Server>) -> Retained {
    unsafe {
        let obj: Id = crate::msg_send![device_class(), alloc];
        let obj: Id = crate::msg_send![obj, init];
        let device = Retained::adopt(obj).expect("mock device");
        server.device.store(device.key(), Ordering::Release);
        *lock(&server.address) = Retained::adopt(objc::nsstring("aa-bb-cc-0d-ee-ff"));
        lock(servers()).insert(device.key(), server);
        device
    }
}

/// Schedule `selector` on `target` for the current thread's run loop.
unsafe fn later(target: Id, selector: &CStr, argument: Id, seconds: f64) {
    unsafe {
        crate::msg_send_void![
            target,
            performSelector: objc::sel(selector),
            withObject: argument,
            afterDelay: seconds
        ];
    }
}

// ── The device ──────────────────────────────────────────────────────────────

extern "C" fn device_is_connected(_this: Id, _cmd: *const c_void) -> bool {
    true
}

extern "C" fn device_open_connection(_this: Id, _cmd: *const c_void) -> i32 {
    0
}

extern "C" fn device_address_string(this: Id, _cmd: *const c_void) -> Id {
    server_for(this)
        .and_then(|s| lock(&s.address).as_ref().map(Retained::as_ptr))
        .unwrap_or(NIL)
}

extern "C" fn device_open_channel(
    this: Id,
    _cmd: *const c_void,
    out: *mut Id,
    _channel_id: u8,
    delegate: Id,
) -> i32 {
    let Some(server) = server_for(this) else {
        return -1;
    };
    *lock(&server.open_thread) = Some(thread_name());
    if server.refuse_with != 0 {
        return server.refuse_with;
    }
    server.delegate.store(delegate as usize, Ordering::Release);
    let channel = unsafe {
        let obj: Id = crate::msg_send![channel_class(), alloc];
        let obj: Id = crate::msg_send![obj, init];
        Retained::adopt(obj).expect("mock channel")
    };
    lock(servers()).insert(channel.key(), server.clone());
    if server.open_silently {
        server.open.store(true, Ordering::Release);
    }
    unsafe {
        *out = channel.as_ptr();
        if server.answer_open && !server.open_silently {
            later(this, c"deliverOpen:", channel.as_ptr(), 0.05);
        }
    }
    *lock(&server.channel) = Some(channel);
    0
}

fn delegate_of(server: &Server) -> Id {
    server.delegate.load(Ordering::Acquire) as Id
}

extern "C" fn device_deliver_open(this: Id, _cmd: *const c_void, channel: Id) {
    let Some(server) = server_for(this) else {
        return;
    };
    lock(&server.callback_threads).push(thread_name());
    let delegate = delegate_of(&server);
    if server.open_status == 0 {
        server.open.store(true, Ordering::Release);
    }
    unsafe {
        crate::msg_send_void![
            delegate,
            rfcommChannelOpenComplete: channel,
            status: server.open_status
        ];
        if server.open_status == 0 && server.all_at_once {
            let chunks: Vec<_> = lock(&server.to_send).drain(..).collect();
            for chunk in chunks {
                crate::msg_send_void![
                    delegate,
                    rfcommChannelData: channel,
                    data: chunk.as_ptr() as *mut c_void,
                    length: chunk.len()
                ];
            }
            if server.close_when_done {
                server.open.store(false, Ordering::Release);
                crate::msg_send_void![delegate, rfcommChannelClosed: channel];
            }
        } else if server.open_status == 0 {
            later(this, c"deliverNext:", channel, 0.01);
        }
    }
}

extern "C" fn device_deliver_next(this: Id, _cmd: *const c_void, channel: Id) {
    let Some(server) = server_for(this) else {
        return;
    };
    if !server.open.load(Ordering::Acquire) || server.delegate_cleared.load(Ordering::Acquire) {
        return;
    }
    lock(&server.callback_threads).push(thread_name());
    let delegate = delegate_of(&server);
    let next = lock(&server.to_send).pop_front();
    match next {
        Some(chunk) => unsafe {
            crate::msg_send_void![
                delegate,
                rfcommChannelData: channel,
                data: chunk.as_ptr() as *mut c_void,
                length: chunk.len()
            ];
            later(this, c"deliverNext:", channel, 0.005);
        },
        None if server.close_when_done => {
            if !server.stale_is_open {
                server.open.store(false, Ordering::Release);
            }
            unsafe { crate::msg_send_void![delegate, rfcommChannelClosed: channel] };
        }
        None => {}
    }
}

// ── The channel ─────────────────────────────────────────────────────────────

extern "C" fn channel_is_open(this: Id, _cmd: *const c_void) -> bool {
    server_for(this).is_some_and(|s| s.open.load(Ordering::Acquire))
}

extern "C" fn channel_get_mtu(this: Id, _cmd: *const c_void) -> u16 {
    server_for(this).map_or(0, |s| s.mtu)
}

extern "C" fn channel_write(this: Id, _cmd: *const c_void, data: *mut c_void, length: u16) -> i32 {
    let Some(server) = server_for(this) else {
        return -1;
    };
    let bytes = unsafe { core::slice::from_raw_parts(data as *const u8, usize::from(length)) };
    if length > server.mtu {
        server
            .oversized_write
            .fetch_max(usize::from(length), Ordering::AcqRel);
    }
    lock(&server.writes).push(bytes.to_vec());
    if server.echo {
        lock(&server.to_send).push_back(bytes.to_vec());
        let device = server.device.load(Ordering::Acquire) as Id;
        unsafe { later(device, c"deliverNext:", this, 0.0) };
    }
    0
}

extern "C" fn channel_close(this: Id, _cmd: *const c_void) -> i32 {
    if let Some(server) = server_for(this) {
        server.closed_by_client.store(true, Ordering::Release);
        server.open.store(false, Ordering::Release);
    }
    0
}

extern "C" fn channel_set_delegate(this: Id, _cmd: *const c_void, delegate: Id) {
    if let Some(server) = server_for(this) {
        if delegate.is_null() {
            server.delegate_cleared.store(true, Ordering::Release);
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;
    use futures_util::StreamExt;

    const CHANNEL: u8 = 25;

    fn open_mock(server: &Arc<Server>) -> webbluetooth_core::Result<RfcommChannel> {
        block_on(open_source(Source::Device(device(server.clone())), CHANNEL))
    }

    /// Read until `want` bytes have arrived, or fail after a few seconds.
    fn read_exactly(incoming: &mut Incoming, want: usize) -> Vec<u8> {
        block_on(async {
            let mut got = Vec::new();
            while got.len() < want {
                let next = webbluetooth_core::timeout(Duration::from_secs(5), incoming.next())
                    .await
                    .expect("no data within 5 s")
                    .expect("stream ended early");
                got.extend(next);
            }
            got
        })
    }

    fn wait_for(what: &str, check: impl Fn() -> bool) {
        wait_within(Duration::from_secs(3), what, check);
    }

    fn wait_within(limit: Duration, what: &str, check: impl Fn() -> bool) {
        let deadline = Instant::now() + limit;
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 63-byte packets in the shape a streaming device sends them — a sync
    /// byte and a counter — split across reads at awkward places, as a radio
    /// would split them.
    fn packets(count: usize) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut stream = Vec::new();
        for counter in 0..count {
            let mut packet = vec![0u8; 63];
            packet[0] = 0xAA;
            packet[1] = 239;
            packet[3] = counter as u8;
            for (i, byte) in packet.iter_mut().enumerate().skip(4) {
                *byte = (i * 7 + counter) as u8;
            }
            stream.extend(packet);
        }
        let mut chunks = Vec::new();
        let mut rest = stream.as_slice();
        for size in [40usize, 23, 1, 127, 63, 200].iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let take = (*size).min(rest.len());
            chunks.push(rest[..take].to_vec());
            rest = &rest[take..];
        }
        (stream, chunks)
    }

    #[test]
    fn data_arrives_intact_and_in_order() {
        let (stream, chunks) = packets(50);
        let server = Arc::new(Server::new().sending(chunks));
        let channel = open_mock(&server).expect("open");

        assert_eq!(channel.peer_id(), "AA:BB:CC:0D:EE:FF");
        let mut incoming = channel.take_incoming().expect("incoming");
        assert_eq!(read_exactly(&mut incoming, stream.len()), stream);

        // The open is asked for on the pump thread, so that its blocking
        // writes never stall the caller.
        let pump = format!("webbluetooth-rfcomm-{CHANNEL}");
        assert_eq!(lock(&server.open_thread).as_deref(), Some(pump.as_str()));
    }

    #[test]
    fn writes_are_split_to_the_mtu_and_kept_in_order() {
        let server = Arc::new(Server {
            echo: true,
            mtu: 50,
            ..Server::new()
        });
        let channel = open_mock(&server).expect("open");
        let mut incoming = channel.take_incoming().expect("incoming");

        let first: Vec<u8> = (0..=255u8).collect();
        let second = b"and a short one".to_vec();
        channel.send(&first).expect("send");
        channel.send(&second).expect("send");
        // Bounded: a pump that has stopped turning its run loop never writes,
        // and an unbounded wait would hang the suite rather than fail it.
        block_on(webbluetooth_core::timeout(
            Duration::from_secs(5),
            channel.flushed(),
        ))
        .expect("nothing was written within 5 s")
        .expect("flushed");

        let writes = lock(&server.writes).clone();
        assert_eq!(writes.concat(), [first.clone(), second.clone()].concat());
        assert!(
            writes.iter().all(|w| w.len() <= 50),
            "a write exceeded the MTU"
        );
        assert_eq!(server.oversized_write.load(Ordering::Acquire), 0);

        // And the far end's echo comes back through the delegate.
        let echoed = read_exactly(&mut incoming, first.len() + second.len());
        assert_eq!(echoed, [first, second].concat());
    }

    #[test]
    fn closing_locally_closes_the_channel_and_detaches_the_delegate() {
        let server = Arc::new(Server::new());
        let channel = open_mock(&server).expect("open");
        channel.close();

        wait_for("closeChannel", || {
            server.closed_by_client.load(Ordering::Acquire)
        });
        wait_for("setDelegate: nil", || {
            server.delegate_cleared.load(Ordering::Acquire)
        });
        assert_eq!(channel.closed(), Some(Closed::Locally));
        assert!(channel.send(b"too late").is_err());
    }

    #[test]
    fn dropping_the_channel_closes_it() {
        let server = Arc::new(Server::new());
        drop(open_mock(&server).expect("open"));
        wait_for("closeChannel", || {
            server.closed_by_client.load(Ordering::Acquire)
        });
    }

    #[test]
    fn the_peer_closing_ends_the_stream_after_its_data() {
        let server = Arc::new(Server {
            close_when_done: true,
            ..Server::new().sending([b"last words".to_vec()])
        });
        let channel = open_mock(&server).expect("open");
        let mut incoming = channel.take_incoming().expect("incoming");

        let (got, reason) = block_on(webbluetooth_core::timeout(Duration::from_secs(5), async {
            let mut got = Vec::new();
            while let Some(chunk) = incoming.next().await {
                got.extend(chunk);
            }
            (got, channel.on_close().await)
        }))
        .expect("the stream did not end within 5 s of the peer closing");
        assert_eq!(got, b"last words");
        assert_eq!(reason, Closed::ByPeer);
    }

    /// Opened, sent and closed in one turn: the open still succeeds and the
    /// data is still delivered, rather than the close cancelling the open.
    #[test]
    fn a_peer_that_answers_and_hangs_up_at_once_still_delivers() {
        let server = Arc::new(Server {
            close_when_done: true,
            all_at_once: true,
            ..Server::new().sending([b"one".to_vec(), b" shot".to_vec()])
        });
        let channel = open_mock(&server).expect("open");
        let mut incoming = channel.take_incoming().expect("incoming");

        let (got, reason) = block_on(webbluetooth_core::timeout(Duration::from_secs(5), async {
            let mut got = Vec::new();
            while let Some(chunk) = incoming.next().await {
                got.extend(chunk);
            }
            (got, channel.on_close().await)
        }))
        .expect("the stream did not end within 5 s of the peer closing");
        assert_eq!(got, b"one shot");
        assert_eq!(reason, Closed::ByPeer);
    }

    /// The delegate's close callback on its own has to end the stream; the
    /// `isOpen` poll is a second line, not the only one.
    #[test]
    fn the_peer_closing_is_heard_from_the_delegate_alone() {
        let server = Arc::new(Server {
            close_when_done: true,
            stale_is_open: true,
            ..Server::new().sending([b"bye".to_vec()])
        });
        let channel = open_mock(&server).expect("open");
        let reason = block_on(webbluetooth_core::timeout(
            Duration::from_secs(5),
            channel.on_close(),
        ))
        .expect("the close was not reported within 5 s");
        assert_eq!(reason, Closed::ByPeer);
    }

    #[test]
    fn an_open_refused_outright_is_reported_with_its_cause() {
        let server = Arc::new(Server {
            refuse_with: IO_RETURN_NOT_PERMITTED as i32,
            ..Server::new()
        });
        let Err(webbluetooth_core::Error::Network(message)) = open_mock(&server) else {
            panic!("expected a NetworkError");
        };
        assert!(message.contains("kIOReturnNotPermitted"), "{message}");
        assert!(message.contains("entitlement"), "{message}");
    }

    #[test]
    fn an_open_that_completes_with_an_error_is_reported() {
        let server = Arc::new(Server {
            open_status: 0xe000_02c7_u32 as i32, // kIOReturnAborted
            ..Server::new()
        });
        let Err(webbluetooth_core::Error::Network(message)) = open_mock(&server) else {
            panic!("expected a NetworkError");
        };
        assert!(message.contains("did not open"), "{message}");
        assert!(message.contains("kIOReturnAborted"), "{message}");
    }

    /// The channel says it is open but no callback ever comes — a process
    /// whose main run loop is not turning. Succeeding here would hand back a
    /// channel that can write but never receives; the open has to fail and
    /// say why.
    #[test]
    fn an_open_without_callbacks_names_the_main_run_loop() {
        let server = Arc::new(Server {
            open_silently: true,
            ..Server::new()
        });
        let Err(webbluetooth_core::Error::Network(message)) = open_mock(&server) else {
            panic!("expected a NetworkError");
        };
        assert!(message.contains("main thread's run loop"), "{message}");
        wait_for("closeChannel", || {
            server.closed_by_client.load(Ordering::Acquire)
        });
    }

    #[test]
    fn an_open_that_never_completes_times_out_and_is_closed() {
        let server = Arc::new(Server {
            answer_open: false,
            ..Server::new()
        });
        let started = Instant::now();
        let Err(webbluetooth_core::Error::Network(message)) = open_mock(&server) else {
            panic!("expected a NetworkError");
        };
        assert!(message.contains("did not open within"), "{message}");
        assert!(started.elapsed() >= OPEN_TIMEOUT);
        assert!(started.elapsed() < OPEN_TIMEOUT * 2);
        wait_for("closeChannel", || {
            server.closed_by_client.load(Ordering::Acquire)
        });
    }

    /// The caller giving up mid-open stops the pump and closes the channel,
    /// rather than leaving a thread and a half-open channel behind.
    #[test]
    fn abandoning_the_open_stops_the_pump() {
        let server = Arc::new(Server {
            answer_open: false,
            ..Server::new()
        });
        let opening = open_source(Source::Device(device(server.clone())), CHANNEL);
        let result = block_on(webbluetooth_core::timeout(
            Duration::from_millis(300),
            opening,
        ));
        assert!(result.is_err(), "the open should still have been pending");
        // Well inside the open timeout: the pump has to notice the caller left,
        // not merely give up on its own a little later.
        wait_within(OPEN_TIMEOUT / 4, "closeChannel", || {
            server.closed_by_client.load(Ordering::Acquire)
        });
    }
}
