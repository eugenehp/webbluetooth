//! LE scanning over a raw HCI socket.
//!
//! The one thing ATT cannot do. An L2CAP socket can reach a peer you already
//! know the address of, but *finding* one means listening to advertising
//! reports, and those only come from the controller.
//!
//! ```text
//!   socket(AF_BLUETOOTH, SOCK_RAW, BTPROTO_HCI)
//!   bind(&sockaddr_hci { dev, channel = HCI_CHANNEL_RAW })
//!   setsockopt(HCI_FILTER)                 -- or every packet arrives
//!   << LE Set Scan Parameters
//!   << LE Set Scan Enable
//!   >> HCI Event 0x3E (LE Meta) / 0x02 (Advertising Report)
//! ```
//!
//! **This needs `CAP_NET_RAW`**, and a controller that `bluetoothd` is not
//! already scanning on. That is the whole cost of the daemon-free path: reading
//! and writing GATT is unprivileged, discovery is not.
//!
//! The advertising-data parser is portable and unit-tested anywhere; only the
//! socket is Linux-only.

/// One parsed advertising report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Advertisement {
    /// `AA:BB:CC:DD:EE:FF`, written order.
    pub address: String,
    /// `true` if the peer uses a random (often resolvable) address.
    pub random_address: bool,
    pub rssi: i8,
    pub local_name: Option<String>,
    pub tx_power: Option<i8>,
    /// GAP Appearance, from AD type `0x19`.
    pub appearance: Option<u16>,
    /// Canonical 128-bit service UUIDs.
    pub service_uuids: Vec<String>,
    /// Company identifier → payload.
    pub manufacturer_data: Option<(u16, Vec<u8>)>,
    /// Service UUID → payload.
    pub service_data: Vec<(String, Vec<u8>)>,
    /// Whether the advertisement said connections are accepted.
    pub connectable: bool,
}

/// Format a wire-order address as it is written.
///
/// Mirrors [`crate::sys::format_address`], which is Linux-only; the report
/// parser is portable so that it can be tested anywhere.
#[cfg(target_os = "linux")]
use crate::sys::format_address;

