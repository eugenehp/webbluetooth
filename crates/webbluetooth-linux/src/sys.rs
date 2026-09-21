//! The Bluetooth socket family.
//!
//! Both Linux transports here are `AF_BLUETOOTH` sockets, so the address
//! layout, the address parsing and the libc externs live once rather than once
//! per transport.

use std::ffi::c_int;

pub const AF_BLUETOOTH: c_int = 31;
pub const SOCK_SEQPACKET: c_int = 5;
pub const SOCK_RAW: c_int = 3;
pub const BTPROTO_L2CAP: c_int = 0;
pub const BTPROTO_HCI: c_int = 1;
pub const BDADDR_LE_PUBLIC: u8 = 1;
/// `BDADDR_ANY` — let the kernel pick the adapter.
pub const BDADDR_ANY: [u8; 6] = [0; 6];
pub const BDADDR_LE_RANDOM: u8 = 2;
pub const SHUT_RDWR: c_int = 2;
/// `EINTR` — a syscall interrupted by a signal, which is a retry not a failure.
pub const EINTR: i32 = 4;

unsafe extern "C" {
    pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
    pub fn read(fd: c_int, buf: *mut u8, count: usize) -> isize;
    pub fn write(fd: c_int, buf: *const u8, count: usize) -> isize;
    pub fn close(fd: c_int) -> c_int;
    pub fn shutdown(fd: c_int, how: c_int) -> c_int;
    fn __errno_location() -> *mut c_int;
}

/// The current `errno`.
pub fn errno() -> i32 {
    unsafe { *__errno_location() }
}

/// `struct sockaddr_l2` from `bluetooth/l2cap.h`.
///
/// Packed, 13 bytes, no trailing padding — a mismatch here is a `connect` that
/// fails for no visible reason.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SockAddrL2 {
    pub family: u16,
    pub psm: u16,
    pub bdaddr: [u8; 6],
    pub cid: u16,
    pub bdaddr_type: u8,
}

unsafe extern "C" {
    pub fn connect(fd: c_int, addr: *const SockAddrL2, len: u32) -> c_int;
    /// Takes a `*const c_void` because both socket families here bind with
    /// their own `sockaddr` shape, and one extern declaration has to serve
    /// both — two declarations of the same symbol with different pointer
    /// types do not compile.
    pub fn bind(fd: c_int, addr: *const core::ffi::c_void, len: u32) -> c_int;
    fn poll(fds: *mut PollFd, nfds: core::ffi::c_ulong, timeout: c_int) -> c_int;
}

/// Parse `AA:BB:CC:DD:EE:FF` into wire order.
///
/// **A Bluetooth address goes on the wire least-significant byte first**, so
/// the result is the written order reversed. Getting this wrong produces a
/// connection that times out rather than an error, which is why it is one
/// function with one test rather than a line repeated per transport.
pub fn parse_address(address: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = address.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        out[5 - i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(out)
}

/// Format a wire-order address as it is written.
pub fn format_address(wire: &[u8; 6]) -> String {
    let mut out = String::with_capacity(17);
    for (i, b) in wire.iter().rev().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

pub const POLLIN: i16 = 0x001;
pub const POLLOUT: i16 = 0x004;
const POLLERR: i16 = 0x008;
const POLLHUP: i16 = 0x010;

/// Whether a descriptor became ready, timed out, or failed.
#[derive(Debug, PartialEq, Eq)]
pub enum Ready {
    Yes,
    TimedOut,
    /// The peer hung up or the socket errored.
    Closed,
}

/// Wait for `fd` to be ready, for at most `timeout_ms`.
///
/// Every blocking call on a Bluetooth socket goes through this first.
/// `read` and `write` on an L2CAP socket can both park in the kernel with no
/// deadline of their own — a channel suspended pending security will hold a
/// `write` forever, with `SO_SNDTIMEO` unset — and a caller that has already
/// decided how long it is willing to wait cannot express that to them.
pub fn wait_ready(fd: c_int, events: i16, timeout_ms: c_int) -> Result<Ready, i32> {
    let mut fds = PollFd {
        fd,
        events,
        revents: 0,
    };
    loop {
        let rc = unsafe { poll(&mut fds, 1, timeout_ms) };
        if rc < 0 {
            let e = errno();
            if e == EINTR {
                continue;
            }
            return Err(e);
        }
        if rc == 0 {
            return Ok(Ready::TimedOut);
        }
        if fds.revents & (POLLERR | POLLHUP) != 0 {
            return Ok(Ready::Closed);
        }
        return Ok(Ready::Yes);
    }
}

/// The address type byte for an LE peer.
pub fn address_type(random: bool) -> u8 {
    if random {
        BDADDR_LE_RANDOM
    } else {
        BDADDR_LE_PUBLIC
    }
}

/// Bind an L2CAP socket to the local side of an LE link before connecting.
///
/// Without this the kernel leaves the channel's source type at `BDADDR_BREDR`,
/// and `l2cap_chan_connect` refuses a BR/EDR source with an LE destination —
/// `connect` fails with `EINVAL` and no link is ever attempted. Binding is how
/// the socket is told it is an LE one; BlueZ's own tools do the same.
///
/// `BDADDR_ANY` leaves the choice of adapter to the kernel, which is what we
/// want: the caller names a peer, not a controller.
pub fn bind_le_source(fd: c_int, psm: u16, cid: u16) -> Result<(), i32> {
    let addr = SockAddrL2 {
        family: AF_BLUETOOTH as u16,
        psm: psm.to_le(),
        bdaddr: BDADDR_ANY,
        cid: cid.to_le(),
        bdaddr_type: BDADDR_LE_PUBLIC,
    };
    let len = core::mem::size_of::<SockAddrL2>() as u32;
    if unsafe { bind(fd, (&raw const addr).cast(), len) } < 0 {
        return Err(errno());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_reversed_onto_the_wire() {
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
    fn formatting_is_the_inverse_of_parsing() {
        let written = "AA:BB:CC:DD:EE:FF";
        assert_eq!(format_address(&parse_address(written).unwrap()), written);
    }

    #[test]
    fn malformed_addresses_are_rejected() {
        for bad in [
            "",
            "AA:BB:CC:DD:EE",
            "AA:BB:CC:DD:EE:FF:00",
            "ZZ:BB:CC:DD:EE:FF",
        ] {
            assert_eq!(parse_address(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn the_socket_address_matches_the_kernel_layout() {
        assert_eq!(std::mem::size_of::<SockAddrL2>(), 13);
    }
}
