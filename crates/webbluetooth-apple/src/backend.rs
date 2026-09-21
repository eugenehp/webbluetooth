//! The engine: adapter state, scanning, and correlating delegate callbacks back
//! to the futures that provoked them.
//!
//! # Correlating replies
//!
//! CoreBluetooth's delegate methods carry no request identifier.
//! `readValueForCharacteristic:` returns nothing and, some time later,
//! `peripheral:didUpdateValueForCharacteristic:error:` arrives — and it is the
//! *same* callback a notification arrives through. So a reply is matched to its
//! request by the object it concerns: one FIFO queue of waiters per
//! `(peripheral, attribute)` pair. A value that arrives with no queued reader is
//! a notification, which is exactly the distinction the spec draws.
//!
//! On top of that, GATT operations are serialised per device with an async
//! mutex. The spec requires it, and it means the FIFO never has to disambiguate
//! two concurrent reads of the same characteristic.
//!
//! # Invalidation
//!
//! `CBService` and `CBCharacteristic` objects do not survive a disconnect, and
//! a peripheral can replace its whole attribute table by re-advertising. Each
//! device carries a generation counter that is bumped on both events; handles
//! remember the generation they were minted in and fail with `InvalidStateError`
//! once it moves. Without that, a stale handle is a use-after-free.

use crate::{cb, Central, Event, EventSink, ManagerState, Retained};
use futures_channel::oneshot;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use webbluetooth_core::chooser::DeviceChooser;
use webbluetooth_core::error::{Error, Result};
use webbluetooth_core::filter::{Advertisement, RequestDeviceOptions};
use webbluetooth_core::gatt::{CharacteristicProperties, WriteType};
use webbluetooth_core::registry::DeviceRegistry;
use webbluetooth_core::scan::{self, ScanHub};
use webbluetooth_core::timer;
use webbluetooth_core::uuid::BluetoothUuid;

/// A handle into a device's attribute table.
///
/// On Apple this is a retained `CBService` / `CBCharacteristic`; the Linux
/// backend uses a D-Bus object path instead. Both are cheap to clone and
/// identity-comparable, which is all the shared layer requires.
pub type Handle = Retained;

/// How long to wait for `centralManagerDidUpdateState:` before giving up.
const STATE_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// A pending-reply queue keyed by `(peripheral, attribute)`.
type Key = (usize, usize);

struct Slots<T> {
    waiting: HashMap<Key, VecDeque<oneshot::Sender<Result<T>>>>,
}

impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self {
            waiting: HashMap::new(),
        }
    }
}

impl<T> Slots<T> {
    /// Register interest *before* calling into CoreBluetooth — the callback can
    /// land on the dispatch queue before the calling thread resumes.
    fn enqueue(&mut self, key: Key) -> oneshot::Receiver<Result<T>> {
        let (tx, rx) = oneshot::channel();
        self.waiting.entry(key).or_default().push_back(tx);
        rx
    }

    /// Hand `value` to the oldest waiter. `false` if there was none, which for
    /// a characteristic value means "this was a notification".
    fn resolve(&mut self, key: Key, value: Result<T>) -> bool {
        let Some(queue) = self.waiting.get_mut(&key) else {
            return false;
        };
        let Some(tx) = queue.pop_front() else {
            return false;
        };
        if queue.is_empty() {
            self.waiting.remove(&key);
        }
        let _ = tx.send(value);
        true
    }

    /// Drop the most recently registered waiter for `key`.
    ///
    /// For the case where interest was registered before a check that then
    /// said it was not needed. Leaving it would consume a later callback that
    /// belonged to somebody else.
    fn cancel(&mut self, key: Key) {
        if let Some(queue) = self.waiting.get_mut(&key) {
            queue.pop_back();
            if queue.is_empty() {
                self.waiting.remove(&key);
            }
        }
    }

    fn is_waiting(&self, key: Key) -> bool {
        self.waiting.get(&key).is_some_and(|q| !q.is_empty())
    }

    /// Fail every request outstanding against one peripheral. Called on
    /// disconnect, so a dropped link never leaves a future hanging — and never
    /// leaves the per-device GATT lock held.
    fn fail_peripheral(&mut self, peripheral: usize, error: &Error) {
        self.waiting.retain(|(p, _), queue| {
            if *p != peripheral {
                return true;
            }
            for tx in queue.drain(..) {
                let _ = tx.send(Err(error.clone()));
            }
            false
        });
    }
}

#[derive(Default)]
struct Pending {
    connect: Slots<()>,
    services: Slots<()>,
    characteristics: Slots<()>,
    included_services: Slots<()>,
    descriptors: Slots<()>,
    read_characteristic: Slots<Vec<u8>>,
    write_characteristic: Slots<()>,
    notify: Slots<bool>,
    read_descriptor: Slots<Vec<u8>>,
    write_descriptor: Slots<()>,
    rssi: Slots<i32>,
    l2cap: Slots<Retained>,
    /// Waiters for `peripheralIsReadyToSendWriteWithoutResponse:`.
    ///
    /// Unacknowledged writes have flow control even though they have no
    /// acknowledgement: CoreBluetooth buffers them and, once that buffer is
    /// full, **discards** further ones. `canSendWriteWithoutResponse` says
    /// whether there is room, and this callback says when there is again.
    ready_to_write: Slots<()>,
}

impl Pending {
    fn fail_peripheral(&mut self, peripheral: usize, error: &Error) {
        self.connect.fail_peripheral(peripheral, error);
        self.services.fail_peripheral(peripheral, error);
        self.characteristics.fail_peripheral(peripheral, error);
        self.descriptors.fail_peripheral(peripheral, error);
        self.read_characteristic.fail_peripheral(peripheral, error);
        self.write_characteristic.fail_peripheral(peripheral, error);
        self.notify.fail_peripheral(peripheral, error);
        self.read_descriptor.fail_peripheral(peripheral, error);
        self.write_descriptor.fail_peripheral(peripheral, error);
        self.rssi.fail_peripheral(peripheral, error);
        self.l2cap.fail_peripheral(peripheral, error);
    }
}

/// What the Apple backend needs to reach a device: the peripheral itself.
/// Everything else about a grant lives in [`webbluetooth_core::registry`].
type DeviceData = Retained;

