//! The browser backend — Web Bluetooth over the page's own `navigator.bluetooth`.
//!
//! The other five backends drive a radio. This one does not: in a browser
//! there is no radio to reach, the page is sandboxed beneath one, and
//! `navigator.bluetooth` is the whole interface. So this backend is a client
//! of the standard rather than an implementation of it, and several things
//! that are this crate's job elsewhere belong to the browser here.
//!
//! **The chooser is Chrome's.** `requestDevice` opens the browser's own device
//! picker; the [`DeviceChooser`] passed in is not consulted, because a page
//! cannot be allowed to pick a device on the user's behalf — that is the
//! permission model, and subverting it is the one thing Web Bluetooth exists
//! to prevent. A caller that supplies a chooser gets the browser's instead,
//! which is a difference worth knowing about and not one worth pretending
//! away.
//!
//! **The blocklist is Chrome's.** This crate's own is still applied on top, in
//! [`webbluetooth_core::gatt`], so an attribute blocked here is blocked in the browser
//! too; the reverse is not guaranteed, since the browser ships its own
//! snapshot.
//!
//! **There is one adapter and it has no name.** The web platform exposes no
//! notion of a controller.
//!
//! Everything below the boundary is in [this crate](crate): the message
//! format, the import, and the executor a browser needs because it cannot
//! block.

use crate::codec::{Reader, Writer};
use crate::host::{self, HostError, Op};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use webbluetooth_core::chooser::DeviceChooser;
use webbluetooth_core::error::{Availability, Error, Result};
use webbluetooth_core::filter::{DeviceFilter, RequestDeviceOptions};
use webbluetooth_core::gatt::{CharacteristicProperties, WriteType};
use webbluetooth_core::registry::DeviceRegistry;
use webbluetooth_core::scan::ScanHub;
use webbluetooth_core::state::ManagerState;
use webbluetooth_core::uuid::BluetoothUuid;

/// A handle into a device's attribute table.
///
/// The key is the shim's: the browser holds the real
/// `BluetoothRemoteGATTCharacteristic` and this names it. Paths rather than
/// opaque numbers so that a disconnect can invalidate every handle into one
/// device by prefix, which is what the specification requires.
#[derive(Clone, Debug)]
pub struct Handle {
    key: String,
    uuid: BluetoothUuid,
    properties: u32,
    primary: bool,
}

pub fn attribute_uuid(handle: &Handle) -> Result<BluetoothUuid> {
    Ok(handle.uuid)
}

/// Restoration is the browser's business: a page that is reloaded starts
/// again, and `getDevices` is how it finds what it was granted before.
#[allow(dead_code)]
#[derive(Debug, Default, Clone)]
pub struct RestoredScan {
    pub device_ids: Vec<String>,
    pub scan_services: Vec<BluetoothUuid>,
    pub scan_allow_duplicates: bool,
}

/// Nothing: the browser holds the device, and this side holds its id.
struct DeviceData;

pub struct Inner {
    devices: DeviceRegistry<DeviceData>,
    hub: ScanHub,
    /// Subscribers per characteristic key.
    notifications: Mutex<HashMap<String, Vec<webbluetooth_core::backlog::Sender<Vec<u8>>>>>,
    /// The last value each characteristic reported, for `cached_value`.
    values: Mutex<HashMap<String, Vec<u8>>>,
    /// Which characteristics `startNotifications` succeeded on.
    notifying: Mutex<BTreeSet<String>>,
    /// The last availability the page reported, updated by the event.
    available: Mutex<ManagerState>,
}

/// Turn a host error into this crate's, keeping the distinction the browser
/// drew — the names are the same `DOMException`s the specification throws.
fn host_error(error: HostError, what: &str) -> Error {
    match error {
        HostError::NotFound => Error::NotFound(what.into()),
        HostError::Security => Error::Security(what.into()),
        HostError::Network => Error::Network(what.into()),
        HostError::InvalidState => Error::InvalidState(what.into()),
        HostError::NotSupported => Error::NotSupported(what.into()),
        HostError::InvalidModification => Error::InvalidModification(what.into()),
        HostError::Abort => Error::Aborted(what.into()),
        HostError::NotAllowed => Error::NotAvailable(Availability::Unauthorized),
        HostError::Unknown => Error::Network(format!("{what} failed")),
    }
}

