//! The Linux engine: BlueZ over D-Bus.
//!
//! Structurally simpler than its CoreBluetooth counterpart, because BlueZ is
//! request/reply where CoreBluetooth is delegate-driven:
//!
//! * `ReadValue` **returns the bytes in its method reply**, so there is no
//!   read-versus-notification ambiguity and none of the per-attribute FIFO
//!   correlation the Apple backend needs.
//! * Services are not discovered explicitly. Connecting resolves them, BlueZ
//!   sets `ServicesResolved`, and the objects appear in the tree — so
//!   "discovery" here is a wait followed by a tree walk.
//! * Notifications are `PropertiesChanged` on a characteristic's `Value`.
//!
//! What stays the same is everything above this file: the allowlist, the
//! blocklist, UUID canonicalisation, filters and the chooser are shared.
//!
//! # One visible platform difference
//!
//! `BluetoothDevice::id` is the peer's Bluetooth address here. BlueZ exposes it
//! and keys its object paths on it; Apple never does. Code that treats the id as
//! opaque works on both, but a Linux id is personally identifying in a way a
//! macOS one is not.

use crate::bluez::{self, interfaces, Bluez, Event as BluezEvent, Object};
use crate::dbus::Value;
use futures_channel::oneshot;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use webbluetooth_core::chooser::DeviceChooser;
use webbluetooth_core::error::{Availability, Error, Result};
use webbluetooth_core::filter::{Advertisement, RequestDeviceOptions};
use webbluetooth_core::gatt::{CharacteristicProperties, WriteType};
use webbluetooth_core::registry::DeviceRegistry;
use webbluetooth_core::scan::{self, ScanHub};
use webbluetooth_core::state::ManagerState;
use webbluetooth_core::timer;
use webbluetooth_core::uuid::BluetoothUuid;

/// A handle into a device's attribute table.
///
/// A D-Bus object path plus the UUID it was resolved as. Carrying the UUID
/// makes the handle self-describing, matching the Apple backend where the same
/// question is answered by messaging the object.
#[derive(Clone, Debug)]
pub struct Handle(Arc<HandleData>);

#[derive(Debug)]
struct HandleData {
    path: String,
    uuid: BluetoothUuid,
}

impl Handle {
    fn new(path: &str, uuid: BluetoothUuid) -> Self {
        Self(Arc::new(HandleData {
            path: path.to_owned(),
            uuid,
        }))
    }
    fn path(&self) -> &str {
        &self.0.path
    }
}

/// The canonical UUID of an attribute handle.
pub fn attribute_uuid(handle: &Handle) -> Result<BluetoothUuid> {
    Ok(handle.0.uuid)
}

/// The scan a restored session was running. Never populated on Linux — BlueZ
/// has no state-restoration concept — but the shape is shared.
#[derive(Debug, Default, Clone)]
pub struct RestoredScan {
    pub device_ids: Vec<String>,
    pub scan_services: Vec<BluetoothUuid>,
    pub scan_allow_duplicates: bool,
}

/// What the BlueZ backend needs to reach a device: its object path.
/// Everything else about a grant lives in [`webbluetooth_core::registry`].
type DeviceData = String;

pub struct Inner {
    bluez: Mutex<Option<Bluez>>,
    /// Why BlueZ could not be reached, if it could not.
    unavailable: Mutex<Option<Availability>>,
    /// Everyone watching the scan. The radio is shared, so the scan is
    /// reference-counted rather than owned by whoever started it.
    hub: ScanHub,
    /// Bluetooth address to D-Bus object path, for every device seen this
    /// session. Kept outside the watchers so a finished `requestDevice` can
    /// still resolve the device its chooser picked.
    sightings: Mutex<HashMap<String, String>>,
    devices: DeviceRegistry<DeviceData>,
    notifications: Mutex<HashMap<String, Vec<webbluetooth_core::backlog::Sender<Vec<u8>>>>>,
    /// Resolved when a device's `ServicesResolved` becomes true.
    services_resolved: Mutex<HashMap<String, Vec<oneshot::Sender<()>>>>,
    /// Resolved when `Connected` becomes true.
    connected_waiters: Mutex<HashMap<String, Vec<oneshot::Sender<()>>>>,
    /// Sockets `AcquireWrite` handed over, by characteristic path.
    ///
    /// Held for the life of the connection: acquiring is itself a D-Bus round
    /// trip, so doing it per write would cost exactly what it saves.
    write_sockets: Mutex<HashMap<String, Arc<WriteSocket>>>,
    /// Devices whose GATT tree has finished resolving at least once.
    ///
    /// Services appearing is only a *change* after that: during the first
    /// discovery they are appearing for the first time, and treating that as a
    /// change would invalidate the handles the caller is in the middle of
    /// being given.
    resolved_once: Mutex<HashSet<String>>,
}

/// A socket BlueZ handed over for unacknowledged writes.
///
/// `SOCK_SEQPACKET`, so one write is one ATT PDU — there is no partial write
/// to loop over and no framing to add. The kernel's send buffer provides the
/// flow control that `WriteValue` gets from waiting for a D-Bus reply.
struct WriteSocket {
    fd: std::os::fd::RawFd,
    /// The largest payload this socket accepts, as BlueZ reported it.
    mtu: u16,
}

