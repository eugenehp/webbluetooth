//! L2CAP connection-oriented channels, as Bluetooth sockets.
//!
//! Linux exposes L2CAP directly through the socket API — `AF_BLUETOOTH` with
//! `BTPROTO_L2CAP` — so this needs neither BlueZ nor D-Bus. That is a much
//! shorter path than the Apple side, where the same thing arrives as a pair of
//! runloop-driven `NSStream`s.
//!
//! ```text
//!   socket(AF_BLUETOOTH, SOCK_SEQPACKET, BTPROTO_L2CAP)
//!   connect(&sockaddr_l2 { psm, bdaddr, bdaddr_type })
//!   read / write
//! ```
//!
//! One detail that silently breaks everything if missed: **a Bluetooth address
//! goes on the wire least-significant byte first**, so `AA:BB:CC:DD:EE:FF` is
//! the bytes `FF EE DD CC BB AA`. And an LE channel must name the peer's
//! address *type* — connecting to a random address as though it were public
//! simply times out.

use crate::sys::{
    address_type, bind_le_source, close, connect, errno, parse_address, read, shutdown, socket,
    write, SockAddrL2, AF_BLUETOOTH, BTPROTO_L2CAP, EINTR, SHUT_RDWR, SOCK_SEQPACKET,
};
use std::collections::VecDeque;
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use webbluetooth_core::l2cap::{ChannelSink, Closed, Psm};

struct Socket(c_int);

impl Drop for Socket {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}

/// An open L2CAP channel.
pub struct Channel {
    psm: Psm,
    peer: String,
    fd: Arc<Socket>,
    /// Serialises writes; a partial write must not interleave with another.
    writing: Mutex<VecDeque<u8>>,
    closed: Mutex<Option<Closed>>,
    stopped: Arc<AtomicBool>,
}

impl Channel {
    /// Connect to `address` on `psm`.
    ///
    /// `address_type` is BlueZ's `AddressType`: `"public"` or `"random"`.
    pub fn connect(
        address: &str,
        peer_address_type: &str,
        psm: Psm,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        let bdaddr = parse_address(address)
            .ok_or_else(|| format!("{address:?} is not a Bluetooth address"))?;

        let fd = unsafe { socket(AF_BLUETOOTH, SOCK_SEQPACKET, BTPROTO_L2CAP) };
        if fd < 0 {
            return Err(format!(
                "could not create an L2CAP socket (errno {})",
                errno()
            ));
        }
        let socket = Arc::new(Socket(fd));

        if let Err(e) = bind_le_source(fd, 0, 0) {
            return Err(format!("could not bind an LE L2CAP socket (errno {e})"));
        }

        let addr = SockAddrL2 {
            family: AF_BLUETOOTH as u16,
            psm: psm.to_le(),
            bdaddr,
            cid: 0,
            bdaddr_type: address_type(peer_address_type == "random"),
        };
        let rc = unsafe { connect(fd, &addr, std::mem::size_of::<SockAddrL2>() as u32) };
        if rc < 0 {
            return Err(format!(
                "could not open L2CAP PSM {psm} on {address} (errno {})",
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

    pub fn psm(&self) -> Psm {
        self.psm
    }
    pub fn peer_id(&self) -> &str {
        &self.peer
    }

    /// Write bytes. Blocks only as long as the kernel's socket buffer requires.
    pub fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        if let Some(reason) = self.closed() {
            return Err(reason);
        }
        // Held across the write so two senders cannot interleave a partial one.
        let _order = self.writing.lock().unwrap();
        let mut sent = 0;
        while sent < bytes.len() {
            let n = unsafe { write(self.fd.0, bytes[sent..].as_ptr(), bytes.len() - sent) };
            if n > 0 {
                sent += n as usize;
                continue;
            }
            // EINTR is not a failure; anything else is.
            if n < 0 && errno() == EINTR {
                continue;
            }
            let reason = Closed::Error(format!("L2CAP write failed (errno {})", errno()));
            *self.closed.lock().unwrap() = Some(reason.clone());
            return Err(reason);
        }
        Ok(())
    }

    /// Always zero: writes go straight to the kernel rather than an internal
    /// queue. Present so both platforms answer the same question.
    pub fn pending_bytes(&self) -> usize {
        0
    }

    pub fn closed(&self) -> Option<Closed> {
        self.closed.lock().unwrap().clone()
    }

    pub fn close(&self) {
        {
            let mut closed = self.closed.lock().unwrap();
            if closed.is_none() {
                *closed = Some(Closed::Locally);
            }
        }
        self.stopped.store(true, Ordering::Release);
        // Wakes the reader out of its blocking read.
        unsafe { shutdown(self.fd.0, SHUT_RDWR) };
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.close();
    }
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("L2capChannel")
            .field("psm", &self.psm)
            .field("peer", &self.peer)
            .field("closed", &self.closed())
            .finish()
    }
}

fn spawn_reader(
    socket: Arc<Socket>,
    sink: Arc<dyn ChannelSink>,
    stopped: Arc<AtomicBool>,
    psm: Psm,
) {
    std::thread::Builder::new()
        .name(format!("webbluetooth-l2cap-{psm}"))
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
                    // EINTR
                    _ if errno() == EINTR => continue,
                    _ => {
                        if stopped.load(Ordering::Acquire) {
                            break Closed::Locally;
                        }
                        break Closed::Error(format!("L2CAP read failed (errno {})", errno()));
                    }
                }
            };
            sink.on_closed(reason);
        })
        .expect("could not start the L2CAP reader thread");
}

