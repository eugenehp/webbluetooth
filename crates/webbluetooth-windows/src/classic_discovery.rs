//! Windows Bluetooth Classic discovery through the Win32 Bluetooth API.

#![cfg(windows)]

use webbluetooth_core::{BluetoothAddress, ClassOfDevice, ClassicDevice};
use windows_sys::Win32::Devices::Bluetooth::{
    BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice,
    BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS,
};

/// Enumerate remembered, authenticated, connected, and discoverable Classic devices.
pub fn discover() -> Result<Vec<ClassicDevice>, String> {
    let search = BLUETOOTH_DEVICE_SEARCH_PARAMS {
        dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
        fReturnAuthenticated: 1,
        fReturnRemembered: 1,
        fReturnUnknown: 1,
        fReturnConnected: 1,
        fIssueInquiry: 1,
        cTimeoutMultiplier: 2,
        hRadio: std::ptr::null_mut(),
    };
    let mut info = BLUETOOTH_DEVICE_INFO {
        dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let handle = unsafe {
        BluetoothFindFirstDevice(
            (&search as *const BLUETOOTH_DEVICE_SEARCH_PARAMS).cast_mut(),
            &mut info,
        )
    };
    if handle.is_null() {
        return Ok(Vec::new());
    }
    let mut devices = Vec::new();
    loop {
        devices.push(convert(&info));
        if unsafe { BluetoothFindNextDevice(handle, &mut info) } == 0 {
            break;
        }
        info = BLUETOOTH_DEVICE_INFO {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
    }
    unsafe { BluetoothFindDeviceClose(handle) };
    Ok(devices)
}

fn convert(info: &BLUETOOTH_DEVICE_INFO) -> ClassicDevice {
    let address = unsafe { info.Address.Anonymous.ullLong };
    let name_end = info
        .szName
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(info.szName.len());
    let name = String::from_utf16_lossy(&info.szName[..name_end]);
    ClassicDevice {
        address: Some(BluetoothAddress::from_octets(
            address.to_be_bytes()[2..].try_into().unwrap(),
        )),
        name: (!name.is_empty()).then_some(name),
        class_of_device: Some(ClassOfDevice::from_raw(info.ulClassofDevice)),
        bonded: info.fRemembered != 0,
        authenticated: info.fAuthenticated != 0,
        connected: info.fConnected != 0,
    }
}
