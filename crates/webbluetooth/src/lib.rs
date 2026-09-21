//! # The Web Bluetooth API, in Rust, on macOS and iOS
//!
//! A faithful port of [Web Bluetooth](https://webbluetoothcg.github.io/web-bluetooth/)
//! — `requestDevice`, `gatt.connect()`, `getPrimaryService`, `readValue`,
//! `startNotifications` — over CoreBluetooth. The JavaScript API's promises
//! become `async fn`s and its events become `Stream`s; everything else keeps
//! its name, so a port reads like the original.
//!
//! ```no_run
//! use webbluetooth::prelude::*;
//! use webbluetooth::uuid::{characteristics, services};
//!
//! # async fn example() -> webbluetooth::Result<()> {
//! let bluetooth = Bluetooth::new();
//!
//! let device = bluetooth
//!     .request_device(
//!         RequestDeviceOptions::new()
//!             .filter(DeviceFilter::new().service(services::HEART_RATE)?)
//!             .optional_service(services::BATTERY_SERVICE)?,
//!     )
//!     .await?;
//!
//! println!("connecting to {}", device.name().unwrap_or_else(|| device.id().into()));
//! let gatt = device.gatt();
//! gatt.connect().await?;
//!
//! let service = gatt.get_primary_service(services::HEART_RATE).await?;
//! let measurement = service
//!     .get_characteristic(characteristics::HEART_RATE_MEASUREMENT)
//!     .await?;
//!
//! let mut beats = measurement.start_notifications().await?;
//! while let Some(value) = beats.next().await {
//!     println!("{} bpm", value[1]);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # What is different from a browser
//!
//! **The chooser is yours.** `requestDevice()` is defined around a user gesture
//! and a picker dialog, and that consent step is the reason the API is safe to
//! expose. A library has no chrome, so it is a trait: see [`DeviceChooser`].
//! The default, [`chooser::FirstMatch`], takes the first match automatically —
//! convenient, and a deliberate removal of that consent step. Use
//! `chooser::TerminalChooser` (feature `terminal-chooser`) or your own
//! implementation for anything user-facing.
//!
//! **Everything else about the security model is kept.** The
//! [GATT blocklist](blocklist) is vendored from the Web Bluetooth Community
//! Group registry and enforced; the per-device service allowlist from
//! `filters` + `optional_services` is enforced on every
//! `get_primary_service`; blocklisted attributes are filtered out of discovery
//! rather than reported and refused.
//!
//! **No device addresses.** [`BluetoothDevice::id`] is CoreBluetooth's per-host
//! identifier, not a BD_ADDR — Apple does not expose hardware addresses, and
//! the value differs between machines for the same peripheral.
//!
//! # Authorization
//!
//! Bluetooth is TCC-gated. A process with no `NSBluetoothAlwaysUsageDescription`
//! is reported [`Availability::Unauthorized`] and **is not prompted**, so a bare
//! `cargo run` binary silently sees no adapter. See the README for how to embed
//! an `Info.plist` in a CLI binary and what an app bundle needs.
//!
//! # How it works
//!
//! There is no Swift, no Objective-C source, and no build-time code generation:
//! CoreBluetooth is an Objective-C framework, so `webbluetooth-apple` synthesises
//! the `CBCentralManagerDelegate` class at *run time* with
//! `objc_allocateClassPair`, installing Rust `extern "C"` functions as its
//! method implementations. Callbacks are delivered on a private
//! `dispatch_queue`, so this works in a plain `fn main()` with no `NSRunLoop`.
//!
//! ## License
//! MIT — Copyright © 2025 [Eugene Hauptmann](https://github.com/eugenehp)

// Every public item carries documentation. This is the crate people read, and
// an undocumented method on it is one whose contract lives in somebody's head
// — the same failure the surface tests exist to prevent, one level up.
#![warn(missing_docs)]

// The portable model lives in `webbluetooth-core`, which the backends share.
// Re-exported here under the names it always had, so `webbluetooth::uuid` and
// `webbluetooth::filter` mean what they meant before there was a second crate.
pub use webbluetooth_core::{
    adapter, backlog, blocklist, chooser, error, filter, grants, timer, uuid,
};
// Not public, and not previously: a backend and the GATT tree both need these,
// a caller does not.
use webbluetooth_core::{address, restoration, scan};
/// L2CAP connection-oriented channels.
///
/// Absent on Windows, which exposes no such API. Branch on [`L2CAP`].
#[cfg(l2cap)]
pub mod l2cap;
/// Publishing a GATT server.
///
/// Absent where the platform cannot do it: the CoreBluetooth initialisers this
/// needs are `API_UNAVAILABLE` on watchOS, tvOS and visionOS, and no Linux
/// backend implements it. Branch on [`PERIPHERAL_ROLE`].
#[cfg(peripheral_role)]
pub mod peripheral;