// ── The portable wrapper ────────────────────────────────────────────────────

/// This platform's channel, wrapped in the portable one.
///
/// The wrapper — backlog, async stream, close notification — is
/// `webbluetooth-core`'s and identical on every platform. Only what is above
/// this line is Bluetooth-stack-specific.
pub type L2capChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

impl webbluetooth_core::l2cap::PlatformChannel for Channel {
    fn psm(&self) -> Psm {
        Channel::psm(self)
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

/// Open a channel to `address` on `psm`.
///
/// The Linux path: L2CAP is an ordinary socket here, so a channel is connected
/// rather than handed over.
pub fn open(
    address: &str,
    address_type: &str,
    psm: Psm,
) -> webbluetooth_core::Result<L2capChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::connect(address, address_type, psm, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(L2capChannel::new(channel, sink, incoming))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_reversed_onto_the_wire() {
        // The single easiest thing to get wrong, and it fails as a timeout
        // rather than an error.
        assert_eq!(
            parse_address("AA:BB:CC:DD:EE:FF"),
            Some([0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA])
        );
        assert_eq!(
            parse_address("00:00:00:00:00:01"),
            Some([0x01, 0, 0, 0, 0, 0])
        );
    }

    #[test]
    fn malformed_addresses_are_rejected() {
        assert_eq!(parse_address(""), None);
        assert_eq!(parse_address("AA:BB:CC:DD:EE"), None);
        assert_eq!(parse_address("AA:BB:CC:DD:EE:FF:00"), None);
        assert_eq!(parse_address("ZZ:BB:CC:DD:EE:FF"), None);
    }

    #[test]
    fn the_socket_address_matches_the_kernel_layout() {
        // sockaddr_l2 is packed: 2 + 2 + 6 + 2 + 1 = 13 bytes, no padding.
        assert_eq!(std::mem::size_of::<SockAddrL2>(), 13);
    }

    #[test]
    fn connecting_to_a_bad_address_errors_rather_than_panicking() {
        struct Discard;
        impl ChannelSink for Discard {
            fn on_bytes(&self, _: Vec<u8>) {}
            fn on_closed(&self, _: Closed) {}
        }
        let err = Channel::connect("not-an-address", "public", 0x0080, Arc::new(Discard));
        assert!(err.is_err());
    }
}
