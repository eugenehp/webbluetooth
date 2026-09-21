//! The daemon-free Linux engine: ATT over an L2CAP socket, HCI for scanning.
//!
//! Selected by the `linux-hci` feature. Same public API as the BlueZ backend,
//! reached a completely different way:
//!
//! | | BlueZ backend | this one |
//! |---|---|---|
//! | needs `bluetoothd` | yes | **no** |
//! | GATT transport | D-Bus method calls | ATT over an L2CAP socket |
//! | scanning | `StartDiscovery` | raw HCI socket, needs `CAP_NET_RAW` |
//! | pairing / bonding | `Device1.Pair` | **absent** — SMP lives in the daemon |
//!
//! That last row is the real cost. `bluetoothd` owns the Security Manager, so
//! without it an encrypted characteristic is simply unreachable — the peer
//! answers `INSUFFICIENT_AUTHENTICATION` and there is nothing here to satisfy
//! it with. Unencrypted GATT, which is most of it, works.

use crate::att::{self, Characteristic, Service};
use crate::gatt::{self, Connection, NotificationSink};
use crate::hci::{Advertisement as HciAdvertisement, HciSocket};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, Weak};
use webbluetooth_core::chooser::DeviceChooser;
use webbluetooth_core::error::{Error, Result};
use webbluetooth_core::filter::{Advertisement, RequestDeviceOptions};
use webbluetooth_core::gatt::{CharacteristicProperties, WriteType};
use webbluetooth_core::registry::DeviceRegistry;
use webbluetooth_core::scan::{self, ScanHub};
use webbluetooth_core::state::ManagerState;
use webbluetooth_core::uuid::BluetoothUuid;

/// A handle into a peer's attribute table.
///
/// ATT addresses everything by 16-bit handle, so this carries the handle, the
/// UUID it resolved as, and — for a characteristic — the range its descriptors
/// live in, since discovering them needs it.
#[derive(Clone, Debug)]
pub struct Handle(Arc<HandleData>);

#[derive(Debug)]
struct HandleData {
    /// What reads and writes address. For a service this is its start handle.
    handle: u16,
    uuid: BluetoothUuid,
    properties: u8,
    /// The inclusive handle range this attribute owns, for discovery.
    range: (u16, u16),
}

pub fn attribute_uuid(handle: &Handle) -> Result<BluetoothUuid> {
    Ok(handle.0.uuid)
}

fn to_uuid(u: &att::Uuid) -> Result<BluetoothUuid> {
    BluetoothUuid::parse(&u.to_canonical())
}

/// Mirrors the other backends' shape. This one only ever reports two of them —
/// a raw HCI socket either opens or it does not — but the shared layer matches
/// on all five.
#[allow(dead_code)]
#[derive(Debug, Default, Clone)]
pub struct RestoredScan {
    pub device_ids: Vec<String>,
    pub scan_services: Vec<BluetoothUuid>,
    pub scan_allow_duplicates: bool,
}

/// A device seen during a scan, kept until the chooser picks one.
#[derive(Clone)]
struct Sighting {
    address: String,
    /// LE peers often advertise a random address; connecting to one as though
    /// it were public simply times out.
    random_address: bool,
    name: Option<String>,
}

/// What this backend needs to reach a device. Everything else about a grant
/// lives in [`webbluetooth_core::registry`].
///
/// The ATT tree is cached here because ATT has no cheap way to re-ask: service
/// discovery is a round trip per batch, and there is no "services changed"
/// signal to invalidate on short of the link dropping.
struct DeviceData {
    address: String,
    random_address: bool,
    connection: Option<Arc<Connection>>,
    /// The value handle of the Service Changed characteristic, if the peer has
    /// one and the subscription took. An indication on it is not a value for
    /// anyone to read — it means every cached handle is now stale.
    service_changed: Option<u16>,
    services: Vec<Service>,
    characteristics: HashMap<u16, Vec<Characteristic>>,
}