#[cfg(target_os = "android")]
use webbluetooth_android::backend;
/// The platform engine. Exactly one of these compiles.
///
/// All of them provide the same inherent methods, so everything above — the
/// GATT objects, the allowlist, the blocklist, the chooser — is shared
/// verbatim.
/// They are deliberately *not* a trait: only one ever exists in a build, so a
/// trait would buy dynamic dispatch nobody needs.
/// The platform adapter. Exactly one of these is linked.
///
/// Each lives in its platform's own crate, beside the FFI it is written
/// against: `webbluetooth-apple`'s `backend` is CoreBluetooth expressed as the
/// portable model, and so on for the other five. What is left here is the
/// choice between them.
///
/// They are deliberately not a trait: only one exists in a build, so a trait
/// would buy dynamic dispatch nobody needs, and three of these methods take
/// `self: &Arc<Self>`, which a trait cannot declare. What makes "the same
/// surface" checkable instead is `webbluetooth-core`'s `backend_surface` test,
/// which reads all six and compares them — including the five this build is
/// not compiling.
#[cfg(target_vendor = "apple")]
use webbluetooth_apple::backend;
#[cfg(all(target_os = "linux", not(feature = "linux-hci")))]
use webbluetooth_linux::backend;
#[cfg(all(target_os = "linux", feature = "linux-hci"))]
use webbluetooth_linux::backend_hci as backend;
#[cfg(target_arch = "wasm32")]
use webbluetooth_wasm::backend;
#[cfg(target_os = "windows")]
use webbluetooth_windows::backend;
mod device;
mod gatt;
mod session;

use std::sync::Arc;

pub use adapter::{
    AdapterInfo, ConnectionParameters, ConnectionPhy, ConnectionPriority, Pairing, Phy,
};
pub use chooser::{Candidate, Candidates, DeviceChooser};
pub use device::{BluetoothDevice, DisconnectEvents};
pub use error::{Authorization, Availability, Error, Result};
pub use filter::{Advertisement, DataPrefix, DeviceFilter, RequestDeviceOptions};
pub use gatt::{
    CharacteristicProperties, Notifications, RemoteGattCharacteristic, RemoteGattDescriptor,
    RemoteGattServer, RemoteGattService, Tagged, WriteType,
};

/// Streams, and the pieces needed to work with several at once.
///
/// Every event source in this crate is a [`Stream`](futures_core::Stream), and
/// a device with more than one interesting characteristic gives you several —
/// so merging them is not an exotic requirement, it is the ordinary case. The
/// tool for that is `select_all`, which lives in `futures-util`, and requiring
/// a caller to depend on `futures-util` to combine streams this crate handed
/// them is the same gap that left `BoxFuture` unreachable for
/// [`DeviceChooser`] implementors.
///
/// ```no_run
/// # use webbluetooth::stream::{select_all, StreamExt};
/// # async fn example(cs: Vec<webbluetooth::RemoteGattCharacteristic>)
/// # -> webbluetooth::Result<()> {
/// let mut subscriptions = Vec::new();
/// for characteristic in cs {
///     subscriptions.push(characteristic.start_notifications().await?.tagged());
/// }
///
/// let mut values = select_all(subscriptions);
/// while let Some((uuid, value)) = values.next().await {
///     println!("{uuid}: {} bytes", value.len());
/// }
///
/// // And the merged set can still say which subscription dropped anything.
/// for subscription in values.iter() {
///     if subscription.lost() > 0 {
///         println!("{}: lost {}", subscription.uuid(), subscription.lost());
///     }
/// }
/// # Ok(()) }
/// ```
pub mod stream {
    pub use futures_core::Stream;
    pub use futures_util::stream::{select_all, BoxStream, SelectAll};
    pub use futures_util::StreamExt;
}

/// Futures, and the combinators these APIs are used with.
///
/// The same reasoning as [`stream`]: "stop when the device disconnects" is the
/// correct shape for almost every read loop here, and it is written with
/// `select` over the notification stream and
/// [`watch_disconnect`](BluetoothDevice::watch_disconnect). Every example in
/// this repository had to reach into `futures-util` for that, which is a
/// dependency a caller should not need in order to use two things this crate
/// handed them.
///
/// ```no_run
/// # use webbluetooth::future::{select, Either};
/// # use webbluetooth::stream::StreamExt;
/// # async fn example(
/// #     device: webbluetooth::BluetoothDevice,
/// #     characteristic: webbluetooth::RemoteGattCharacteristic,
/// # ) -> webbluetooth::Result<()> {
/// let mut values = characteristic.start_notifications().await?;
/// let mut disconnected = device.watch_disconnect();
///
/// loop {
///     let next_value = std::pin::pin!(values.next());
///     let next_drop = std::pin::pin!(disconnected.next());
///     match select(next_value, next_drop).await {
///         Either::Left((Some(value), _)) => println!("{value:02x?}"),
///         // The stream ended, or the link dropped first.
///         Either::Left((None, _)) | Either::Right(_) => break,
///     }
/// }
/// # Ok(()) }
/// ```
pub mod future {
    pub use futures_util::future::{join, select, BoxFuture, Either};
}
#[cfg(l2cap)]
pub use l2cap::{L2capChannel, Psm};
/// What a device has been granted access to.
///
/// Re-exported because it is reachable from this crate's own surface —
/// [`Bluetooth::remembered_grant`] returns one and [`grants::Stored`] holds
/// one — and a type a caller can be handed but cannot name is a type they
/// cannot put in a variable, a struct field or a function signature.
pub use webbluetooth_core::registry::Grant;

/// A PSM. Defined even where channels are not, so signatures stay portable.
#[cfg(not(l2cap))]
pub type Psm = u16;

/// Whether L2CAP channels are available on the platform this was built for.
///
/// `false` only on Windows.
pub const L2CAP: bool = cfg!(l2cap);

/// Starting the Android backend.
///
/// Android has no ambient way to reach a `Context`, so one must be supplied
/// before any Bluetooth call — from `JNI_OnLoad`, or from an Activity.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::backend::init;
    pub use webbluetooth_android::jni::{JObject, Vm};
}
#[cfg(peripheral_role)]
pub use peripheral::Peripheral;
pub use restoration::Restoration;
pub use timer::{sleep, timeout};

