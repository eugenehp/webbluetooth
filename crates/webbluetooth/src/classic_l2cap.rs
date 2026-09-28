//! Bluetooth Classic L2CAP facade.

#[cfg(target_os = "linux")]
pub use webbluetooth_linux::l2cap_classic::ClassicL2capChannel;
#[cfg(target_os = "linux")]
pub use webbluetooth_linux::l2cap_classic::ClassicL2capListener;

#[cfg(target_os = "android")]
pub use webbluetooth_android::classic_l2cap::ClassicL2capChannel;
#[cfg(target_os = "android")]
pub use webbluetooth_android::classic_l2cap::ClassicL2capListener;

#[cfg(target_os = "linux")]
pub(crate) fn open(
    address: &str,
    psm: webbluetooth_core::classic::ClassicPsm,
) -> crate::Result<ClassicL2capChannel> {
    webbluetooth_linux::l2cap_classic::open(address, psm)
}

#[cfg(target_os = "linux")]
pub(crate) fn listen(
    psm: webbluetooth_core::classic::ClassicPsm,
) -> crate::Result<ClassicL2capListener> {
    webbluetooth_linux::l2cap_classic::listen(psm)
}

#[cfg(target_os = "linux")]
pub(crate) fn open_with_security(
    address: &str,
    psm: webbluetooth_core::classic::ClassicPsm,
    security: webbluetooth_core::classic::ClassicSecurity,
) -> crate::Result<ClassicL2capChannel> {
    webbluetooth_linux::l2cap_classic::open_with_security(address, psm, security)
}

#[cfg(target_os = "android")]
pub(crate) fn open_android(
    target: webbluetooth_android::jni::JObject,
    psm: webbluetooth_core::classic::ClassicPsm,
    peer: String,
) -> crate::Result<ClassicL2capChannel> {
    webbluetooth_android::classic_l2cap::from_socket(target, psm.get(), peer)
}

#[cfg(target_os = "android")]
pub(crate) fn listen_android(
    bluetooth: &crate::Bluetooth,
    secure: bool,
) -> crate::Result<ClassicL2capListener> {
    bluetooth.inner.backend().listen_classic_l2cap(secure)
}
