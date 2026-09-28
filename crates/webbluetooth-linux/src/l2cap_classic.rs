//! Bluetooth Classic L2CAP connection-oriented channels on Linux.
//!
//! This is deliberately separate from the LE L2CAP implementation: Classic
//! channels use BR/EDR socket addressing and a PSM, while LE channels use an
//! LE address type and CID-based ATT/CoC setup.

use crate::sys::{
    accept, bind, close, connect, errno, format_address, listen as sys_listen, parse_address, read,
    set_classic_security, shutdown, socket, write, SockAddrL2, AF_BLUETOOTH, BDADDR_ANY,
    BTPROTO_L2CAP, EINTR, SHUT_RDWR, SOCK_SEQPACKET,
};
use futures_channel::mpsc;
use futures_core::Stream;
use std::collections::VecDeque;
use std::ffi::c_int;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use webbluetooth_core::classic::ClassicPsm;
pub use webbluetooth_core::l2cap::{ChannelSink, Closed, Incoming};

struct Socket(c_int);

impl Drop for Socket {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}

/// An open Bluetooth Classic L2CAP channel.
pub struct Channel {
    psm: ClassicPsm,
    peer: String,
    fd: Arc<Socket>,
    writing: Mutex<VecDeque<u8>>,
    closed: Mutex<Option<Closed>>,
    stopped: Arc<AtomicBool>,
}

impl Channel {
    /// Connect to a Classic peer's PSM.
    pub fn connect(
        address: &str,
        psm: ClassicPsm,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        Self::connect_with_security(
            address,
            psm,
            webbluetooth_core::classic::ClassicSecurity::None,
            sink,
        )
    }

    /// Connect with an explicit Classic link security requirement.
    pub fn connect_with_security(
        address: &str,
        psm: ClassicPsm,
        security: webbluetooth_core::classic::ClassicSecurity,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        let bdaddr = parse_address(address)
            .ok_or_else(|| format!("{address:?} is not a Bluetooth address"))?;
        let fd = unsafe { socket(AF_BLUETOOTH, SOCK_SEQPACKET, BTPROTO_L2CAP) };
        if fd < 0 {
            return Err(format!(
                "could not create a Classic L2CAP socket (errno {})",
                errno()
            ));
        }
        let socket = Arc::new(Socket(fd));
        set_classic_security(fd, security).map_err(|error| {
            format!("could not configure Classic L2CAP security (errno {error})")
        })?;
        let addr = SockAddrL2 {
            family: AF_BLUETOOTH as u16,
            psm: psm.get().to_le(),
            bdaddr,
            cid: 0,
            bdaddr_type: 0,
        };
        if unsafe { connect(fd, &addr, std::mem::size_of::<SockAddrL2>() as u32) } < 0 {
            return Err(format!(
                "could not open Classic L2CAP PSM 0x{:04x} on {address} (errno {})",
                psm.get(),
                errno()
            ));
        }
        let stopped = Arc::new(AtomicBool::new(false));
        spawn_reader(socket.clone(), sink, stopped.clone(), psm);
        Ok(Self {
            psm,
            peer: address.to_owned(),
            fd: socket,
            writing: Mutex::new(VecDeque::new()),
            closed: Mutex::new(None),
            stopped,
        })
    }

    /// Adopt a connected file descriptor delivered by BlueZ Profile1.
    pub fn adopt_fd(fd: c_int, psm: ClassicPsm, peer: String, sink: Arc<dyn ChannelSink>) -> Self {
        let socket = Arc::new(Socket(fd));
        let stopped = Arc::new(AtomicBool::new(false));
        spawn_reader(socket.clone(), sink, stopped.clone(), psm);
        Self {
            psm,
            peer,
            fd: socket,
            writing: Mutex::new(VecDeque::new()),
            closed: Mutex::new(None),
            stopped,
        }
    }

    /// The Classic PSM.
    pub fn psm(&self) -> ClassicPsm {
        self.psm
    }

    /// The peer's Bluetooth address.
    pub fn peer_id(&self) -> &str {
        &self.peer
    }

    /// Write bytes to the channel.
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
            let reason = Closed::Error(format!("Classic L2CAP write failed (errno {})", errno()));
            *self.closed.lock().unwrap() = Some(reason.clone());
            return Err(reason);
        }
        Ok(())
    }

    /// Classic socket writes go directly to the kernel.
    pub fn pending_bytes(&self) -> usize {
        0
    }

    /// Why the channel closed, or `None` while open.
    pub fn closed(&self) -> Option<Closed> {
        self.closed.lock().unwrap().clone()
    }

    /// Close the channel.
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
    psm: ClassicPsm,
) {
    std::thread::Builder::new()
        .name(format!("webbluetooth-l2cap-classic-{:04x}", psm.get()))
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
                    _ => {
                        break Closed::Error(format!(
                            "Classic L2CAP read failed (errno {})",
                            errno()
                        ))
                    }
                }
            };
            sink.on_closed(reason);
        })
        .expect("could not start the Classic L2CAP reader thread");
}