/// Everything a program needs in scope, in one import.
///
/// The reason this exists is `StreamExt`. Notifications, advertisements and
/// disconnects are all [`Stream`](futures_core::Stream)s, and a stream has no
/// `.next()` without that
/// trait in scope — which, before this module, meant a caller had to add
/// `futures-util` to their own `Cargo.toml` to use the first example in the
/// README. It is re-exported here so that `webbluetooth` on its own is enough.
///
/// ```no_run
/// use webbluetooth::prelude::*;
///
/// # async fn example(c: webbluetooth::RemoteGattCharacteristic) -> webbluetooth::Result<()> {
/// let mut values = c.start_notifications().await?;
/// while let Some(value) = values.next().await {
///     println!("{value:?}");
/// }
/// # Ok(()) }
/// ```
///
/// [`Error`] and [`Result`] are deliberately **not** here. A glob-imported
/// `Result` alias shadows `std::result::Result`, so one `Result<T, E>`
/// anywhere else in the file stops compiling — which happened to this
/// workspace's own iOS harness within minutes of the first attempt. Name them:
/// `webbluetooth::Result<()>`.
///
/// You still need an executor — this crate does not bring one, so that it
/// works with whichever you already have.
pub mod prelude {
    // Writing a chooser needs all three: the trait, the stream it is handed,
    // and the `BoxFuture` its one method returns. Leaving the last two out
    // meant implementing the crate's own extension point pulled `futures-util`
    // into the caller's Cargo.toml — the same thing the paragraph above says
    // this module exists to prevent.
    pub use crate::chooser::{BoxFuture, Candidate, Candidates, DeviceChooser};
    pub use crate::uuid::{BluetoothUuid, IntoUuid};
    pub use crate::{
        Availability, Bluetooth, BluetoothDevice, DataPrefix, DeviceFilter, Grant, LeScanOptions,
        RequestDeviceOptions,
    };
    pub use crate::{
        RemoteGattCharacteristic, RemoteGattDescriptor, RemoteGattServer, RemoteGattService,
    };
    pub use futures_core::Stream;
    pub use futures_util::StreamExt;
}
pub use uuid::{BluetoothUuid, IntoUuid};

use backend::Inner;

/// The entry point — `navigator.bluetooth`.
///
/// Owns one `CBCentralManager`. Cheap to clone; every clone shares the same
/// adapter, device grants and connections.
#[derive(Clone)]
pub struct Bluetooth {
    inner: Arc<session::Session>,
    chooser: Arc<dyn DeviceChooser>,
}

impl Bluetooth {
    /// Open the default adapter, choosing devices with
    /// [`chooser::FirstMatch`].
    ///
    /// Returns immediately; the adapter settles asynchronously, and every
    /// operation waits for it. Use [`Bluetooth::availability`] to find out
    /// whether Bluetooth is actually usable.
    ///
    /// # Call this once and clone it
    ///
    /// Each call opens a *separate* adapter session: its own
    /// `CBCentralManager`, its own grants, its own connections. A
    /// [`BluetoothDevice`] carries the session it was found through, so a
    /// device discovered by one `Bluetooth` cannot be connected by another —
    /// two `new()` calls in one program is the shape of that bug, and it
    /// looks like a device that simply will not connect.
    ///
    /// A browser has exactly one `navigator.bluetooth` and no way to ask for
    /// a second, which is the model to follow: make one, clone it. Cloning
    /// shares everything; `new()` shares nothing. [`Bluetooth::shared`] is
    /// that model already done.
    ///
    /// ```no_run
    /// # use webbluetooth::Bluetooth;
    /// let bluetooth = Bluetooth::new();
    /// let elsewhere = bluetooth.clone(); // same radio, same grants
    /// # let _ = elsewhere;
    /// ```
    pub fn new() -> Self {
        Self::with_chooser(chooser::FirstMatch::default())
    }

    /// The process-wide adapter session — the analogue of
    /// `navigator.bluetooth`.
    ///
    /// A browser exposes exactly one `Bluetooth` and gives a page no way to
    /// ask for a second. This is that: the first call opens an adapter, every
    /// call after it hands back a clone of the same one, and they all share
    /// the radio, the grants and the connections.
    ///
    /// Prefer it to [`new`](Self::new) unless you have a reason not to.
    /// Devices are only usable through the session that found them, so two
    /// sessions in one program is a bug that presents as a device which
    /// simply will not connect — and reaching for a global is exactly what a
    /// caller ends up writing by hand instead.
    ///
    /// It chooses devices with [`chooser::FirstMatch`], because a shared
    /// session cannot ask each caller which chooser it wanted. For anything
    /// user-facing, build one with [`with_chooser`](Self::with_chooser) and
    /// clone *that* around your program.
    ///
    /// ```no_run
    /// # use webbluetooth::Bluetooth;
    /// // Anywhere in the program, without threading a handle through.
    /// let bluetooth = Bluetooth::shared();
    /// # let _ = bluetooth;
    /// ```
    pub fn shared() -> Self {
        static SHARED: std::sync::OnceLock<Bluetooth> = std::sync::OnceLock::new();
        SHARED.get_or_init(Self::new).clone()
    }

    /// Open the default adapter with a specific chooser.
    pub fn with_chooser(chooser: impl DeviceChooser) -> Self {
        Self {
            inner: session::Session::new(Inner::new(false), None),
            chooser: Arc::new(chooser),
        }
    }