impl WriteSocket {
    fn write(&self, value: &[u8]) -> std::io::Result<()> {
        // SAFETY: the descriptor is owned by this struct and closed only in
        // its `Drop`.
        let n = unsafe { crate::sys::write(self.fd, value.as_ptr(), value.len()) };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n as usize != value.len() {
            // A sequenced-packet socket does not do partial writes; if it
            // somehow did, the peer received a truncated value and the caller
            // must know.
            return Err(std::io::Error::other(format!(
                "wrote {n} of {} bytes",
                value.len()
            )));
        }
        Ok(())
    }
}

impl Drop for WriteSocket {
    fn drop(&mut self) {
        unsafe { crate::sys::close(self.fd) };
    }
}

// `write` and `close` are declared once, in `sys` — this adapter used to
// declare its own because it was in another crate, and the two disagreed about
// whether the buffer was `*const u8` or `*const c_void`.

struct Sink(Weak<Inner>);

impl bluez::EventSink for Sink {
    fn emit(&self, event: BluezEvent) {
        if let Some(inner) = self.0.upgrade() {
            inner.handle(event);
        }
    }
}

/// `/org/bluez/hci0` shortened to `hci0`.
///
/// The object path is what BlueZ uses internally, but `hci0` is what
/// `hciconfig`, `bluetoothctl` and `WEBBLUETOOTH_ADAPTER` all use, so that is
/// what a caller is shown. `select_adapter` takes either.
fn short_adapter_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

impl Inner {
    pub fn new(show_power_alert: bool) -> Arc<Self> {
        Self::with_restoration(show_power_alert, None, BTreeSet::new())
    }