/// Shared state behind every handle the API hands out.
pub struct Inner {
    central: Central,
    state: Mutex<ManagerState>,
    state_waiters: Mutex<Vec<oneshot::Sender<ManagerState>>>,
    /// Everyone watching the scan; the radio is shared and reference-counted.
    hub: ScanHub,
    /// Peripherals seen so far, retained so the chosen one survives past the
    /// callback that reported it.
    sightings: Mutex<HashMap<String, (Retained, Option<String>)>>,
    devices: DeviceRegistry<DeviceData>,
    pending: Mutex<Pending>,
    /// Subscribers per characteristic, for `start_notifications`.
    notifications: Mutex<HashMap<usize, Vec<webbluetooth_core::backlog::Sender<Vec<u8>>>>>,
    /// Services a restored session is allowed to use, declared up front.
    restore_allowed: BTreeSet<BluetoothUuid>,
    /// What `willRestoreState:` handed back, if anything.
    restored: Mutex<Option<RestoredScan>>,
}

/// The scan the system interrupted when it terminated this process.
#[derive(Debug, Default, Clone)]
pub struct RestoredScan {
    pub device_ids: Vec<String>,
    pub scan_services: Vec<BluetoothUuid>,
    pub scan_allow_duplicates: bool,
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

    pub fn with_restoration(
        show_power_alert: bool,
        restore_identifier: Option<&str>,
        restore_allowed: BTreeSet<BluetoothUuid>,
    ) -> Arc<Self> {
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let sink = Arc::new(Sink(weak.clone()));
            Inner {
                central: Central::with_restore_identifier(
                    sink,
                    show_power_alert,
                    restore_identifier,
                ),
                state: Mutex::new(ManagerState::Unknown),
                state_waiters: Mutex::new(Vec::new()),
                hub: ScanHub::new(),
                sightings: Mutex::new(HashMap::new()),
                devices: DeviceRegistry::default(),
                pending: Mutex::new(Pending::default()),
                notifications: Mutex::new(HashMap::new()),
                restore_allowed,
                restored: Mutex::new(None),
            }
        });
        // `centralManagerDidUpdateState:` can fire while `new_cyclic` is still
        // running, when the weak reference cannot yet be upgraded and the event
        // is dropped. The state is also readable synchronously, so reconcile.
        let actual = inner.central.state();
        if actual != ManagerState::Unknown {
            inner.set_state(actual);
        }
        inner
    }

    // ── Event handling ──────────────────────────────────────────────────────

    fn handle(&self, event: Event) {
        match event {
            Event::StateChanged(state) => self.on_state_changed(state),

            Event::Discovered {
                peripheral,
                advertisement,
                rssi,
            } => self.on_discovered(peripheral, &advertisement, rssi),

            Event::Connected { peripheral } => {
                let key = peripheral.key();
                self.devices
                    .update_where(|d| d.inner.key() == key, |d| d.connected = true);
                self.pending
                    .lock()
                    .unwrap()
                    .connect
                    .resolve((key, key), Ok(()));
            }

            Event::ConnectFailed { peripheral, error } => {
                let key = peripheral.key();
                self.pending
                    .lock()
                    .unwrap()
                    .connect
                    .resolve((key, key), Err(network("connection failed", error)));
            }

            Event::Disconnected { peripheral, error } => self.on_disconnected(&peripheral, error),

            Event::ServicesModified { peripheral, .. } => {
                // Every handle into this device now points at freed objects.
                let key = peripheral.key();
                if let Some(id) = self.devices.find(|d| d.inner.key() == key) {
                    self.devices.mark_services_changed(&id);
                }
            }

            Event::ServicesDiscovered { peripheral, error } => {
                let key = peripheral.key();
                let value = match error {
                    Some(e) => Err(network("service discovery failed", Some(e))),
                    None => Ok(()),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .services
                    .resolve((key, key), value);
            }

            Event::IncludedServicesDiscovered {
                peripheral,
                service,
                error,
            } => {
                let value = match error {
                    Some(e) => Err(network("included service discovery failed", Some(e))),
                    None => Ok(()),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .included_services
                    .resolve((peripheral.key(), service.key()), value);
            }

            Event::CharacteristicsDiscovered {
                peripheral,
                service,
                error,
            } => {
                let value = match error {
                    Some(e) => Err(network("characteristic discovery failed", Some(e))),
                    None => Ok(()),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .characteristics
                    .resolve((peripheral.key(), service.key()), value);
            }

            Event::DescriptorsDiscovered {
                peripheral,
                characteristic,
                error,
            } => {
                let value = match error {
                    Some(e) => Err(network("descriptor discovery failed", Some(e))),
                    None => Ok(()),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .descriptors
                    .resolve((peripheral.key(), characteristic.key()), value);
            }

            Event::CharacteristicValue {
                peripheral,
                characteristic,
                value,
                error,
            } => self.on_characteristic_value(&peripheral, &characteristic, value, error),

            Event::CharacteristicWritten {
                peripheral,
                characteristic,
                error,
            } => {
                let value = match error {
                    Some(e) => Err(network("write failed", Some(e))),
                    None => Ok(()),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .write_characteristic
                    .resolve((peripheral.key(), characteristic.key()), value);
            }

            Event::NotifyStateChanged {
                peripheral,
                characteristic,
                notifying,
                error,
            } => {
                let value = match error {
                    Some(e) => Err(network("could not change notification state", Some(e))),
                    None => Ok(notifying),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .notify
                    .resolve((peripheral.key(), characteristic.key()), value);
            }

            Event::DescriptorValue {
                peripheral,
                descriptor,
                value,
                error,
            } => {
                let value = match error {
                    Some(e) => Err(network("descriptor read failed", Some(e))),
                    None => Ok(value.unwrap_or_default()),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .read_descriptor
                    .resolve((peripheral.key(), descriptor.key()), value);
            }

            Event::DescriptorWritten {
                peripheral,
                descriptor,
                error,
            } => {
                let value = match error {
                    Some(e) => Err(network("descriptor write failed", Some(e))),
                    None => Ok(()),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .write_descriptor
                    .resolve((peripheral.key(), descriptor.key()), value);
            }

            Event::RssiRead {
                peripheral,
                rssi,
                error,
            } => {
                let key = peripheral.key();
                let value = match error {
                    Some(e) => Err(network("RSSI read failed", Some(e))),
                    None => Ok(rssi),
                };
                self.pending.lock().unwrap().rssi.resolve((key, key), value);
            }

            Event::NameChanged { peripheral, name } => {
                let key = peripheral.key();
                self.devices
                    .update_where(|d| d.inner.key() == key, |d| d.name = name.clone());
            }

            Event::ReadyToWrite { peripheral } => {
                // Whoever is blocked on room in the write buffer may go.
                self.pending
                    .lock()
                    .unwrap()
                    .ready_to_write
                    .resolve((peripheral.key(), peripheral.key()), Ok(()));
            }

            Event::L2capChannelOpened {
                peripheral,
                channel,
                error,
            } => {
                let key = peripheral.key();
                let value = match (channel, error) {
                    (_, Some(e)) => Err(network("could not open L2CAP channel", Some(e))),
                    (Some(c), None) => Ok(c),
                    (None, None) => Err(Error::Network("L2CAP channel opened but was nil".into())),
                };
                self.pending
                    .lock()
                    .unwrap()
                    .l2cap
                    .resolve((key, key), value);
            }

            Event::WillRestoreState(restored) => self.on_restore(*restored),
        }
    }

    fn on_state_changed(&self, state: ManagerState) {
        self.set_state(state);
        if state == ManagerState::PoweredOn {
            return;
        }
        // The adapter went away: every link is gone with it.
        let error = Error::Network("the Bluetooth adapter became unavailable".into());
        let mut pending = self.pending.lock().unwrap();
        for id in self.devices.ids() {
            if !self.devices.is_connected(&id) {
                continue;
            }
            if let Ok(key) = self.devices.get(&id, |d| d.inner.key()) {
                pending.fail_peripheral(key, &error);
            }
            self.devices.mark_disconnected(&id);
        }
    }

    fn set_state(&self, state: ManagerState) {
        *self.state.lock().unwrap() = state;
        for tx in self.state_waiters.lock().unwrap().drain(..) {
            let _ = tx.send(state);
        }
    }

    fn on_discovered(&self, peripheral: Retained, advertisement: &cb::Advertisement, rssi: i32) {
        if !self.hub.is_watching() {
            return;
        }
        let Some(id) = (unsafe { cb::peripheral_identifier(peripheral.as_ptr()) }) else {
            return;
        };
        let name = unsafe { cb::peripheral_name(peripheral.as_ptr()) };
        let advertisement = advertisement_from_sys(advertisement, rssi);
        let display_name = name.clone().or_else(|| advertisement.local_name.clone());

        self.sightings
            .lock()
            .unwrap()
            .insert(id.clone(), (peripheral, display_name.clone()));
        // Each watcher applies its own filter; this one just reports.
        self.hub
            .publish(&id, display_name.as_deref(), &advertisement);
    }

    pub fn scan_hub(&self) -> &ScanHub {
        &self.hub
    }

    /// Start or stop the central's scan.
    ///
    /// Starting is idempotent: CoreBluetooth replaces the previous scan's
    /// filter, which is what a newly added watcher needs when it widens it.
    pub fn set_radio_scanning(&self, on: bool) -> Result<()> {
        if !on {
            unsafe { cb::central_stop_scan(self.central.as_ptr()) };
            return Ok(());
        }
        let uuids: Vec<Retained> = self
            .hub
            .scan_services()
            .iter()
            .filter_map(|u| unsafe { cb::uuid_from_string(u.as_str()) })
            .collect();
        let ptrs: Vec<_> = uuids.iter().map(|u| u.as_ptr()).collect();
        let duplicates = self.hub.wants_duplicates();
        unsafe { cb::central_scan(self.central.as_ptr(), &ptrs, duplicates) };
        Ok(())
    }

    fn on_disconnected(&self, peripheral: &Retained, error: Option<(i64, String)>) {
        let key = peripheral.key();
        let disconnected = self.devices.find(|d| d.inner.key() == key);

        // Everything still in flight dies with the link — including whoever
        // holds this device's GATT lock.
        let reason = match error {
            Some((code, message)) => {
                Error::Network(format!("device disconnected: {message} (CBError {code})"))
            }
            None => Error::Network("device disconnected".into()),
        };
        self.pending.lock().unwrap().fail_peripheral(key, &reason);

        // A notification stream on a dead characteristic should end, not hang.
        self.notifications.lock().unwrap().retain(|_, subscribers| {
            subscribers.retain(|tx| tx.is_open());
            !subscribers.is_empty()
        });

        if let Some(id) = disconnected {
            self.devices.mark_disconnected(&id);
        }
    }

    fn on_characteristic_value(
        &self,
        peripheral: &Retained,
        characteristic: &Retained,
        value: Option<Vec<u8>>,
        error: Option<(i64, String)>,
    ) {
        let key = (peripheral.key(), characteristic.key());
        let mut pending = self.pending.lock().unwrap();
        if pending.read_characteristic.is_waiting(key) {
            let result = match error {
                Some(e) => Err(network("read failed", Some(e))),
                None => Ok(value.unwrap_or_default()),
            };
            pending.read_characteristic.resolve(key, result);
            return;
        }
        drop(pending);

        // No queued reader: this is a notification.
        let Some(bytes) = value else { return };
        let mut subscribers = self.notifications.lock().unwrap();
        if let Some(list) = subscribers.get_mut(&characteristic.key()) {
            list.retain(|tx| tx.send(bytes.clone()).is_ok());
            if list.is_empty() {
                subscribers.remove(&characteristic.key());
            }
        }
    }

    /// Re-adopt what the system preserved.
    ///
    /// Arrives before the first `StateChanged`. Restored peripherals are
    /// granted the allowlist declared on [`webbluetooth_core::Restoration`] — the Web
    /// Bluetooth grant itself does not survive process death, so the caller
    /// states up front what a restored session may touch.
    fn on_restore(&self, restored: cb::RestoredCentralState) {
        let mut ids = Vec::new();
        for peripheral in restored.peripherals {
            let Some(id) = (unsafe { cb::peripheral_identifier(peripheral.as_ptr()) }) else {
                continue;
            };
            let name = unsafe { cb::peripheral_name(peripheral.as_ptr()) };
            let connected = unsafe { cb::peripheral_state(peripheral.as_ptr()) }
                == crate::PeripheralState::Connected;
            unsafe { self.central.adopt_peripheral(peripheral.as_ptr()) };
            self.devices.insert(
                &id,
                name,
                self.restore_allowed.clone().into(),
                connected,
                peripheral,
            );
            ids.push(id);
        }
        *self.restored.lock().unwrap() = Some(RestoredScan {
            device_ids: ids,
            scan_services: restored
                .scan_services
                .iter()
                .filter_map(|s| BluetoothUuid::from_cbuuid_string(s).ok())
                .collect(),
            scan_allow_duplicates: restored.scan_allow_duplicates,
        });
    }

    pub fn restored(&self) -> Option<RestoredScan> {
        self.restored.lock().unwrap().clone()
    }

    /// Open an L2CAP channel to this device.
    pub async fn l2cap_target(&self, id: &str, psm: u16) -> Result<Retained> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;
        if !self.is_connected(id) {
            return Err(Error::InvalidState("the device is not connected".into()));
        }
        let key = peripheral.key();
        let rx = self.pending.lock().unwrap().l2cap.enqueue((key, key));
        unsafe { cb::peripheral_open_l2cap(peripheral.as_ptr(), psm) };
        let handle = rx
            .await
            .map_err(|_| Error::Aborted("opening the L2CAP channel was cancelled".into()))??;
        Ok(handle)
    }

    // ── Availability ────────────────────────────────────────────────────────

    /// The adapter's state, as the shared layer spells it.
    ///
    /// `settled_state` below still works in CoreBluetooth's own terms, because
    /// it has to distinguish `Resetting` from `Unknown` while waiting.
    pub fn state(&self) -> webbluetooth_core::state::ManagerState {
        manager_state(*self.state.lock().unwrap())
    }

    /// Resolve `Unknown` into a real state, waiting for the first
    /// `centralManagerDidUpdateState:` if it has not arrived yet.
    pub async fn settled_state(&self) -> webbluetooth_core::state::ManagerState {
        let mut state = self.state();
        let deadline = std::time::Instant::now() + STATE_SETTLE_TIMEOUT;
        while state == webbluetooth_core::state::ManagerState::Unknown {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return webbluetooth_core::state::ManagerState::Unknown;
            }
            let rx = {
                let (tx, rx) = oneshot::channel();
                // Re-check under the lock: the event may have landed in between.
                let current = self.state();
                if current != webbluetooth_core::state::ManagerState::Unknown {
                    return current;
                }
                self.state_waiters.lock().unwrap().push(tx);
                rx
            };
            match timer::timeout(remaining, rx).await {
                Ok(Ok(s)) => state = manager_state(s),
                _ => return self.state(),
            }
        }
        state
    }

    pub async fn require_powered_on(&self) -> Result<()> {
        self.settled_state().await.require_powered_on()
    }

    // ── Scanning ────────────────────────────────────────────────────────────

    /// Scan, let `chooser` pick, and grant access to the result.
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
        let Some((peripheral, name)) = self.sightings.lock().unwrap().get(&id).cloned() else {
            return Err(Error::NotFound(format!(
                "the chooser returned {id:?}, which was not among the devices it was offered"
            )));
        };

        unsafe { self.central.adopt_peripheral(peripheral.as_ptr()) };
        self.devices.insert(&id, name, allowed, false, peripheral);
        Ok(id)
    }

    /// Adopt a peripheral by its identifier.
    ///
    /// `retrievePeripheralsWithIdentifiers:` is CoreBluetooth's own answer to
    /// this and needs no scan — it is the same call restoration uses. The
    /// identifier is the `NSUUID` this host assigned, not an address, because
    /// Apple never exposes an address; an id from another machine will not
    /// resolve here, which is what the `NotFound` says.
    pub async fn adopt_device(
        &self,
        id: &str,
        allowed: webbluetooth_core::registry::Grant,
    ) -> Result<String> {
        self.require_powered_on().await?;
        let found = unsafe { cb::central_retrieve_by_identifier(self.central.as_ptr(), &[id]) };
        let Some(peripheral) = found.into_iter().next() else {
            return Err(Error::NotFound(format!(
                "CoreBluetooth knows no peripheral with identifier {id:?}; it is a \
                 per-host UUID, so an id from another machine will not resolve"
            )));
        };

        let name = unsafe { cb::peripheral_name(peripheral.as_ptr()) };
        let connected = unsafe { cb::peripheral_state(peripheral.as_ptr()) }
            == crate::PeripheralState::Connected;
        unsafe { self.central.adopt_peripheral(peripheral.as_ptr()) };
        self.devices
            .insert(id, name, allowed, connected, peripheral);
        Ok(id.to_owned())
    }

    // ── Device access ───────────────────────────────────────────────────────

    // ── Attribute accessors ─────────────────────────────────────────────────

    /// Whether a discovered service is primary.
    pub fn service_is_primary(&self, handle: &Handle) -> bool {
        unsafe { cb::service_is_primary(handle.as_ptr()) }
    }

    /// What a characteristic supports.
    pub fn characteristic_properties(&self, handle: &Handle) -> CharacteristicProperties {
        // CoreBluetooth's bit values are the Bluetooth ones, so this is a
        // change of type rather than of meaning.
        CharacteristicProperties(unsafe { cb::characteristic_properties(handle.as_ptr()) }.0)
    }

    /// The last value read or notified, without going to the device.
    pub fn cached_value(&self, handle: &Handle) -> Option<Vec<u8>> {
        unsafe { cb::characteristic_value(handle.as_ptr()) }
    }

    /// Whether the peer currently has notifications enabled.
    pub fn is_notifying(&self, handle: &Handle) -> bool {
        unsafe { cb::characteristic_is_notifying(handle.as_ptr()) }
    }

    // ── Grants ──────────────────────────────────────────────────────────────
    // The registry owns grants, generations and the GATT lock; these forward.
    // The registry answers these identically for every backend.
    webbluetooth_core::registry::forward_to_registry!();

    // ── PHY ─────────────────────────────────────────────────────────────────

    /// CoreBluetooth exposes no PHY at all — neither to read nor to choose.
    /// The stack negotiates 2M by itself where both ends support it, and
    /// does not say whether it did.
    pub async fn phy(&self, id: &str) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.peripheral(id)?;
        Err(Error::NotSupported(
            "CoreBluetooth does not expose the connection PHY".into(),
        ))
    }

    /// Asking is not possible where even reading is not.
    pub async fn set_preferred_phy(
        &self,
        id: &str,
        _tx: webbluetooth_core::Phy,
        _rx: webbluetooth_core::Phy,
    ) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.peripheral(id)?;
        Err(Error::NotSupported(
            "CoreBluetooth does not expose the connection PHY".into(),
        ))
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    /// Nothing, because CoreBluetooth offers nothing.
    ///
    /// There is no pairing API on any Apple platform. The system runs the
    /// ceremony itself the first time an encrypted attribute is touched, and
    /// shows its own dialog. A caller that wants a bond should simply read the
    /// characteristic they were going to read.
    pub async fn pair(&self, id: &str) -> Result<webbluetooth_core::Pairing> {
        let _ = self.peripheral(id)?;
        Ok(webbluetooth_core::Pairing::Implicit)
    }

    /// CoreBluetooth does not say, so neither does this.
    ///
    /// `false` rather than an error: nothing a caller could do differs on the
    /// answer, since the way to get a bond here is to use the device.
    pub async fn is_paired(&self, id: &str) -> Result<bool> {
        let _ = self.peripheral(id)?;
        Ok(false)
    }

    // ── Controllers ─────────────────────────────────────────────────────

    /// CoreBluetooth's one implicit controller.
    ///
    /// A `CBCentralManager` *is* the radio. There is no API to enumerate
    /// controllers, none to name the one in use, and none to pick another —
    /// not a gap in this crate but an absence in the framework, on every Apple
    /// platform. Reported as a single unnamed adapter so callers see the same
    /// shape they see elsewhere.
    pub async fn adapters(&self) -> Result<Vec<webbluetooth_core::AdapterInfo>> {
        Ok(vec![self.adapter().await?])
    }

    pub async fn adapter(&self) -> Result<webbluetooth_core::AdapterInfo> {
        Ok(webbluetooth_core::AdapterInfo {
            id: webbluetooth_core::adapter::DEFAULT_ADAPTER.to_owned(),
            name: None,
            address: None,
            powered: self.state().availability().is_ok(),
            is_default: true,
        })
    }

    pub async fn select_adapter(&self, id: &str) -> Result<()> {
        if id == webbluetooth_core::adapter::DEFAULT_ADAPTER {
            return Ok(());
        }
        Err(Error::NotSupported(format!(
            "CoreBluetooth exposes one implicit controller and no way to choose \
             another, so {id:?} cannot be selected"
        )))
    }

    /// Not reachable from an application on this platform.
    ///
    /// CoreBluetooth negotiates from the peripheral's preferred values and
    /// never reports what it settled on.
    pub async fn connection_parameters(
        &self,
        id: &str,
    ) -> Result<webbluetooth_core::ConnectionParameters> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "CoreBluetooth does not report negotiated connection parameters".into(),
        ))
    }

    /// The `CBPeripheral` behind a device id.
    fn peripheral(&self, id: &str) -> Result<Retained> {
        self.devices.get(id, |d| d.inner.clone())
    }

    async fn gatt_lock(&self, id: &str) -> Result<futures_util::lock::OwnedMutexGuard<()>> {
        self.devices.gatt_lock(id).await
    }

    /// Revoke a grant, dropping the link with it.
    pub fn forget(&self, id: &str) {
        if let Some(device) = self.devices.remove(id) {
            if device.connected {
                unsafe { cb::central_disconnect(self.central.as_ptr(), device.inner.as_ptr()) };
            }
        }
    }

    // ── GATT operations ─────────────────────────────────────────────────────

    pub async fn connect(&self, id: &str) -> Result<()> {
        self.require_powered_on().await?;
        let peripheral = self.peripheral(id)?;
        if self.is_connected(id) {
            return Ok(());
        }
        let key = peripheral.key();
        let rx = self.pending.lock().unwrap().connect.enqueue((key, key));
        unsafe {
            self.central.adopt_peripheral(peripheral.as_ptr());
            cb::central_connect(self.central.as_ptr(), peripheral.as_ptr());
        }
        rx.await
            .map_err(|_| Error::Aborted("connect was cancelled".into()))?
    }

    pub fn disconnect(&self, id: &str) {
        if let Ok(peripheral) = self.peripheral(id) {
            unsafe { cb::central_disconnect(self.central.as_ptr(), peripheral.as_ptr()) };
        }
    }

    /// Discover services and return those matching `uuid` (or all of them).
    pub async fn discover_services(
        &self,
        id: &str,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Retained>> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;
        if !self.is_connected(id) {
            return Err(Error::InvalidState("the device is not connected".into()));
        }
        let key = peripheral.key();

        let filter: Vec<Retained> = uuid
            .and_then(|u| unsafe { cb::uuid_from_string(u.as_str()) })
            .into_iter()
            .collect();
        let rx = self.pending.lock().unwrap().services.enqueue((key, key));
        // Scoped so the raw pointers are gone before the await below. They
        // are only needed for the synchronous call, and a `Vec<*mut c_void>`
        // held across a suspension point makes the whole future `!Send` —
        // which would stop a caller running it on any executor but their own
        // thread. The pointers borrow `filter`, which outlives the call.
        {
            let ptrs: Vec<_> = filter.iter().map(|u| u.as_ptr()).collect();
            unsafe { cb::peripheral_discover_services(peripheral.as_ptr(), &ptrs) };
        }
        rx.await
            .map_err(|_| Error::Aborted("service discovery was cancelled".into()))??;

        let found = unsafe { cb::peripheral_services(peripheral.as_ptr()) };
        Ok(match uuid {
            None => found,
            Some(want) => found
                .into_iter()
                .filter(|s| matches_uuid(s, want))
                .collect(),
        })
    }

    pub async fn discover_included_services(
        &self,
        id: &str,
        service: &Retained,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Retained>> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;
        if !self.is_connected(id) {
            return Err(Error::InvalidState("the device is not connected".into()));
        }

        // CoreBluetooth will not populate `includedServices` until asked, and
        // the filter is a CBUUID array exactly as for primary services.
        let filter: Vec<Retained> = uuid
            .and_then(|u| unsafe { cb::uuid_from_string(u.as_str()) })
            .into_iter()
            .collect();
        let rx = self
            .pending
            .lock()
            .unwrap()
            .included_services
            .enqueue((peripheral.key(), service.key()));
        // Scoped so the raw pointers are gone before the await. They are only
        // needed for the synchronous call, and a `Vec<*mut c_void>` held
        // across a suspension point makes the whole future `!Send`.
        {
            let ptrs: Vec<_> = filter.iter().map(|u| u.as_ptr()).collect();
            unsafe {
                cb::peripheral_discover_included_services(
                    peripheral.as_ptr(),
                    &ptrs,
                    service.as_ptr(),
                )
            };
        }
        rx.await
            .map_err(|_| Error::Aborted("included service discovery was cancelled".into()))??;

        let found = unsafe { cb::service_included(service.as_ptr()) };
        Ok(match uuid {
            None => found,
            Some(want) => found
                .into_iter()
                .filter(|s| matches_uuid(s, want))
                .collect(),
        })
    }

    pub async fn discover_characteristics(
        &self,
        id: &str,
        service: &Retained,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Retained>> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;

        let filter: Vec<Retained> = uuid
            .and_then(|u| unsafe { cb::uuid_from_string(u.as_str()) })
            .into_iter()
            .collect();
        let rx = self
            .pending
            .lock()
            .unwrap()
            .characteristics
            .enqueue((peripheral.key(), service.key()));
        // Scoped so the raw pointers are gone before the await. They are only
        // needed for the synchronous call, and a `Vec<*mut c_void>` held
        // across a suspension point makes the whole future `!Send`.
        {
            let ptrs: Vec<_> = filter.iter().map(|u| u.as_ptr()).collect();
            unsafe {
                cb::peripheral_discover_characteristics(
                    peripheral.as_ptr(),
                    &ptrs,
                    service.as_ptr(),
                )
            };
        }
        rx.await
            .map_err(|_| Error::Aborted("characteristic discovery was cancelled".into()))??;

        let found = unsafe { cb::service_characteristics(service.as_ptr()) };
        Ok(match uuid {
            None => found,
            Some(want) => found
                .into_iter()
                .filter(|c| matches_uuid(c, want))
                .collect(),
        })
    }

    pub async fn discover_descriptors(
        &self,
        id: &str,
        characteristic: &Retained,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Retained>> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;

        let rx = self
            .pending
            .lock()
            .unwrap()
            .descriptors
            .enqueue((peripheral.key(), characteristic.key()));
        unsafe {
            cb::peripheral_discover_descriptors(peripheral.as_ptr(), characteristic.as_ptr())
        };
        rx.await
            .map_err(|_| Error::Aborted("descriptor discovery was cancelled".into()))??;

        let found = unsafe { cb::characteristic_descriptors(characteristic.as_ptr()) };
        Ok(match uuid {
            None => found,
            Some(want) => found
                .into_iter()
                .filter(|d| matches_uuid(d, want))
                .collect(),
        })
    }

    pub async fn read_characteristic(
        &self,
        id: &str,
        characteristic: &Retained,
    ) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;
        let rx = self
            .pending
            .lock()
            .unwrap()
            .read_characteristic
            .enqueue((peripheral.key(), characteristic.key()));
        unsafe { cb::peripheral_read_characteristic(peripheral.as_ptr(), characteristic.as_ptr()) };
        rx.await
            .map_err(|_| Error::Aborted("read was cancelled".into()))?
    }

    pub async fn write_characteristic(
        &self,
        id: &str,
        characteristic: &Retained,
        value: &[u8],
        write_type: WriteType,
    ) -> Result<()> {
        let peripheral = self.peripheral(id)?;
        let write_type = match write_type {
            WriteType::WithResponse => cb::WriteType::WithResponse,
            WriteType::WithoutResponse => cb::WriteType::WithoutResponse,
        };

        // The per-device lock is taken only for the acknowledged form.
        //
        // It exists because ATT allows one outstanding *request* per bearer,
        // and an acknowledged write is a request. An unacknowledged one is a
        // *command* — the protocol places no such limit on it, CoreBluetooth's
        // `writeValue:` is safe to call from any thread, and the framework has
        // its own flow control for exactly this case, which the branch below
        // waits on. Taking the lock anyway would make a bulk transfer queue
        // behind every read's round trip for no protocol reason.
        let _guard = match write_type {
            cb::WriteType::WithResponse => Some(self.gatt_lock(id).await?),
            cb::WriteType::WithoutResponse => None,
        };

        match write_type {
            cb::WriteType::WithResponse => {
                let rx = self
                    .pending
                    .lock()
                    .unwrap()
                    .write_characteristic
                    .enqueue((peripheral.key(), characteristic.key()));
                unsafe {
                    cb::peripheral_write_characteristic(
                        peripheral.as_ptr(),
                        characteristic.as_ptr(),
                        value,
                        write_type,
                    )
                };
                rx.await
                    .map_err(|_| Error::Aborted("write was cancelled".into()))?
            }
            // Write-without-response has no acknowledgement, but it does have
            // flow control, and the two are easy to confuse. CoreBluetooth
            // buffers these writes and silently **discards** any that arrive
            // once the buffer is full — no error, no callback, the bytes are
            // simply gone. A transfer that ignores this does not fail, it
            // corrupts.
            //
            // So: ask whether there is room, and if not, wait to be told there
            // is. The waiter is registered before the check, because the
            // callback can land on the dispatch queue between the two.
            cb::WriteType::WithoutResponse => {
                let key = peripheral.key();
                let rx = self
                    .pending
                    .lock()
                    .unwrap()
                    .ready_to_write
                    .enqueue((key, key));

                if unsafe { cb::peripheral_can_write_without_response(peripheral.as_ptr()) } {
                    // There was room all along; drop the waiter rather than
                    // leave it to be resolved by someone else's callback.
                    self.pending
                        .lock()
                        .unwrap()
                        .ready_to_write
                        .cancel((key, key));
                } else {
                    rx.await
                        .map_err(|_| Error::Aborted("the write was cancelled".into()))??;
                }

                unsafe {
                    cb::peripheral_write_characteristic(
                        peripheral.as_ptr(),
                        characteristic.as_ptr(),
                        value,
                        write_type,
                    )
                };
                Ok(())
            }
        }
    }

    pub async fn set_notify(
        &self,
        id: &str,
        characteristic: &Retained,
        enabled: bool,
    ) -> Result<bool> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;
        let rx = self
            .pending
            .lock()
            .unwrap()
            .notify
            .enqueue((peripheral.key(), characteristic.key()));
        unsafe { cb::peripheral_set_notify(peripheral.as_ptr(), characteristic.as_ptr(), enabled) };
        rx.await
            .map_err(|_| Error::Aborted("notification change was cancelled".into()))?
    }

    pub fn subscribe(
        &self,
        characteristic: &Retained,
    ) -> webbluetooth_core::backlog::Receiver<Vec<u8>> {
        // Bounded: a notification arrives on a platform callback
        // thread that must return promptly, so there is nobody to
        // apply backpressure to. See `webbluetooth_core::backlog`.
        let (tx, rx) = webbluetooth_core::backlog::channel();
        self.notifications
            .lock()
            .unwrap()
            .entry(characteristic.key())
            .or_default()
            .push(tx);
        rx
    }

    pub async fn read_descriptor(&self, id: &str, descriptor: &Retained) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;
        let rx = self
            .pending
            .lock()
            .unwrap()
            .read_descriptor
            .enqueue((peripheral.key(), descriptor.key()));
        unsafe { cb::peripheral_read_descriptor(peripheral.as_ptr(), descriptor.as_ptr()) };
        rx.await
            .map_err(|_| Error::Aborted("descriptor read was cancelled".into()))?
    }

    pub async fn write_descriptor(
        &self,
        id: &str,
        descriptor: &Retained,
        value: &[u8],
    ) -> Result<()> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;
        let rx = self
            .pending
            .lock()
            .unwrap()
            .write_descriptor
            .enqueue((peripheral.key(), descriptor.key()));
        unsafe { cb::peripheral_write_descriptor(peripheral.as_ptr(), descriptor.as_ptr(), value) };
        rx.await
            .map_err(|_| Error::Aborted("descriptor write was cancelled".into()))?
    }

    pub async fn read_rssi(&self, id: &str) -> Result<i32> {
        let _guard = self.gatt_lock(id).await?;
        let peripheral = self.peripheral(id)?;
        let key = peripheral.key();
        let rx = self.pending.lock().unwrap().rssi.enqueue((key, key));
        unsafe { cb::peripheral_read_rssi(peripheral.as_ptr()) };
        rx.await
            .map_err(|_| Error::Aborted("RSSI read was cancelled".into()))?
    }

    pub async fn request_connection_priority(
        &self,
        _id: &str,
        _priority: webbluetooth_core::ConnectionPriority,
    ) -> Result<()> {
        Err(Error::NotSupported(
            "CoreBluetooth does not expose connection parameters at all: the \
             system negotiates them and does not offer a way to influence them"
                .into(),
        ))
    }

    pub fn max_write_len(&self, id: &str, write_type: WriteType) -> Result<usize> {
        let peripheral = self.peripheral(id)?;
        let write_type = match write_type {
            WriteType::WithResponse => cb::WriteType::WithResponse,
            WriteType::WithoutResponse => cb::WriteType::WithoutResponse,
        };
        Ok(unsafe { cb::peripheral_max_write_len(peripheral.as_ptr(), write_type) })
    }
}