    /// Open the default adapter, keeping grants in a file.
    ///
    /// The specification models permissions as *storage*: `getDevices()` reads
    /// it and `requestDevice` adds to it, which is what lets a page reconnect
    /// to something the user chose last week without asking again. Without
    /// this, grants live for the run and go with it.
    ///
    /// Opting in has a consequence worth being deliberate about: the file is
    /// the permission. A later run — or anything else that can read it —
    /// reaches those devices and those services with no chooser and nobody
    /// present. It is written `0600`, and where you put it is part of the
    /// decision. See [`grants`].
    ///
    /// Losing the file costs a trip through the chooser and nothing else.
    pub fn with_grants(store: grants::GrantStore, chooser: impl DeviceChooser) -> Self {
        Self {
            inner: session::Session::new(Inner::new(false), Some(Arc::new(store))),
            chooser: Arc::new(chooser),
        }
    }

    /// What was granted to a device in an earlier run, if anything.
    ///
    /// Held apart from the registry on purpose. The registry holds *devices* —
    /// each with whatever handle its backend needs to reach one — and a
    /// remembered permission has no handle yet, because nothing has found the
    /// device. The specification draws the same line: storage holds
    /// `allowedDevices`, and a `BluetoothDevice` appears when the device does.
    ///
    /// [`Self::adopt_device`] is what turns one back into a usable device.
    pub fn remembered_grant(&self, id: &str) -> Option<Grant> {
        let store = self.inner.grants.as_ref()?;
        store.load().get(id).map(|stored| stored.grant.clone())
    }

