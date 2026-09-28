//! Windows Classic L2CAP facade.

#[cfg(target_os = "windows")]
pub use webbluetooth_windows::classic_l2cap::ClassicL2capChannel;
#[cfg(target_os = "windows")]
pub use webbluetooth_windows::classic_l2cap::ClassicL2capListener;

#[cfg(target_os = "windows")]
pub(crate) fn open(
    address: &str,
    psm: webbluetooth_core::classic::ClassicPsm,
) -> crate::Result<ClassicL2capChannel> {
    webbluetooth_windows::classic_l2cap::open(address, psm)
}

#[cfg(target_os = "windows")]
pub(crate) fn listen(
    psm: webbluetooth_core::classic::ClassicPsm,
) -> crate::Result<ClassicL2capListener> {
    webbluetooth_windows::classic_l2cap::listen(psm)
}
