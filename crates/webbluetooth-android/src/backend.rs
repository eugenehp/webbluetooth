//! The Android engine: JNI to the framework, with callback classes generated at
//! run time.
//!
//! Structurally closest to the Apple backend, because Android's GATT is also
//! callback-driven and also refuses to say which request a reply belongs to.
//! The difference is that Android allows **one GATT operation at a time per
//! connection** and simply drops a second, so a single pending slot per device
//! is enough where CoreBluetooth needs a FIFO. The shared per-device lock in
//! [`webbluetooth_core::registry`] is what makes that safe.
//!
//! # Starting it
//!
//! Android has no ambient way to reach a `Context`, so the app must hand one
//! over before any Bluetooth call:
//!
//! ```ignore
//! // From JNI_OnLoad, or an Activity.
//! webbluetooth::android::init(vm, context)?;
//! ```
//!
//! With a null context the crate falls back to
//! `ActivityThread.currentApplication()`, which works inside a normal app
//! process.
//!
//! # Permissions
//!
//! `BLUETOOTH_SCAN` and `BLUETOOTH_CONNECT` on API 31+, `ACCESS_FINE_LOCATION`
//! for scanning on 23–30, and `neverForLocation` on the scan permission if you
//! do not want the location grant. A missing one surfaces as
//! [`Availability::Unauthorized`].

use crate::ble::{self, Callback};
use crate::bluetooth::{Event, EventSink, Ref};
use crate::jni::{JObject, Vm};
use crate::runtime::Runtime;
use futures_channel::oneshot;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};
use webbluetooth_core::chooser::DeviceChooser;
use webbluetooth_core::error::{Availability, Error, Result};
use webbluetooth_core::filter::{Advertisement, RequestDeviceOptions};
use webbluetooth_core::gatt::{CharacteristicProperties, WriteType};
use webbluetooth_core::registry::DeviceRegistry;
use webbluetooth_core::scan::{self, ScanHub};
use webbluetooth_core::state::ManagerState;
use webbluetooth_core::uuid::BluetoothUuid;

/// A handle into a device's attribute table.
///
/// A global reference to the Java object, plus what it resolved as — asking
/// Android again would be another JNI round trip per access.
#[derive(Clone, Debug)]
pub struct Handle(Arc<HandleData>);

#[derive(Debug)]
struct HandleData {
    object: Ref,
    uuid: BluetoothUuid,
    properties: u32,
}

impl Handle {
    fn as_ptr(&self) -> JObject {
        self.0.object.as_ptr()
    }
}

pub fn attribute_uuid(handle: &Handle) -> Result<BluetoothUuid> {
    Ok(handle.0.uuid)
}

#[allow(dead_code)]
#[derive(Debug, Default, Clone)]
pub struct RestoredScan {
    pub device_ids: Vec<String>,
    pub scan_services: Vec<BluetoothUuid>,
    pub scan_allow_duplicates: bool,
}

/// Start the Android backend.
///
/// Call once, before anything else, with a `JavaVM` and an Android `Context`.
/// `context` may be null to fall back to `ActivityThread.currentApplication()`.
pub fn init(vm: Vm, context: JObject) -> Result<()> {
    Runtime::init(vm, context)
        .map(|_| ())
        .map_err(|e| Error::NotSupported(format!("could not start the Android runtime: {e}")))
}

/// What the backend needs to reach a device.
struct DeviceData {
    /// `BluetoothDevice`. The address is the registry key, so it is not
    /// repeated here.
    device: Ref,
    /// `BluetoothGatt`, once connected.
    gatt: Option<Ref>,
    /// Held so the callback object outlives the connection using it.
    callback: Option<Arc<Callback>>,
    mtu: i32,
}

/// The reply to whichever operation is in flight. Android permits one per
/// connection, so these are slots rather than queues.
#[derive(Default)]
struct Pending {
    connect: Vec<oneshot::Sender<Result<()>>>,
    services: Vec<oneshot::Sender<Result<()>>>,
    read: Vec<oneshot::Sender<Result<Vec<u8>>>>,
    write: Vec<oneshot::Sender<Result<()>>>,
    notify: Vec<oneshot::Sender<Result<bool>>>,
    read_descriptor: Vec<oneshot::Sender<Result<Vec<u8>>>>,
    write_descriptor: Vec<oneshot::Sender<Result<()>>>,
    rssi: Vec<oneshot::Sender<Result<i32>>>,
    /// `onPhyUpdate` and `onPhyRead` both land here; only one can be
    /// outstanding, and the difference is only whether a change was asked for.
    phy: Vec<oneshot::Sender<Result<(i32, i32)>>>,
}

impl Pending {
    /// Fail everything outstanding, so a dropped link never leaves a future
    /// hanging or the device's lock held.
    fn fail(&mut self, error: &Error) {
        fn drain<T>(slots: &mut Vec<oneshot::Sender<Result<T>>>, error: &Error) {
            for tx in slots.drain(..) {
                let _ = tx.send(Err(error.clone()));
            }
        }
        drain(&mut self.connect, error);
        drain(&mut self.services, error);
        drain(&mut self.read, error);
        drain(&mut self.write, error);
        drain(&mut self.notify, error);
        drain(&mut self.read_descriptor, error);
        drain(&mut self.write_descriptor, error);
        drain(&mut self.rssi, error);
    }
}

