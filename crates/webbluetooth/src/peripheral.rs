//! The peripheral role — publishing a GATT server and advertising it.
//!
//! **This is not part of Web Bluetooth.** The specification covers only the
//! central role: a web page is always the GATT *client*. CoreBluetooth,
//! Android, WinRT and BlueZ all let a device *be* a peripheral, so it is here
//! as a sibling API rather than a port of anything.
//!
//! ```no_run
//! use webbluetooth::peripheral::{Advertising, Characteristic, Peripheral, Request, Service};
//! use webbluetooth::uuid::{characteristics, services};
//! use webbluetooth::stream::StreamExt;
//!
//! # async fn example() -> webbluetooth::Result<()> {
//! let (peripheral, mut requests) = Peripheral::new();
//!
//! let published = peripheral
//!     .publish(Service::new(services::BATTERY_SERVICE)?.characteristic(
//!         Characteristic::new(characteristics::BATTERY_LEVEL)?.read().notify(),
//!     ))
//!     .await?;
//! let level = published.characteristic(characteristics::BATTERY_LEVEL).unwrap();
//!
//! peripheral
//!     .start_advertising(Advertising::new().local_name("Rust").service(services::BATTERY_SERVICE)?)
//!     .await?;
//!
//! while let Some(request) = requests.next().await {
//!     match request {
//!         Request::Read(read) => read.respond(&[87])?,
//!         Request::Write(write) => write.accept(),
//!         Request::Subscribed { .. } => level.notify(&[87]).await?,
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Answer every request
//!
//! A [`crate::peripheral::ReadRequest`] or [`crate::peripheral::WriteRequest`] that is dropped without an answer is
//! answered [`crate::peripheral::AttError::RequestNotSupported`] automatically, because leaving it
//! silent stalls the central until the ATT timeout — but say what you mean.
//!
//! The request stream is single-consumer by construction: a read must be
//! answered exactly once, so it cannot be handed to two listeners.
//!
//! # What each platform accepts differs
//!
//! The definitions in [`crate::peripheral::defs`] are shared; what a platform
//! will publish is not,
//! so validation is per-engine. CoreBluetooth is the stricter of the two:
//!
//! * **A characteristic with a fixed value must be read-only.** Android has no
//!   such rule — a `BluetoothGattCharacteristic` carries a value and its
//!   permissions independently.
//! * **Only user description (`0x2901`) and presentation format (`0x2904`)
//!   descriptors can be created.** Android accepts any descriptor UUID, though
//!   it still manages the Client Characteristic Configuration descriptor
//!   itself.
//!
//! Both rules are checked before anything is published, because on Apple the
//! alternative is an Objective-C exception that aborts the process.
//!
//! # The GATT blocklist does not apply here
//!
//! [`crate::blocklist`] exists to stop a *client* reaching dangerous attributes
//! on someone else's device. Publishing a service of your own is a different
//! act, so nothing is filtered on this side.

/// What a GATT server publishes: services, characteristics, descriptors.
///
/// Lives in `webbluetooth-core` because all four adapters build from it, and
/// because the rules it enforces — a characteristic needs a property, the CCCD
/// is not yours to declare — were written out once per adapter before they
/// were written once here.
pub use webbluetooth_core::peripheral as defs;

#[cfg(target_os = "android")]
use webbluetooth_android::peripheral_backend as imp;
/// The platform engine. Exactly one is linked, and all four present the same
/// API — which `webbluetooth-core`'s `peripheral_surface` test checks by
/// reading all four, since only one of them is ever compiled.
///
/// Each lives in its platform's own crate now, beside the FFI it is written
/// against. What kept them here until recently was [`Request::ChannelOpened`],
/// which carries an [`crate::L2capChannel`]: that type used to be assembled
/// above the backends, so an adapter could not name the type its own public
/// enum contained. It is `webbluetooth_core::l2cap::L2capChannel` now, generic
/// over the platform socket, and the problem went with it.
#[cfg(target_vendor = "apple")]
use webbluetooth_apple::peripheral_backend as imp;
#[cfg(target_os = "linux")]
use webbluetooth_linux::peripheral_backend as imp;
#[cfg(target_os = "windows")]
use webbluetooth_windows::peripheral_backend as imp;

pub use defs::{
    Advertising, AttError, Characteristic, Descriptor, Permissions, RestoredPeripheral, Service,
    Write,
};
pub use imp::{
    Peripheral, PublishedCharacteristic, PublishedService, ReadRequest, RemoteCentral, Request,
    Requests, WriteRequest,
};

/// What a characteristic supports.
pub use crate::gatt::CharacteristicProperties as Properties;