    /// Every device this instance has been granted in any run.
    ///
    /// `navigator.bluetooth.getDevices()` is defined against the permission
    /// store, so with one configured this is the union of what is live now and
    /// what was remembered. Being listed does not mean a device is in range,
    /// connected, or even still exists — the specification says as much.
    pub fn remembered_devices(&self) -> Vec<String> {
        let mut ids = self.inner.granted_devices();
        if let Some(store) = &self.inner.grants {
            for id in store.load().into_keys() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids.sort();
        ids
    }

    /// Open the adapter with state preservation and restoration.
    ///
    /// See [`Restoration`] — and read its platform note before relying on this.
    pub fn with_restoration(restoration: Restoration, chooser: impl DeviceChooser) -> Self {
        Self {
            inner: session::Session::new(
                Inner::with_restoration(
                    false,
                    Some(&restoration.identifier),
                    restoration.allowed_services.into_iter().collect(),
                ),
                None,
            ),
            chooser: Arc::new(chooser),
        }
    }

    /// What the system preserved across a relaunch, if this adapter was created
    /// with [`Bluetooth::with_restoration`] and was in fact restored.
    ///
    /// Waits for the adapter to settle first, because
    /// `centralManager:willRestoreState:` is delivered before the first state
    /// report — checking too early would always see `None`.
    pub async fn restored_session(&self) -> Option<RestoredSession> {
        let _ = self.inner.settled_state().await;
        let restored = self.inner.restored()?;
        Some(RestoredSession {
            devices: restored
                .device_ids
                .into_iter()
                .map(|id| BluetoothDevice {
                    inner: self.inner.clone(),
                    id,
                })
                .collect(),
            scan_services: restored.scan_services,
            scan_allow_duplicates: restored.scan_allow_duplicates,
        })
    }

    /// Ask the system to offer to turn Bluetooth on if it is off
    /// (`CBCentralManagerOptionShowPowerAlertKey`).
    ///
    /// Off by default, because a browser never raises that dialog.
    pub fn with_power_alert(chooser: impl DeviceChooser) -> Self {
        Self {
            inner: session::Session::new(Inner::new(true), None),
            chooser: Arc::new(chooser),
        }
    }

    /// `navigator.bluetooth.getAvailability()` — is there a usable radio?
    pub async fn get_availability(&self) -> bool {
        self.availability().await.is_ok()
    }

    /// Why Bluetooth is not usable, if it is not.
    ///
    /// Waits for the adapter's first state report, so this is the right thing
    /// to call at startup to distinguish "no radio" from "not yet known".
    /// # Example
    ///
    /// ```no_run
    /// # use webbluetooth::{Availability, Bluetooth};
    /// # async fn example() {
    /// match Bluetooth::new().availability().await {
    ///     Ok(()) => println!("usable"),
    ///     Err(Availability::PoweredOff) => println!("turn Bluetooth on"),
    ///     Err(Availability::Unauthorized) => println!("this process may not use it"),
    ///     Err(why) => println!("unusable: {why:?}"),
    /// }
    /// # }
    /// ```
    pub async fn availability(&self) -> std::result::Result<(), Availability> {
        match self.inner.require_powered_on().await {
            Ok(()) => Ok(()),
            Err(Error::NotAvailable(a)) => Err(a),
            Err(_) => Err(Availability::Unknown),
        }
    }

    // ── Controllers ─────────────────────────────────────────────────────
    //
    // Outside the standard: a browser has one Bluetooth and no way to ask for
    // another. See the [`adapter`] module for what each platform can say.

    /// Every Bluetooth controller this system is managing.
    ///
    /// Platforms that cannot enumerate report exactly one, named
    /// [`adapter::DEFAULT_ADAPTER`], rather than an empty list — so a caller
    /// that iterates gets the same shape everywhere.
    pub async fn adapters(&self) -> Result<Vec<AdapterInfo>> {
        self.inner.adapters().await
    }

    /// The controller this instance is using.
    pub async fn adapter(&self) -> Result<AdapterInfo> {
        self.inner.adapter().await
    }

    /// Use a different controller from here on.
    ///
    /// Takes an [`AdapterInfo::id`]. Fails with `NotFound` if no such
    /// controller exists, and `NotSupported` on a platform that has no way to
    /// choose — rather than silently carrying on with the wrong radio, which
    /// is the failure worth preventing: a two-adapter test rig that quietly
    /// puts both ends on the same controller passes while testing nothing.
    ///
    /// Anything already connected stays on the controller it was connected
    /// through; this changes what later scans and connections use.
    pub async fn select_adapter(&self, id: &str) -> Result<()> {
        self.inner.select_adapter(id).await
    }

    /// Grant access to a device you already know, without scanning or asking.
    ///
    /// Outside the standard, and deliberately so: Web Bluetooth has no such
    /// call because a web page must not be able to reach a device the user did
    /// not pick. This crate is not a web page — a headless bridge with a
    /// device address in its config has nobody to ask — so the capability
    /// exists, under a name that says it is a grant rather than a lookup.
    ///
    /// `id` is what [`BluetoothDevice::id`] would return: a Bluetooth address
    /// everywhere except Apple, where it is the system-assigned UUID. The
    /// grant is taken from `options` exactly as [`Self::request_device`] takes
    /// it, so `optional_services` is what decides which services become
    /// reachable; the filters are not used, since nothing is being matched.
    ///
    /// This does not connect, and does not require the device to be in range
    /// or advertising — call [`BluetoothDevice::gatt`] and connect as usual.
    /// It fails with `NotFound` if the platform cannot resolve the identifier
    /// at all.
    pub async fn adopt_device(&self, id: &str, grant: impl Into<Grant>) -> Result<BluetoothDevice> {
        let mut grant = grant.into();
        grant.validate()?;
        // Anything remembered for this device is merged in, so adopting with
        // a narrow grant does not silently discard a wider one the user
        // already gave — the same rule `insert` follows for a re-grant.
        if let Some(store) = &self.inner.grants {
            if let Some(stored) = store.load().get(id) {
                grant = stored.grant.union(&grant);
            }
        }
        let id = self.inner.backend().adopt_device(id, grant).await?;
        self.inner.persist_grants();
        Ok(BluetoothDevice {
            inner: self.inner.clone(),
            id,
        })
    }

    /// Grant access to a device a scan just reported.
    ///
    /// [`Bluetooth::request_le_scan`] reports advertisements and grants
    /// nothing, by design — it is `requestLEScan`, not `requestDevice`. This
    /// is how a sighting becomes something connectable without going back
    /// through a chooser: the same grant [`Self::adopt_device`] takes, applied
    /// to a [`Candidate`] rather than to an identifier copied out of one.
    ///
    /// Outside the standard for the same reason `adopt_device` is, and subject
    /// to the same caveat: adopting re-resolves the device through the
    /// platform, which can fail for something the scan reported a moment ago.
    /// # Example
    ///
    /// ```no_run
    /// # use futures_util::StreamExt;
    /// # use webbluetooth::{Bluetooth, Grant, LeScanOptions};
    /// # use webbluetooth::uuid::services;
    /// # async fn example() -> webbluetooth::Result<()> {
    /// let bluetooth = Bluetooth::new();
    /// let mut scan = bluetooth
    ///     .request_le_scan(LeScanOptions::accept_all_advertisements())
    ///     .await?;
    ///
    /// while let Some(sighting) = scan.next().await {
    ///     if sighting.name.as_deref() == Some("the one") {
    ///         let grant = Grant::new().service(services::HEART_RATE)?;
    ///         let device = bluetooth.adopt_candidate(&sighting, grant).await?;
    ///         device.gatt().connect().await?;
    ///         break;
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn adopt_candidate(
        &self,
        candidate: &Candidate,
        grant: impl Into<Grant>,
    ) -> Result<BluetoothDevice> {
        self.adopt_device(&candidate.id, grant).await
    }

    /// `navigator.bluetooth.requestDevice()` — scan, choose, and grant.
    ///
    /// Scans with the request's filters, hands matches to this instance's
    /// chooser, and grants access to whichever it returns. The grant covers
    /// exactly the services named in `filters` and `optional_services`.
    /// # Example
    ///
    /// ```no_run
    /// # use webbluetooth::{Bluetooth, DeviceFilter, RequestDeviceOptions};
    /// # use webbluetooth::uuid::services;
    /// # async fn example() -> webbluetooth::Result<()> {
    /// let bluetooth = Bluetooth::new();
    /// let device = bluetooth
    ///     .request_device(
    ///         RequestDeviceOptions::new()
    ///             .filter(DeviceFilter::new().service(services::HEART_RATE)?)
    ///             // Reachable afterwards, but not filtered on.
    ///             .optional_service(services::BATTERY_SERVICE)?,
    ///     )
    ///     .await?;
    ///
    /// println!("{}", device.name().unwrap_or_else(|| "(unnamed)".into()));
    /// # Ok(()) }
    /// ```
    pub async fn request_device(&self, options: RequestDeviceOptions) -> Result<BluetoothDevice> {
        self.request_device_with(options, self.chooser.as_ref())
            .await
    }

    /// As [`Bluetooth::request_device`], with a chooser for this call only.
    pub async fn request_device_with(
        &self,
        options: RequestDeviceOptions,
        chooser: &dyn DeviceChooser,
    ) -> Result<BluetoothDevice> {
        let id = self
            .inner
            .backend()
            .request_device(options, chooser)
            .await?;
        // "Add device to storage", which is where the specification puts it:
        // a grant is a permission, and a permission outlives the call that
        // created it. Both request paths end here, so both record it.
        self.inner.persist_grants();
        Ok(BluetoothDevice {
            inner: self.inner.clone(),
            id,
        })
    }

    /// `navigator.bluetooth.getDevices()` — devices already granted.
    ///
    /// The grant lives as long as this `Bluetooth` does. Unlike a browser,
    /// nothing is persisted across process restarts, so this is empty at
    /// startup.
    /// Watch advertisements without asking for a device — `requestLEScan()`.
    ///
    /// Unlike [`Bluetooth::request_device`] this grants nothing: it reports
    /// advertisements and hands back no [`BluetoothDevice`]. Filters here
    /// choose what is *reported*, and a scan runs until the returned
    /// [`LeScan`] is stopped or dropped.
    ///
    /// The radio scan is shared, so this coexists with a `request_device` in
    /// flight and with any number of [`BluetoothDevice::watch_advertisements`].
    /// # Example
    ///
    /// ```no_run
    /// # use futures_util::StreamExt;
    /// # use webbluetooth::{Bluetooth, LeScanOptions};
    /// # async fn example() -> webbluetooth::Result<()> {
    /// let bluetooth = Bluetooth::new();
    /// let mut scan = bluetooth.request_le_scan(LeScanOptions::accept_all_advertisements()).await?;
    ///
    /// while let Some(sighting) = scan.next().await {
    ///     match sighting.rssi() {
    ///         Some(dbm) => println!("{} at {dbm} dBm", sighting.id),
    ///         None => println!("{}, no signal strength reported", sighting.id),
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn request_le_scan(&self, options: LeScanOptions) -> Result<LeScan> {
        options.validate()?;
        self.inner.require_powered_on().await?;

        let (watcher, stream, restart) = self.inner.scan_hub().add(scan::Want::Scan {
            options: options.as_request(),
            keep_repeated: options.keep_repeated_devices,
        });
        if restart {
            if let Err(e) = self.inner.backend().set_radio_scanning(true) {
                self.inner.scan_hub().remove(watcher);
                return Err(e);
            }
        }
        Ok(LeScan {
            inner: self.inner.clone(),
            watcher,
            stream,
            options,
        })
    }

    /// Watch for availability changing — `Bluetooth.onavailabilitychanged`.
    ///
    /// The first value is the state at the time of the call, so a caller does
    /// not have to ask separately and then race the first change. See
    /// [`AvailabilityEvents`] for why this is polled.
    /// # Example
    ///
    /// ```no_run
    /// # use futures_util::StreamExt;
    /// # use webbluetooth::Bluetooth;
    /// # async fn example() {
    /// let bluetooth = Bluetooth::new();
    /// let mut changes = bluetooth.watch_availability();
    ///
    /// // Each item is the new answer to `availability()`, reported when it
    /// // changes rather than polled by the caller.
    /// while let Some(state) = changes.next().await {
    ///     match state {
    ///         Ok(()) => println!("radio up"),
    ///         Err(why) => println!("radio down: {why:?}"),
    ///     }
    /// }
    /// # }
    /// ```
    pub fn watch_availability(&self) -> AvailabilityEvents {
        const INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);

        let (tx, rx) = futures_channel::mpsc::unbounded();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let inner = self.inner.clone();
        let flag = stop.clone();

        std::thread::Builder::new()
            .name("webbluetooth-availability".into())
            .spawn(move || {
                let mut last: Option<std::result::Result<(), Availability>> = None;
                while !flag.load(std::sync::atomic::Ordering::Acquire) {
                    let now = inner.state().availability();
                    if last.as_ref() != Some(&now) {
                        last = Some(now);
                        if tx.unbounded_send(now).is_err() {
                            return;
                        }
                    }
                    std::thread::sleep(INTERVAL);
                }
            })
            .ok();

        AvailabilityEvents { rx, stop }
    }