/// Does this `CBAttribute`'s UUID equal `want`, whatever width CoreBluetooth
/// spells it in?
fn matches_uuid(attribute: &Retained, want: &BluetoothUuid) -> bool {
    let actual = unsafe {
        cb::uuid_string(cb::attribute_uuid(attribute.as_ptr()))
            .and_then(|s| BluetoothUuid::from_cbuuid_string(&s).ok())
    };
    actual.as_ref() == Some(want)
}

/// The canonical UUID of a `CBAttribute`.
pub fn attribute_uuid(attribute: &Retained) -> Result<BluetoothUuid> {
    unsafe { cb::uuid_string(cb::attribute_uuid(attribute.as_ptr())) }
        .ok_or_else(|| Error::InvalidState("attribute has no UUID".into()))
        .and_then(|s| BluetoothUuid::from_cbuuid_string(&s))
}

/// The TCC verdict for this process.
pub fn authorization() -> webbluetooth_core::Authorization {
    use crate::Authorization as Sys;
    match crate::authorization() {
        Sys::NotDetermined => webbluetooth_core::Authorization::NotDetermined,
        Sys::Restricted => webbluetooth_core::Authorization::Restricted,
        Sys::Denied => webbluetooth_core::Authorization::Denied,
        Sys::Allowed => webbluetooth_core::Authorization::Allowed,
    }
}