pub struct Inner {
    devices: DeviceRegistry<DeviceData>,
    pending: Mutex<HashMap<String, Pending>>,
    notifications: Mutex<HashMap<usize, Vec<webbluetooth_core::backlog::Sender<Vec<u8>>>>>,
    /// Everyone watching the scan; the scanner is shared and reference-counted.
    hub: ScanHub,
    /// Device address to name for every sighting, so the chooser's answer is
    /// still resolvable after it returns.
    sightings: Mutex<HashMap<String, Option<String>>>,
    scan_callback: Mutex<Option<Callback>>,
}

struct Sink(Weak<Inner>);

impl EventSink for Sink {
    fn emit(&self, event: Event) {
        if let Some(inner) = self.0.upgrade() {
            inner.handle(event);
        }
    }
}

impl Inner {
    pub fn new(show_power_alert: bool) -> Arc<Self> {
        Self::with_restoration(show_power_alert, None, BTreeSet::new())
    }

    /// Android has no state preservation, so the identifier is ignored.
    pub fn with_restoration(
        _show_power_alert: bool,
        _restore_identifier: Option<&str>,
        _restore_allowed: BTreeSet<BluetoothUuid>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|_weak: &Weak<Inner>| {
            // The adapter is deliberately *not* resolved here. Android brings
            // the Bluetooth stack up asynchronously, so a process that starts
            // early sees no adapter for a second or two — and caching that
            // verdict at construction made the engine permanently
            // "unsupported" for the life of the process. It is opened on
            // demand instead; the cost is a handful of JNI calls per
            // operation, against a BLE round trip that takes milliseconds.
            Inner {
                devices: DeviceRegistry::default(),
                pending: Mutex::new(HashMap::new()),
                notifications: Mutex::new(HashMap::new()),
                hub: ScanHub::new(),
                sightings: Mutex::new(HashMap::new()),
                scan_callback: Mutex::new(None),
            }
        })
    }

    fn runtime(&self) -> Result<&'static Runtime> {
        Runtime::get().ok_or_else(|| {
            Error::NotSupported(
                "the Android backend is not started — call webbluetooth::android::init(vm, context)"
                    .into(),
            )
        })
    }

    // ── Events ──────────────────────────────────────────────────────────────

    fn handle(&self, event: Event) {
        match event {
            Event::Discovered {
                address,
                name,
                rssi,
                service_uuids,
                manufacturer_data,
                service_data,
                connectable,
            } => {
                if !self.hub.is_watching() {
                    return;
                }
                let advertisement = Advertisement {
                    local_name: name.clone(),
                    tx_power: None,
                    // ScanRecord has no accessor for it.
                    appearance: None,
                    is_connectable: Some(connectable),
                    service_uuids: service_uuids
                        .iter()
                        .filter_map(|u| BluetoothUuid::parse(u).ok())
                        .collect(),
                    overflow_service_uuids: Vec::new(),
                    solicited_service_uuids: Vec::new(),
                    manufacturer_data: manufacturer_data.into_iter().collect(),
                    service_data: service_data
                        .into_iter()
                        .filter_map(|(u, d)| Some((BluetoothUuid::parse(&u).ok()?, d)))
                        .collect(),
                    rssi,
                };
                self.sightings
                    .lock()
                    .unwrap()
                    .insert(address.clone(), name.clone());
                // Each watcher applies its own filter; this one just reports.
                self.hub.publish(&address, name.as_deref(), &advertisement);
            }

            Event::ScanFailed { code } => {
                // Nothing will arrive, so end every stream rather than hang.
                self.hub.close_all();
                let _ = code;
            }

            Event::ConnectionStateChanged {
                gatt,
                status,
                connected,
            } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                if connected && status == ble::GATT_SUCCESS {
                    self.devices.update(&id, |d| d.connected = true);
                    self.resolve(&id, |p| &mut p.connect, Ok(()));
                } else {
                    let error = Error::Network(format!("the link dropped (GATT status {status})"));
                    self.pending
                        .lock()
                        .unwrap()
                        .entry(id.clone())
                        .or_default()
                        .fail(&error);
                    self.devices.update(&id, |d| d.inner.gatt = None);
                    self.devices.mark_disconnected(&id);
                }
            }

            Event::ServicesDiscovered { gatt, status } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                self.resolve(&id, |p| &mut p.services, gatt_result(status, ()));
            }

            Event::CharacteristicRead {
                gatt,
                value,
                status,
                ..
            } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                self.resolve(&id, |p| &mut p.read, gatt_result(status, value));
            }

            Event::CharacteristicWritten { gatt, status, .. } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                self.resolve(&id, |p| &mut p.write, gatt_result(status, ()));
            }

            Event::CharacteristicChanged {
                characteristic,
                value,
                ..
            } => {
                let key = characteristic.key();
                let mut subscribers = self.notifications.lock().unwrap();
                if let Some(list) = subscribers.get_mut(&key) {
                    list.retain(|tx| tx.send(value.clone()).is_ok());
                    if list.is_empty() {
                        subscribers.remove(&key);
                    }
                }
            }

            Event::DescriptorRead {
                gatt,
                value,
                status,
                ..
            } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                self.resolve(&id, |p| &mut p.read_descriptor, gatt_result(status, value));
            }

            Event::DescriptorWritten { gatt, status, .. } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                // Subscribing finishes when its CCCD write does.
                self.resolve(&id, |p| &mut p.write_descriptor, gatt_result(status, ()));
                self.resolve(&id, |p| &mut p.notify, gatt_result(status, true));
            }

            Event::PhyChanged {
                gatt,
                tx,
                rx,
                status,
            } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                self.resolve(&id, |p| &mut p.phy, gatt_result(status, (tx, rx)));
            }

            Event::MtuChanged { gatt, mtu, .. } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                self.devices.update(&id, |d| d.inner.mtu = mtu);
            }

            Event::RssiRead { gatt, rssi, status } => {
                let Some(id) = self.id_for_gatt(&gatt) else {
                    return;
                };
                self.resolve(&id, |p| &mut p.rssi, gatt_result(status, rssi));
            }

            Event::ServicesChanged { gatt } => {
                // Every handle into this device now points at a stale object.
                if let Some(id) = self.id_for_gatt(&gatt) {
                    self.devices.mark_services_changed(&id);
                }
            }

            // The peripheral role is not wired on Android yet; the callback
            // classes exist but nothing subscribes to them.
            _ => {}
        }
    }

    fn id_for_gatt(&self, gatt: &Ref) -> Option<String> {
        let key = gatt.key();
        self.devices
            .find(|d| d.inner.gatt.as_ref().is_some_and(|g| g.key() == key))
    }

    /// Hand a reply to whoever is waiting for that kind of operation.
    fn resolve<T>(
        &self,
        id: &str,
        slot: impl Fn(&mut Pending) -> &mut Vec<oneshot::Sender<Result<T>>>,
        value: Result<T>,
    ) where
        T: Clone,
    {
        let mut pending = self.pending.lock().unwrap();
        let entry = pending.entry(id.to_owned()).or_default();
        for tx in slot(entry).drain(..) {
            let _ = tx.send(value.clone());
        }
    }

    fn enqueue<T>(
        &self,
        id: &str,
        slot: impl Fn(&mut Pending) -> &mut Vec<oneshot::Sender<Result<T>>>,
    ) -> oneshot::Receiver<Result<T>> {
        let (tx, rx) = oneshot::channel();
        let mut pending = self.pending.lock().unwrap();
        slot(pending.entry(id.to_owned()).or_default()).push(tx);
        rx
    }

    // ── Availability ────────────────────────────────────────────────────────

    /// The `BluetoothAdapter` object, resolved now rather than at
    /// construction. Distinct from [`Inner::adapter`], which describes the
    /// controller to a caller.
    fn ble_adapter(&self) -> Result<ble::Adapter> {
        let runtime = self.runtime()?;
        ble::Adapter::open(runtime).map_err(|e| {
            // Distinguish "this device has no Bluetooth" from "we could not
            // reach it", instead of reporting every failure as the former.
            match e {
                crate::runtime::Error::MissingClass(_) => {
                    Error::NotAvailable(Availability::Unsupported)
                }
                other => Error::Network(format!("could not reach the Bluetooth adapter: {other}")),
            }
        })
    }

    pub fn state(&self) -> ManagerState {
        if Runtime::get().is_none() {
            return ManagerState::Unauthorized;
        }
        let Ok(adapter) = self.ble_adapter() else {
            return ManagerState::Unsupported;
        };
        let Ok(runtime) = self.runtime() else {
            return ManagerState::Unauthorized;
        };
        let Ok(env) = runtime.env() else {
            return ManagerState::Unauthorized;
        };
        if adapter.is_enabled(env) {
            ManagerState::PoweredOn
        } else {
            ManagerState::PoweredOff
        }
    }

    pub async fn settled_state(&self) -> ManagerState {
        self.state()
    }

    pub async fn require_powered_on(&self) -> Result<()> {
        self.state().require_powered_on()
    }

    pub fn restored(&self) -> Option<RestoredScan> {
        None
    }

    // ── Scanning ────────────────────────────────────────────────────────────

    pub fn scan_hub(&self) -> &ScanHub {
        &self.hub
    }

    /// Start or stop the LE scanner.
    ///
    /// Android takes the service filter when the scan starts, so widening it
    /// means stopping and starting again — which is what the hub asks for.
    pub fn set_radio_scanning(self: &Arc<Self>, on: bool) -> Result<()> {
        let runtime = self.runtime()?;
        let env = runtime.env().map_err(jni_error)?;
        let adapter = self.ble_adapter()?;
        let scanner = adapter.scanner(env).map_err(jni_error)?;
        if scanner.is_null() {
            return Err(Error::NotAvailable(Availability::PoweredOff));
        }

        // Always stop first: a running scan keeps the filter it started with.
        if let Some(callback) = self.scan_callback.lock().unwrap().take() {
            let _ = ble::stop_scan(env, scanner, callback.as_ptr());
        }
        if !on {
            return Ok(());
        }

        let scan_services: Vec<String> = self
            .hub
            .scan_services()
            .iter()
            .map(|u| u.as_str().to_owned())
            .collect();

        // One callback object per scan, bound to this engine.
        let callback = Callback::new(
            runtime,
            "dev.webbluetooth.ScanCallback",
            crate::scan_callback_dex(),
            &crate::bluetooth::scan_natives(),
            Arc::new(Sink(Arc::downgrade(self))),
        )
        .map_err(jni_error)?;

        ble::start_scan(env, scanner, callback.as_ptr(), &scan_services)
            .map_err(|e| Error::Network(format!("could not start scanning: {e}")))?;
        *self.scan_callback.lock().unwrap() = Some(callback);
        Ok(())
    }

    pub async fn request_device(
        self: &Arc<Self>,
        options: RequestDeviceOptions,
        chooser: &dyn DeviceChooser,
    ) -> Result<String> {
        options.validate()?;
        self.require_powered_on().await?;
        let allowed = options.grant();
        let id = scan::choose(
            &self.hub,
            options,
            chooser,
            |on| self.set_radio_scanning(on),
            || {},
        )
        .await?;
        let Some(name) = self.sightings.lock().unwrap().get(&id).cloned() else {
            return Err(Error::NotFound(format!(
                "the chooser returned {id:?}, which was not among the devices it was offered"
            )));
        };

        // Acquired *after* the chooser, never before. A `JNIEnv*` belongs to
        // the thread that asked for it — JNI says so — and an executor is free
        // to resume this future somewhere else, which would leave the pointer
        // pointing at another thread's environment. Holding one across an
        // await is also what makes the whole future `!Send`, so the compiler
        // objects before the undefined behaviour can happen.
        let runtime = self.runtime()?;
        let env = runtime.env().map_err(jni_error)?;
        let adapter = self.ble_adapter()?;

        let device = adapter.remote_device(env, &id).map_err(jni_error)?;
        let device =
            Ref::new(env, device).ok_or_else(|| Error::NotFound(format!("no device at {id}")))?;

        self.devices.insert(
            &id,
            name,
            allowed,
            false,
            DeviceData {
                device,
                gatt: None,
                callback: None,
                mtu: 23,
            },
        );
        Ok(id)
    }

    // ── Device access ───────────────────────────────────────────────────────

    // The registry answers these identically for every backend.
    webbluetooth_core::registry::forward_to_registry!();

    // ── PHY ─────────────────────────────────────────────────────────────────

    /// `gatt.readPhy()` — the answer arrives in `onPhyRead`.
    pub async fn phy(&self, id: &str) -> Result<webbluetooth_core::ConnectionPhy> {
        let rx = {
            let runtime = self.runtime()?;
            let env = runtime.env().map_err(jni_error)?;
            let gatt = self.gatt(id)?;
            let rx = self.enqueue(id, |p| &mut p.phy);
            ble::read_phy(env, gatt.as_ptr()).map_err(jni_error)?;
            rx
        };
        let (tx, rx_phy) = rx
            .await
            .map_err(|_| Error::Aborted("reading the PHY was cancelled".into()))??;
        Ok(webbluetooth_core::ConnectionPhy {
            tx: phy_from_android(tx),
            rx: phy_from_android(rx_phy),
        })
    }

    /// `gatt.setPreferredPhy(txMask, rxMask, options)`.
    ///
    /// A request. Android reports what the link settled on through
    /// `onPhyUpdate`, and that may be what it already was — the peer, either
    /// controller, or the connection parameters can all decline, and 2M and
    /// coded are optional even in Bluetooth 5.
    pub async fn set_preferred_phy(
        &self,
        id: &str,
        tx: webbluetooth_core::Phy,
        rx: webbluetooth_core::Phy,
    ) -> Result<webbluetooth_core::ConnectionPhy> {
        // `PHY_OPTION_NO_PREFERRED`: the coded-PHY sub-rate, ignored unless
        // coded is in the mask.
        const NO_PREFERRED: i32 = 0;
        let reply = {
            let runtime = self.runtime()?;
            let env = runtime.env().map_err(jni_error)?;
            let gatt = self.gatt(id)?;
            let reply = self.enqueue(id, |p| &mut p.phy);
            ble::set_preferred_phy(env, gatt.as_ptr(), phy_mask(tx), phy_mask(rx), NO_PREFERRED)
                .map_err(jni_error)?;
            reply
        };
        let (got_tx, got_rx) = reply
            .await
            .map_err(|_| Error::Aborted("the PHY request was cancelled".into()))??;
        Ok(webbluetooth_core::ConnectionPhy {
            tx: phy_from_android(got_tx),
            rx: phy_from_android(got_rx),
        })
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    /// `BluetoothDevice.createBond`, then wait for the bond to settle.
    ///
    /// Android reports the outcome through an `ACTION_BOND_STATE_CHANGED`
    /// broadcast, which needs a `BroadcastReceiver` registered against a
    /// `Context` this crate does not assume it has. So the state is polled
    /// instead — `getBondState` is cheap, and a pairing ceremony is a
    /// human-scale event where a quarter-second granularity is invisible.
    ///
    /// `createBond` returning `true` only means the ceremony *started*: the
    /// user may still have a dialog to answer, or dismiss.
    pub async fn pair(&self, id: &str) -> Result<webbluetooth_core::Pairing> {
        // `BluetoothDevice.BOND_*`. `BOND_BONDING` (11) is deliberately not
        // named: the loop below treats anything that is neither bonded nor
        // none as still in progress, which also covers a state Android adds
        // later without this stopping early on a value it does not recognise.
        const BOND_NONE: i32 = 10;
        const BOND_BONDED: i32 = 12;

        if self.bond_state(id)? == BOND_BONDED {
            return Ok(webbluetooth_core::Pairing::AlreadyPaired);
        }
        {
            let runtime = self.runtime()?;
            let env = runtime.env().map_err(jni_error)?;
            let device = self.devices.get(id, |d| d.inner.device.clone())?;
            if !ble::create_bond(env, device.as_ptr()).map_err(jni_error)? {
                return Err(Error::Network(
                    "createBond was refused; the device may be out of range".into(),
                ));
            }
        }

        // Generous, because the ceiling is a person reading a dialog.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            match self.bond_state(id)? {
                BOND_BONDED => return Ok(webbluetooth_core::Pairing::Paired),
                BOND_NONE => {
                    return Err(Error::Network(
                        "pairing ended without a bond; it was refused or cancelled".into(),
                    ))
                }
                // Still bonding, or a state Android added later.
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Aborted("pairing was not answered".into()));
            }
            webbluetooth_core::timer::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    pub async fn is_paired(&self, id: &str) -> Result<bool> {
        const BOND_BONDED: i32 = 12;
        Ok(self.bond_state(id)? == BOND_BONDED)
    }

    /// `BluetoothDevice.getBondState`.
    fn bond_state(&self, id: &str) -> Result<i32> {
        let runtime = self.runtime()?;
        let env = runtime.env().map_err(jni_error)?;
        let device = self.devices.get(id, |d| d.inner.device.clone())?;
        ble::bond_state(env, device.as_ptr()).map_err(jni_error)
    }

    // ── Controllers ─────────────────────────────────────────────────────

    /// Android's one adapter.
    ///
    /// `BluetoothAdapter.getDefaultAdapter()` is the whole API: there is no
    /// enumeration and no way to select, because the platform assumes a phone
    /// has one radio. Reported as a single entry so callers see the same shape
    /// they see on Linux.
    pub async fn adapters(&self) -> Result<Vec<webbluetooth_core::AdapterInfo>> {
        Ok(vec![self.adapter_info().await?])
    }

    pub async fn adapter(&self) -> Result<webbluetooth_core::AdapterInfo> {
        self.adapter_info().await
    }

    async fn adapter_info(&self) -> Result<webbluetooth_core::AdapterInfo> {
        let runtime = self.runtime()?;
        let env = runtime.env().map_err(jni_error)?;
        let adapter = self.ble_adapter()?;
        Ok(webbluetooth_core::AdapterInfo {
            id: webbluetooth_core::adapter::DEFAULT_ADAPTER.to_owned(),
            name: adapter.name(env),
            // Reported only if it is a real address; see `Adapter::address`.
            address: adapter
                .address(env)
                .filter(|a| !webbluetooth_core::adapter::is_placeholder_address(a)),
            powered: adapter.is_enabled(env),
            is_default: true,
        })
    }

    pub async fn select_adapter(&self, id: &str) -> Result<()> {
        if id == webbluetooth_core::adapter::DEFAULT_ADAPTER {
            return Ok(());
        }
        Err(Error::NotSupported(format!(
            "Android exposes one default adapter and no way to choose another, \
             so {id:?} cannot be selected"
        )))
    }

    /// Not reachable from an application on this platform.
    ///
    /// Android offers `requestConnectionPriority` and nothing that reads the
    /// result back.
    pub async fn connection_parameters(
        &self,
        id: &str,
    ) -> Result<webbluetooth_core::ConnectionParameters> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "Android does not report negotiated connection parameters".into(),
        ))
    }

    /// Adopt a device by address.
    ///
    /// `BluetoothAdapter.getRemoteDevice` builds a `BluetoothDevice` from an
    /// address without any discovery, which is what this needs. It throws on a
    /// malformed address, so the format is checked first rather than letting a
    /// Java exception be the error message.
    pub async fn adopt_device(
        &self,
        id: &str,
        allowed: webbluetooth_core::registry::Grant,
    ) -> Result<String> {
        if !webbluetooth_core::address::is_bluetooth_address(id) {
            return Err(Error::NotFound(format!(
                "{id:?} is not a Bluetooth address"
            )));
        }
        let runtime = self.runtime()?;
        let env = runtime.env().map_err(jni_error)?;
        let adapter = self.ble_adapter()?;
        // Android wants it upper-case and rejects anything else.
        let device = adapter
            .remote_device(env, &id.to_uppercase())
            .map_err(|e| Error::NotFound(format!("getRemoteDevice({id:?}) failed: {e}")))?;
        let device =
            Ref::new(env, device).ok_or_else(|| Error::NotFound(format!("no device at {id}")))?;

        self.devices.insert(
            id,
            None,
            allowed,
            false,
            DeviceData {
                device,
                gatt: None,
                callback: None,
                mtu: 23,
            },
        );
        Ok(id.to_owned())
    }

    async fn gatt_lock(&self, id: &str) -> Result<futures_util::lock::OwnedMutexGuard<()>> {
        self.devices.gatt_lock(id).await
    }

    /// Revoke a grant, and close the connection.
    ///
    /// `close()` matters: Android allows only a handful of live GATT clients,
    /// and leaking them stops the app connecting at all until it restarts.
    pub fn forget(&self, id: &str) {
        if let Some(device) = self.devices.remove(id) {
            if let (Ok(runtime), Some(gatt)) = (self.runtime(), device.inner.gatt) {
                if let Ok(env) = runtime.env() {
                    let _ = ble::disconnect(env, gatt.as_ptr());
                    let _ = ble::close(env, gatt.as_ptr());
                }
            }
        }
    }

    fn gatt(&self, id: &str) -> Result<Ref> {
        self.devices
            .get(id, |d| d.inner.gatt.clone())?
            .ok_or_else(|| Error::InvalidState("the device is not connected".into()))
    }

    // ── Attribute accessors ─────────────────────────────────────────────────

    pub fn service_is_primary(&self, _handle: &Handle) -> bool {
        // Only primary services are returned by getServices().
        true
    }

    pub fn characteristic_properties(&self, handle: &Handle) -> CharacteristicProperties {
        // Android's PROPERTY_* constants are the Bluetooth bit values.
        CharacteristicProperties(handle.0.properties)
    }

    pub fn cached_value(&self, handle: &Handle) -> Option<Vec<u8>> {
        let env = self.runtime().ok()?.env().ok()?;
        ble::characteristic_value(env, handle.as_ptr())
    }

    pub fn is_notifying(&self, handle: &Handle) -> bool {
        self.notifications
            .lock()
            .unwrap()
            .contains_key(&handle.0.object.key())
    }

    // ── GATT operations ─────────────────────────────────────────────────────

    pub async fn connect(self: &Arc<Self>, id: &str) -> Result<()> {
        self.require_powered_on().await?;
        if self.is_connected(id) {
            return Ok(());
        }
        // Each phase takes its own `JNIEnv*` and lets it go before awaiting.
        //
        // JNI ties an environment to the thread that asked for it. An executor
        // may resume this future on a different one, which would leave the
        // pointer addressing another thread's environment — undefined
        // behaviour, and not the kind that announces itself. Keeping each
        // `env` inside a block that ends before the `await` is what makes that
        // impossible rather than merely unlikely; it is also what makes the
        // future `Send`, so the compiler checks the rule for us.
        let rx = {
            let runtime = self.runtime()?;
            let env = runtime.env().map_err(jni_error)?;
            let device = self.devices.get(id, |d| d.inner.device.clone())?;

            let callback = Arc::new(
                Callback::new(
                    runtime,
                    "dev.webbluetooth.GattCallback",
                    crate::gatt_callback_dex(),
                    &crate::bluetooth::gatt_natives(),
                    Arc::new(Sink(Arc::downgrade(self))),
                )
                .map_err(jni_error)?,
            );

            // Enqueued before the call, so a reply that arrives immediately
            // still has somewhere to land.
            let rx = self.enqueue(id, |p| &mut p.connect);
            let gatt = ble::connect_gatt(env, runtime, device.as_ptr(), callback.as_ptr())
                .map_err(jni_error)?;
            let gatt = Ref::new(env, gatt)
                .ok_or_else(|| Error::Network("connectGatt returned null".into()))?;
            self.devices.update(id, |d| {
                d.inner.gatt = Some(gatt);
                d.inner.callback = Some(callback.clone());
            });
            rx
        };
        rx.await
            .map_err(|_| Error::Aborted("connect was cancelled".into()))??;

        let rx = {
            let runtime = self.runtime()?;
            let env = runtime.env().map_err(jni_error)?;
            let gatt = self.gatt(id)?;
            // Ask for a larger MTU up front; the reply updates the record.
            let _ = ble::request_mtu(env, gatt.as_ptr(), 517);

            // Android does not populate the service list until asked.
            let rx = self.enqueue(id, |p| &mut p.services);
            ble::discover_services(env, gatt.as_ptr()).map_err(jni_error)?;
            rx
        };
        rx.await
            .map_err(|_| Error::Aborted("service discovery was cancelled".into()))?
    }

    pub fn disconnect(&self, id: &str) {
        if let (Ok(runtime), Ok(gatt)) = (self.runtime(), self.gatt(id)) {
            if let Ok(env) = runtime.env() {
                let _ = ble::disconnect(env, gatt.as_ptr());
            }
        }
    }

    pub async fn discover_services(
        &self,
        id: &str,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let gatt = self.gatt(id)?;
        let list = ble::services(env, gatt.as_ptr()).map_err(jni_error)?;
        Ok(collect(env, list, uuid, 0))
    }

    pub async fn discover_included_services(
        &self,
        id: &str,
        service: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let list = ble::service_included_services(env, service.as_ptr()).map_err(jni_error)?;
        // Included services are `BluetoothGattService`s like any other, so the
        // plain collector applies; `0` is the property mask a service has none
        // of.
        Ok(collect(env, list, uuid, 0))
    }

    pub async fn discover_characteristics(
        &self,
        id: &str,
        service: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let list = ble::service_characteristics(env, service.as_ptr()).map_err(jni_error)?;
        Ok(collect_characteristics(env, list, uuid))
    }

    pub async fn discover_descriptors(
        &self,
        id: &str,
        characteristic: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let list =
            ble::characteristic_descriptors(env, characteristic.as_ptr()).map_err(jni_error)?;
        Ok(collect(env, list, uuid, 0))
    }

    pub async fn read_characteristic(&self, id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let gatt = self.gatt(id)?;
        let rx = self.enqueue(id, |p| &mut p.read);
        if !ble::read_characteristic(env, gatt.as_ptr(), handle.as_ptr()).map_err(jni_error)? {
            return Err(Error::Network("readCharacteristic was refused".into()));
        }
        rx.await
            .map_err(|_| Error::Aborted("read was cancelled".into()))?
    }

    pub async fn write_characteristic(
        &self,
        id: &str,
        handle: &Handle,
        value: &[u8],
        write_type: WriteType,
    ) -> Result<()> {
        // The lock is taken for *both* write types here, unlike every other
        // backend.
        //
        // Not an oversight. Android's `BluetoothGatt` permits one outstanding
        // operation of any kind: a second `writeCharacteristic` while one is
        // in flight returns `false` and the value is simply not sent. The
        // unacknowledged form is no exception — it still goes through the same
        // queue and still reports completion through `onCharacteristicWrite`,
        // which is why this awaits one. Dropping the lock here would not gain
        // throughput, it would turn concurrent writes into refusals.
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let gatt = self.gatt(id)?;
        let kind = match write_type {
            WriteType::WithResponse => ble::WRITE_TYPE_DEFAULT,
            WriteType::WithoutResponse => ble::WRITE_TYPE_NO_RESPONSE,
        };
        let rx = self.enqueue(id, |p| &mut p.write);
        if !ble::write_characteristic(env, gatt.as_ptr(), handle.as_ptr(), value, kind)
            .map_err(jni_error)?
        {
            return Err(Error::Network("writeCharacteristic was refused".into()));
        }
        rx.await
            .map_err(|_| Error::Aborted("write was cancelled".into()))?
    }

    pub async fn set_notify(&self, id: &str, handle: &Handle, enabled: bool) -> Result<bool> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let gatt = self.gatt(id)?;
        let properties = CharacteristicProperties(handle.0.properties);

        let rx = self.enqueue(id, |p| &mut p.notify);
        let wrote = ble::set_notify(
            env,
            gatt.as_ptr(),
            handle.as_ptr(),
            enabled,
            enabled && !properties.notify() && properties.indicate(),
        )
        .map_err(jni_error)?;
        if !wrote {
            // No CCCD: local delivery is enabled but the peer was not told.
            return Ok(enabled);
        }
        rx.await
            .map_err(|_| Error::Aborted("subscribe was cancelled".into()))?
    }

    pub fn subscribe(&self, handle: &Handle) -> webbluetooth_core::backlog::Receiver<Vec<u8>> {
        // Bounded: a notification arrives on a platform callback
        // thread that must return promptly, so there is nobody to
        // apply backpressure to. See `webbluetooth_core::backlog`.
        let (tx, rx) = webbluetooth_core::backlog::channel();
        self.notifications
            .lock()
            .unwrap()
            .entry(handle.0.object.key())
            .or_default()
            .push(tx);
        rx
    }

    pub async fn read_descriptor(&self, id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let gatt = self.gatt(id)?;
        let rx = self.enqueue(id, |p| &mut p.read_descriptor);
        if !ble::read_descriptor(env, gatt.as_ptr(), handle.as_ptr()).map_err(jni_error)? {
            return Err(Error::Network("readDescriptor was refused".into()));
        }
        rx.await
            .map_err(|_| Error::Aborted("descriptor read was cancelled".into()))?
    }

    pub async fn write_descriptor(&self, id: &str, handle: &Handle, value: &[u8]) -> Result<()> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let gatt = self.gatt(id)?;
        let rx = self.enqueue(id, |p| &mut p.write_descriptor);
        if !ble::write_descriptor(env, gatt.as_ptr(), handle.as_ptr(), value).map_err(jni_error)? {
            return Err(Error::Network("writeDescriptor was refused".into()));
        }
        rx.await
            .map_err(|_| Error::Aborted("descriptor write was cancelled".into()))?
    }

    pub async fn read_rssi(&self, id: &str) -> Result<i32> {
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let gatt = self.gatt(id)?;
        let rx = self.enqueue(id, |p| &mut p.rssi);
        if !ble::read_remote_rssi(env, gatt.as_ptr()).map_err(jni_error)? {
            return Err(Error::Network("readRemoteRssi was refused".into()));
        }
        rx.await
            .map_err(|_| Error::Aborted("RSSI read was cancelled".into()))?
    }

    pub async fn request_connection_priority(
        &self,
        id: &str,
        priority: webbluetooth_core::ConnectionPriority,
    ) -> Result<()> {
        use webbluetooth_core::ConnectionPriority as P;
        // `BluetoothGatt.CONNECTION_PRIORITY_*`.
        let value = match priority {
            P::Balanced => 0,
            P::High => 1,
            P::LowPower => 2,
        };
        let _guard = self.gatt_lock(id).await?;
        let env = self.runtime()?.env().map_err(jni_error)?;
        let gatt = self.gatt(id)?;
        // Android answers immediately with whether it asked, not with what was
        // agreed — there is no callback for the outcome.
        if !ble::request_connection_priority(env, gatt.as_ptr(), value).map_err(jni_error)? {
            return Err(Error::Network(
                "requestConnectionPriority was refused".into(),
            ));
        }
        Ok(())
    }

    pub fn max_write_len(&self, id: &str, _write_type: WriteType) -> Result<usize> {
        // Three bytes of ATT header come off whatever MTU was negotiated.
        Ok(self.devices.get(id, |d| d.inner.mtu)?.max(23) as usize - 3)
    }

    pub async fn l2cap_target(&self, id: &str, psm: u16) -> Result<crate::jni::JObject> {
        let runtime = self.runtime()?;
        let env = runtime.env().map_err(jni_error)?;
        let device = self.devices.get(id, |d| d.inner.device.clone())?;
        let socket =
            ble::create_l2cap_channel(env, device.as_ptr(), psm as i32).map_err(jni_error)?;
        if socket.is_null() {
            return Err(Error::NotSupported(
                "createL2capChannel is unavailable — it needs API 29 or newer".into(),
            ));
        }
        Ok(socket)
    }
}

