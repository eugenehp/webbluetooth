//! Linux raw HCI ACL/SCO packet sockets.

#![cfg(target_os = "linux")]

use crate::sys::{bind, close, errno, read, socket, write, AF_BLUETOOTH, BTPROTO_HCI, SOCK_RAW};
use std::ffi::c_int;

const HCI_CHANNEL_RAW: u16 = 0;
const HCI_ACLDATA_PKT: u8 = 0x02;
const HCI_SCODATA_PKT: u8 = 0x03;
const HCI_ISODATA_PKT: u8 = 0x05;

#[repr(C, packed)]
struct SockAddrHci {
    family: u16,
    dev: u16,
    channel: u16,
}

struct Socket(c_int);

impl Drop for Socket {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}

/// A raw Linux HCI packet socket bound to one controller.
pub struct RawHciSocket {
    socket: Socket,
    packet_type: u8,
}

impl RawHciSocket {
    fn open(device: u16, packet_type: u8) -> Result<Self, String> {
        let fd = unsafe { socket(AF_BLUETOOTH, SOCK_RAW, BTPROTO_HCI) };
        if fd < 0 {
            return Err(format!(
                "could not create raw HCI socket (errno {})",
                errno()
            ));
        }
        let address = SockAddrHci {
            family: AF_BLUETOOTH as u16,
            dev: device,
            channel: HCI_CHANNEL_RAW,
        };
        if unsafe {
            bind(
                fd,
                (&raw const address).cast(),
                std::mem::size_of::<SockAddrHci>() as u32,
            )
        } < 0
        {
            return Err(format!("could not bind raw HCI socket (errno {})", errno()));
        }
        Ok(Self {
            socket: Socket(fd),
            packet_type,
        })
    }

    /// Read one packet payload, discarding packets for other HCI types.
    pub fn read(&self) -> Result<Vec<u8>, String> {
        let mut buffer = vec![0; 4096];
        loop {
            let length = unsafe { read(self.socket.0, buffer.as_mut_ptr(), buffer.len()) };
            if length < 0 {
                return Err(format!("raw HCI read failed (errno {})", errno()));
            }
            if length == 0 {
                return Err("raw HCI socket closed".into());
            }
            let length = length as usize;
            if buffer[0] == self.packet_type {
                return Ok(buffer[1..length].to_vec());
            }
        }
    }

    /// Write one HCI packet payload.
    pub fn write(&self, payload: &[u8]) -> Result<(), String> {
        let mut packet = Vec::with_capacity(payload.len() + 1);
        packet.push(self.packet_type);
        packet.extend_from_slice(payload);
        let written = unsafe { write(self.socket.0, packet.as_ptr(), packet.len()) };
        if written < 0 || written as usize != packet.len() {
            return Err(format!("raw HCI write failed (errno {})", errno()));
        }
        Ok(())
    }
}

#[cfg(feature = "raw-acl")]
/// Open a raw ACL HCI socket.
pub fn open_acl(device: u16) -> Result<RawHciSocket, String> {
    RawHciSocket::open(device, HCI_ACLDATA_PKT)
}

#[cfg(feature = "raw-sco")]
/// Open a raw SCO/eSCO HCI socket.
pub fn open_sco(device: u16) -> Result<RawHciSocket, String> {
    RawHciSocket::open(device, HCI_SCODATA_PKT)
}

#[cfg(feature = "le-audio")]
/// Open a raw ISO HCI socket for LE Audio CIS/BIS packets.
pub fn open_iso(device: u16) -> Result<RawHciSocket, String> {
    RawHciSocket::open(device, HCI_ISODATA_PKT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_types_match_hci_assignments() {
        assert_eq!(HCI_ACLDATA_PKT, 0x02);
        assert_eq!(HCI_SCODATA_PKT, 0x03);
        assert_eq!(HCI_ISODATA_PKT, 0x05);
    }
}