/// Run a request and turn the answer into a reader.
async fn ask(op: Op, message: &[u8], what: &str) -> Result<Vec<u8>> {
    host::call(op, message)
        .await
        .map_err(|e| host_error(e, what))
}

/// A malformed answer is the shim and this file disagreeing, which is a bug
/// here rather than anything the peer did.
fn malformed(what: &str) -> Error {
    Error::Network(format!("the page returned a malformed {what}"))
}

impl Inner {
    pub fn new(_show_power_alert: bool) -> Arc<Self> {
        Self::with_restoration(false, None, BTreeSet::new())
    }

    /// A page is reloaded rather than restored, and `getDevices` is how it
    /// finds what it was granted before, so the identifier is ignored.
    pub fn with_restoration(
        _show_power_alert: bool,
        _restore_identifier: Option<&str>,
        _restore_allowed: BTreeSet<BluetoothUuid>,
    ) -> Arc<Self> {
        let inner = Arc::new(Inner {
            devices: DeviceRegistry::default(),
            hub: ScanHub::new(),
            notifications: Mutex::new(HashMap::new()),
            values: Mutex::new(HashMap::new()),
            notifying: Mutex::new(BTreeSet::new()),
            available: Mutex::new(ManagerState::Unknown),
        });
        inner.clone().listen();
        inner
    }

    /// Route the page's events into the registry and the notification
    /// channels.
    fn listen(self: Arc<Self>) {
        let engine = Arc::downgrade(&self);
        crate::set_sink(move |event| {
            let Some(engine) = engine.upgrade() else {
                return;
            };
            use crate::Event;
            match event {
                Event::CharacteristicValue {
                    characteristic,
                    value,
                    ..
                } => {
                    engine
                        .values
                        .lock()
                        .unwrap()
                        .insert(characteristic.clone(), value.clone());
                    let mut subscribers = engine.notifications.lock().unwrap();
                    if let Some(list) = subscribers.get_mut(&characteristic) {
                        list.retain(|tx| tx.send(value.clone()).is_ok());
                        if list.is_empty() {
                            subscribers.remove(&characteristic);
                        }
                    }
                }
                Event::Disconnected { device } => {
                    // Every handle into the device goes stale with the link,
                    // and the values behind them with it.
                    engine
                        .values
                        .lock()
                        .unwrap()
                        .retain(|key, _| !key.starts_with(&format!("{device}/")));
                    engine
                        .notifying
                        .lock()
                        .unwrap()
                        .retain(|key| !key.starts_with(&format!("{device}/")));
                    engine.devices.mark_disconnected(&device);
                }
                Event::ServiceChanged { device } => {
                    engine.devices.mark_services_changed(&device);
                }
                Event::Advertisement {
                    device,
                    name,
                    rssi,
                    tx_power,
                    appearance,
                    uuids,
                    manufacturer_data,
                    service_data,
                } => {
                    let mut advertisement = webbluetooth_core::filter::Advertisement {
                        rssi,
                        tx_power: tx_power.map(|p| p as i16),
                        appearance: appearance.map(|a| a as u16),
                        local_name: name.clone(),
                        ..Default::default()
                    };
                    advertisement.service_uuids = uuids
                        .iter()
                        .filter_map(|u| BluetoothUuid::parse(u).ok())
                        .collect();
                    advertisement.manufacturer_data = manufacturer_data.into_iter().collect();
                    advertisement.service_data = service_data
                        .into_iter()
                        .filter_map(|(u, d)| BluetoothUuid::parse(&u).ok().map(|u| (u, d)))
                        .collect();
                    engine.hub.publish(&device, name.as_deref(), &advertisement);
                }
                Event::AvailabilityChanged { available } => {
                    *engine.available.lock().unwrap() = match available {
                        true => ManagerState::PoweredOn,
                        false => ManagerState::PoweredOff,
                    };
                }
            }
        });
    }

    // ── Availability ────────────────────────────────────────────────────────