/// Turn a `GATT_*` status into a result.
fn gatt_result<T>(status: i32, value: T) -> Result<T> {
    if status == ble::GATT_SUCCESS {
        Ok(value)
    } else {
        Err(match status {
            2 => Error::Security("read not permitted".into()),
            3 => Error::Security("write not permitted".into()),
            5 | 15 => Error::Security("the link must be encrypted — pair the device first".into()),
            6 => Error::NotSupported("the peer does not support that request".into()),
            13 => Error::InvalidModification("wrong attribute value length".into()),
            _ => Error::Network(format!("the operation failed (GATT status {status})")),
        })
    }
}

fn jni_error(e: crate::runtime::Error) -> Error {
    Error::Network(e.to_string())
}

/// Turn a `List<BluetoothGattService|Descriptor>` into handles.
fn collect(
    env: crate::jni::Env,
    list: JObject,
    want: Option<&BluetoothUuid>,
    properties: u32,
) -> Vec<Handle> {
    let mut out = Vec::new();
    ble::for_each(env, list, |item| {
        let Some(raw) = ble::attribute_uuid(env, item) else {
            return;
        };
        let Ok(uuid) = BluetoothUuid::parse(&raw) else {
            return;
        };
        if want.is_some_and(|w| *w != uuid) {
            return;
        }
        if let Some(object) = Ref::new(env, item) {
            out.push(Handle(Arc::new(HandleData {
                object,
                uuid,
                properties,
            })));
        }
    });
    out
}