/// The portable async wrapper around a Linux Classic L2CAP socket.
pub type ClassicL2capChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

/// A Linux Bluetooth Classic L2CAP server listener.
pub struct ClassicL2capListener {
    fd: Arc<Socket>,
    incoming: mpsc::UnboundedReceiver<webbluetooth_core::Result<ClassicL2capChannel>>,
}

impl Stream for ClassicL2capListener {
    type Item = webbluetooth_core::Result<ClassicL2capChannel>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming).poll_next(cx)
    }
}

impl Drop for ClassicL2capListener {
    fn drop(&mut self) {
        unsafe { shutdown(self.fd.0, SHUT_RDWR) };
    }
}

impl webbluetooth_core::l2cap::PlatformChannel for Channel {
    fn psm(&self) -> u16 {
        self.psm.get()
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

/// Open a Classic L2CAP channel to `address` on `psm`.
pub fn open(address: &str, psm: ClassicPsm) -> webbluetooth_core::Result<ClassicL2capChannel> {
    open_with_security(
        address,
        psm,
        webbluetooth_core::classic::ClassicSecurity::None,
    )
}

/// Open a Classic L2CAP channel with an explicit security requirement.
pub fn open_with_security(
    address: &str,
    psm: ClassicPsm,
    security: webbluetooth_core::classic::ClassicSecurity,
) -> webbluetooth_core::Result<ClassicL2capChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::connect_with_security(address, psm, security, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(ClassicL2capChannel::new(channel, sink, incoming))
}

/// Listen for incoming Classic L2CAP connections on a PSM.
pub fn listen(psm: ClassicPsm) -> webbluetooth_core::Result<ClassicL2capListener> {
    let fd = unsafe { socket(AF_BLUETOOTH, SOCK_SEQPACKET, BTPROTO_L2CAP) };
    if fd < 0 {
        return Err(webbluetooth_core::Error::Network(format!(
            "could not create Classic L2CAP listener (errno {})",
            errno()
        )));
    }
    let socket = Arc::new(Socket(fd));
    let address = SockAddrL2 {
        family: AF_BLUETOOTH as u16,
        psm: psm.get().to_le(),
        bdaddr: BDADDR_ANY,
        cid: 0,
        bdaddr_type: 0,
    };
    if unsafe {
        bind(
            fd,
            (&raw const address).cast(),
            std::mem::size_of::<SockAddrL2>() as u32,
        )
    } < 0
        || unsafe { sys_listen(fd, 8) } < 0
    {
        return Err(webbluetooth_core::Error::Network(format!(
            "could not bind Classic L2CAP PSM 0x{:04x} (errno {})",
            psm.get(),
            errno()
        )));
    }
    let (tx, rx) = mpsc::unbounded();
    let listener_fd = socket.clone();
    let accept_fd = listener_fd.clone();
    std::thread::Builder::new()
        .name(format!(
            "webbluetooth-l2cap-classic-listener-{:04x}",
            psm.get()
        ))
        .spawn(move || loop {
            let mut peer = SockAddrL2 {
                family: 0,
                psm: 0,
                bdaddr: [0; 6],
                cid: 0,
                bdaddr_type: 0,
            };
            let mut length = std::mem::size_of::<SockAddrL2>() as u32;
            let accepted = unsafe { accept(accept_fd.0, (&raw mut peer).cast(), &mut length) };
            if accepted < 0 {
                break;
            }
            let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
            let connected =
                Channel::adopt_fd(accepted, psm, format_address(&peer.bdaddr), channel_sink);
            if tx
                .unbounded_send(Ok(ClassicL2capChannel::new(connected, sink, incoming)))
                .is_err()
            {
                break;
            }
        })
        .map_err(|error| webbluetooth_core::Error::Network(error.to_string()))?;
    Ok(ClassicL2capListener {
        fd: listener_fd,
        incoming: rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_addresses_are_rejected_before_socket_creation() {
        let psm = ClassicPsm::new(0x1003).unwrap();
        let (sink, _, _) = webbluetooth_core::l2cap::sink();
        assert!(Channel::connect("not-an-address", psm, sink).is_err());
    }
}