    pub fn state(&self) -> ManagerState {
        *self.available.lock().unwrap()
    }

    /// Ask the page, rather than returning the last thing it said.
    ///
    /// `getAvailability` is a promise, so unlike every other platform there is
    /// no state to have already settled — the first answer requires a round
    /// trip, which is exactly what this is for.
    pub async fn settled_state(&self) -> ManagerState {
        let Ok(answer) = host::call_empty(Op::Availability).await else {
            return ManagerState::Unsupported;
        };
        let state = match Reader::new(&answer).u32() {
            Some(1) => ManagerState::Unsupported,
            Some(2) => ManagerState::Unauthorized,
            Some(3) => ManagerState::PoweredOff,
            Some(4) => ManagerState::PoweredOn,
            _ => ManagerState::Unknown,
        };
        *self.available.lock().unwrap() = state;
        state
    }

    pub async fn require_powered_on(&self) -> Result<()> {
        self.settled_state().await.require_powered_on()
    }

    pub fn restored(&self) -> Option<RestoredScan> {
        None
    }

    pub fn scan_hub(&self) -> &ScanHub {
        &self.hub
    }

    /// The web platform has no raw scan to start or stop.
    ///
    /// Advertisements arrive per device through `watchAdvertisements`, which
    /// [`Self::watch_advertisements`] drives, so there is no radio-wide switch
    /// for the hub to flip.
    pub fn set_radio_scanning(self: &Arc<Self>, _on: bool) -> Result<()> {
        Ok(())
    }

    // ── Devices ─────────────────────────────────────────────────────────────

    /// `navigator.bluetooth.requestDevice()` — the browser's picker.
    ///
    /// `chooser` is deliberately unused. In a browser the picker belongs to
    /// the user agent, and a page choosing for itself would defeat the
    /// permission prompt entirely.
    pub async fn request_device(
        self: &Arc<Self>,
        options: RequestDeviceOptions,
        _chooser: &dyn DeviceChooser,
    ) -> Result<String> {
        options.validate()?;
        self.require_powered_on().await?;

        let answer = ask(
            Op::RequestDevice,
            &encode_for_host(&options),
            "requestDevice",
        )
        .await?;

        let mut r = Reader::new(&answer);
        let (Some(id), Some(name)) = (r.str(), r.option_str()) else {
            return Err(malformed("device"));
        };
        self.devices
            .insert(&id, name, options.grant(), false, DeviceData);
        Ok(id)
    }

    /// Adopt a device the browser has already granted.
    ///
    /// `getDevices()` is the web platform's own version of this, and it is
    /// what the page consults: a device the user has not granted cannot be
    /// adopted here at all, which is the difference between this backend and
    /// every other one.
    pub async fn adopt_device(
        &self,
        id: &str,
        allowed: webbluetooth_core::registry::Grant,
    ) -> Result<String> {
        let answer = ask(Op::GetDevices, &[], "getDevices").await?;
        let mut r = Reader::new(&answer);
        let known = r
            .list(|r| Some((r.str()?, r.option_str()?)))
            .ok_or_else(|| malformed("device list"))?;

        let Some((_, name)) = known.into_iter().find(|(known, _)| known == id) else {
            return Err(Error::NotFound(format!(
                "the browser has not granted {id:?}; a page can only reach devices \
                 the user has chosen"
            )));
        };
        self.devices.insert(id, name, allowed, false, DeviceData);
        Ok(id.to_owned())
    }

    // The registry answers these identically for every backend.
    webbluetooth_core::registry::forward_to_registry!();

    // ── PHY ─────────────────────────────────────────────────────────────────

