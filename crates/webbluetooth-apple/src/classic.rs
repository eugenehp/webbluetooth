//! Bluetooth Classic devices through IOBluetooth — macOS only, behind the
//! `apple-classic` feature.
//!
//! CoreBluetooth is LE-only and always will be; Bluetooth Classic on a Mac is
//! a different framework, IOBluetooth, with a different idea of what a device
//! is. CoreBluetooth hides the address behind a per-host UUID and IOBluetooth
//! knows nothing about that UUID, so there is no way to get from a
//! `CBPeripheral` to its `IOBluetoothDevice` except through something both
//! frameworks report. [`Target`](crate::classic::Target) is those two things: an address, or — for a
//! dual-mode device found over LE — its name.
//!
//! This module finds devices. What is opened on them lives beside it:
//! [`crate::rfcomm`], behind the `rfcomm` feature.

use crate::objc::{self, Id, Retained};

pub use webbluetooth_core::classic::ClassicDevice;

/// Which Bluetooth Classic device to open a channel on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A Bluetooth address: `AA:BB:CC:DD:EE:FF`, or with `-` as IOBluetooth
    /// itself writes it.
    Address(String),
    /// The name of a paired or recently used device, matched exactly.
    ///
    /// The only bridge from a CoreBluetooth peripheral, whose address is
    /// hidden, to the same physical device seen through IOBluetooth.
    Name(String),
}

/// Six bytes from `AA:BB:CC:DD:EE:FF` or `aa-bb-cc-dd-ee-ff`.
pub(crate) fn parse_address(address: &str) -> Option<[u8; 6]> {
    let mut bytes = [0u8; 6];
    let mut parts = address.split([':', '-']);
    for byte in &mut bytes {
        let part = parts.next()?;
        if part.len() != 2 {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    parts.next().is_none().then_some(bytes)
}

/// The form the rest of this workspace uses: `AA:BB:CC:DD:EE:FF`.
pub(crate) fn format_address(bytes: &[u8; 6]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

// ── IOBluetoothDevice ───────────────────────────────────────────────────────

pub(crate) fn device_class() -> Result<Id, String> {
    let class = objc::class(c"IOBluetoothDevice");
    if class.is_null() {
        return Err("IOBluetooth is not loaded in this process".into());
    }
    Ok(class)
}

/// # Safety
/// `device` must be an `IOBluetoothDevice`.
unsafe fn device_name(device: Id) -> Option<String> {
    unsafe { objc::to_string(crate::msg_send![device, name]) }
}

/// # Safety
/// `device` must be an `IOBluetoothDevice`.
pub(crate) unsafe fn device_address(device: Id) -> Option<String> {
    let raw = unsafe { objc::to_string(crate::msg_send![device, addressString]) }?;
    parse_address(&raw).map(|bytes| format_address(&bytes))
}

// Only RFCOMM opens channels on a device today; listing does not need this.
#[cfg_attr(not(feature = "rfcomm"), allow(dead_code))]
/// The devices IOBluetooth knows without an inquiry: everything paired, then
/// whatever was used recently.
///
/// # Safety
/// Must run inside an autorelease pool.
unsafe fn known_devices(class: Id) -> Vec<Retained> {
    unsafe {
        let mut devices = Vec::new();
        let paired: Id = crate::msg_send![class, pairedDevices];
        devices.extend(objc::array_map(paired, |d| Retained::retain(d)));
        let recent: Id = crate::msg_send![class, recentDevices: 10usize];
        devices.extend(objc::array_map(recent, |d| Retained::retain(d)));
        devices.into_iter().flatten().collect()
    }
}

// Only RFCOMM opens channels on a device today; listing does not need this.
#[cfg_attr(not(feature = "rfcomm"), allow(dead_code))]
/// # Safety
/// Must run inside an autorelease pool.
pub(crate) unsafe fn find_device(target: &Target) -> Result<Retained, String> {
    let class = device_class()?;
    match target {
        Target::Address(address) => {
            let bytes = parse_address(address)
                .ok_or_else(|| format!("{address:?} is not a Bluetooth address"))?;
            // IOBluetooth writes addresses with dashes and reads them back the
            // same way.
            let dashed = format_address(&bytes).replace(':', "-");
            unsafe {
                let string = Retained::adopt(objc::nsstring(&dashed))
                    .ok_or_else(|| "could not build an address string".to_owned())?;
                let device: Id = crate::msg_send![class, deviceWithAddressString: string.as_ptr()];
                Retained::retain(device)
                    .ok_or_else(|| format!("IOBluetooth knows no device at {address}"))
            }
        }
        Target::Name(name) => unsafe { known_devices(class) }
            .into_iter()
            .find(|device| unsafe { device_name(device.as_ptr()) }.as_deref() == Some(name))
            .ok_or_else(|| {
                format!(
                    "no paired or recently used Bluetooth Classic device is named {name:?}; \
                     pair it in System Settings first"
                )
            }),
    }
}

/// Every paired Bluetooth Classic device.
///
/// Answered from what the system already knows: no inquiry is run, so this
/// returns at once and lists devices whether or not they are in range.
pub fn paired_devices() -> webbluetooth_core::Result<Vec<ClassicDevice>> {
    let _pool = objc::AutoreleasePool::new();
    let class = device_class().map_err(webbluetooth_core::Error::NotSupported)?;
    Ok(unsafe {
        let paired: Id = crate::msg_send![class, pairedDevices];
        objc::array_map(paired, |device| ClassicDevice {
            address: device_address(device).and_then(|a| a.parse().ok()),
            name: device_name(device),
            class_of_device: None,
            bonded: true,
            authenticated: false,
            connected: crate::msg_send_t![bool; device, isConnected],
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_read_in_either_spelling() {
        let bytes = [0xAA, 0xBB, 0xCC, 0x0D, 0xEE, 0xFF];
        assert_eq!(parse_address("AA:BB:CC:0D:EE:FF"), Some(bytes));
        // The form `addressString` returns.
        assert_eq!(parse_address("aa-bb-cc-0d-ee-ff"), Some(bytes));
        assert_eq!(format_address(&bytes), "AA:BB:CC:0D:EE:FF");

        assert_eq!(parse_address("AA:BB:CC:DD:EE"), None);
        assert_eq!(parse_address("AA:BB:CC:DD:EE:FF:00"), None);
        assert_eq!(parse_address("AA:BB:CC:DD:EE:GG"), None);
        assert_eq!(parse_address("A:BB:CC:DD:EE:FF"), None);
        // A CoreBluetooth identifier, which is what a caller is most likely to
        // pass by mistake.
        assert_eq!(parse_address("6E3C8F1A-2B4D-4E5F-8A9B-0C1D2E3F4A5B"), None);
    }

    /// The framework is linked by `build.rs`; if it were not, every lookup
    /// here would be null and fail at run time instead.
    #[test]
    fn iobluetooth_is_linked() {
        assert!(device_class().is_ok());
    }

    /// Listing asks nothing of the radio or of TCC, so it runs anywhere
    /// IOBluetooth does.
    #[test]
    fn paired_devices_can_be_listed() {
        assert!(paired_devices().is_ok());
    }
}