/// CoreBluetooth reports the same five states, so this is a rename rather than
/// a widening. Keeping the platform type out of the shared layer is what lets
/// there be one [`ManagerState::availability`](webbluetooth_core::ManagerState::availability) instead of one per backend.
pub fn manager_state(state: ManagerState) -> webbluetooth_core::state::ManagerState {
    use webbluetooth_core::state::ManagerState as Portable;
    match state {
        ManagerState::PoweredOn => Portable::PoweredOn,
        ManagerState::PoweredOff => Portable::PoweredOff,
        ManagerState::Unauthorized => Portable::Unauthorized,
        ManagerState::Unsupported => Portable::Unsupported,
        ManagerState::Resetting | ManagerState::Unknown => Portable::Unknown,
    }
}

fn advertisement_from_sys(adv: &cb::Advertisement, rssi: i32) -> Advertisement {
    let uuids = |v: &Vec<String>| -> Vec<BluetoothUuid> {
        v.iter()
            .filter_map(|s| BluetoothUuid::from_cbuuid_string(s).ok())
            .collect()
    };

    // CoreBluetooth hands over one blob whose first two little-endian bytes
    // are the company identifier; the spec keys the map by that identifier
    // and excludes it from the payload.
    let mut manufacturer_data = HashMap::new();
    if let Some(blob) = &adv.manufacturer_data {
        if blob.len() >= 2 {
            let company = u16::from_le_bytes([blob[0], blob[1]]);
            manufacturer_data.insert(company, blob[2..].to_vec());
        }
    }

    Advertisement {
        local_name: adv.local_name.clone(),
        tx_power: adv.tx_power,
        // CoreBluetooth does not expose it.
        appearance: None,
        is_connectable: adv.is_connectable,
        service_uuids: uuids(&adv.service_uuids),
        overflow_service_uuids: uuids(&adv.overflow_service_uuids),
        solicited_service_uuids: uuids(&adv.solicited_service_uuids),
        manufacturer_data,
        service_data: adv
            .service_data
            .iter()
            .filter_map(|(u, d)| Some((BluetoothUuid::from_cbuuid_string(u).ok()?, d.clone())))
            .collect(),
        rssi,
    }
}