    /// The devices this session has been granted — `Bluetooth.getDevices()`.
    ///
    /// A browser answers this from a permission store that outlives the page.
    /// There is no such store here unless you provide one with
    /// [`Bluetooth::with_grants`], so by default this is what the *process*
    /// has been granted: whatever `request_device` has returned so far, minus
    /// anything [`BluetoothDevice::forget`] has revoked.
    /// # Example
    ///
    /// ```no_run
    /// # use webbluetooth::Bluetooth;
    /// # async fn example() -> webbluetooth::Result<()> {
    /// let bluetooth = Bluetooth::new();
    /// for device in bluetooth.get_devices() {
    ///     // Already granted, so this needs no chooser and no user gesture.
    ///     device.gatt().connect().await?;
    /// }
    /// # Ok(()) }
    /// ```
    pub fn get_devices(&self) -> Vec<BluetoothDevice> {
        self.inner
            .granted_devices()
            .into_iter()
            .map(|id| BluetoothDevice {
                inner: self.inner.clone(),
                id,
            })
            .collect()
    }
}

impl Default for Bluetooth {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Bluetooth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bluetooth")
            .field("state", &self.inner.state())
            .field("granted_devices", &self.inner.granted_devices().len())
            .finish()
    }
}

/// What [`Bluetooth::with_restoration`] recovered.
#[derive(Debug)]
pub struct RestoredSession {
    /// Peripherals that were connected or connecting when the process ended.
    /// Already granted the services declared on [`Restoration`].
    pub devices: Vec<BluetoothDevice>,
    /// The services the interrupted scan was filtering on.
    pub scan_services: Vec<BluetoothUuid>,
    /// Whether that scan allowed duplicate advertisements.
    pub scan_allow_duplicates: bool,
}