    /// Web Bluetooth has no notion of a PHY.
    pub async fn phy(&self, id: &str) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.generation(id)?;
        Err(Error::NotSupported(
            "Web Bluetooth does not expose the connection PHY".into(),
        ))
    }

    /// Asking is not possible where even reading is not.
    pub async fn set_preferred_phy(
        &self,
        id: &str,
        _tx: webbluetooth_core::Phy,
        _rx: webbluetooth_core::Phy,
    ) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.generation(id)?;
        Err(Error::NotSupported(
            "Web Bluetooth does not expose the connection PHY".into(),
        ))
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    /// The browser's, exactly as it is the operating system's on Apple.
    ///
    /// Web Bluetooth exposes no pairing API — the user agent runs the ceremony
    /// when a peer demands one and shows its own prompt.
    pub async fn pair(&self, id: &str) -> Result<webbluetooth_core::Pairing> {
        let _ = self.generation(id)?;
        Ok(webbluetooth_core::Pairing::Implicit)
    }

    pub async fn is_paired(&self, id: &str) -> Result<bool> {
        let _ = self.generation(id)?;
        Ok(false)
    }

    pub fn forget(&self, id: &str) {
        let id = id.to_owned();
        // `forget()` is a promise and this is not async, so it is spawned. The
        // registry entry goes now either way: the grant is revoked here
        // whether or not the browser has finished revoking its own.
        self.devices.remove(&id);
        crate::spawn(async move {
            let mut w = Writer::new();
            w.str(&id);
            let _ = host::call(Op::Forget, &w.finish()).await;
        });
    }

    // ── Controllers ─────────────────────────────────────────────────────────

    /// The web platform has no controllers, only Bluetooth.
    pub async fn adapters(&self) -> Result<Vec<webbluetooth_core::AdapterInfo>> {
        Ok(vec![self.adapter().await?])
    }

    pub async fn adapter(&self) -> Result<webbluetooth_core::AdapterInfo> {
        Ok(webbluetooth_core::AdapterInfo {
            id: webbluetooth_core::adapter::DEFAULT_ADAPTER.to_owned(),
            name: None,
            address: None,
            powered: self.settled_state().await.availability().is_ok(),
            is_default: true,
        })
    }

    pub async fn select_adapter(&self, id: &str) -> Result<()> {
        if id == webbluetooth_core::adapter::DEFAULT_ADAPTER {
            return Ok(());
        }
        Err(Error::NotSupported(format!(
            "the web platform exposes no controllers, so {id:?} cannot be selected"
        )))
    }

    pub async fn connection_parameters(
        &self,
        id: &str,
    ) -> Result<webbluetooth_core::ConnectionParameters> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "Web Bluetooth does not expose connection parameters".into(),
        ))
    }

    // ── Attribute accessors ─────────────────────────────────────────────────

    pub fn service_is_primary(&self, handle: &Handle) -> bool {
        handle.primary
    }

    pub fn characteristic_properties(&self, handle: &Handle) -> CharacteristicProperties {
        CharacteristicProperties(handle.properties)
    }

    pub fn cached_value(&self, handle: &Handle) -> Option<Vec<u8>> {
        self.values.lock().unwrap().get(&handle.key).cloned()
    }

    pub fn is_notifying(&self, handle: &Handle) -> bool {
        self.notifying.lock().unwrap().contains(&handle.key)
    }

    // ── GATT ────────────────────────────────────────────────────────────────

    pub async fn connect(self: &Arc<Self>, id: &str) -> Result<()> {
        if self.is_connected(id) {
            return Ok(());
        }
        let mut w = Writer::new();
        w.str(id);
        ask(Op::Connect, &w.finish(), "connect").await?;
        self.devices.update(id, |d| d.connected = true);
        Ok(())
    }

    pub fn disconnect(&self, id: &str) {
        let id = id.to_owned();
        self.devices.mark_disconnected(&id);
        crate::spawn(async move {
            let mut w = Writer::new();
            w.str(&id);
            let _ = host::call(Op::Disconnect, &w.finish()).await;
        });
    }

    /// Read a list of services from an answer.
    fn read_services(answer: &[u8]) -> Result<Vec<Handle>> {
        let mut r = Reader::new(answer);
        r.list(|r| {
            Some(Handle {
                key: r.str()?,
                uuid: BluetoothUuid::parse(&r.str()?).ok()?,
                properties: 0,
                primary: r.bool()?,
            })
        })
        .ok_or_else(|| malformed("service list"))
    }

    pub async fn discover_services(
        &self,
        id: &str,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let mut w = Writer::new();
        w.str(id).option_str(uuid.map(|u| u.as_str()));
        Self::read_services(&ask(Op::DiscoverServices, &w.finish(), "getPrimaryServices").await?)
    }

    pub async fn discover_included_services(
        &self,
        _id: &str,
        service: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let mut w = Writer::new();
        w.str(&service.key).option_str(uuid.map(|u| u.as_str()));
        Self::read_services(
            &ask(
                Op::DiscoverIncludedServices,
                &w.finish(),
                "getIncludedServices",
            )
            .await?,
        )
    }

    pub async fn discover_characteristics(
        &self,
        _id: &str,
        service: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let mut w = Writer::new();
        w.str(&service.key).option_str(uuid.map(|u| u.as_str()));
        let answer = ask(
            Op::DiscoverCharacteristics,
            &w.finish(),
            "getCharacteristics",
        )
        .await?;

        let mut r = Reader::new(&answer);
        r.list(|r| {
            let key = r.str()?;
            let uuid = BluetoothUuid::parse(&r.str()?).ok()?;
            // The page sends nine flags in order; the bit each one occupies is
            // decided here, from the same constants every other backend uses.
            // There is no second copy of them to drift.
            let mut properties = 0;
            for bit in [
                CharacteristicProperties::BROADCAST,
                CharacteristicProperties::READ,
                CharacteristicProperties::WRITE_WITHOUT_RESPONSE,
                CharacteristicProperties::WRITE,
                CharacteristicProperties::NOTIFY,
                CharacteristicProperties::INDICATE,
                CharacteristicProperties::AUTHENTICATED_SIGNED_WRITES,
                CharacteristicProperties::RELIABLE_WRITE,
                CharacteristicProperties::WRITABLE_AUXILIARIES,
            ] {
                if r.bool()? {
                    properties |= bit;
                }
            }
            Some(Handle {
                key,
                uuid,
                properties,
                primary: false,
            })
        })
        .ok_or_else(|| malformed("characteristic list"))
    }

    pub async fn discover_descriptors(
        &self,
        _id: &str,
        characteristic: &Handle,
        uuid: Option<&BluetoothUuid>,
    ) -> Result<Vec<Handle>> {
        let mut w = Writer::new();
        w.str(&characteristic.key)
            .option_str(uuid.map(|u| u.as_str()));
        let answer = ask(Op::DiscoverDescriptors, &w.finish(), "getDescriptors").await?;

        let mut r = Reader::new(&answer);
        r.list(|r| {
            Some(Handle {
                key: r.str()?,
                uuid: BluetoothUuid::parse(&r.str()?).ok()?,
                properties: 0,
                primary: false,
            })
        })
        .ok_or_else(|| malformed("descriptor list"))
    }

    pub async fn read_characteristic(&self, _id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        w.str(&handle.key);
        let answer = ask(Op::ReadCharacteristic, &w.finish(), "readValue").await?;
        let value = Reader::new(&answer)
            .bytes()
            .ok_or_else(|| malformed("value"))?;
        self.values
            .lock()
            .unwrap()
            .insert(handle.key.clone(), value.clone());
        Ok(value)
    }

    pub async fn write_characteristic(
        &self,
        _id: &str,
        handle: &Handle,
        value: &[u8],
        write_type: WriteType,
    ) -> Result<()> {
        let mut w = Writer::new();
        w.str(&handle.key)
            .bytes(value)
            .bool(matches!(write_type, WriteType::WithResponse));
        ask(Op::WriteCharacteristic, &w.finish(), "writeValue").await?;
        Ok(())
    }

    /// Returns whether the peer is now notifying.
    ///
    /// The browser decides between notification and indication from the
    /// characteristic's own properties, so unlike the other backends there is
    /// no CCCD bit to choose here.
    pub async fn set_notify(&self, _id: &str, handle: &Handle, enabled: bool) -> Result<bool> {
        let mut w = Writer::new();
        w.str(&handle.key).bool(enabled);
        ask(Op::SetNotify, &w.finish(), "startNotifications").await?;
        let mut notifying = self.notifying.lock().unwrap();
        if enabled {
            notifying.insert(handle.key.clone());
        } else {
            notifying.remove(&handle.key);
        }
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
            .entry(handle.key.clone())
            .or_default()
            .push(tx);
        rx
    }

    pub async fn read_descriptor(&self, _id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        w.str(&handle.key);
        let answer = ask(Op::ReadDescriptor, &w.finish(), "readValue").await?;
        Reader::new(&answer)
            .bytes()
            .ok_or_else(|| malformed("value"))
    }

    pub async fn write_descriptor(&self, _id: &str, handle: &Handle, value: &[u8]) -> Result<()> {
        let mut w = Writer::new();
        w.str(&handle.key).bytes(value);
        ask(Op::WriteDescriptor, &w.finish(), "writeValue").await?;
        Ok(())
    }

    /// Start or stop `watchAdvertisements` for a device.
    ///
    /// Called by `BluetoothDevice::watch_advertisements` through the
    /// hub; on every other platform the hub owns a radio-wide scan, and here
    /// it is per device because that is all the web platform offers.
    pub async fn watch_advertisements(&self, id: &str, on: bool) -> Result<()> {
        let mut w = Writer::new();
        w.str(id);
        let op = match on {
            true => Op::WatchAdvertisements,
            false => Op::UnwatchAdvertisements,
        };
        ask(op, &w.finish(), "watchAdvertisements").await?;
        Ok(())
    }

    /// Not available to a page.
    ///
    /// `BluetoothAdvertisingEvent.rssi` carries it for an advertisement, which
    /// [`webbluetooth_core::Advertisement::rssi`] reports; there is no way to ask a
    /// connected device.
    pub async fn read_rssi(&self, id: &str) -> Result<i32> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "Web Bluetooth reports RSSI only on an advertisement".into(),
        ))
    }

    pub async fn request_connection_priority(
        &self,
        id: &str,
        _priority: webbluetooth_core::ConnectionPriority,
    ) -> Result<()> {
        let _ = self.is_connected(id);
        Err(Error::NotSupported(
            "Web Bluetooth does not expose connection priority".into(),
        ))
    }

    /// The guaranteed minimum, because a page is never told the real one.
    ///
    /// The browser splits a long write itself, so this is a floor rather than
    /// a limit: writing more than this works, it simply takes more than one
    /// PDU on the wire.
    pub fn max_write_len(&self, id: &str, _write_type: WriteType) -> Result<usize> {
        let _ = self.generation(id)?;
        Ok(20)
    }
}

