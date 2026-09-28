//! RFCOMM over Linux's Bluetooth socket API.

use crate::sys::{
    accept, bind, close, connect, errno, format_address, listen as sys_listen, parse_address, read,
    set_classic_security, shutdown, socket, write, SockAddrRc, AF_BLUETOOTH, BDADDR_ANY,
    BTPROTO_RFCOMM, EINTR, SHUT_RDWR, SOCK_STREAM,
};
use futures_channel::mpsc;
use futures_core::Stream;
use std::collections::VecDeque;
use std::ffi::c_int;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use webbluetooth_core::l2cap::{ChannelSink, Closed, Incoming};

struct Socket(c_int);

impl Drop for Socket {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}

/// An open RFCOMM byte stream.
pub struct Channel {
    channel: u8,
    peer: String,
    fd: Arc<Socket>,
    writing: Mutex<VecDeque<u8>>,
    closed: Mutex<Option<Closed>>,
    stopped: Arc<AtomicBool>,
}

impl Channel {
    /// Adopt a connected file descriptor delivered by BlueZ `Profile1`.
    pub fn adopt_fd(fd: c_int, channel: u8, peer: String, sink: Arc<dyn ChannelSink>) -> Self {
        Self::adopt(fd, channel, peer, sink)
    }
    /// Connect to a Bluetooth Classic RFCOMM server channel.
    pub fn connect(address: &str, channel: u8, sink: Arc<dyn ChannelSink>) -> Result<Self, String> {
        Self::connect_with_security(
            address,
            channel,
            webbluetooth_core::classic::ClassicSecurity::None,
            sink,
        )
    }

    /// Connect with an explicit Classic link security requirement.
    pub fn connect_with_security(
        address: &str,
        channel: u8,
        security: webbluetooth_core::classic::ClassicSecurity,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        let bdaddr = parse_address(address)
            .ok_or_else(|| format!("{address:?} is not a Bluetooth address"))?;
        let fd = unsafe { socket(AF_BLUETOOTH, SOCK_STREAM, BTPROTO_RFCOMM) };
        if fd < 0 {
            return Err(format!(
                "could not create an RFCOMM socket (errno {})",
                errno()
            ));
        }
        let socket = Arc::new(Socket(fd));
        set_classic_security(fd, security)
            .map_err(|error| format!("could not configure RFCOMM security (errno {error})"))?;
        let addr = SockAddrRc {
            family: AF_BLUETOOTH as u16,
            bdaddr,
            channel,
        };
        if unsafe {
            connect(
                fd,
                (&raw const addr).cast(),
                std::mem::size_of::<SockAddrRc>() as u32,
            )
        } < 0
        {
            return Err(format!(
                "could not open RFCOMM channel {channel} on {address} (errno {})",
                errno()
            ));
        }
        let stopped = Arc::new(AtomicBool::new(false));
        spawn_reader(socket.clone(), sink, stopped.clone(), channel);
        Ok(Self {
            channel,
            peer: address.to_owned(),
            fd: socket,
            writing: Mutex::new(VecDeque::new()),
            closed: Mutex::new(None),
            stopped,
        })
    }

    fn adopt(fd: c_int, channel: u8, peer: String, sink: Arc<dyn ChannelSink>) -> Self {
        let socket = Arc::new(Socket(fd));
        let stopped = Arc::new(AtomicBool::new(false));
        spawn_reader(socket.clone(), sink, stopped.clone(), channel);
        Self {
            channel,
            peer,
            fd: socket,
            writing: Mutex::new(VecDeque::new()),
            closed: Mutex::new(None),
            stopped,
        }
    }

    /// The RFCOMM server channel number.
    pub fn channel(&self) -> u8 {
        self.channel
    }

    /// The peer's Bluetooth address.
    pub fn peer_id(&self) -> &str {
        &self.peer
    }

    /// Write bytes to the stream.
    pub fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        if let Some(reason) = self.closed() {
            return Err(reason);
        }
        let _order = self.writing.lock().unwrap();
        let mut sent = 0;
        while sent < bytes.len() {
            let n = unsafe { write(self.fd.0, bytes[sent..].as_ptr(), bytes.len() - sent) };
            if n > 0 {
                sent += n as usize;
                continue;
            }
            if n < 0 && errno() == EINTR {
                continue;
            }
            let reason = Closed::Error(format!("RFCOMM write failed (errno {})", errno()));
            *self.closed.lock().unwrap() = Some(reason.clone());
            return Err(reason);
        }
        Ok(())
    }

    /// RFCOMM writes go directly to the kernel.
    pub fn pending_bytes(&self) -> usize {
        0
    }

    /// Why the stream closed, or `None` while open.
    pub fn closed(&self) -> Option<Closed> {
        self.closed.lock().unwrap().clone()
    }

    /// Close the stream.
    pub fn close(&self) {
        if self.closed().is_none() {
            *self.closed.lock().unwrap() = Some(Closed::Locally);
        }
        self.stopped.store(true, Ordering::Release);
        unsafe { shutdown(self.fd.0, SHUT_RDWR) };
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.close();
    }
}