/// As [`collect`], but reading each characteristic's property bits.
fn collect_characteristics(
    env: crate::jni::Env,
    list: JObject,
    want: Option<&BluetoothUuid>,
) -> Vec<Handle> {
    let mut out = Vec::new();
    ble::for_each(env, list, |item| {
        let Some(raw) = ble::attribute_uuid(env, item) else {
            return;
        };
        let Ok(uuid) = BluetoothUuid::parse(&raw) else {
            return;
        };
        if want.is_some_and(|w| *w != uuid) {
            return;
        }
        let properties = ble::characteristic_properties(env, item) as u32;
        if let Some(object) = Ref::new(env, item) {
            out.push(Handle(Arc::new(HandleData {
                object,
                uuid,
                properties,
            })));
        }
    });
    out
}

/// Whether this process may use Bluetooth.
///
/// Android's answer depends on runtime permission grants, which the framework
/// only reports by throwing when a call is made. Treating a usable adapter as
/// the signal is the closest honest equivalent.
pub fn authorization() -> webbluetooth_core::Authorization {
    match Runtime::get() {
        None => webbluetooth_core::Authorization::NotDetermined,
        Some(runtime) => match ble::Adapter::open(runtime) {
            Ok(_) => webbluetooth_core::Authorization::Allowed,
            Err(_) => webbluetooth_core::Authorization::Denied,
        },
    }
}

