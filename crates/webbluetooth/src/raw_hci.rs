//! Linux raw HCI transport facade.

#[cfg(target_os = "linux")]
pub use webbluetooth_linux::raw_hci::RawHciSocket;

#[cfg(all(target_os = "linux", feature = "raw-acl"))]
pub(crate) fn open_acl(device: u16) -> crate::Result<RawHciSocket> {
    webbluetooth_linux::raw_hci::open_acl(device).map_err(crate::Error::Network)
}

#[cfg(all(target_os = "linux", feature = "raw-sco"))]
pub(crate) fn open_sco(device: u16) -> crate::Result<RawHciSocket> {
    webbluetooth_linux::raw_hci::open_sco(device).map_err(crate::Error::Network)
}

#[cfg(all(target_os = "linux", feature = "le-audio"))]
pub(crate) fn open_iso(device: u16) -> crate::Result<RawHciSocket> {
    webbluetooth_linux::raw_hci::open_iso(device).map_err(crate::Error::Network)
}