#[cfg(not(target_os = "linux"))]
fn format_address(wire: &[u8; 6]) -> String {
    let mut out = String::with_capacity(17);
    for (i, b) in wire.iter().rev().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// AD types, from the Core Specification Supplement.
mod ad {
    pub const FLAGS: u8 = 0x01;
    pub const INCOMPLETE_16: u8 = 0x02;
    pub const COMPLETE_16: u8 = 0x03;
    pub const INCOMPLETE_32: u8 = 0x04;
    pub const COMPLETE_32: u8 = 0x05;
    pub const INCOMPLETE_128: u8 = 0x06;
    pub const COMPLETE_128: u8 = 0x07;
    pub const SHORTENED_NAME: u8 = 0x08;
    pub const COMPLETE_NAME: u8 = 0x09;
    pub const TX_POWER: u8 = 0x0A;
    pub const APPEARANCE: u8 = 0x19;
    pub const SERVICE_DATA_16: u8 = 0x16;
    pub const SERVICE_DATA_32: u8 = 0x20;
    pub const SERVICE_DATA_128: u8 = 0x21;
    pub const MANUFACTURER_DATA: u8 = 0xFF;
}

/// Expand a 16-bit assigned number into the canonical 128-bit form.
fn canonical_16(value: u16) -> String {
    format!("0000{value:04x}-0000-1000-8000-00805f9b34fb")
}

fn canonical_32(value: u32) -> String {
    format!("{value:08x}-0000-1000-8000-00805f9b34fb")
}

/// Format 16 wire-order bytes as a canonical UUID.
fn canonical_128(wire: &[u8]) -> Option<String> {
    if wire.len() != 16 {
        return None;
    }
    let hex: String = wire.iter().rev().map(|b| format!("{b:02x}")).collect();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// Parse the AD structures in an advertising payload.
///
/// The payload is a sequence of `length, type, data…` records. A zero length
/// terminates it — padding after that is not a record, and treating it as one
/// is a classic way to read garbage.
pub fn parse_advertising_data(data: &[u8], into: &mut Advertisement) {
    let mut i = 0;
    while i < data.len() {
        let length = data[i] as usize;
        if length == 0 {
            break;
        }
        // A length that runs past the end means a truncated or hostile report.
        if i + 1 + length > data.len() {
            break;
        }
        let ad_type = data[i + 1];
        let value = &data[i + 2..i + 1 + length];

        match ad_type {
            ad::FLAGS => {
                // Bit 0 LE limited discoverable, bit 1 LE general discoverable.
                if let Some(flags) = value.first() {
                    into.connectable = flags & 0x03 != 0;
                }
            }
            ad::SHORTENED_NAME | ad::COMPLETE_NAME => {
                if let Ok(name) = std::str::from_utf8(value) {
                    // A complete name wins over a shortened one.
                    if ad_type == ad::COMPLETE_NAME || into.local_name.is_none() {
                        into.local_name = Some(name.to_owned());
                    }
                }
            }
            ad::TX_POWER => into.tx_power = value.first().map(|b| *b as i8),
            ad::INCOMPLETE_16 | ad::COMPLETE_16 => {
                for pair in value.chunks_exact(2) {
                    into.service_uuids
                        .push(canonical_16(u16::from_le_bytes([pair[0], pair[1]])));
                }
            }
            ad::INCOMPLETE_32 | ad::COMPLETE_32 => {
                for quad in value.chunks_exact(4) {
                    into.service_uuids.push(canonical_32(u32::from_le_bytes([
                        quad[0], quad[1], quad[2], quad[3],
                    ])));
                }
            }
            ad::INCOMPLETE_128 | ad::COMPLETE_128 => {
                for block in value.chunks_exact(16) {
                    if let Some(uuid) = canonical_128(block) {
                        into.service_uuids.push(uuid);
                    }
                }
            }
            ad::APPEARANCE => {
                if value.len() >= 2 {
                    into.appearance = Some(u16::from_le_bytes([value[0], value[1]]));
                }
            }
            ad::MANUFACTURER_DATA => {
                if value.len() >= 2 {
                    into.manufacturer_data = Some((
                        u16::from_le_bytes([value[0], value[1]]),
                        value[2..].to_vec(),
                    ));
                }
            }
            ad::SERVICE_DATA_16 => {
                if value.len() >= 2 {
                    into.service_data.push((
                        canonical_16(u16::from_le_bytes([value[0], value[1]])),
                        value[2..].to_vec(),
                    ));
                }
            }
            ad::SERVICE_DATA_32 => {
                if value.len() >= 4 {
                    into.service_data.push((
                        canonical_32(u32::from_le_bytes([value[0], value[1], value[2], value[3]])),
                        value[4..].to_vec(),
                    ));
                }
            }
            ad::SERVICE_DATA_128 => {
                if value.len() >= 16 {
                    if let Some(uuid) = canonical_128(&value[..16]) {
                        into.service_data.push((uuid, value[16..].to_vec()));
                    }
                }
            }
            _ => {}
        }
        i += 1 + length;
    }
}

/// Parse an HCI `LE Advertising Report` sub-event body.
///
/// Layout: number of reports, then per report — event type, address type,
/// 6-byte address, data length, data, RSSI.
pub fn parse_advertising_report(body: &[u8]) -> Vec<Advertisement> {
    let mut out = Vec::new();
    let Some(&count) = body.first() else {
        return out;
    };
    let mut i = 1;
    for _ in 0..count {
        if i + 9 > body.len() {
            break;
        }
        let event_type = body[i];
        let address_type = body[i + 1];
        let mut wire = [0u8; 6];
        wire.copy_from_slice(&body[i + 2..i + 8]);
        let data_len = body[i + 8] as usize;
        if i + 9 + data_len + 1 > body.len() {
            break;
        }
        let data = &body[i + 9..i + 9 + data_len];
        let rssi = body[i + 9 + data_len] as i8;

        let mut advertisement = Advertisement {
            address: format_address(&wire),
            random_address: address_type == 0x01 || address_type == 0x03,
            rssi,
            // Event type 0x00 ADV_IND and 0x01 ADV_DIRECT_IND accept connections.
            connectable: event_type == 0x00 || event_type == 0x01,
            ..Default::default()
        };
        parse_advertising_data(data, &mut advertisement);
        out.push(advertisement);

        i += 9 + data_len + 1;
    }
    out
}

/// `LE Extended Advertising Report`, the 5.0 form of the same event.
///
/// A legacy scan gets legacy reports, so this is not on the usual path — but a
/// controller configured through the extended commands can emit these anyway,
/// and a peer advertising extended-only PDUs is invisible without it. The
/// layout is fixed-width per report and much longer than the legacy one, with
/// the data length near the end rather than at offset 8.
pub fn parse_extended_advertising_report(body: &[u8]) -> Vec<Advertisement> {
    // event_type(2) addr_type(1) addr(6) primary_phy(1) secondary_phy(1)
    // sid(1) tx_power(1) rssi(1) periodic_interval(2) direct_addr_type(1)
    // direct_addr(6) data_len(1)
    const FIXED: usize = 24;

    let mut out = Vec::new();
    let Some(&count) = body.first() else {
        return out;
    };
    let mut i = 1;
    for _ in 0..count {
        if i + FIXED > body.len() {
            break;
        }
        let event_type = u16::from_le_bytes([body[i], body[i + 1]]);
        let address_type = body[i + 2];
        let mut wire = [0u8; 6];
        wire.copy_from_slice(&body[i + 3..i + 9]);
        let tx_power = body[i + 12] as i8;
        let rssi = body[i + 13] as i8;
        let data_len = body[i + FIXED - 1] as usize;
        if i + FIXED + data_len > body.len() {
            break;
        }
        let data = &body[i + FIXED..i + FIXED + data_len];

        let mut advertisement = Advertisement {
            address: format_address(&wire),
            random_address: address_type == 0x01 || address_type == 0x03,
            rssi,
            // Bit 0 of the event-type properties is "connectable", which
            // replaces the legacy PDU-type comparison.
            connectable: event_type & 0x0001 != 0,
            ..Default::default()
        };
        parse_advertising_data(data, &mut advertisement);
        // 127 means "not available" in this field, unlike the legacy report
        // where tx power only appears as an AD record.
        if tx_power != 127 && advertisement.tx_power.is_none() {
            advertisement.tx_power = Some(tx_power);
        }
        out.push(advertisement);

        i += FIXED + data_len;
    }
    out
}

// ── The socket ──────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod socket {
    use super::{parse_advertising_report, parse_extended_advertising_report, Advertisement};
    use crate::sys::{
        close, errno, read, socket, write, AF_BLUETOOTH, BTPROTO_HCI, EINTR, SOCK_RAW,
    };
    use std::ffi::{c_int, c_ulong};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const HCI_CHANNEL_RAW: u16 = 0;
    const SOL_HCI: c_int = 0;
    const HCI_FILTER: c_int = 2;
    const HCI_EVENT_PKT: u8 = 0x04;
    const EVT_LE_META: u8 = 0x3E;
    const SUBEVT_LE_ADVERTISING_REPORT: u8 = 0x02;
    const SUBEVT_LE_EXTENDED_ADVERTISING_REPORT: u8 = 0x0D;
    const OGF_LE_CTL: u16 = 0x08;
    const OCF_LE_SET_SCAN_PARAMETERS: u16 = 0x000B;
    const OCF_LE_SET_SCAN_ENABLE: u16 = 0x000C;

    // Only the HCI-specific calls; the rest come from `crate::sys`.
    use crate::sys::bind;
    unsafe extern "C" {
        /// Declared variadic because it is: giving it a concrete third
        /// parameter would be a different function to the one that exists.
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;

        fn setsockopt(
            fd: c_int,
            level: c_int,
            name: c_int,
            val: *const HciFilter,
            len: u32,
        ) -> c_int;
    }

    #[repr(C, packed)]
    struct SockAddrHci {
        family: u16,
        dev: u16,
        channel: u16,
    }

    /// `struct hci_filter`. Without one the socket delivers every packet the
    /// controller produces, including ACL data for other connections.
    #[repr(C)]
    struct HciFilter {
        type_mask: u32,
        event_mask: [u32; 2],
        opcode: u16,
    }

    /// A raw HCI socket bound to one controller.
    pub struct HciSocket {
        fd: c_int,
        scanning: AtomicBool,
    }

    impl Drop for HciSocket {
        fn drop(&mut self) {
            if self.scanning.load(Ordering::Acquire) {
                let _ = self.set_scan_enable(false);
            }
            unsafe { close(self.fd) };
        }
    }

    /// One controller, as `HCIGETDEVLIST` and `HCIGETDEVINFO` describe it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DeviceInfo {
        /// The controller index: 0 for `hci0`.
        pub id: u16,
        /// The kernel's name for it, which is `hci` followed by the index.
        pub name: String,
        /// The controller's own address, big-endian as written down.
        pub address: String,
        /// Whether the controller is up, which is `HCI_UP` in its flags.
        pub powered: bool,
    }

    // The ioctl numbers are built rather than written down. `_IOR(type, nr,
    // size)` packs a direction, a size, a type letter and a number into one
    // word; hardcoding the result would be four transcription errors waiting
    // to happen, and the layout is the same on every architecture this crate
    // targets (all of them use asm-generic's `ioctl.h`).
    const IOC_READ: u32 = 2;
    const fn ior(kind: u8, number: u8, size: u32) -> u32 {
        (IOC_READ << 30) | (size << 16) | ((kind as u32) << 8) | number as u32
    }
    /// `HCIGETDEVLIST` — `_IOR('H', 210, int)`.
    const HCIGETDEVLIST: u32 = ior(b'H', 210, 4);
    /// `HCIGETDEVINFO` — `_IOR('H', 211, int)`.
    const HCIGETDEVINFO: u32 = ior(b'H', 211, 4);

    /// `HCI_UP` is the first entry of the device-flags enum, so bit 0.
    const HCI_UP: u32 = 1 << 0;

    /// `struct hci_dev_info` is 92 bytes; the buffer is rounded up because the
    /// kernel writes exactly `sizeof(di)` and a longer one costs nothing.
    const DEV_INFO_LEN: usize = 128;
    /// Offsets into `struct hci_dev_info`, from `include/net/bluetooth/hci_sock.h`.
    const DEV_INFO_NAME: usize = 2;
    const DEV_INFO_BDADDR: usize = 10;
    const DEV_INFO_FLAGS: usize = 16;

    /// Every controller the kernel knows about.
    ///
    /// This is what `hciconfig` prints. It needs no elevated privilege — the
    /// ioctls are on an ordinary HCI socket, and reading the list is not a
    /// privileged operation the way opening a raw controller is.
    pub fn devices() -> Result<Vec<DeviceInfo>, String> {
        // A control socket, only to have a descriptor to ioctl on. Which
        // controller it is bound to does not matter and it is never bound.
        let fd = unsafe { socket(AF_BLUETOOTH, SOCK_RAW, BTPROTO_HCI) };
        if fd < 0 {
            return Err(format!(
                "could not open an HCI control socket (errno {})",
                errno()
            ));
        }
        struct Fd(c_int);
        impl Drop for Fd {
            fn drop(&mut self) {
                unsafe { close(self.0) };
            }
        }
        let fd = Fd(fd);

        // `struct hci_dev_list_req` is a count followed by that many
        // `struct hci_dev_req`, which the kernel pads to 8 bytes each. The
        // request array starts at offset 4, not 2, because `dev_opt` is a
        // `__u32` and so the struct is 4-aligned.
        const MAX: u16 = 16;
        const REQ: usize = 8;
        let mut buffer = vec![0u8; 4 + REQ * MAX as usize];
        buffer[0..2].copy_from_slice(&MAX.to_ne_bytes());
        let rc = unsafe { ioctl(fd.0, HCIGETDEVLIST as _, buffer.as_mut_ptr()) };
        if rc < 0 {
            return Err(format!("HCIGETDEVLIST failed (errno {})", errno()));
        }

        let count = u16::from_ne_bytes([buffer[0], buffer[1]]).min(MAX);
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let at = 4 + i * REQ;
            let id = u16::from_ne_bytes([buffer[at], buffer[at + 1]]);
            if let Some(info) = device_info(fd.0, id) {
                out.push(info);
            }
        }
        out.sort_by_key(|d| d.id);
        Ok(out)
    }

    /// `HCIGETDEVINFO` for one controller, or `None` if it vanished between
    /// being listed and being asked about.
    fn device_info(fd: c_int, id: u16) -> Option<DeviceInfo> {
        let mut buffer = vec![0u8; DEV_INFO_LEN];
        buffer[0..2].copy_from_slice(&id.to_ne_bytes());
        let rc = unsafe { ioctl(fd, HCIGETDEVINFO as _, buffer.as_mut_ptr()) };
        if rc < 0 {
            return None;
        }

        let name = &buffer[DEV_INFO_NAME..DEV_INFO_NAME + 8];
        let name = name.iter().position(|b| *b == 0).unwrap_or(name.len());
        let name =
            String::from_utf8_lossy(&buffer[DEV_INFO_NAME..DEV_INFO_NAME + name]).into_owned();

        let flags = u32::from_ne_bytes([
            buffer[DEV_INFO_FLAGS],
            buffer[DEV_INFO_FLAGS + 1],
            buffer[DEV_INFO_FLAGS + 2],
            buffer[DEV_INFO_FLAGS + 3],
        ]);

        Some(DeviceInfo {
            id,
            name: if name.is_empty() {
                format!("hci{id}")
            } else {
                name
            },
            address: format_address(&buffer[DEV_INFO_BDADDR..DEV_INFO_BDADDR + 6]),
            powered: flags & HCI_UP != 0,
        })
    }

    /// A `bdaddr_t` is little-endian on the wire and written big-endian, so
    /// the bytes are reversed before formatting — the same convention the
    /// advertisement parser uses.
    fn format_address(bytes: &[u8]) -> String {
        bytes
            .iter()
            .rev()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    impl HciSocket {
        /// Open controller `device` — 0 for `hci0`.
        pub fn open(device: u16) -> Result<Self, String> {
            let fd = unsafe { socket(AF_BLUETOOTH, SOCK_RAW, BTPROTO_HCI) };
            if fd < 0 {
                return Err(format!(
                    "could not open an HCI socket (errno {}); this needs CAP_NET_RAW",
                    errno()
                ));
            }
            let socket = Self {
                fd,
                scanning: AtomicBool::new(false),
            };

            // Take only HCI events, and within them only LE Meta.
            let filter = HciFilter {
                type_mask: 1 << HCI_EVENT_PKT,
                event_mask: [0, 1 << (EVT_LE_META - 32)],
                opcode: 0,
            };
            let rc = unsafe {
                setsockopt(
                    fd,
                    SOL_HCI,
                    HCI_FILTER,
                    &filter,
                    std::mem::size_of::<HciFilter>() as u32,
                )
            };
            if rc < 0 {
                return Err(format!("could not set the HCI filter (errno {})", errno()));
            }

            let addr = SockAddrHci {
                family: AF_BLUETOOTH as u16,
                dev: device,
                channel: HCI_CHANNEL_RAW,
            };
            let len = std::mem::size_of::<SockAddrHci>() as u32;
            let rc = unsafe { bind(fd, (&raw const addr).cast(), len) };
            if rc < 0 {
                return Err(format!(
                    "could not bind hci{device} (errno {}); is the adapter present and up?",
                    errno()
                ));
            }
            Ok(socket)
        }

        fn command(&self, ogf: u16, ocf: u16, params: &[u8]) -> Result<(), String> {
            // HCI command packet: 0x01, opcode (le16), length, parameters.
            let opcode = (ogf << 10) | ocf;
            let mut packet = vec![0x01];
            packet.extend_from_slice(&opcode.to_le_bytes());
            packet.push(params.len() as u8);
            packet.extend_from_slice(params);

            let n = unsafe { write(self.fd, packet.as_ptr(), packet.len()) };
            if n as usize != packet.len() {
                return Err(format!(
                    "HCI command 0x{opcode:04x} failed (errno {})",
                    errno()
                ));
            }
            Ok(())
        }

        /// Passive scan, 10 ms window every 10 ms — continuous coverage.
        pub fn set_scan_parameters(&self) -> Result<(), String> {
            // type=0 passive, interval=0x0010, window=0x0010, own addr public,
            // filter policy accept all.
            self.command(
                OGF_LE_CTL,
                OCF_LE_SET_SCAN_PARAMETERS,
                &[0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00],
            )
        }

        pub fn set_scan_enable(&self, enable: bool) -> Result<(), String> {
            // enable, filter_duplicates = 0 so RSSI keeps updating.
            self.command(
                OGF_LE_CTL,
                OCF_LE_SET_SCAN_ENABLE,
                &[u8::from(enable), 0x00],
            )?;
            self.scanning.store(enable, Ordering::Release);
            Ok(())
        }

        /// Scan until `stop` is set, reporting each advertisement.
        pub fn scan(
            &self,
            stop: Arc<AtomicBool>,
            mut on_advertisement: impl FnMut(Advertisement),
        ) -> Result<(), String> {
            self.set_scan_parameters()?;
            self.set_scan_enable(true)?;

            let mut buf = [0u8; 260];
            while !stop.load(Ordering::Acquire) {
                let n = unsafe { read(self.fd, buf.as_mut_ptr(), buf.len()) };
                if n < 0 {
                    if errno() == EINTR {
                        continue; // EINTR
                    }
                    break;
                }
                let packet = &buf[..n as usize];
                // 0x04 event, event code, length, body.
                if packet.len() < 4 || packet[0] != HCI_EVENT_PKT || packet[1] != EVT_LE_META {
                    continue;
                }
                let body = &packet[3..];
                let advertisements = match body.first() {
                    Some(&SUBEVT_LE_ADVERTISING_REPORT) => parse_advertising_report(&body[1..]),
                    Some(&SUBEVT_LE_EXTENDED_ADVERTISING_REPORT) => {
                        parse_extended_advertising_report(&body[1..])
                    }
                    _ => continue,
                };
                for advertisement in advertisements {
                    on_advertisement(advertisement);
                }
            }
            let _ = self.set_scan_enable(false);
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
pub use socket::{devices, DeviceInfo, HciSocket};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_printed_in_written_order() {
        // The wire carries least-significant first.
        assert_eq!(
            format_address(&[0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA]),
            "AA:BB:CC:DD:EE:FF"
        );
    }

    #[test]
    fn advertising_data_records_parse() {
        let data = [
            0x02, 0x01, 0x06, // flags: general discoverable
            0x03, 0x03, 0x0D, 0x18, // complete 16-bit services: 0x180D
            0x09, 0x09, b'P', b'o', b'l', b'a', b'r', b' ', b'H', b'1', // complete name
            0x02, 0x0A, 0xF4, // tx power -12
        ];
        let mut advertisement = Advertisement::default();
        parse_advertising_data(&data, &mut advertisement);

        assert_eq!(advertisement.local_name.as_deref(), Some("Polar H1"));
        assert_eq!(
            advertisement.service_uuids,
            vec!["0000180d-0000-1000-8000-00805f9b34fb"]
        );
        assert_eq!(advertisement.tx_power, Some(-12));
        assert!(advertisement.connectable);
    }

    #[test]
    fn a_complete_name_wins_over_a_shortened_one() {
        let data = [
            0x03, 0x08, b'P', b'o', // shortened "Po"
            0x06, 0x09, b'P', b'o', b'l', b'a', b'r', // complete "Polar"
        ];
        let mut a = Advertisement::default();
        parse_advertising_data(&data, &mut a);
        assert_eq!(a.local_name.as_deref(), Some("Polar"));
    }

    #[test]
    fn manufacturer_and_service_data_split_their_identifiers() {
        let data = [
            0x05, 0xFF, 0x4C, 0x00, 0x02, 0x15, // Apple, payload 02 15
            0x05, 0x16, 0x0F, 0x18, 0x5B, 0x01, // battery service data
        ];
        let mut a = Advertisement::default();
        parse_advertising_data(&data, &mut a);
        assert_eq!(a.manufacturer_data, Some((0x004C, vec![0x02, 0x15])));
        assert_eq!(a.service_data.len(), 1);
        assert_eq!(a.service_data[0].0, "0000180f-0000-1000-8000-00805f9b34fb");
        assert_eq!(a.service_data[0].1, vec![0x5B, 0x01]);
    }

    #[test]
    fn a_128_bit_service_uuid_is_reversed() {
        let mut data = vec![0x11, 0x07];
        // 6e400001-… on the wire, least-significant first.
        data.extend_from_slice(&[
            0x9E, 0xCA, 0xDC, 0x24, 0x0E, 0xE5, 0xA9, 0xE0, 0x93, 0xF3, 0xA3, 0xB5, 0x01, 0x00,
            0x40, 0x6E,
        ]);
        let mut a = Advertisement::default();
        parse_advertising_data(&data, &mut a);
        assert_eq!(
            a.service_uuids,
            vec!["6e400001-b5a3-f393-e0a9-e50e24dcca9e"]
        );
    }

    #[test]
    fn a_zero_length_record_terminates_the_payload() {
        // Padding after the records must not be decoded as more records.
        let data = [0x02, 0x01, 0x06, 0x00, 0xFF, 0xFF, 0xFF];
        let mut a = Advertisement::default();
        parse_advertising_data(&data, &mut a);
        assert!(
            a.manufacturer_data.is_none(),
            "padding was decoded as a record"
        );
    }

    #[test]
    fn a_record_longer_than_the_payload_is_dropped() {
        // Reachable from any broken or hostile peer.
        let data = [0x20, 0x09, b'x'];
        let mut a = Advertisement::default();
        parse_advertising_data(&data, &mut a);
        assert_eq!(a.local_name, None);
    }

    #[test]
    fn an_advertising_report_decodes() {
        let mut body = vec![
            0x01, // one report
            0x00, // ADV_IND — connectable
            0x00, // public address
            0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA, // address, wire order
            0x05, // data length
            0x02, 0x01, 0x06, 0x00, 0x00,
        ];
        body.push(0xC5); // RSSI -59
        let reports = parse_advertising_report(&body);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].address, "AA:BB:CC:DD:EE:FF");
        assert_eq!(reports[0].rssi, -59);
        assert!(reports[0].connectable);
        assert!(!reports[0].random_address);
    }

    #[test]
    fn a_truncated_report_stops_rather_than_over_reading() {
        // Claims two reports but carries one.
        let body = [0x02, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 0x00, 0xC5];
        let reports = parse_advertising_report(&body);
        assert_eq!(reports.len(), 1);
        assert!(parse_advertising_report(&[]).is_empty());
        assert!(parse_advertising_report(&[0x01, 0x00]).is_empty());
    }

    /// The extended report is a different layout, not a longer one: the data
    /// length sits at the end of a 24-byte fixed part, and connectability is a
    /// property bit rather than a PDU-type comparison.
    #[test]
    fn extended_advertising_reports_parse() {
        let mut body = vec![0x01]; // one report
        body.extend_from_slice(&0x0013u16.to_le_bytes()); // connectable|scannable|legacy
        body.push(0x00); // public address
        body.extend_from_slice(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06]); // address, LSB first
        body.push(0x01); // primary phy
        body.push(0x01); // secondary phy
        body.push(0x00); // sid
        body.push(0xF4u8); // tx power -12
        body.push(0xC0u8); // rssi -64
        body.extend_from_slice(&0u16.to_le_bytes()); // periodic interval
        body.push(0x00); // direct address type
        body.extend_from_slice(&[0u8; 6]); // direct address
        let data = [
            0x03, 0x03, 0x0D, 0x18, // complete 16-bit services: 0x180D
            0x06, 0x09, b'P', b'o', b'l', b'a', b'r', // complete name
        ];
        body.push(data.len() as u8);
        body.extend_from_slice(&data);

        let reports = parse_extended_advertising_report(&body);
        assert_eq!(reports.len(), 1);
        let a = &reports[0];
        assert_eq!(a.address, "06:05:04:03:02:01");
        assert_eq!(a.rssi, -64);
        assert_eq!(a.tx_power, Some(-12));
        assert!(a.connectable);
        assert_eq!(a.local_name.as_deref(), Some("Polar"));
        assert_eq!(
            a.service_uuids,
            vec!["0000180d-0000-1000-8000-00805f9b34fb"]
        );
    }

    #[test]
    fn a_truncated_extended_report_is_dropped_not_guessed() {
        // One report promised, fixed part cut short.
        assert!(parse_extended_advertising_report(&[0x01, 0x13, 0x00, 0x00]).is_empty());
        assert!(parse_extended_advertising_report(&[]).is_empty());
    }

    #[test]
    fn a_non_connectable_extended_report_says_so() {
        let mut body = vec![0x01];
        body.extend_from_slice(&0x0012u16.to_le_bytes()); // scannable, not connectable
        body.push(0x01); // random address
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(&[0x01, 0x01, 0x00, 127, 0xC0]);
        body.extend_from_slice(&0u16.to_le_bytes());
        body.push(0x00);
        body.extend_from_slice(&[0u8; 6]);
        body.push(0);
        let reports = parse_extended_advertising_report(&body);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].connectable);
        assert!(reports[0].random_address);
        // 127 means "unavailable" and must not be reported as a real reading.
        assert_eq!(reports[0].tx_power, None);
    }
}