/// `BluetoothDevice.PHY_LE_*` — what a callback reports.
///
/// Anything unrecognised reads as 1M: every LE device has it, so treating an
/// unknown value as the baseline understates rather than invents.
fn phy_from_android(value: i32) -> webbluetooth_core::Phy {
    const PHY_LE_2M: i32 = 2;
    const PHY_LE_CODED: i32 = 3;
    match value {
        PHY_LE_2M => webbluetooth_core::Phy::Le2M,
        PHY_LE_CODED => webbluetooth_core::Phy::LeCoded,
        // `PHY_LE_1M` (1), and anything Android adds later.
        _ => webbluetooth_core::Phy::Le1M,
    }
}

/// `BluetoothDevice.PHY_LE_*_MASK` — what a request takes.
///
/// Masks, not values: `setPreferredPhy` states what a caller will accept and
/// the controller chooses. The numbers differ from the ones a callback
/// reports, which is an easy and silent mistake — `PHY_LE_CODED` is 3 while
/// `PHY_LE_CODED_MASK` is 4.
fn phy_mask(phy: webbluetooth_core::Phy) -> i32 {
    match phy {
        webbluetooth_core::Phy::Le1M => 1,
        webbluetooth_core::Phy::Le2M => 2,
        webbluetooth_core::Phy::LeCoded => 4,
    }
}