/// Whether the peripheral role is available on the platform this was built for.
///
/// `true` on macOS, iOS, Android, Windows and Linux. `false` on watchOS, tvOS
/// and visionOS, where CoreBluetooth's `CBPeripheralManager` and
/// mutable-attribute initialisers are `API_UNAVAILABLE`, and on the web, which
/// has no peripheral role at all. The `peripheral` module does not exist where
/// this is `false`, so it is the value to branch on in cross-platform code.
///
/// This said "`false` … on Linux, where no backend implements it" until a
/// backend implemented it; `build.rs` is what decides, and it is the thing to
/// read if this ever disagrees with a build again.
pub const PERIPHERAL_ROLE: bool = cfg!(peripheral_role);

/// Whether this process is permitted to use Bluetooth at all.
///
/// Distinct from [`Bluetooth::availability`], which also reports on the radio.
/// `NotDetermined` here with no prompt appearing means the process has no
/// `NSBluetoothAlwaysUsageDescription`.
pub fn authorization() -> Authorization {
    backend::authorization()
}

// ── Scanning without a grant ────────────────────────────────────────────────

/// What a sighting reports — `BluetoothAdvertisingEvent`.
///
/// The same shape the chooser is offered, because it is the same information:
/// which device, what it called itself, and what the packet contained.
pub type AdvertisementEvent = chooser::Candidate;

/// Options for [`Bluetooth::request_le_scan`] — `BluetoothLEScanOptions`.
///
/// The scanning specification defines this as its own dictionary —
/// `{ filters, keepRepeatedDevices, acceptAllAdvertisements }` — and
/// deliberately not as a `RequestDeviceOptions`: a scan reports
/// advertisements and grants nothing, so it has no `optionalServices` and no
/// exclusion filters. Taking the request type here meant accepting fields
/// that silently did nothing, which is a worse way to say "not applicable"
/// than not having them.
#[derive(Debug, Clone, Default)]
pub struct LeScanOptions {
    filters: Vec<DeviceFilter>,
    keep_repeated_devices: bool,
    accept_all_advertisements: bool,
}

impl LeScanOptions {
    /// An empty scan request. It needs either a filter or
    /// [`accept_all_advertisements`](Self::accept_all_advertisements).
    pub fn new() -> Self {
        Self::default()
    }

    /// Report every advertisement from every device —
    /// `acceptAllAdvertisements`.
    pub fn accept_all_advertisements() -> Self {
        Self {
            accept_all_advertisements: true,
            ..Self::default()
        }
    }

    /// Report only advertisements matching this filter.
    ///
    /// A sighting matching any filter is reported. Mutually exclusive with
    /// [`accept_all_advertisements`](Self::accept_all_advertisements).
    /// # Example
    ///
    /// ```no_run
    /// # use webbluetooth::{DeviceFilter, LeScanOptions};
    /// # fn example() -> webbluetooth::Result<()> {
    /// let options = LeScanOptions::new()
    ///     .filter(DeviceFilter::new().name_prefix("Muse"))
    ///     .keep_repeated_devices(true);
    /// # let _ = options;
    /// # Ok(()) }
    /// ```
    pub fn filter(mut self, filter: DeviceFilter) -> Self {
        self.filters.push(filter);
        self
    }

    /// Add several filters at once.
    pub fn filters(mut self, filters: impl IntoIterator<Item = DeviceFilter>) -> Self {
        self.filters.extend(filters);
        self
    }

    /// Report repeat sightings of a device rather than only the first.
    ///
    /// Off by default, as in the specification. Turn it on to track signal
    /// strength or a value carried in service data, both of which change while
    /// the device identity does not.
    pub fn keep_repeated_devices(mut self, keep: bool) -> Self {
        self.keep_repeated_devices = keep;
        self
    }

    /// Check the combination the way the specification's algorithm does,
    /// before the radio is touched.
    pub fn validate(&self) -> Result<()> {
        match (self.accept_all_advertisements, self.filters.is_empty()) {
            (true, false) => {
                return Err(Error::InvalidModification(
                    "accept_all_advertisements() and filter() are mutually exclusive".into(),
                ))
            }
            (false, true) => {
                return Err(Error::InvalidModification(
                    "provide at least one filter(), or call accept_all_advertisements()".into(),
                ))
            }
            _ => {}
        }
        self.as_request().validate()
    }

    /// The same thing expressed as a request, which is what the scan hub and
    /// the backends match against.
    ///
    /// Private: a scan grants nothing, so handing this out would reintroduce
    /// exactly the `optionalServices`-shaped confusion this type exists to
    /// remove.
    fn as_request(&self) -> RequestDeviceOptions {
        let mut request = RequestDeviceOptions::new();
        if self.accept_all_advertisements {
            request = request.accept_all_devices();
        }
        for filter in &self.filters {
            request = request.filter(filter.clone());
        }
        request
    }
}

/// A running scan — `BluetoothLEScan`.
///
/// Yields [`AdvertisementEvent`]s. The scan stops when this is dropped, so a
/// caller who wants it to keep running has to keep it alive.
pub struct LeScan {
    inner: Arc<crate::session::Session>,
    watcher: u64,
    stream: crate::backlog::Receiver<AdvertisementEvent>,
    options: LeScanOptions,
}