    /// `restore_identifier` is accepted and ignored: BlueZ has no state
    /// preservation, and nothing on Linux relaunches a process for Bluetooth.
    pub fn with_restoration(
        _show_power_alert: bool,
        _restore_identifier: Option<&str>,
        _restore_allowed: BTreeSet<BluetoothUuid>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak: &Weak<Inner>| {
            let inner = Inner {
                bluez: Mutex::new(None),
                unavailable: Mutex::new(None),
                hub: ScanHub::new(),
                sightings: Mutex::new(HashMap::new()),
                devices: DeviceRegistry::default(),
                notifications: Mutex::new(HashMap::new()),
                services_resolved: Mutex::new(HashMap::new()),
                connected_waiters: Mutex::new(HashMap::new()),
                write_sockets: Mutex::new(HashMap::new()),
                resolved_once: Mutex::new(HashSet::new()),
            };
            match Bluez::new(Arc::new(Sink(weak.clone()))) {
                Ok(bluez) => {
                    if bluez.adapter().is_none() {
                        *inner.unavailable.lock().unwrap() = Some(Availability::Unsupported);
                    }
                    *inner.bluez.lock().unwrap() = Some(bluez);
                }
                // A missing bus or a refused connection is the Linux shape of
                // "not authorised": bluetoothd is not reachable from here.
                Err(_) => {
                    *inner.unavailable.lock().unwrap() = Some(Availability::Unauthorized);
                }
            }
            inner
        })
    }

    fn with_bluez<T>(&self, f: impl FnOnce(&Bluez) -> T) -> Result<T> {
        let guard = self.bluez.lock().unwrap();
        let bluez = guard
            .as_ref()
            .ok_or(Error::NotAvailable(Availability::Unauthorized))?;
        Ok(f(bluez))
    }

    // ── Controllers ─────────────────────────────────────────────────────────
    //
    // BlueZ is the platform that genuinely has several: every controller is an
    // `org.bluez.Adapter1` object in the tree this backend already mirrors, so
    // enumerating is a filter over a map that is always up to date.

    pub async fn adapters(&self) -> Result<Vec<webbluetooth_core::AdapterInfo>> {
        self.with_bluez(|b| {
            let current = b.adapter();
            b.adapters()
                .into_iter()
                .map(|path| {
                    let object = b.object(&path);
                    webbluetooth_core::AdapterInfo {
                        // `hci0` rather than the full object path: it is what
                        // `hciconfig` and `bluetoothctl` call it, and
                        // `select_adapter` accepts either.
                        name: object
                            .as_ref()
                            .and_then(|o| o.string(interfaces::ADAPTER, "Alias")),
                        address: object
                            .as_ref()
                            .and_then(|o| o.string(interfaces::ADAPTER, "Address")),
                        powered: object
                            .as_ref()
                            .and_then(|o| o.bool(interfaces::ADAPTER, "Powered"))
                            .unwrap_or(false),
                        is_default: current.as_deref() == Some(path.as_str()),
                        id: short_adapter_name(&path),
                    }
                })
                .collect()
        })
    }

    pub async fn adapter(&self) -> Result<webbluetooth_core::AdapterInfo> {
        let adapters = self.adapters().await?;
        adapters
            .into_iter()
            .find(webbluetooth_core::AdapterInfo::is_default)
            .ok_or(Error::NotAvailable(Availability::Unsupported))
    }

    pub async fn select_adapter(&self, id: &str) -> Result<()> {
        self.with_bluez(|b| b.select_adapter(id))?
            .map_err(|e| Error::NotFound(e.to_string()))
    }

    // ── Events ──────────────────────────────────────────────────────────────

    fn handle(&self, event: BluezEvent) {
        match event {
            BluezEvent::DeviceSeen { path, properties } => self.on_device_seen(&path, &properties),

            BluezEvent::Connected { path } => {
                if let Some(id) = self.id_for_path(&path) {
                    self.devices.update(&id, |d| d.connected = true);
                    for tx in self
                        .connected_waiters
                        .lock()
                        .unwrap()
                        .remove(&id)
                        .into_iter()
                        .flatten()
                    {
                        let _ = tx.send(());
                    }
                }
            }

            BluezEvent::Disconnected { path } => {
                let Some(id) = self.id_for_path(&path) else {
                    return;
                };
                // Waiters would otherwise hang until their own timeout.
                for tx in self
                    .connected_waiters
                    .lock()
                    .unwrap()
                    .remove(&id)
                    .into_iter()
                    .flatten()
                {
                    drop(tx);
                }
                for tx in self
                    .services_resolved
                    .lock()
                    .unwrap()
                    .remove(&id)
                    .into_iter()
                    .flatten()
                {
                    drop(tx);
                }
                // The next resolve is a first discovery again, not a change.
                self.resolved_once.lock().unwrap().remove(&id);
                // The sockets belong to the connection that just ended. Kept,
                // they would be stale descriptors for characteristics that no
                // longer exist — and a reconnect would write into them.
                self.write_sockets
                    .lock()
                    .unwrap()
                    .retain(|characteristic, _| !characteristic.starts_with(&*path));
                self.devices.mark_disconnected(&id);
            }

            BluezEvent::ServicesResolved { path } => {
                let Some(id) = self.id_for_path(&path) else {
                    return;
                };
                // Resolving a second time means BlueZ rebuilt the tree, which
                // it does after a Service Changed indication.
                if !self.resolved_once.lock().unwrap().insert(id.clone()) {
                    self.devices.mark_services_changed(&id);
                }
                for tx in self
                    .services_resolved
                    .lock()
                    .unwrap()
                    .remove(&id)
                    .into_iter()
                    .flatten()
                {
                    let _ = tx.send(());
                }
            }

            BluezEvent::ServicesChanged { path } => {
                let Some(id) = self.id_for_path(&path) else {
                    return;
                };
                if self.resolved_once.lock().unwrap().contains(&id) {
                    self.devices.mark_services_changed(&id);
                }
            }

            BluezEvent::CharacteristicValue { path, value } => {
                let mut subscribers = self.notifications.lock().unwrap();
                if let Some(list) = subscribers.get_mut(&*path) {
                    list.retain(|tx| tx.send(value.clone()).is_ok());
                    if list.is_empty() {
                        subscribers.remove(&*path);
                    }
                }
            }

            BluezEvent::DeviceRemoved { .. }
            | BluezEvent::AdapterChanged { .. }
            | BluezEvent::Disconnected_ => {}
        }
    }

    fn on_device_seen(&self, path: &str, properties: &HashMap<String, Value>) {
        if !self.hub.is_watching() {
            return;
        }

        // A PropertiesChanged sighting carries only what changed, so merge it
        // over the cached object rather than trusting it alone.
        let cached = self
            .bluez
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|b| b.object(path));
        let mut merged: HashMap<String, Value> = cached
            .as_ref()
            .and_then(|o| o.interfaces.get(interfaces::DEVICE).cloned())
            .unwrap_or_default();
        for (k, v) in properties {
            merged.insert(k.clone(), v.clone());
        }

        let Some(address) = merged
            .get("Address")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let name = merged
            .get("Name")
            .or_else(|| merged.get("Alias"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let advertisement = advertisement_from(&merged);

        self.sightings
            .lock()
            .unwrap()
            .insert(address.clone(), path.to_owned());
        // Each watcher applies its own filter; this one just reports.
        self.hub.publish(&address, name.as_deref(), &advertisement);
    }

    pub fn scan_hub(&self) -> &ScanHub {
        &self.hub
    }

    /// Start or stop the adapter's discovery.
    ///
    /// Starting is idempotent and re-applies the filter, because a new watcher
    /// can widen it and BlueZ only accepts a filter change while discovery is
    /// stopped.
    pub fn set_radio_scanning(&self, on: bool) -> Result<()> {
        let adapter = self
            .with_bluez(|b| b.adapter())?
            .ok_or(Error::NotAvailable(Availability::Unsupported))?;

        if !on {
            let _ =
                self.with_bluez(|b| b.call(&adapter, interfaces::ADAPTER, "StopDiscovery", vec![]));
            return Ok(());
        }

        // A discovery filter narrows what bluetoothd reports and, with
        // Transport=le, keeps classic devices out of the results entirely.
        let mut filter = vec![("Transport".to_string(), Value::Str("le".into()))];
        let services = self.hub.scan_services();
        if !services.is_empty() {
            filter.push((
                "UUIDs".to_string(),
                Value::string_array(services.iter().map(|u| u.as_str().to_owned())),
            ));
        }
        filter.push((
            "DuplicateData".to_string(),
            Value::Bool(self.hub.wants_duplicates()),
        ));

        let result = self.with_bluez(|b| {
            let _ = b.call(&adapter, interfaces::ADAPTER, "StopDiscovery", vec![]);
            let _ = b.call(
                &adapter,
                interfaces::ADAPTER,
                "SetDiscoveryFilter",
                vec![Value::dict(filter)],
            );
            b.call(&adapter, interfaces::ADAPTER, "StartDiscovery", vec![])
        })?;
        result.map_err(|e| Error::Network(format!("could not start discovery: {e}")))?;

        // Devices BlueZ already knows about do not re-announce themselves, so
        // replay the cache rather than waiting on signals that will not come.
        self.replay_known_devices();
        Ok(())
    }

    fn id_for_path(&self, path: &str) -> Option<String> {
        self.devices.find(|d| d.inner == path)
    }

    // ── Availability ────────────────────────────────────────────────────────

    pub fn state(&self) -> ManagerState {
        if let Some(a) = *self.unavailable.lock().unwrap() {
            return match a {
                Availability::Unsupported => ManagerState::Unsupported,
                Availability::Unauthorized => ManagerState::Unauthorized,
                _ => ManagerState::Unknown,
            };
        }
        match self.with_bluez(|b| b.is_powered()) {
            Ok(true) => ManagerState::PoweredOn,
            Ok(false) => ManagerState::PoweredOff,
            Err(_) => ManagerState::Unauthorized,
        }
    }

    /// BlueZ answers synchronously, so there is nothing to settle — unlike
    /// CoreBluetooth, which reports `Unknown` until its first callback.
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
            || self.replay_known_devices(),
        )
        .await?;
        let Some(path) = self.sightings.lock().unwrap().get(&id).cloned() else {
            return Err(Error::NotFound(format!(
                "the chooser returned {id:?}, which was not among the devices it was offered"
            )));
        };

        let object = self.with_bluez(|b| b.object(&path))?;
        let name = object.as_ref().and_then(|o| {
            o.string(interfaces::DEVICE, "Name")
                .or_else(|| o.string(interfaces::DEVICE, "Alias"))
        });
        let connected = object
            .as_ref()
            .and_then(|o| o.bool(interfaces::DEVICE, "Connected"))
            .unwrap_or(false);

        self.devices.insert(&id, name, allowed, connected, path);
        Ok(id)
    }

    /// Write down the socket `AcquireWrite` gives us, if there is one.
    ///
    /// `Ok(None)` means there is no socket and the caller should fall back to
    /// `WriteValue`: BlueZ only offers this when nothing else holds the
    /// characteristic, and older versions do not offer it at all. That is a
    /// slower path, not a broken one, so it is not an error.
    ///
    /// The gain is the round trip: `WriteValue` is a D-Bus method call, so
    /// every packet crosses into `bluetoothd` and back. This socket is the
    /// same one `bluetoothd` would write to, handed over directly, with the
    /// kernel's own send buffer for flow control.
    async fn write_over_socket(
        &self,
        id: &str,
        handle: &Handle,
        value: &[u8],
    ) -> Result<Option<()>> {
        let Some(socket) = self.acquire_write(id, handle).await else {
            return Ok(None);
        };
        // The socket is `SOCK_SEQPACKET`: one write is one PDU, so a value
        // larger than the MTU it reported cannot be split and must go the
        // other way rather than be silently truncated.
        if value.len() > socket.mtu as usize {
            return Ok(None);
        }
        match socket.write(value) {
            Ok(()) => Ok(Some(())),
            // The peer went away or the socket was revoked — BlueZ takes it
            // back when someone else acquires the characteristic. Forget it
            // and let the caller use `WriteValue`.
            Err(e) => {
                self.write_sockets.lock().unwrap().remove(handle.path());
                Err(Error::Network(format!("the write socket failed: {e}")))
            }
        }
    }

    /// The socket for this characteristic, acquiring one if needed.
    async fn acquire_write(&self, id: &str, handle: &Handle) -> Option<Arc<WriteSocket>> {
        if let Some(socket) = self.write_sockets.lock().unwrap().get(handle.path()) {
            return Some(socket.clone());
        }
        if !self.is_connected(id) {
            return None;
        }

        // An empty options dictionary: no offset, no MTU preference.
        let acquired = self
            .call_with_fds(
                handle.path(),
                interfaces::CHARACTERISTIC,
                "AcquireWrite",
                vec![Value::dict([])],
            )
            .await
            .ok()?;

        let (body, fds) = acquired;
        let mtu = body.get(1).and_then(Value::as_u64)? as u16;
        let fd = fds.into_iter().next()?;
        // Three bytes of ATT header are already accounted for by BlueZ: the
        // MTU it reports is the payload this socket carries.
        let socket = Arc::new(WriteSocket { fd, mtu });
        self.write_sockets
            .lock()
            .unwrap()
            .insert(handle.path().to_owned(), socket.clone());
        Some(socket)
    }

    /// Adopt a device by address.
    ///
    /// BlueZ keeps an object for every device it has ever paired with or seen
    /// recently, so the usual case is that one already exists. Its path is
    /// derived from the address rather than searched for — BlueZ names them
    /// `dev_AA_BB_CC_DD_EE_FF` under the adapter — and the path is checked
    /// against the tree rather than assumed, so an unknown device is a
    /// `NotFound` and not a handle that fails later.
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
        let adapter = self
            .with_bluez(|b| b.adapter())?
            .ok_or(Error::NotAvailable(Availability::Unsupported))?;
        let path = format!("{adapter}/dev_{}", id.to_uppercase().replace(':', "_"));

        let object = self.with_bluez(|b| b.object(&path))?;
        let Some(object) = object.filter(|o| o.has(interfaces::DEVICE)) else {
            return Err(Error::NotFound(format!(
                "BlueZ has no device object for {id}; it has to have been seen or \
                 paired at least once"
            )));
        };

        let name = object
            .string(interfaces::DEVICE, "Name")
            .or_else(|| object.string(interfaces::DEVICE, "Alias"));
        let connected = object
            .bool(interfaces::DEVICE, "Connected")
            .unwrap_or(false);

        self.sightings
            .lock()
            .unwrap()
            .insert(id.to_owned(), path.clone());
        self.devices.insert(id, name, allowed, connected, path);
        Ok(id.to_owned())
    }

    /// Feed already-known devices to an in-progress scan.
    fn replay_known_devices(&self) {
        let Ok(paths) = self.with_bluez(|b| b.paths_with(interfaces::DEVICE)) else {
            return;
        };
        for path in paths {
            let Ok(Some(object)) = self.with_bluez(|b| b.object(&path)) else {
                continue;
            };
            let Some(props) = object.interfaces.get(interfaces::DEVICE) else {
                continue;
            };
            self.on_device_seen(&path, props);
        }
    }

    // ── Device access ───────────────────────────────────────────────────────

    // The registry owns grants, generations and the GATT lock; these forward.
    fn device_path(&self, id: &str) -> Result<String> {
        self.devices.get(id, |d| d.inner.clone())
    }

    // The registry answers these identically for every backend.
    webbluetooth_core::registry::forward_to_registry!();

    // ── PHY ─────────────────────────────────────────────────────────────────

    /// BlueZ has no D-Bus API for the PHY. The kernel knows, and `btmgmt` can
    /// be made to say, but nothing on the bus carries it.
    pub async fn phy(&self, id: &str) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "BlueZ does not expose the connection PHY over D-Bus".into(),
        ))
    }

    /// Asking is not possible where even reading is not.
    pub async fn set_preferred_phy(
        &self,
        id: &str,
        _tx: webbluetooth_core::Phy,
        _rx: webbluetooth_core::Phy,
    ) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "BlueZ does not expose the connection PHY over D-Bus".into(),
        ))
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    /// `org.bluez.Device1.Pair`.
    ///
    /// The call returns when the ceremony finishes, so there is nothing to
    /// poll. If the peer wants a passkey or a confirmation, BlueZ asks the
    /// registered agent — usually the desktop's — and without one the call
    /// fails rather than hanging, which is the right way round.
    pub async fn pair(&self, id: &str) -> Result<webbluetooth_core::Pairing> {
        if self.is_paired(id).await? {
            return Ok(webbluetooth_core::Pairing::AlreadyPaired);
        }
        let Some(path) = self.sightings.lock().unwrap().get(id).cloned() else {
            return Err(Error::NotFound(format!(
                "BlueZ has no device object for {id}"
            )));
        };
        self.call(&path, interfaces::DEVICE, "Pair", vec![]).await?;
        Ok(webbluetooth_core::Pairing::Paired)
    }

    pub async fn is_paired(&self, id: &str) -> Result<bool> {
        let Some(path) = self.sightings.lock().unwrap().get(id).cloned() else {
            return Ok(false);
        };
        Ok(self
            .with_bluez(|b| b.object(&path))?
            .and_then(|o| o.bool(interfaces::DEVICE, "Paired"))
            .unwrap_or(false))
    }

    /// Not reachable from an application on this platform.
    ///
    /// BlueZ keeps the negotiated parameters in the kernel; no D-Bus property
    /// carries them.
    pub async fn connection_parameters(
        &self,
        id: &str,
    ) -> Result<webbluetooth_core::ConnectionParameters> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "BlueZ does not expose negotiated connection parameters over D-Bus".into(),
        ))
    }

    async fn gatt_lock(&self, id: &str) -> Result<futures_util::lock::OwnedMutexGuard<()>> {
        self.devices.gatt_lock(id).await
    }

    /// Revoke a grant, dropping the link with it.
    pub fn forget(&self, id: &str) {
        if let Some(device) = self.sightings.lock().unwrap().get(id).cloned() {
            self.write_sockets
                .lock()
                .unwrap()
                .retain(|characteristic, _| !characteristic.starts_with(&device));
        }
        if let Some(device) = self.devices.remove(id) {
            if device.connected {
                let _ = self.with_bluez(|b| {
                    b.call(&device.inner, interfaces::DEVICE, "Disconnect", vec![])
                });
            }
        }
    }

    // ── Attribute accessors, answered from the cached tree ──────────────────

    fn object(&self, path: &str) -> Option<Object> {
        self.with_bluez(|b| b.object(path)).ok().flatten()
    }

    pub fn service_is_primary(&self, handle: &Handle) -> bool {
        self.object(handle.path())
            .and_then(|o| o.bool(interfaces::SERVICE, "Primary"))
            .unwrap_or(true)
    }

    pub fn characteristic_properties(&self, handle: &Handle) -> CharacteristicProperties {
        // BlueZ reports properties as flag strings rather than a bitmask.
        let flags = self
            .object(handle.path())
            .and_then(|o| o.property(interfaces::CHARACTERISTIC, "Flags").cloned())
            .map(|v| v.as_strings())
            .unwrap_or_default();
        CharacteristicProperties(flags.iter().fold(0, |bits, flag| {
            bits | match flag.as_str() {
                "broadcast" => CharacteristicProperties::BROADCAST,
                "read" => CharacteristicProperties::READ,
                "write-without-response" => CharacteristicProperties::WRITE_WITHOUT_RESPONSE,
                "write" => CharacteristicProperties::WRITE,
                "notify" => CharacteristicProperties::NOTIFY,
                "indicate" => CharacteristicProperties::INDICATE,
                "authenticated-signed-writes" => {
                    CharacteristicProperties::AUTHENTICATED_SIGNED_WRITES
                }
                // BlueZ lists these separately, so keep them separate: the
                // presence of the descriptor and what it says are different
                // questions.
                "extended-properties" => CharacteristicProperties::EXTENDED_PROPERTIES,
                "reliable-write" => CharacteristicProperties::RELIABLE_WRITE,
                "writable-auxiliaries" => CharacteristicProperties::WRITABLE_AUXILIARIES,
                "encrypt-notify" | "encrypt-authenticated-notify" => {
                    CharacteristicProperties::NOTIFY_ENCRYPTION_REQUIRED
                }
                "encrypt-indicate" | "encrypt-authenticated-indicate" => {
                    CharacteristicProperties::INDICATE_ENCRYPTION_REQUIRED
                }
                _ => 0,
            }
        }))
    }

    pub fn cached_value(&self, handle: &Handle) -> Option<Vec<u8>> {
        self.object(handle.path())?
            .property(interfaces::CHARACTERISTIC, "Value")?
            .as_bytes()
    }

    pub fn is_notifying(&self, handle: &Handle) -> bool {
        self.object(handle.path())
            .and_then(|o| o.bool(interfaces::CHARACTERISTIC, "Notifying"))
            .unwrap_or(false)
    }

    // ── GATT operations ─────────────────────────────────────────────────────

    /// Wrap BlueZ's callback-based call in a future.
    /// As [`Inner::call`], keeping any descriptors the reply carried.
    async fn call_with_fds(
        &self,
        path: &str,
        interface: &str,
        member: &str,
        args: Vec<Value>,
    ) -> Result<(Vec<Value>, Vec<std::os::fd::RawFd>)> {
        let (tx, rx) = oneshot::channel();
        self.with_bluez(|b| {
            b.call_async_with_fds(path, interface, member, args, move |reply| {
                let _ = tx.send(reply);
            });
        })?;
        rx.await
            .map_err(|_| Error::Network("the D-Bus call was dropped".into()))?
            .map_err(|e| Error::Network(e.to_string()))
    }

    async fn call(
        &self,
        path: &str,
        interface: &str,
        member: &str,
        args: Vec<Value>,
    ) -> Result<Vec<Value>> {
        let (tx, rx) = oneshot::channel();
        self.with_bluez(|b| {
            b.call_async(path, interface, member, args, move |reply| {
                let _ = tx.send(reply.map(|m| m.body));
            });
        })?;
        match rx.await {
            Ok(Ok(body)) => Ok(body),
            Ok(Err(e)) => Err(map_error(member, e)),
            Err(_) => Err(Error::Aborted(format!("{member} was cancelled"))),
        }
    }

    pub async fn connect(&self, id: &str) -> Result<()> {
        self.require_powered_on().await?;
        if self.is_connected(id) {
            return Ok(());
        }
        let path = self.device_path(id)?;

        // Register interest before calling: `Connected` can change before the
        // reply lands.
        let connected = {
            let (tx, rx) = oneshot::channel();
            self.connected_waiters
                .lock()
                .unwrap()
                .entry(id.into())
                .or_default()
                .push(tx);
            rx
        };
        let resolved = {
            let (tx, rx) = oneshot::channel();
            self.services_resolved
                .lock()
                .unwrap()
                .entry(id.into())
                .or_default()
                .push(tx);
            rx
        };

        self.call(&path, interfaces::DEVICE, "Connect", vec![])
            .await?;
        let _ = connected.await;
        self.devices.update(id, |d| d.connected = true);

        // BlueZ populates the GATT tree asynchronously after connecting, so a
        // read issued too early finds no objects at all. Wait for
        // `ServicesResolved`, but do not fail if it is already true.
        if !self.services_are_resolved(&path) {
            let _ = timer::timeout(Duration::from_secs(20), resolved).await;
        }
        Ok(())
    }

    fn services_are_resolved(&self, path: &str) -> bool {
        self.object(path)
            .and_then(|o| o.bool(interfaces::DEVICE, "ServicesResolved"))
            .unwrap_or(false)
    }

    pub fn disconnect(&self, id: &str) {
        if let Ok(path) = self.device_path(id) {
            let _ = self.with_bluez(|b| {
                b.call_async(&path, interfaces::DEVICE, "Disconnect", vec![], |_| {})
            });
        }
    }

    pub async fn discover_services(
        &self,
        id: &str,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        let path = self.device_path(id)?;
        if !self.is_connected(id) {
            return Err(Error::InvalidState("the device is not connected".into()));
        }
        Ok(self.children(&path, interfaces::SERVICE, uuid))
    }

    pub async fn discover_characteristics(
        &self,
        id: &str,
        service: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        Ok(self.children(service.path(), interfaces::CHARACTERISTIC, uuid))
    }

    pub async fn discover_descriptors(
        &self,
        id: &str,
        characteristic: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        Ok(self.children(characteristic.path(), interfaces::DESCRIPTOR, uuid))
    }

    /// The services an `Includes` declaration points at.
    ///
    /// Unlike characteristics, an included service is not a child of the
    /// service that includes it: BlueZ lists it as an array of object paths in
    /// the `Includes` property, and the object itself sits beside its includer
    /// under the device. So this resolves paths rather than walking the tree.
    pub async fn discover_included_services(
        &self,
        id: &str,
        service: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        if !self.is_connected(id) {
            return Err(Error::InvalidState("the device is not connected".into()));
        }

        // `Includes` is optional, and absent rather than empty when a service
        // includes nothing.
        let included = self
            .object(service.path())
            .and_then(|o| o.property(interfaces::SERVICE, "Includes").cloned());
        let Some(included) = included else {
            return Ok(Vec::new());
        };

        let mut out = Vec::new();
        for entry in included.as_array().unwrap_or(&[]) {
            let Some(path) = entry.peel().as_str() else {
                continue;
            };
            let Some(raw) = self
                .object(path)
                .and_then(|o| o.string(interfaces::SERVICE, "UUID"))
            else {
                continue;
            };
            let Ok(found) = BluetoothUuid::parse(&raw) else {
                continue;
            };
            if uuid.is_some_and(|want| *want != found) {
                continue;
            }
            out.push(Handle::new(path, found));
        }
        Ok(out)
    }

    /// Walk the cached tree for children of `parent` on `interface`.
    fn children(&self, parent: &str, interface: &str, uuid: Option<&BluetoothUuid>) -> Vec<Handle> {
        let Ok(paths) = self.with_bluez(|b| b.children_with(parent, interface)) else {
            return Vec::new();
        };
        paths
            .into_iter()
            .filter_map(|p| {
                let raw = self.object(&p)?.string(interface, "UUID")?;
                let found = BluetoothUuid::parse(&raw).ok()?;
                match uuid {
                    Some(want) if *want != found => None,
                    _ => Some(Handle::new(&p, found)),
                }
            })
            .collect()
    }

    pub async fn read_characteristic(&self, id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let body = self
            .call(
                handle.path(),
                interfaces::CHARACTERISTIC,
                "ReadValue",
                vec![Value::dict([])],
            )
            .await?;
        body.first()
            .and_then(Value::as_bytes)
            .ok_or_else(|| Error::Network("ReadValue returned no bytes".into()))
    }

    pub async fn write_characteristic(
        &self,
        id: &str,
        handle: &Handle,
        value: &[u8],
        write_type: WriteType,
    ) -> Result<()> {
        // An unacknowledged write can go straight down a socket BlueZ hands
        // out, skipping the round trip into `bluetoothd` that `WriteValue`
        // costs on every packet. Tried first, and only for this write type —
        // `AcquireWrite` has no acknowledged form.
        if write_type == WriteType::WithoutResponse {
            if let Some(sent) = self.write_over_socket(id, handle, value).await? {
                return Ok(sent);
            }
        }

        // Only the acknowledged form takes the lock. `WriteValue` with
        // `type: "command"` is an independent D-Bus call that BlueZ turns into
        // an ATT command, which is not subject to the one-outstanding-request
        // rule the lock exists for.
        let _guard = match write_type {
            WriteType::WithResponse => Some(self.gatt_lock(id).await?),
            WriteType::WithoutResponse => None,
        };
        let kind = match write_type {
            WriteType::WithResponse => "request",
            WriteType::WithoutResponse => "command",
        };
        self.call(
            handle.path(),
            interfaces::CHARACTERISTIC,
            "WriteValue",
            vec![
                Value::bytes(value),
                Value::dict([("type".to_string(), Value::Str(kind.into()))]),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn set_notify(&self, id: &str, handle: &Handle, enabled: bool) -> Result<bool> {
        let _guard = self.gatt_lock(id).await?;
        let member = if enabled { "StartNotify" } else { "StopNotify" };
        self.call(handle.path(), interfaces::CHARACTERISTIC, member, vec![])
            .await?;
        Ok(enabled)
    }

    pub fn subscribe(&self, handle: &Handle) -> webbluetooth_core::backlog::Receiver<Vec<u8>> {
        // Bounded: a notification arrives on a platform callback
        // thread that must return promptly, so there is nobody to
        // apply backpressure to. See `webbluetooth_core::backlog`.
        let (tx, rx) = webbluetooth_core::backlog::channel();
        self.notifications
            .lock()
            .unwrap()
            .entry(handle.path().to_owned())
            .or_default()
            .push(tx);
        rx
    }

    pub async fn read_descriptor(&self, id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let body = self
            .call(
                handle.path(),
                interfaces::DESCRIPTOR,
                "ReadValue",
                vec![Value::dict([])],
            )
            .await?;
        body.first()
            .and_then(Value::as_bytes)
            .ok_or_else(|| Error::Network("ReadValue returned no bytes".into()))
    }

    pub async fn write_descriptor(&self, id: &str, handle: &Handle, value: &[u8]) -> Result<()> {
        let _guard = self.gatt_lock(id).await?;
        self.call(
            handle.path(),
            interfaces::DESCRIPTOR,
            "WriteValue",
            vec![Value::bytes(value), Value::dict([])],
        )
        .await?;
        Ok(())
    }

    pub async fn read_rssi(&self, id: &str) -> Result<i32> {
        let path = self.device_path(id)?;
        // BlueZ keeps RSSI as a property rather than offering a read, and drops
        // the property entirely once a device is no longer being advertised at.
        // 127 is the "no reading" sentinel, which must not be handed back as if
        // it were a measurement.
        self.object(&path)
            .and_then(|o| o.int(interfaces::DEVICE, "RSSI"))
            .map(|v| v as i32)
            .filter(|v| *v != webbluetooth_core::filter::UNAVAILABLE_RSSI)
            .ok_or_else(|| Error::NotSupported("no RSSI is available for this device".into()))
    }

    pub async fn request_connection_priority(
        &self,
        _id: &str,
        _priority: webbluetooth_core::ConnectionPriority,
    ) -> Result<()> {
        Err(Error::NotSupported(
            "BlueZ has no D-Bus API for connection parameters; they are set \
             through the kernel management socket, which bluetoothd owns"
                .into(),
        ))
    }

    pub fn max_write_len(&self, id: &str, _write_type: WriteType) -> Result<usize> {
        // BlueZ exposes MTU per characteristic; without one, fall back to the
        // smallest an ATT write is guaranteed to carry.
        let path = self.device_path(id)?;
        let mtu = self
            .with_bluez(|b| b.children_with(&path, interfaces::CHARACTERISTIC))?
            .into_iter()
            .find_map(|p| self.object(&p)?.int(interfaces::CHARACTERISTIC, "MTU"));
        Ok(mtu.map(|m| (m as usize).saturating_sub(3)).unwrap_or(20))
    }

    pub async fn l2cap_target(&self, id: &str, _psm: u16) -> Result<(String, String)> {
        let path = self.device_path(id)?;
        let address = self
            .object(&path)
            .and_then(|o| o.string(interfaces::DEVICE, "Address"))
            .ok_or_else(|| Error::InvalidState("the device has no address".into()))?;
        let address_type = self
            .object(&path)
            .and_then(|o| o.string(interfaces::DEVICE, "AddressType"))
            .unwrap_or_else(|| "public".into());
        Ok((address, address_type))
    }
}

/// Turn a BlueZ D-Bus error into the spec's vocabulary.
fn map_error(member: &str, e: crate::dbus::connection::Error) -> Error {
    use crate::dbus::connection::Error as D;
    match &e {
        D::Call { name, message } => match name.as_str() {
            "org.bluez.Error.NotPermitted" | "org.bluez.Error.NotAuthorized" => {
                Error::Security(format!("{member} refused: {message}"))
            }
            "org.bluez.Error.NotSupported" => Error::NotSupported(message.clone()),
            "org.bluez.Error.InvalidValueLength" => Error::InvalidModification(message.clone()),
            "org.bluez.Error.NotConnected" => Error::InvalidState(message.clone()),
            _ => Error::Network(format!("{member} failed: {message}")),
        },
        D::Timeout => Error::Timeout(format!("{member} timed out")),
        D::Disconnected => Error::InvalidState("the connection to BlueZ was lost".into()),
        other => Error::Network(format!("{member} failed: {other}")),
    }
}

/// Build the shared advertisement view from BlueZ device properties.
fn advertisement_from(props: &HashMap<String, Value>) -> Advertisement {
    let mut manufacturer_data = HashMap::new();
    if let Some(md) = props.get("ManufacturerData") {
        // `a{qv}`: company identifier → bytes.
        for item in md.as_array().unwrap_or(&[]) {
            if let Value::DictEntry(k, v) = item.peel() {
                if let (Some(company), Some(bytes)) = (k.as_u64(), v.as_bytes()) {
                    manufacturer_data.insert(company as u16, bytes);
                }
            }
        }
    }

    let mut service_data = HashMap::new();
    if let Some(sd) = props.get("ServiceData") {
        for (uuid, bytes) in sd.as_map() {
            if let (Ok(uuid), Some(bytes)) = (BluetoothUuid::parse(&uuid), bytes.as_bytes()) {
                service_data.insert(uuid, bytes);
            }
        }
    }

    Advertisement {
        local_name: props
            .get("Name")
            .or_else(|| props.get("Alias"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        tx_power: props
            .get("TxPower")
            .and_then(Value::as_i64)
            .map(|v| v as i16),
        appearance: props
            .get("Appearance")
            .and_then(Value::as_u64)
            .map(|v| v as u16),
        // BlueZ only surfaces connectable devices through Device1.
        is_connectable: Some(true),
        service_uuids: props
            .get("UUIDs")
            .map(|v| v.as_strings())
            .unwrap_or_default()
            .iter()
            .filter_map(|s| BluetoothUuid::parse(s).ok())
            .collect(),
        // No Apple-style overflow area on Linux.
        overflow_service_uuids: Vec::new(),
        solicited_service_uuids: Vec::new(),
        manufacturer_data,
        service_data,
        rssi: props
            .get("RSSI")
            .and_then(Value::as_i64)
            .map(|v| v as i32)
            .unwrap_or(127),
    }
}

/// Whether this process may use Bluetooth.
///
/// Linux has no per-application Bluetooth permission. What it does have is
/// D-Bus policy: reaching `org.bluez` at all requires the caller be in a group
/// the system bus configuration allows. So reachability is the honest answer to
/// the same question.
pub fn authorization() -> webbluetooth_core::Authorization {
    match crate::dbus::Connection::system() {
        Ok(_) => webbluetooth_core::Authorization::Allowed,
        Err(_) => webbluetooth_core::Authorization::Denied,
    }
}