fn spawn_reader(
    socket: Arc<Socket>,
    sink: Arc<dyn ChannelSink>,
    stopped: Arc<AtomicBool>,
    channel: u8,
) {
    std::thread::Builder::new()
        .name(format!("webbluetooth-rfcomm-{channel}"))
        .spawn(move || {
            let mut buf = vec![0u8; 4096];
            let reason = loop {
                if stopped.load(Ordering::Acquire) {
                    break Closed::Locally;
                }
                let n = unsafe { read(socket.0, buf.as_mut_ptr(), buf.len()) };
                match n {
                    0 => break Closed::ByPeer,
                    n if n > 0 => sink.on_bytes(buf[..n as usize].to_vec()),
                    _ if errno() == EINTR => continue,
                    _ => break Closed::Error(format!("RFCOMM read failed (errno {})", errno())),
                }
            };
            sink.on_closed(reason);
        })
        .expect("could not start the RFCOMM reader thread");
}

/// The portable async wrapper around a Linux RFCOMM socket.
pub type RfcommChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

/// A Linux RFCOMM server listener.
pub struct RfcommListener {
    fd: Arc<Socket>,
    incoming: mpsc::UnboundedReceiver<webbluetooth_core::Result<RfcommChannel>>,
}

impl Stream for RfcommListener {
    type Item = webbluetooth_core::Result<RfcommChannel>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming).poll_next(cx)
    }
}

impl Drop for RfcommListener {
    fn drop(&mut self) {
        unsafe { shutdown(self.fd.0, SHUT_RDWR) };
    }
}

impl webbluetooth_core::l2cap::PlatformChannel for Channel {
    fn psm(&self) -> u16 {
        self.channel as u16
    }

    fn peer_id(&self) -> &str {
        Channel::peer_id(self)
    }

    fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        Channel::send(self, bytes)
    }

    fn pending_bytes(&self) -> usize {
        Channel::pending_bytes(self)
    }

    fn closed(&self) -> Option<Closed> {
        Channel::closed(self)
    }

    fn close(&self) {
        Channel::close(self)
    }
}

/// Open an RFCOMM stream to `address` on `channel`.
pub fn open(address: &str, channel: u8) -> webbluetooth_core::Result<RfcommChannel> {
    open_with_security(
        address,
        channel,
        webbluetooth_core::classic::ClassicSecurity::None,
    )
}

/// Open an RFCOMM stream with an explicit security requirement.
pub fn open_with_security(
    address: &str,
    channel: u8,
    security: webbluetooth_core::classic::ClassicSecurity,
) -> webbluetooth_core::Result<RfcommChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::connect_with_security(address, channel, security, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(RfcommChannel::new(channel, sink, incoming))
}

/// Listen for incoming RFCOMM connections on a server channel.
pub fn listen(channel: u8) -> webbluetooth_core::Result<RfcommListener> {
    if !(1..=30).contains(&channel) {
        return Err(webbluetooth_core::Error::InvalidModification(
            "RFCOMM channel must be between 1 and 30".into(),
        ));
    }
    let fd = unsafe { socket(AF_BLUETOOTH, SOCK_STREAM, BTPROTO_RFCOMM) };
    if fd < 0 {
        return Err(webbluetooth_core::Error::Network(format!(
            "could not create RFCOMM listener (errno {})",
            errno()
        )));
    }
    let socket = Arc::new(Socket(fd));
    let address = SockAddrRc {
        family: AF_BLUETOOTH as u16,
        bdaddr: BDADDR_ANY,
        channel,
    };
    if unsafe {
        bind(
            fd,
            (&raw const address).cast(),
            std::mem::size_of::<SockAddrRc>() as u32,
        )
    } < 0
        || unsafe { sys_listen(fd, 8) } < 0
    {
        return Err(webbluetooth_core::Error::Network(format!(
            "could not bind RFCOMM channel {channel} (errno {})",
            errno()
        )));
    }
    let (tx, rx) = mpsc::unbounded();
    let listener_fd = socket.clone();
    std::thread::Builder::new()
        .name(format!("webbluetooth-rfcomm-listener-{channel}"))
        .spawn(move || loop {
            let mut peer = SockAddrRc {
                family: 0,
                bdaddr: [0; 6],
                channel: 0,
            };
            let mut length = std::mem::size_of::<SockAddrRc>() as u32;
            let accepted = unsafe { accept(listener_fd.0, (&raw mut peer).cast(), &mut length) };
            if accepted < 0 {
                break;
            }
            let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
            let connected = Channel::adopt(
                accepted,
                channel,
                format_address(&peer.bdaddr),
                channel_sink,
            );
            let result = Ok(RfcommChannel::new(connected, sink, incoming));
            if tx.unbounded_send(result).is_err() {
                break;
            }
        })
        .map_err(|error| webbluetooth_core::Error::Network(error.to_string()))?;
    Ok(RfcommListener {
        fd: socket,
        incoming: rx,
    })
}