/// Turn a CoreBluetooth `NSError` into the spec's `NetworkError`. The Linux
/// backend maps BlueZ's D-Bus error names instead.
#[cfg(target_vendor = "apple")]
fn network(context: &str, error: Option<(i64, String)>) -> Error {
    match error {
        Some((code, message)) => Error::Network(format!("{context}: {message} (CBError {code})")),
        None => Error::Network(context.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;

    const PERIPHERAL: usize = 0xAA;
    const OTHER_PERIPHERAL: usize = 0xBB;
    const CHARACTERISTIC: usize = 0x11;

    #[test]
    fn a_reply_goes_to_the_oldest_waiter() {
        // Two reads of the same characteristic must resolve in the order they
        // were issued — CoreBluetooth gives no request id to match on.
        let mut slots: Slots<Vec<u8>> = Slots::default();
        let key = (PERIPHERAL, CHARACTERISTIC);
        let first = slots.enqueue(key);
        let second = slots.enqueue(key);

        assert!(slots.resolve(key, Ok(vec![1])));
        assert!(slots.resolve(key, Ok(vec![2])));

        assert_eq!(block_on(first).unwrap().unwrap(), vec![1]);
        assert_eq!(block_on(second).unwrap().unwrap(), vec![2]);
    }

    #[test]
    fn a_value_with_no_waiter_is_a_notification() {
        // This is the whole basis for telling a read reply from a notification.
        let mut slots: Slots<Vec<u8>> = Slots::default();
        let key = (PERIPHERAL, CHARACTERISTIC);
        assert!(!slots.is_waiting(key));
        assert!(!slots.resolve(key, Ok(vec![9])), "should report no waiter");

        let pending = slots.enqueue(key);
        assert!(slots.is_waiting(key));
        assert!(slots.resolve(key, Ok(vec![9])));
        assert!(!slots.is_waiting(key), "queue should be empty again");
        drop(pending);
    }

    #[test]
    fn disconnect_fails_only_the_peripheral_that_dropped() {
        let mut slots: Slots<Vec<u8>> = Slots::default();
        let doomed = slots.enqueue((PERIPHERAL, CHARACTERISTIC));
        let survivor = slots.enqueue((OTHER_PERIPHERAL, CHARACTERISTIC));

        slots.fail_peripheral(PERIPHERAL, &Error::Network("link lost".into()));

        assert!(matches!(block_on(doomed).unwrap(), Err(Error::Network(_))));
        assert!(slots.is_waiting((OTHER_PERIPHERAL, CHARACTERISTIC)));

        // The survivor still resolves normally.
        assert!(slots.resolve((OTHER_PERIPHERAL, CHARACTERISTIC), Ok(vec![7])));
        assert_eq!(block_on(survivor).unwrap().unwrap(), vec![7]);
    }

    #[test]
    fn disconnect_releases_every_kind_of_pending_request() {
        // A GATT operation holds the per-device lock across its await, so a
        // request that is never failed would deadlock the device forever.
        let mut pending = Pending::default();
        let key = (PERIPHERAL, CHARACTERISTIC);
        let waiters = (
            pending.connect.enqueue((PERIPHERAL, PERIPHERAL)),
            pending.services.enqueue((PERIPHERAL, PERIPHERAL)),
            pending.read_characteristic.enqueue(key),
            pending.write_characteristic.enqueue(key),
            pending.notify.enqueue(key),
            pending.rssi.enqueue((PERIPHERAL, PERIPHERAL)),
        );

        pending.fail_peripheral(PERIPHERAL, &Error::Network("device disconnected".into()));

        assert!(block_on(waiters.0).unwrap().is_err());
        assert!(block_on(waiters.1).unwrap().is_err());
        assert!(block_on(waiters.2).unwrap().is_err());
        assert!(block_on(waiters.3).unwrap().is_err());
        assert!(block_on(waiters.4).unwrap().is_err());
        assert!(block_on(waiters.5).unwrap().is_err());
    }

    #[test]
    fn the_same_attribute_on_two_peripherals_is_two_queues() {
        // Keys are (peripheral, attribute): CoreBluetooth can hand back the
        // same pointer value for attributes of different devices over time.
        let mut slots: Slots<Vec<u8>> = Slots::default();
        let a = slots.enqueue((PERIPHERAL, CHARACTERISTIC));
        let b = slots.enqueue((OTHER_PERIPHERAL, CHARACTERISTIC));

        slots.resolve((OTHER_PERIPHERAL, CHARACTERISTIC), Ok(vec![2]));
        slots.resolve((PERIPHERAL, CHARACTERISTIC), Ok(vec![1]));

        assert_eq!(block_on(a).unwrap().unwrap(), vec![1]);
        assert_eq!(block_on(b).unwrap().unwrap(), vec![2]);
    }

    #[test]
    fn a_dropped_future_does_not_wedge_the_queue() {
        // If the caller gives up, the next reply must still find a waiter.
        let mut slots: Slots<()> = Slots::default();
        let key = (PERIPHERAL, CHARACTERISTIC);
        let abandoned = slots.enqueue(key);
        drop(abandoned);

        // The stale sender is consumed and reports success; the queue empties.
        assert!(slots.resolve(key, Ok(())));
        assert!(!slots.is_waiting(key));

        let live = slots.enqueue(key);
        assert!(slots.resolve(key, Ok(())));
        assert!(block_on(live).unwrap().is_ok());
    }

    #[test]
    // Reaches into the Objective-C runtime, which Miri cannot call.
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn adapter_reports_a_state_even_without_a_radio_grant() {
        // Proof the delegate is wired end to end: the first
        // `centralManagerDidUpdateState:` always arrives, whatever it says.
        let inner = Inner::new(false);
        let state = block_on(inner.settled_state());
        assert_ne!(
            state,
            webbluetooth_core::state::ManagerState::Unknown,
            "no state was ever reported"
        );
    }

    #[test]
    // Reaches into the Objective-C runtime, which Miri cannot call.
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn operations_refuse_early_when_the_adapter_is_unusable() {
        let inner = Inner::new(false);
        let state = block_on(inner.settled_state());
        let result = block_on(inner.require_powered_on());
        match state {
            webbluetooth_core::state::ManagerState::PoweredOn => assert!(result.is_ok()),
            _ => assert!(
                matches!(result, Err(Error::NotAvailable(_))),
                "expected NotAvailable for state {state:?}, got {result:?}"
            ),
        }
    }
}
