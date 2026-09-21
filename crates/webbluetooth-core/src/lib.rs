//! The portable half of `webbluetooth`, and the contract its backends meet.
//!
//! Everything here is platform-independent: the error vocabulary, the UUID
//! registry, the filters, the two blocklists, the grant store, the scan hub.
//! None of it knows whether it is talking to CoreBluetooth, BlueZ, WinRT, the
//! Android framework, a raw HCI socket or the browser's own Web Bluetooth.
//!
//! # Why this is a crate and not a module
//!
//! A backend has to reach this code — it converts platform errors into
//! [`Error`], platform UUIDs into [`BluetoothUuid`], and publishes sightings
//! into a [`ScanHub`](scan::ScanHub). While the backends lived in the same
//! crate as the public API, "reach this code" meant `crate::`, and that cost
//! two things.
//!
//! The first is that a backend could reach *anything*, including the public
//! API above it, and three of them did. The second is that anything two
//! backends both needed had nowhere to live: [`RestoredScan`] was written out
//! six times, once per backend, because a shared type would have had to sit in
//! a module that every backend could see and no backend could be sure of.
//!
//! # What checks that the backends agree
//!
//! Only one of them is ever compiled, so five are invisible to any one build.
//! `tests/backend_surface.rs` reads all six as source and compares the methods
//! they declare, on whatever host it runs on. A trait would have been the
//! obvious mechanism and does not fit — three of those methods take
//! `self: &Arc<Self>`, which a trait cannot declare, and a trait would still
//! only check the backend already being compiled.
//!
//! # What is not here
//!
//! The Web Bluetooth objects themselves — `Bluetooth`, `BluetoothDevice`, the
//! GATT tree — are in `webbluetooth`, above this. They are what a caller
//! holds; this is what they are built out of.
//!
//! L2CAP is not here either, and for a different reason: a channel is a
//! platform socket rather than a portable value, so the glue that adapts one
//! lives in `webbluetooth` beside the backend selection. The specification has
//! no L2CAP, so nothing in the portable model is missing it.
//!
//! # Stability
//!
//! This crate exists so `webbluetooth` and the six backend crates can share a
//! vocabulary. Everything it exports is public because a backend in another
//! crate has to name it — not because it is an API to build on. Depend on
//! `webbluetooth`.

// Re-exported so `forward_to_registry!`, expanded in a backend crate, can name
// the channel type without that crate having to depend on futures-channel.
// Everything public here is named by a backend in another crate, which makes
// "what is this for" a question somebody actually has to answer.
#![warn(missing_docs)]

pub use futures_channel;

pub mod adapter;
pub mod address;
pub mod backlog;
pub mod blocklist;
pub mod chooser;
pub mod error;
pub mod filter;
pub mod gatt;
pub mod grants;
pub mod l2cap;
/// The peripheral role's definitions: what a GATT server publishes.
pub mod peripheral;
pub mod registry;
pub mod restoration;
pub mod scan;
pub mod state;
pub mod timer;
pub mod uuid;
/// Conversions to and from the `uuid` crate's `Uuid`. Feature `uuid`.
#[cfg(feature = "uuid")]
mod uuid_interop;

pub use adapter::{
    AdapterInfo, ConnectionParameters, ConnectionPhy, ConnectionPriority, Pairing, Phy,
};
pub use chooser::{Candidate, Candidates, DeviceChooser};
pub use error::{Authorization, Availability, Error, Result};
pub use filter::{Advertisement, DataPrefix, DeviceFilter, RequestDeviceOptions};
pub use gatt::{CharacteristicProperties, WriteType};
pub use l2cap::{ChannelSink, Closed, L2capChannel, PlatformChannel, Psm};
pub use restoration::{Restoration, RestoredScan};
pub use state::ManagerState;
pub use timer::{sleep, timeout};
pub use uuid::{BluetoothUuid, IntoUuid};