/// Subscribers to one characteristic's notifications, keyed by `(device, value
/// handle)` — handles are per-connection, so the device has to be part of it.
type Subscribers = HashMap<(String, u16), Vec<webbluetooth_core::backlog::Sender<Vec<u8>>>>;

pub struct Inner {
    devices: DeviceRegistry<DeviceData>,
    /// Subscribers keyed by `(device id, value handle)`.
    notifications: Mutex<Subscribers>,
    /// Cached last-read values, so `value()` can answer without traffic.
    cached: Mutex<HashMap<(String, u16), Vec<u8>>>,
    adapter_present: bool,
    /// Which controller to open: 0 is `hci0`.
    ///
    /// An index rather than a name because that is what the kernel takes;
    /// `select_adapter` maps `hci1` onto it.
    controller: AtomicU16,
    /// Everyone watching the scan; the scan thread is shared and
    /// reference-counted rather than owned by whoever started it.
    hub: ScanHub,
    /// Every sighting so far, so the chooser's answer is still resolvable after
    /// it returns.
    sightings: Mutex<HashMap<String, Sighting>>,
    /// The running scan thread's stop flag, if one is running.
    scan_stop: Mutex<Option<Arc<AtomicBool>>>,
}

/// Routes a connection's unsolicited traffic back into the engine.
struct Notifications {
    inner: Weak<Inner>,
    device_id: String,
}

impl NotificationSink for Notifications {
    fn on_notification(&self, handle: u16, value: Vec<u8>) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        // An indication on the Service Changed handle is not a value: it says
        // the attribute table was rearranged, so every handle handed out from
        // the cached tree is stale. Reported as a change rather than delivered
        // to a subscriber, which is what the specification asks for.
        let service_changed = inner
            .devices
            .get(&self.device_id, |d| d.inner.service_changed)
            .ok()
            .flatten();
        if service_changed == Some(handle) {
            inner.devices.mark_services_changed(&self.device_id);
            return;
        }
        inner
            .cached
            .lock()
            .unwrap()
            .insert((self.device_id.clone(), handle), value.clone());
        let mut subscribers = inner.notifications.lock().unwrap();
        let key = (self.device_id.clone(), handle);
        if let Some(list) = subscribers.get_mut(&key) {
            list.retain(|tx| tx.send(value.clone()).is_ok());
            if list.is_empty() {
                subscribers.remove(&key);
            }
        }
    }

    fn on_disconnected(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        inner.devices.update(&self.device_id, |d| {
            d.inner.connection = None;
            d.inner.services.clear();
            d.inner.characteristics.clear();
        });
        inner.devices.mark_disconnected(&self.device_id);
    }
}

impl Inner {
    pub fn new(show_power_alert: bool) -> Arc<Self> {
        Self::with_restoration(show_power_alert, None, BTreeSet::new())
    }

    /// Restoration is meaningless here: nothing on Linux relaunches a process
    /// for Bluetooth, and there is no daemon holding state to hand back.
    pub fn with_restoration(
        _show_power_alert: bool,
        _restore_identifier: Option<&str>,
        _restore_allowed: BTreeSet<BluetoothUuid>,
    ) -> Arc<Self> {
        // Opening and dropping a socket is the only way to learn whether there
        // is a controller we may drive.
        let adapter_present = HciSocket::open(DEFAULT_CONTROLLER).is_ok();
        Arc::new(Inner {
            devices: DeviceRegistry::default(),
            notifications: Mutex::new(HashMap::new()),
            cached: Mutex::new(HashMap::new()),
            controller: AtomicU16::new(DEFAULT_CONTROLLER),
            adapter_present,
            hub: ScanHub::new(),
            sightings: Mutex::new(HashMap::new()),
            scan_stop: Mutex::new(None),
        })
    }

