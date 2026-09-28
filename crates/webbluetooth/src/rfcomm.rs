//! Bluetooth Classic RFCOMM channels.

#[cfg(target_os = "linux")]
pub use webbluetooth_linux::rfcomm::RfcommChannel;
#[cfg(target_os = "linux")]
pub use webbluetooth_linux::rfcomm::RfcommListener;

#[cfg(target_os = "linux")]
pub(crate) fn open(address: &str, channel: u8) -> crate::Result<RfcommChannel> {
    webbluetooth_linux::rfcomm::open(address, channel)
}

#[cfg(target_os = "linux")]
pub(crate) fn listen(channel: u8) -> crate::Result<RfcommListener> {
    webbluetooth_linux::rfcomm::listen(channel)
}

#[cfg(target_os = "linux")]
pub(crate) fn open_with_security(
    address: &str,
    channel: u8,
    security: webbluetooth_core::classic::ClassicSecurity,
) -> crate::Result<RfcommChannel> {
    webbluetooth_linux::rfcomm::open_with_security(address, channel, security)
}

#[cfg(target_os = "android")]
pub use webbluetooth_android::rfcomm::RfcommChannel;
#[cfg(target_os = "android")]
pub use webbluetooth_android::rfcomm::RfcommListener;

#[cfg(target_os = "android")]
pub(crate) fn open_android(
    target: webbluetooth_android::jni::JObject,
    peer: String,
) -> crate::Result<RfcommChannel> {
    webbluetooth_android::rfcomm::from_socket(target, peer)
}

#[cfg(target_os = "android")]
pub(crate) fn listen_android(
    bluetooth: &crate::Bluetooth,
    name: &str,
    service_uuid: &str,
) -> crate::Result<RfcommListener> {
    bluetooth.inner.backend().listen_rfcomm(name, service_uuid)
}

#[cfg(target_os = "windows")]
pub use webbluetooth_windows::classic::RfcommChannel;
#[cfg(target_os = "windows")]
pub use webbluetooth_windows::classic::RfcommListener;

#[cfg(target_os = "windows")]
pub(crate) fn open_windows(address: &str, channel: u8) -> crate::Result<RfcommChannel> {
    webbluetooth_windows::classic::open(address, channel)
}

#[cfg(target_os = "windows")]
pub(crate) fn open_windows_service(
    address: &str,
    service_uuid: &str,
) -> crate::Result<RfcommChannel> {
    webbluetooth_windows::classic::open_service(address, service_uuid)
}

#[cfg(target_os = "windows")]
pub(crate) fn listen_windows(channel: u8) -> crate::Result<RfcommListener> {
    webbluetooth_windows::classic::listen(channel)
}