impl LeScan {
    /// Whether this scan is still running — `BluetoothLEScan.active`.
    pub fn is_active(&self) -> bool {
        self.inner.scan_hub().contains(self.watcher)
    }

    /// `keepRepeatedDevices`, as asked for.
    pub fn keep_repeated_devices(&self) -> bool {
        self.options.keep_repeated_devices
    }

    /// `acceptAllAdvertisements`, as asked for.
    pub fn accept_all_advertisements(&self) -> bool {
        self.options.accept_all_advertisements
    }

    /// The filters this scan is reporting against — `BluetoothLEScan.filters`.
    ///
    /// Empty when it accepts all advertisements.
    pub fn filters(&self) -> &[DeviceFilter] {
        &self.options.filters
    }

    /// Stop scanning. Dropping does the same thing.
    pub fn stop(self) {}
}

impl futures_core::Stream for LeScan {
    type Item = AdvertisementEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<AdvertisementEvent>> {
        std::pin::Pin::new(&mut self.stream).poll_next(cx)
    }
}

impl Drop for LeScan {
    fn drop(&mut self) {
        // Stop the radio only if this was the last one interested in it.
        if self.inner.scan_hub().remove(self.watcher) {
            let _ = self.inner.backend().set_radio_scanning(false);
        }
    }
}

impl std::fmt::Debug for LeScan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeScan")
            .field("active", &self.is_active())
            .field("keep_repeated_devices", &self.keep_repeated_devices())
            .field(
                "accept_all_advertisements",
                &self.accept_all_advertisements(),
            )
            .finish()
    }
}

/// Advertisements from one device — what `watchAdvertisements()` starts.
///
/// Yields [`AdvertisementEvent`]s for that device only. Watching stops when
/// this is dropped.
pub struct Advertisements {
    inner: Arc<crate::session::Session>,
    watcher: u64,
    stream: crate::backlog::Receiver<AdvertisementEvent>,
    device_id: String,
}

impl Advertisements {
    /// How many advertisements were dropped because this stream was not being
    /// read fast enough.
    ///
    /// A busy room produces hundreds a second and the radio's callback cannot
    /// be made to wait, so a consumer that falls behind loses the oldest. That
    /// is the right thing to lose — an advertisement describes a moment, and a
    /// stale one is not worth delivering — but it is worth being able to see,
    /// because a number that climbs steadily means the loop reading this is
    /// too slow for the environment it is in.
    pub fn lost(&self) -> u64 {
        self.stream.lost()
    }

    /// `BluetoothDevice.watchingAdvertisements`.
    pub fn is_watching(&self) -> bool {
        self.inner.scan_hub().contains(self.watcher)
    }

    /// The device being watched.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Stop watching. Dropping does the same thing.
    pub fn stop(self) {}
}

impl futures_core::Stream for Advertisements {
    type Item = AdvertisementEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<AdvertisementEvent>> {
        std::pin::Pin::new(&mut self.stream).poll_next(cx)
    }
}

impl Drop for Advertisements {
    fn drop(&mut self) {
        if self.inner.scan_hub().remove(self.watcher) {
            let _ = self.inner.backend().set_radio_scanning(false);
        }
        // On the web the watch belongs to the device, so stopping it is a
        // call rather than a reference count reaching zero. Spawned because
        // `drop` cannot await, and the page will abort the watch whether or
        // not anything is still listening.
        #[cfg(target_arch = "wasm32")]
        {
            let inner = self.inner.clone();
            let id = self.device_id.clone();
            webbluetooth_wasm::spawn(async move {
                let _ = inner.watch_advertisements(&id, false).await;
            });
        }
    }
}

impl std::fmt::Debug for Advertisements {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Advertisements")
            .field("device", &self.device_id)
            .field("watching", &self.is_watching())
            .finish()
    }
}

/// The service set changed — see
/// [`RemoteGattServer::watch_services_changed`](crate::gatt::RemoteGattServer::watch_services_changed).
pub struct ServiceEvents {
    pub(crate) rx: futures_channel::mpsc::UnboundedReceiver<()>,
}

impl futures_core::Stream for ServiceEvents {
    type Item = ();

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<()>> {
        std::pin::Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl std::fmt::Debug for ServiceEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceEvents")
    }
}

// ── Availability changes ────────────────────────────────────────────────────

/// Adapter availability, as it changes — `Bluetooth.onavailabilitychanged`.
///
/// Yields each time the answer to [`Bluetooth::availability`] changes: the
/// radio is switched off, a dongle is unplugged, a permission is revoked.
///
/// **Polled, not pushed.** The platforms disagree about whether they report
/// this at all — CoreBluetooth and BlueZ do, a raw HCI socket does not, and
/// Android needs a `BroadcastReceiver` this crate does not install — so rather
/// than have the event fire promptly on two platforms and never on the others,
/// it is derived by asking. The interval is the latency, not the resolution:
/// a change is reported once, when it is noticed.
pub struct AvailabilityEvents {
    rx: futures_channel::mpsc::UnboundedReceiver<std::result::Result<(), Availability>>,
    /// Dropping the stream stops the poller.
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl futures_core::Stream for AvailabilityEvents {
    type Item = std::result::Result<(), Availability>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl Drop for AvailabilityEvents {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl std::fmt::Debug for AvailabilityEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AvailabilityEvents")
    }
}