/// A page has Bluetooth if the browser gave it any; there is no separate
/// permission to check before asking.
pub fn authorization() -> webbluetooth_core::Authorization {
    webbluetooth_core::Authorization::Allowed
}

fn encode_for_host(options: &RequestDeviceOptions) -> Vec<u8> {
    let options = options.parts();
    use crate::codec::Writer;

    fn filters(w: &mut Writer, list: &[DeviceFilter]) {
        w.list(list, |w, f| {
            let f = f.parts();
            w.list(f.services, |w, u| {
                w.str(u.as_str());
            });
            w.option_str(f.name);
            w.option_str(f.name_prefix);
            w.list(f.manufacturer_data, |w, (company, prefix)| {
                w.u16(*company);
                w.bytes(prefix.bytes());
                match prefix.mask() {
                    Some(mask) => {
                        w.bool(true);
                        w.bytes(mask);
                    }
                    None => {
                        w.bool(false);
                    }
                }
            });
            w.list(f.service_data, |w, (service, prefix)| {
                w.str(service.as_str());
                w.bytes(prefix.bytes());
                match prefix.mask() {
                    Some(mask) => {
                        w.bool(true);
                        w.bytes(mask);
                    }
                    None => {
                        w.bool(false);
                    }
                }
            });
        });
    }

    let mut w = Writer::new();
    filters(&mut w, options.filters);
    filters(&mut w, options.exclusion_filters);
    w.list(options.optional_services, |w, u| {
        w.str(u.as_str());
    });
    w.list(options.optional_manufacturer_data, |w, id| {
        w.u16(*id);
    });
    w.bool(options.accept_all_devices);
    w.finish()
}