    pub fn state(&self) -> ManagerState {
        if self.adapter_present {
            ManagerState::PoweredOn
        } else {
            // Indistinguishable from here: no controller, or no CAP_NET_RAW.
            ManagerState::Unauthorized
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

    /// Start or stop the HCI scan thread.
    ///
    /// The scan itself takes no filter — a raw HCI scan reports everything the
    /// radio hears — so a watcher that widens the hub's filter needs nothing
    /// done here. Starting is a no-op when the thread is already running.
    pub fn set_radio_scanning(self: &Arc<Self>, on: bool) -> Result<()> {
        let mut running = self.scan_stop.lock().unwrap();
        if !on {
            if let Some(stop) = running.take() {
                stop.store(true, Ordering::Release);
            }
            return Ok(());
        }
        if running.is_some() {
            return Ok(());
        }

        let stop = Arc::new(AtomicBool::new(false));
        // HCI reads block, so scanning runs on its own thread and publishes to
        // the hub, which is what fans it out to each watcher.
        let engine = Arc::downgrade(self);
        let thread_stop = stop.clone();
        // Read once, here: a scan stays on the controller it started on even
        // if another is selected while it runs.
        let controller = self.controller.load(Ordering::Relaxed);
        std::thread::Builder::new()
            .name("webbluetooth-hci-scan".into())
            .spawn(move || {
                let Ok(socket) = HciSocket::open(controller) else {
                    // Without a socket nothing will ever arrive, so end the
                    // streams rather than leave every watcher waiting.
                    if let Some(engine) = engine.upgrade() {
                        engine.hub.close_all();
                    }
                    return;
                };
                // What a device has said so far, by address. A single report is
                // only part of the picture — see `Advertisement::merge` — so
                // the filter is applied to the accumulated view rather than to
                // each packet.
                let mut accumulated: HashMap<String, Advertisement> = HashMap::new();

                let _ = socket.scan(thread_stop, |report| {
                    let Some(engine) = engine.upgrade() else {
                        return;
                    };
                    if !engine.hub.is_watching() {
                        return;
                    }
                    let entry = accumulated.entry(report.address.clone()).or_default();
                    entry.merge(advertisement_from(&report));
                    let advertisement = entry.clone();
                    let name = advertisement.local_name.clone();

                    engine.sightings.lock().unwrap().insert(
                        report.address.clone(),
                        Sighting {
                            address: report.address.clone(),
                            random_address: report.random_address,
                            name: name.clone(),
                        },
                    );
                    engine
                        .hub
                        .publish(&report.address, name.as_deref(), &advertisement);
                });
            })
            .map_err(|e| Error::Network(format!("could not start scanning: {e}")))?;
        *running = Some(stop);
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
        let Some(sighting) = self.sightings.lock().unwrap().get(&id).cloned() else {
            return Err(Error::NotFound(format!(
                "the chooser returned {id:?}, which was not among the devices it was offered"
            )));
        };

        self.devices.insert(
            &id,
            sighting.name,
            allowed,
            false,
            DeviceData {
                address: sighting.address,
                random_address: sighting.random_address,
                connection: None,
                service_changed: None,
                services: Vec::new(),
                characteristics: HashMap::new(),
            },
        );
        Ok(id)
    }

    /// Adopt a device by address.
    ///
    /// Nothing has to be resolved: this backend addresses peers by address
    /// already, and a connection is an L2CAP socket to one. An address that
    /// was never advertised is still adoptable — whether anything answers is
    /// settled at connect time, not here.
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
        let known = self.sightings.lock().unwrap().get(id).cloned();
        self.devices.insert(
            id,
            known.as_ref().and_then(|s| s.name.clone()),
            allowed,
            false,
            DeviceData {
                address: id.to_owned(),
                service_changed: None,
                // Public unless a sighting said otherwise: a random address
                // that was never seen cannot be distinguished from a public
                // one, and public is what a written-down address usually is.
                random_address: known.map(|s| s.random_address).unwrap_or(false),
                connection: None,
                services: Vec::new(),
                characteristics: HashMap::new(),
            },
        );
        Ok(id.to_owned())
    }

    // ── Device access ───────────────────────────────────────────────────────

    // The registry owns grants, generations and the GATT lock; these forward.
    fn connection(&self, id: &str) -> Result<Arc<Connection>> {
        self.devices
            .get(id, |d| d.inner.connection.clone())?
            .ok_or_else(|| Error::InvalidState("the device is not connected".into()))
    }

    fn peer(&self, id: &str) -> Result<(String, bool)> {
        self.devices
            .get(id, |d| (d.inner.address.clone(), d.inner.random_address))
    }

    // The registry answers these identically for every backend.
    webbluetooth_core::registry::forward_to_registry!();

    // ── PHY ─────────────────────────────────────────────────────────────────

    /// The `LE Set PHY` command exists and this backend speaks HCI, but it
    /// addresses a connection *handle* — and the handle belongs to the kernel,
    /// which made the connection when an L2CAP socket was connected. Reading it
    /// back would mean a monitor socket and correlation by address.
    pub async fn phy(&self, id: &str) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported("the connection handle belongs to the kernel; PHY control would need an HCI monitor socket".into()))
    }

    /// Asking is not possible where even reading is not.
    pub async fn set_preferred_phy(
        &self,
        id: &str,
        _tx: webbluetooth_core::Phy,
        _rx: webbluetooth_core::Phy,
    ) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported("the connection handle belongs to the kernel; PHY control would need an HCI monitor socket".into()))
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    /// Not possible without a daemon, and saying so is the honest answer.
    ///
    /// Pairing is the Security Manager Protocol, and SMP lives in the kernel
    /// driven by `bluetoothd`: key generation, the ceremony, and a bond store
    /// that outlives the process. This backend exists to avoid that daemon, so
    /// it cannot borrow its Security Manager either. A device whose
    /// characteristics require encryption needs the BlueZ backend.
    pub async fn pair(&self, id: &str) -> Result<webbluetooth_core::Pairing> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "pairing needs a Security Manager, which lives in bluetoothd; use the \
             BlueZ backend for a device that requires encryption"
                .into(),
        ))
    }

    pub async fn is_paired(&self, id: &str) -> Result<bool> {
        let _ = self.is_connected(id);
        // Bonds are the daemon's, stored where this backend cannot see them.
        Ok(false)
    }

    // ── Controllers ─────────────────────────────────────────────────────

    pub async fn adapters(&self) -> Result<Vec<webbluetooth_core::AdapterInfo>> {
        let current = self.controller.load(Ordering::Relaxed);
        let found = crate::hci::devices().map_err(Error::NotSupported)?;
        Ok(found
            .into_iter()
            .map(|d| webbluetooth_core::AdapterInfo {
                is_default: d.id == current,
                id: d.name.clone(),
                name: Some(d.name),
                address: Some(d.address),
                powered: d.powered,
            })
            .collect())
    }

    pub async fn adapter(&self) -> Result<webbluetooth_core::AdapterInfo> {
        let adapters = self.adapters().await?;
        adapters
            .into_iter()
            .find(webbluetooth_core::AdapterInfo::is_default)
            .ok_or(Error::NotAvailable(
                webbluetooth_core::Availability::Unsupported,
            ))
    }

    pub async fn select_adapter(&self, id: &str) -> Result<()> {
        let adapters = self.adapters().await?;
        let Some(found) = adapters.iter().find(|a| a.id == id) else {
            return Err(Error::NotFound(format!(
                "no controller called {id:?}; this system has {}",
                adapters
                    .iter()
                    .map(|a| a.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        // `hci3` is the kernel's name for controller 3; the index is what an
        // HCI socket is opened with.
        let index = found
            .id
            .strip_prefix("hci")
            .and_then(|n| n.parse::<u16>().ok())
            .ok_or_else(|| Error::NotFound(format!("{id:?} is not an hciN controller name")))?;
        self.controller.store(index, Ordering::Relaxed);
        Ok(())
    }

    /// Not reachable here, despite this backend speaking HCI.
    ///
    /// The kernel owns the LE link — the connection is made by connecting an
    /// L2CAP socket — and the negotiated values appear only in the
    /// `LE Connection Complete` event at the moment it comes up. Reading them
    /// would mean holding an HCI monitor socket open for the whole session and
    /// correlating events by address, which is a feature rather than an
    /// accessor. `HCIGETCONNINFO`, the obvious candidate, carries the handle
    /// and link mode but not the interval.
    pub async fn connection_parameters(
        &self,
        id: &str,
    ) -> Result<webbluetooth_core::ConnectionParameters> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "the kernel owns the LE link; negotiated parameters are only in the \
             HCI connection-complete event"
                .into(),
        ))
    }

    async fn gatt_lock(&self, id: &str) -> Result<futures_util::lock::OwnedMutexGuard<()>> {
        self.devices.gatt_lock(id).await
    }

    /// Revoke a grant, dropping the link with it.
    pub fn forget(&self, id: &str) {
        if let Some(device) = self.devices.remove(id) {
            if let Some(connection) = device.inner.connection {
                connection.close();
            }
        }
    }

    // ── Attribute accessors ─────────────────────────────────────────────────

    pub fn service_is_primary(&self, _handle: &Handle) -> bool {
        // Only primary services are discovered, by construction.
        true
    }

    pub fn characteristic_properties(&self, handle: &Handle) -> CharacteristicProperties {
        // ATT's property bits are the Bluetooth ones, already in the right
        // places — unlike BlueZ, which spells them as strings.
        CharacteristicProperties(handle.0.properties as u32)
    }

    pub fn cached_value(&self, handle: &Handle) -> Option<Vec<u8>> {
        let key = handle.0.handle;
        self.cached
            .lock()
            .unwrap()
            .iter()
            .find(|((_, h), _)| *h == key)
            .map(|(_, v)| v.clone())
    }

    pub fn is_notifying(&self, handle: &Handle) -> bool {
        let key = handle.0.handle;
        self.notifications
            .lock()
            .unwrap()
            .keys()
            .any(|(_, h)| *h == key)
    }

    // ── GATT operations ─────────────────────────────────────────────────────

    pub async fn connect(self: &Arc<Self>, id: &str) -> Result<()> {
        self.require_powered_on().await?;
        if self.is_connected(id) {
            return Ok(());
        }
        let (address, random) = self.peer(id)?;
        let sink = Arc::new(Notifications {
            inner: Arc::downgrade(self),
            device_id: id.to_owned(),
        });

        // The socket connect blocks while the LE link comes up.
        let connection = Connection::open(&address, random, sink)
            .map_err(|e| Error::Network(format!("could not connect to {address}: {e}")))?;
        let connection = Arc::new(connection);

        // Discovery is a round trip per batch, so the tree is cached for the
        // life of the connection and invalidated by the Service Changed
        // indication below.
        let services = connection
            .discover_services()
            .map_err(|e| Error::Network(format!("service discovery failed: {e}")))?;

        // "Before discovering any of these attributes for the purpose of
        // exposing them to a web page the UA MUST subscribe to Indications
        // from the Service Changed characteristic, if it exists." Every other
        // backend inherits this from its stack; here it is ours to do, and
        // without it a cached tree silently outlives the device rearranging
        // itself.
        let service_changed = subscribe_to_service_changed(&connection, &services);

        self.devices.update(id, |d| {
            d.connected = true;
            d.inner.connection = Some(connection);
            d.inner.services = services;
            d.inner.characteristics.clear();
            d.inner.service_changed = service_changed;
        });
        Ok(())
    }

    pub fn disconnect(&self, id: &str) {
        if let Ok(Some(connection)) = self.devices.get(id, |d| d.inner.connection.clone()) {
            connection.close();
        }
    }

    pub async fn discover_services(
        &self,
        id: &str,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        if !self.is_connected(id) {
            return Err(Error::InvalidState("the device is not connected".into()));
        }
        let services = self.devices.get(id, |d| d.inner.services.clone())?;
        Ok(services
            .into_iter()
            .filter_map(|s| {
                let found = to_uuid(&s.uuid).ok()?;
                match uuid {
                    Some(want) if *want != found => None,
                    _ => Some(Handle(Arc::new(HandleData {
                        handle: s.start_handle,
                        uuid: found,
                        properties: 0,
                        range: (s.start_handle, s.end_handle),
                    }))),
                }
            })
            .collect())
    }

    pub async fn discover_included_services(
        &self,
        id: &str,
        service: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        let connection = self.connection(id)?;
        let (start, end) = service.0.range;

        // Not cached: an included service is discovered from the includer's
        // handle range, and the range is already known.
        let declared = Service {
            start_handle: start,
            end_handle: end,
            uuid: att::Uuid::Short(0),
        };
        let found = connection
            .discover_included_services(&declared)
            .map_err(|e| Error::Network(format!("included service discovery failed: {e}")))?;

        let mut out = Vec::new();
        for included in found {
            let Ok(u) = to_uuid(&included.uuid) else {
                continue;
            };
            if uuid.is_some_and(|want| *want != u) {
                continue;
            }
            out.push(Handle(Arc::new(HandleData {
                handle: included.start_handle,
                uuid: u,
                // A service declaration has no properties.
                properties: 0,
                range: (included.start_handle, included.end_handle),
            })));
        }
        Ok(out)
    }

    pub async fn discover_characteristics(
        &self,
        id: &str,
        service: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        let connection = self.connection(id)?;
        let (start, end) = service.0.range;

        let cached = self
            .devices
            .get(id, |d| d.inner.characteristics.get(&start).cloned())?;
        let characteristics = match cached {
            Some(c) => c,
            None => {
                let service = Service {
                    start_handle: start,
                    end_handle: end,
                    uuid: att::Uuid::Short(0),
                };
                let found = connection
                    .discover_characteristics(&service)
                    .map_err(|e| Error::Network(format!("characteristic discovery failed: {e}")))?;
                self.devices.update(id, |d| {
                    d.inner.characteristics.insert(start, found.clone());
                });
                found
            }
        };

        // A characteristic owns every handle up to the next one, which is how
        // its descriptors are located later.
        let mut out = Vec::new();
        for (i, c) in characteristics.iter().enumerate() {
            let owns_until = characteristics
                .get(i + 1)
                .map(|next| next.handle - 1)
                .unwrap_or(end);
            let Ok(found) = to_uuid(&c.uuid) else {
                continue;
            };
            if let Some(want) = uuid {
                if *want != found {
                    continue;
                }
            }
            out.push(Handle(Arc::new(HandleData {
                handle: c.value_handle,
                uuid: found,
                properties: c.properties,
                range: (c.value_handle + 1, owns_until),
            })));
        }
        Ok(out)
    }

    pub async fn discover_descriptors(
        &self,
        id: &str,
        characteristic: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let _guard = self.gatt_lock(id).await?;
        let connection = self.connection(id)?;
        let (start, end) = characteristic.0.range;
        let descriptors = connection
            .discover_descriptors(start, end)
            .map_err(|e| Error::Network(format!("descriptor discovery failed: {e}")))?;

        Ok(descriptors
            .into_iter()
            .filter_map(|d| {
                let found = to_uuid(&d.uuid).ok()?;
                match uuid {
                    Some(want) if *want != found => None,
                    _ => Some(Handle(Arc::new(HandleData {
                        handle: d.handle,
                        uuid: found,
                        properties: 0,
                        range: (d.handle, d.handle),
                    }))),
                }
            })
            .collect())
    }

    pub async fn read_characteristic(&self, id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let connection = self.connection(id)?;
        let value = connection
            .read(handle.0.handle)
            .map_err(|e| map_error("read", e))?;
        self.cached
            .lock()
            .unwrap()
            .insert((id.to_owned(), handle.0.handle), value.clone());
        Ok(value)
    }

    pub async fn write_characteristic(
        &self,
        id: &str,
        handle: &Handle,
        value: &[u8],
        write_type: WriteType,
    ) -> Result<()> {
        // Only the acknowledged form takes the lock. An ATT command is not a
        // request and does not consume the one-outstanding-request budget, and
        // the socket underneath is `SOCK_SEQPACKET` — one write is one packet,
        // so two threads writing at once cannot interleave a PDU. Serialising
        // commands behind a read's round trip would cost throughput and buy
        // nothing.
        let _guard = match write_type {
            WriteType::WithResponse => Some(self.gatt_lock(id).await?),
            WriteType::WithoutResponse => None,
        };
        let connection = self.connection(id)?;
        match write_type {
            WriteType::WithResponse => connection.write(handle.0.handle, value),
            WriteType::WithoutResponse => connection.write_command(handle.0.handle, value),
        }
        .map_err(|e| map_error("write", e))
    }

    pub async fn set_notify(&self, id: &str, handle: &Handle, enabled: bool) -> Result<bool> {
        let _guard = self.gatt_lock(id).await?;
        let connection = self.connection(id)?;
        let (start, end) = handle.0.range;

        // Subscribing is a descriptor write, so the CCCD has to be found first.
        let descriptors = connection
            .discover_descriptors(start, end)
            .map_err(|e| map_error("descriptor discovery", e))?;
        let cccd = crate::gatt::find_cccd(&descriptors).ok_or_else(|| {
            Error::NotSupported(
                "this characteristic has no client characteristic configuration descriptor".into(),
            )
        })?;

        let properties = CharacteristicProperties(handle.0.properties as u32);
        connection
            .set_notify(
                cccd,
                enabled && properties.notify(),
                enabled && properties.indicate(),
            )
            .map_err(|e| map_error("subscribe", e))?;
        Ok(enabled)
    }

    pub fn subscribe(&self, handle: &Handle) -> webbluetooth_core::backlog::Receiver<Vec<u8>> {
        // Bounded: a notification arrives on a platform callback
        // thread that must return promptly, so there is nobody to
        // apply backpressure to. See `webbluetooth_core::backlog`.
        let (tx, rx) = webbluetooth_core::backlog::channel();
        // Keyed by value handle; the device id is filled by whichever device
        // notifies, which is unambiguous because handles are per-connection.
        let mut subscribers = self.notifications.lock().unwrap();
        for id in self.devices.ids() {
            subscribers
                .entry((id, handle.0.handle))
                .or_default()
                .push(tx.clone());
        }
        rx
    }

    pub async fn read_descriptor(&self, id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let connection = self.connection(id)?;
        connection
            .read(handle.0.handle)
            .map_err(|e| map_error("descriptor read", e))
    }

    pub async fn write_descriptor(&self, id: &str, handle: &Handle, value: &[u8]) -> Result<()> {
        let _guard = self.gatt_lock(id).await?;
        let connection = self.connection(id)?;
        connection
            .write(handle.0.handle, value)
            .map_err(|e| map_error("descriptor write", e))
    }

    pub async fn read_rssi(&self, _id: &str) -> Result<i32> {
        // RSSI of a live connection needs an HCI command on the connection
        // handle, which this backend does not track.
        Err(Error::NotSupported(
            "connected RSSI is not available without bluetoothd; the scan reports it instead"
                .into(),
        ))
    }

    pub async fn request_connection_priority(
        &self,
        _id: &str,
        _priority: webbluetooth_core::ConnectionPriority,
    ) -> Result<()> {
        Err(Error::NotSupported(
            "the kernel owns the ACL link, so an LE Connection Update sent \
             behind its back would desynchronise it"
                .into(),
        ))
    }

    pub fn max_write_len(&self, id: &str, _write_type: WriteType) -> Result<usize> {
        let connection = self.connection(id)?;
        Ok(connection.mtu().saturating_sub(3) as usize)
    }

    pub async fn l2cap_target(&self, id: &str, _psm: u16) -> Result<(String, String)> {
        let (address, random) = self.peer(id)?;
        let kind = if random { "random" } else { "public" };
        Ok((address, kind.to_string()))
    }
}

fn map_error(what: &str, e: crate::gatt::Error) -> Error {
    use crate::gatt::Error as G;
    match &e {
        G::Att(code) => match code.0 {
            att::AttError::READ_NOT_PERMITTED | att::AttError::WRITE_NOT_PERMITTED => {
                Error::Security(format!("{what} refused: {code}"))
            }
            att::AttError::REQUEST_NOT_SUPPORTED | att::AttError::ATTRIBUTE_NOT_LONG => {
                Error::NotSupported(format!("{what}: {code}"))
            }
            att::AttError::INVALID_ATTRIBUTE_VALUE_LENGTH => {
                Error::InvalidModification(format!("{what}: {code}"))
            }
            att::AttError::INSUFFICIENT_AUTHENTICATION
            | att::AttError::INSUFFICIENT_ENCRYPTION
            | att::AttError::INSUFFICIENT_AUTHORIZATION => Error::Security(format!(
                "{what} needs an encrypted link, and pairing needs bluetoothd — \
                 build without the linux-hci feature to use it"
            )),
            _ => Error::Network(format!("{what} failed: {code}")),
        },
        G::Timeout => Error::Timeout(format!("{what} timed out")),
        G::Disconnected => Error::InvalidState("the link dropped".into()),
        other => Error::Network(format!("{what} failed: {other}")),
    }
}

fn advertisement_from(report: &HciAdvertisement) -> Advertisement {
    let mut manufacturer_data = HashMap::new();
    if let Some((company, payload)) = &report.manufacturer_data {
        manufacturer_data.insert(*company, payload.clone());
    }
    Advertisement {
        local_name: report.local_name.clone(),
        tx_power: report.tx_power.map(i16::from),
        appearance: report.appearance,
        is_connectable: Some(report.connectable),
        service_uuids: report
            .service_uuids
            .iter()
            .filter_map(|u| BluetoothUuid::parse(u).ok())
            .collect(),
        overflow_service_uuids: Vec::new(),
        solicited_service_uuids: Vec::new(),
        manufacturer_data,
        service_data: report
            .service_data
            .iter()
            .filter_map(|(u, d)| Some((BluetoothUuid::parse(u).ok()?, d.clone())))
            .collect(),
        rssi: report.rssi as i32,
    }
}

/// Subscribe to the Service Changed characteristic, and report its handle.
///
/// `0x1801` is the Generic Attribute service and `0x2A05` the characteristic
/// in it. Both are optional — a peer with a fixed attribute table need not
/// have either — so every step here is allowed to come up empty, and `None`
/// means "this device will not tell us", not "something went wrong".
///
/// Indications rather than notifications: Service Changed is specified as
/// indicate-only, because the peer needs to know the client saw it before it
/// assumes the client has re-discovered.
fn subscribe_to_service_changed(connection: &Connection, services: &[Service]) -> Option<u16> {
    const GENERIC_ATTRIBUTE: u16 = 0x1801;
    const SERVICE_CHANGED: u16 = 0x2A05;

    let service = services
        .iter()
        .find(|s| s.uuid == att::Uuid::Short(GENERIC_ATTRIBUTE))?;
    let characteristic = connection
        .discover_characteristics(service)
        .ok()?
        .into_iter()
        .find(|c| c.uuid == att::Uuid::Short(SERVICE_CHANGED))?;

    // The CCCD sits between this characteristic's value and the next
    // declaration; with only one characteristic in the service, that is the
    // rest of the service.
    let cccd = gatt::find_cccd(
        &connection
            .discover_descriptors(characteristic.value_handle + 1, service.end_handle)
            .ok()?,
    )?;
    // A peer that refuses the write keeps its handles cached, which is what
    // happened before this existed — no worse, so not an error.
    connection.set_notify(cccd, false, true).ok()?;
    Some(characteristic.value_handle)
}

/// `hci0`, which is what every Linux tool defaults to.
const DEFAULT_CONTROLLER: u16 = 0;

/// Whether this process may drive a controller.
///
/// Scanning needs `CAP_NET_RAW`; GATT over an L2CAP socket needs nothing. From
/// here the two are indistinguishable, so this reports the stricter answer.
pub fn authorization() -> webbluetooth_core::Authorization {
    match HciSocket::open(DEFAULT_CONTROLLER) {
        Ok(_) => webbluetooth_core::Authorization::Allowed,
        Err(_) => webbluetooth_core::Authorization::Denied,
    }
}
