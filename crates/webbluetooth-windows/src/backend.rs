//! The Windows engine: WinRT, reached through hand-written COM.
//!
//! Closest in shape to the Linux BlueZ backend, because WinRT is also
//! request/reply: `ReadValueAsync` returns the bytes in its own completion,
//! so there is no read-versus-notification ambiguity and no correlation FIFO.
//!
//! # Connecting is implicit
//!
//! WinRT has no `Connect`. `BluetoothLEDevice.FromBluetoothAddressAsync` hands
//! back a device object whether or not the radio has a link, and the link is
//! established by the first thing that needs it — here, service discovery. So
//! `Inner::connect` resolves the device and discovers its services, and
//! treats *that* as the connection.
//!
//! # One visible platform difference
//!
//! `BluetoothDevice::id` is the peer's Bluetooth address, as on Linux and
//! Android. Apple never exposes one.

use crate::ble;
use crate::com::{ComPtr, Delegate};
use crate::guid::{Guid, Signature};
use crate::iids::{classes, generics, interfaces, slots};
use crate::winrt;
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

/// `GattCommunicationStatus.Success`.
const GATT_SUCCESS: i32 = 0;
/// `GattClientCharacteristicConfigurationDescriptorValue`.
const CCCD_NONE: i32 = 0;
const CCCD_NOTIFY: i32 = 1;
const CCCD_INDICATE: i32 = 2;
/// `GattWriteOption`.
const WRITE_WITH_RESPONSE: i32 = 0;
const WRITE_WITHOUT_RESPONSE: i32 = 1;
/// `BluetoothLEScanningMode.Active` — asks for scan responses, which carry the
/// name most peripherals leave out of the advertisement itself.
const SCANNING_MODE_ACTIVE: i32 = 1;

/// The signature of a runtime class, for computing a handler's IID.
/// The signature of `DeviceInformationCollection`.
///
/// Built here rather than taken from the generated table because its default
/// interface is `IVectorView<DeviceInformation>` — parameterised, so it has no
/// fixed IID to extract and the generator skips the class entirely. The IID is
/// computed the same way the runtime computes it: hash the generic's GUID
/// together with its argument's signature.
fn device_information_collection() -> Signature {
    let element = Signature::Parameterized {
        generic: generics::I_VECTOR_VIEW,
        arguments: vec![class_signature(classes::DEVICE_INFORMATION)],
    };
    Signature::Class {
        name: "Windows.Devices.Enumeration.DeviceInformationCollection",
        default_interface: element
            .iid()
            .expect("a parameterised signature always has an IID"),
    }
}

/// The ATT MTU every LE link is guaranteed to carry before negotiation.
const ATT_DEFAULT_MTU: u16 = 23;

fn class_signature(class: classes::Class) -> Signature {
    Signature::Class {
        name: class.0,
        default_interface: class.1,
    }
}

/// A handle into a device's attribute table.
#[derive(Clone, Debug)]
pub struct Handle(Arc<HandleData>);

#[derive(Debug)]
struct HandleData {
    object: ComPtr,
    uuid: BluetoothUuid,
    properties: u32,
    /// The last value read or notified.
    ///
    /// Every other platform hands this back from its own cache —
    /// `CBCharacteristic.value`, BlueZ's `Value` property, Android's
    /// `getValue()`. WinRT keeps nothing, so it is kept here, on the handle
    /// rather than in a map beside it: a map would have to be keyed by the COM
    /// pointer, and a freed characteristic's address can be handed to the next
    /// one, which would serve one characteristic's value for another. Living
    /// on the handle, the entry cannot outlive the object it describes.
    value: Mutex<Option<Vec<u8>>>,
}

impl Handle {
    fn object(&self) -> &ComPtr {
        &self.0.object
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

/// What this backend needs to reach a device.
struct DeviceData {
    address: u64,
    /// `BluetoothLEDevice`, once resolved.
    device: Option<ComPtr>,
    /// Services discovered on connect; WinRT caches them behind the same call,
    /// but each round trip is an IPC hop worth avoiding.
    services: Vec<(BluetoothUuid, ComPtr)>,
}

/// The last thing an advertisement said about a device.
struct Sighting {
    address: u64,
    name: Option<String>,
    rssi: i32,
    at: std::time::Instant,
}

pub struct Inner {
    /// `false` when WinRT could not be reached at all.
    available: bool,
    /// Everyone watching the scan; the watcher is shared and reference-counted.
    hub: ScanHub,
    /// What each advertisement told us, kept so the chooser's answer is still
    /// resolvable after it returns — and so signal strength can be reported at
    /// all. See [`Inner::read_rssi`].
    sightings: Mutex<HashMap<String, Sighting>>,
    /// The advertisement watcher, and the delegate it calls.
    watcher: Mutex<Option<(ComPtr, ComPtr)>>,
    devices: DeviceRegistry<DeviceData>,
    /// Subscribers per characteristic, and the handler keeping them fed.
    notifications: Mutex<HashMap<usize, Vec<webbluetooth_core::backlog::Sender<Vec<u8>>>>>,
    value_handlers: Mutex<HashMap<usize, ComPtr>>,
    /// The `GattServicesChanged` delegate per device, held so it outlives the
    /// registration. Keyed by device id rather than pointer: one per device.
    services_changed_handlers: Mutex<HashMap<String, ComPtr>>,
    /// Handed to event handlers so they can reach the registry without owning
    /// the engine — a strong reference here would be a cycle that never frees.
    self_ref: Weak<Inner>,
    /// The controller `select_adapter` chose, if one was chosen.
    selected: Mutex<Option<String>>,
    /// `GattSession.MaxPduSize` per device, read once when the link comes up.
    pdu_sizes: Mutex<HashMap<String, u16>>,
}

impl Inner {
    pub fn new(show_power_alert: bool) -> Arc<Self> {
        Self::with_restoration(show_power_alert, None, BTreeSet::new())
    }

    /// Windows preserves nothing across a process restart, so the identifier is
    /// accepted and ignored.
    pub fn with_restoration(
        _show_power_alert: bool,
        _restore_identifier: Option<&str>,
        _restore_allowed: BTreeSet<BluetoothUuid>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak: &Weak<Inner>| Inner {
            // Joining the multi-threaded apartment is what makes every later
            // call legal on any thread.
            available: winrt::initialize(),
            hub: ScanHub::new(),
            sightings: Mutex::new(HashMap::new()),
            watcher: Mutex::new(None),
            devices: DeviceRegistry::default(),
            notifications: Mutex::new(HashMap::new()),
            value_handlers: Mutex::new(HashMap::new()),
            services_changed_handlers: Mutex::new(HashMap::new()),
            self_ref: weak.clone(),
            selected: Mutex::new(None),
            pdu_sizes: Mutex::new(HashMap::new()),
        })
    }

    // ── Availability ────────────────────────────────────────────────────────

    pub fn state(&self) -> ManagerState {
        if !self.available {
            return ManagerState::Unsupported;
        }
        // Reaching the statics proves the Bluetooth stack is present; whether a
        // radio is switched on only shows when something is asked of it.
        match ble::statics(
            classes::BLUETOOTH_LE_DEVICE,
            interfaces::I_BLUETOOTH_LE_DEVICE_STATICS,
        ) {
            Some(_) => ManagerState::PoweredOn,
            None => ManagerState::Unsupported,
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

    /// Await a WinRT operation, given the signature of what it returns.
    async fn await_operation(&self, operation: ComPtr, result: Signature) -> Result<ComPtr> {
        let (tx, rx) = oneshot::channel();
        let sender = Mutex::new(Some(tx));
        let attached = ble::on_completed(&operation, result, move |value| {
            if let Ok(mut guard) = sender.lock() {
                if let Some(tx) = guard.take() {
                    let _ = tx.send(value);
                }
            }
        });
        if !attached {
            return Err(Error::Network(
                "could not attach a completion handler".into(),
            ));
        }
        // The operation must outlive its handler, which fires on a pool thread.
        let held = operation;
        let value = rx
            .await
            .map_err(|_| Error::Aborted("the operation was cancelled".into()))?;
        drop(held);
        value.ok_or_else(|| Error::Network("the operation reported no result".into()))
    }

    // ── Scanning ────────────────────────────────────────────────────────────

    pub fn scan_hub(&self) -> &ScanHub {
        &self.hub
    }

    /// Start or stop the advertisement watcher.
    ///
    /// WinRT's watcher takes no service filter, so a watcher that widens the
    /// hub's filter needs nothing done here: starting is a no-op when one is
    /// already running, and only the last watcher leaving stops it.
    pub fn set_radio_scanning(self: &Arc<Self>, on: bool) -> Result<()> {
        if !on {
            if let Some((watcher, _handler)) = self.watcher.lock().unwrap().take() {
                ble::call_void(&watcher, slots::ibluetooth_leadvertisement_watcher::STOP);
            }
            return Ok(());
        }
        if self.watcher.lock().unwrap().is_some() {
            return Ok(());
        }

        let watcher = ble::activate(classes::BLUETOOTH_LE_ADVERTISEMENT_WATCHER)
            .ok_or(Error::NotAvailable(Availability::Unsupported))?;

        // Active scanning asks for a scan response, which is where most
        // peripherals put their name.
        unsafe {
            let f: unsafe extern "system" fn(*mut std::ffi::c_void, i32) -> i32 =
                watcher.method(slots::ibluetooth_leadvertisement_watcher::SET_SCANNING_MODE);
            f(watcher.as_raw(), SCANNING_MODE_ACTIVE);
        }

        // `Received` is a TypedEventHandler, whose IID is computed from both
        // its argument types.
        let handler_iid = Signature::Parameterized {
            generic: generics::TYPED_EVENT_HANDLER,
            arguments: vec![
                class_signature(classes::BLUETOOTH_LE_ADVERTISEMENT_WATCHER),
                class_signature(classes::BLUETOOTH_LE_ADVERTISEMENT_RECEIVED_EVENT_ARGS),
            ],
        }
        .iid()
        .expect("a parameterised signature always has an IID");

        let engine = Arc::downgrade(self);
        let handler = Delegate::new(handler_iid, move |_sender, args| {
            let Some(engine) = engine.upgrade() else {
                return;
            };
            if args.is_null() {
                return;
            }
            // Borrowed for the duration of the callback; not ours to release.
            let args = std::mem::ManuallyDrop::new(unsafe { ComPtr::adopt(args) });
            if let Some(args) = args.as_ref() {
                engine.on_advertisement(args);
            }
        });

        let mut token: i64 = 0;
        unsafe {
            let f: unsafe extern "system" fn(
                *mut std::ffi::c_void,
                *mut std::ffi::c_void,
                *mut i64,
            ) -> i32 = watcher.method(slots::ibluetooth_leadvertisement_watcher::RECEIVED);
            f(watcher.as_raw(), handler.as_raw(), &mut token);
        }

        if !ble::call_void(&watcher, slots::ibluetooth_leadvertisement_watcher::START) {
            return Err(Error::Network(
                "the advertisement watcher would not start".into(),
            ));
        }
        *self.watcher.lock().unwrap() = Some((watcher, handler));
        Ok(())
    }

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
        let Some((address, name)) = self
            .sightings
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| (s.address, s.name.clone()))
        else {
            return Err(Error::NotFound(format!(
                "the chooser returned {id:?}, which was not among the devices it was offered"
            )));
        };

        self.devices.insert(
            &id,
            name,
            allowed,
            false,
            DeviceData {
                address,
                device: None,
                services: Vec::new(),
            },
        );
        Ok(id)
    }

    /// One advertisement, from the watcher's callback.
    fn on_advertisement(&self, args: &ComPtr) {
        if !self.hub.is_watching() {
            return;
        }

        use slots::ibluetooth_leadvertisement_received_event_args as ev;
        let Some(address) = ble::get_u64(args, ev::BLUETOOTH_ADDRESS) else {
            return;
        };
        let rssi = ble::get_i16(args, ev::RAW_SIGNAL_STRENGTH_IN_D_BM).unwrap_or(127) as i32;

        let mut name = None;
        let mut service_uuids = Vec::new();
        if let Some(advertisement) = ble::get_object(args, ev::ADVERTISEMENT) {
            use slots::ibluetooth_leadvertisement as ad;
            name = ble::get_string(&advertisement, ad::LOCAL_NAME).filter(|s| !s.is_empty());
            if let Some(uuids) = ble::get_object(&advertisement, ad::SERVICE_UUIDS) {
                // An IVectorView<Guid> of by-value GUIDs rather than objects,
                // so it is read directly instead of through `for_each`.
                service_uuids = read_guid_vector(&uuids);
            }
        }

        let advertisement = Advertisement {
            local_name: name.clone(),
            tx_power: None,
            // Would mean walking the raw data sections.
            appearance: None,
            is_connectable: Some(true),
            service_uuids: service_uuids
                .iter()
                .filter_map(|g| BluetoothUuid::parse(&g.to_string()).ok())
                .collect(),
            overflow_service_uuids: Vec::new(),
            solicited_service_uuids: Vec::new(),
            manufacturer_data: HashMap::new(),
            service_data: HashMap::new(),
            rssi,
        };

        let id = ble::format_address(address);
        self.sightings.lock().unwrap().insert(
            id.clone(),
            Sighting {
                address,
                name: name.clone(),
                rssi,
                at: std::time::Instant::now(),
            },
        );
        // Each watcher applies its own filter; this one just reports.
        self.hub.publish(&id, name.as_deref(), &advertisement);
    }

    // ── Device access ───────────────────────────────────────────────────────

    // The registry answers these identically for every backend.
    webbluetooth_core::registry::forward_to_registry!();

    // ── PHY ─────────────────────────────────────────────────────────────────

    /// `BluetoothLEDevice.GetConnectionPhy`, on Windows 10 2004 or newer.
    ///
    /// WinRT reports the PHY and offers no way to choose one, so this is the
    /// half of the pair that exists here.
    pub async fn phy(&self, id: &str) -> Result<webbluetooth_core::ConnectionPhy> {
        let device = self.device(id)?;
        let device6 = device
            .cast(interfaces::I_BLUETOOTH_LE_DEVICE6)
            .ok_or_else(|| {
                Error::NotSupported(
                    "IBluetoothLEDevice6 is unavailable; the connection PHY needs \
                     Windows 10 2004 or newer"
                        .into(),
                )
            })?;
        let phy = ble::get_object(&device6, slots::ibluetooth_ledevice6::GET_CONNECTION_PHY)
            .ok_or_else(|| Error::Network("GetConnectionPhy was refused".into()))?;

        // `BluetoothLEConnectionPhy` carries a `TransmitInfo` and a
        // `ReceiveInfo`, each a `BluetoothLEConnectionPhyInfo` with three
        // booleans — the PHY is reported as which one it is, not as a number.
        let read = |slot| -> Result<webbluetooth_core::Phy> {
            let info = ble::get_object(&phy, slot)
                .ok_or_else(|| Error::Network("a PHY direction was unreadable".into()))?;
            if ble::get_bool(
                &info,
                slots::ibluetooth_leconnection_phy_info::IS_UNCODED2_M_PHY,
            )
            .unwrap_or(false)
            {
                return Ok(webbluetooth_core::Phy::Le2M);
            }
            if ble::get_bool(&info, slots::ibluetooth_leconnection_phy_info::IS_CODED_PHY)
                .unwrap_or(false)
            {
                return Ok(webbluetooth_core::Phy::LeCoded);
            }
            // `IsUncoded1MPhy`, or none set at all — which is what an
            // unsupported or not-yet-established link reports.
            Ok(webbluetooth_core::Phy::Le1M)
        };

        Ok(webbluetooth_core::ConnectionPhy {
            tx: read(slots::ibluetooth_leconnection_phy::TRANSMIT_INFO)?,
            rx: read(slots::ibluetooth_leconnection_phy::RECEIVE_INFO)?,
        })
    }

    /// Not possible: WinRT reports the PHY and offers no way to ask for one.
    pub async fn set_preferred_phy(
        &self,
        id: &str,
        _tx: webbluetooth_core::Phy,
        _rx: webbluetooth_core::Phy,
    ) -> Result<webbluetooth_core::ConnectionPhy> {
        let _ = self.device(id)?;
        Err(Error::NotSupported(
            "WinRT reports the connection PHY but has no way to request one".into(),
        ))
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    /// `DeviceInformationPairing.PairAsync`.
    ///
    /// Three hops to reach it: the device carries a `DeviceInformation` on a
    /// later interface, which carries a `Pairing` on a later interface still,
    /// which is what runs the ceremony. Windows picks the ceremony itself here
    /// and shows its own prompt — the custom form, where an application drives
    /// passkey entry, is a different interface and a different design
    /// decision.
    pub async fn pair(&self, id: &str) -> Result<webbluetooth_core::Pairing> {
        let pairing = self.pairing_of(id)?;
        if ble::get_bool(&pairing, slots::idevice_information_pairing::IS_PAIRED).unwrap_or(false) {
            return Ok(webbluetooth_core::Pairing::AlreadyPaired);
        }

        let operation = invoke_async(&pairing, slots::idevice_information_pairing::PAIR_ASYNC)
            .ok_or_else(|| Error::Network("PairAsync was refused".into()))?;
        let result = self
            .await_operation(operation, class_signature(classes::DEVICE_PAIRING_RESULT))
            .await?;

        // `DevicePairingResultStatus`: 0 is Paired, 1 is NotReadyToPair, and
        // 16 is AlreadyPaired. Everything else is a refusal of some kind, and
        // the number is carried through because the distinctions matter to
        // someone debugging a device that will not bond.
        match ble::get_i32(&result, slots::idevice_pairing_result::STATUS) {
            Some(0) => Ok(webbluetooth_core::Pairing::Paired),
            Some(16) => Ok(webbluetooth_core::Pairing::AlreadyPaired),
            Some(status) => Err(Error::Network(format!(
                "pairing did not complete (DevicePairingResultStatus {status})"
            ))),
            None => Err(Error::Network("the pairing result was unreadable".into())),
        }
    }

    pub async fn is_paired(&self, id: &str) -> Result<bool> {
        let pairing = self.pairing_of(id)?;
        Ok(ble::get_bool(&pairing, slots::idevice_information_pairing::IS_PAIRED).unwrap_or(false))
    }

    /// The `DeviceInformationPairing` for a connected device.
    fn pairing_of(&self, id: &str) -> Result<ComPtr> {
        let device = self.device(id)?;
        let device2 = device
            .cast(interfaces::I_BLUETOOTH_LE_DEVICE2)
            .ok_or_else(|| Error::NotSupported("IBluetoothLEDevice2 is unavailable".into()))?;
        let information =
            ble::get_object(&device2, slots::ibluetooth_ledevice2::DEVICE_INFORMATION)
                .ok_or_else(|| Error::Network("the device has no DeviceInformation".into()))?;
        let information2 = information
            .cast(interfaces::I_DEVICE_INFORMATION2)
            .ok_or_else(|| Error::NotSupported("IDeviceInformation2 is unavailable".into()))?;
        ble::get_object(&information2, slots::idevice_information2::PAIRING)
            .ok_or_else(|| Error::Network("the device exposes no pairing information".into()))
    }

    // ── Controllers ─────────────────────────────────────────────────────
    //
    // Windows genuinely has several, and finds them the way it finds any
    // device class: `BluetoothAdapter.GetDeviceSelector()` builds an AQS
    // query, `DeviceInformation.FindAllAsync` runs it, and each result is
    // opened by id. Three round trips rather than one call, because WinRT has
    // no "list the radios" method.

    pub async fn adapters(&self) -> Result<Vec<webbluetooth_core::AdapterInfo>> {
        let statics = ble::statics(
            classes::BLUETOOTH_ADAPTER,
            interfaces::I_BLUETOOTH_ADAPTER_STATICS,
        )
        .ok_or(Error::NotAvailable(Availability::Unsupported))?;

        let selector = ble::get_string(
            &statics,
            slots::ibluetooth_adapter_statics::GET_DEVICE_SELECTOR,
        )
        .ok_or_else(|| Error::Network("GetDeviceSelector was refused".into()))?;

        let found = self.find_all(&selector).await?;
        let default = self.default_adapter_id().await;

        let mut out = Vec::new();
        ble::for_each(&found, |information| {
            let Some(id) = ble::get_string(&information, slots::idevice_information::ID) else {
                return;
            };
            out.push(webbluetooth_core::AdapterInfo {
                name: ble::get_string(&information, slots::idevice_information::NAME)
                    .filter(|n| !n.is_empty()),
                // `IsEnabled` on the device rather than the radio's own state:
                // reading the radio needs `Radio.GetRadiosAsync`, a separate
                // capability, and a disabled device is an unusable one either
                // way.
                powered: ble::get_bool(&information, slots::idevice_information::IS_ENABLED)
                    .unwrap_or(false),
                address: None,
                is_default: default.as_deref() == Some(id.as_str()),
                id,
            });
        });

        // The address needs the adapter itself opened, which is a round trip
        // each, so it is done after the cheap listing rather than inside it.
        for adapter in &mut out {
            adapter.address = self.adapter_address(&adapter.id).await;
        }
        Ok(out)
    }

    pub async fn adapter(&self) -> Result<webbluetooth_core::AdapterInfo> {
        let adapters = self.adapters().await?;
        adapters
            .iter()
            .find(|a| Some(a.id.as_str()) == self.selected.lock().unwrap().as_deref())
            .or_else(|| adapters.iter().find(|a| a.is_default))
            .or_else(|| adapters.first())
            .cloned()
            .ok_or(Error::NotAvailable(Availability::Unsupported))
    }

    pub async fn select_adapter(&self, id: &str) -> Result<()> {
        let adapters = self.adapters().await?;
        if !adapters.iter().any(|a| a.id == id) {
            return Err(Error::NotFound(format!(
                "no controller with id {id:?}; this system has {}",
                adapters
                    .iter()
                    .map(|a| a.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        *self.selected.lock().unwrap() = Some(id.to_owned());
        Ok(())
    }

    /// `DeviceInformation.FindAllAsync(selector)`.
    async fn find_all(&self, selector: &str) -> Result<ComPtr> {
        let statics = ble::statics(
            classes::DEVICE_INFORMATION,
            interfaces::I_DEVICE_INFORMATION_STATICS,
        )
        .ok_or(Error::NotAvailable(Availability::Unsupported))?;
        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        // The `HString` goes with it. A `WinRtString` owns a `*mut c_void`,
        // so keeping one alive over the await below would make this future
        // `!Send` just as surely as the out-parameter does — and it is only
        // needed for the call itself.
        let operation = {
            let selector = winrt::WinRtString::new(selector)
                .ok_or_else(|| Error::Network("could not build the device selector".into()))?;
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    winrt::HString,
                    *mut *mut std::ffi::c_void,
                ) -> i32 =
                    statics.method(slots::idevice_information_statics::FIND_ALL_ASYNC_AQS_FILTER);
                f(statics.as_raw(), selector.as_raw(), &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
        .ok_or_else(|| Error::Network("FindAllAsync was refused".into()))?;

        self.await_operation(operation, device_information_collection())
            .await
    }

    /// The id of the controller `GetDefaultAsync` returns, if there is one.
    async fn default_adapter_id(&self) -> Option<String> {
        let statics = ble::statics(
            classes::BLUETOOTH_ADAPTER,
            interfaces::I_BLUETOOTH_ADAPTER_STATICS,
        )?;
        let operation = invoke_async(
            &statics,
            slots::ibluetooth_adapter_statics::GET_DEFAULT_ASYNC,
        )?;
        let adapter = self
            .await_operation(operation, class_signature(classes::BLUETOOTH_ADAPTER))
            .await
            .ok()?;
        ble::get_string(&adapter, slots::ibluetooth_adapter::DEVICE_ID)
    }

    /// Open one controller by id and read its own address.
    async fn adapter_address(&self, id: &str) -> Option<String> {
        let statics = ble::statics(
            classes::BLUETOOTH_ADAPTER,
            interfaces::I_BLUETOOTH_ADAPTER_STATICS,
        )?;
        // The raw out-parameter is confined to this block, and so is the
        // `HString`: both are `*mut c_void` under the name, and either one
        // alive across the await below would make this future `!Send`.
        let operation = {
            let id = winrt::WinRtString::new(id)?;
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    winrt::HString,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = statics.method(slots::ibluetooth_adapter_statics::FROM_ID_ASYNC);
                f(statics.as_raw(), id.as_raw(), &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }?;
        let adapter = self
            .await_operation(operation, class_signature(classes::BLUETOOTH_ADAPTER))
            .await
            .ok()?;
        let address = ble::get_u64(&adapter, slots::ibluetooth_adapter::BLUETOOTH_ADDRESS)?;
        Some(ble::format_address(address))
    }

    /// `BluetoothLEDevice.ConnectionParameters`, on Windows 10 2004 or newer.
    ///
    /// The one platform that hands an application the negotiated values
    /// directly. It lives on `IBluetoothLEDevice6`, so an older Windows
    /// answers `NotSupported` rather than failing.
    pub async fn connection_parameters(
        &self,
        id: &str,
    ) -> Result<webbluetooth_core::ConnectionParameters> {
        let device = self.device(id)?;
        let device6 = device
            .cast(interfaces::I_BLUETOOTH_LE_DEVICE6)
            .ok_or_else(|| {
                Error::NotSupported(
                    "IBluetoothLEDevice6 is unavailable; connection parameters need \
                     Windows 10 2004 or newer"
                        .into(),
                )
            })?;
        let parameters = ble::get_object(
            &device6,
            slots::ibluetooth_ledevice6::GET_CONNECTION_PARAMETERS,
        )
        .ok_or_else(|| Error::Network("GetConnectionParameters was refused".into()))?;

        let read = |slot| {
            ble::get_u16(&parameters, slot)
                .ok_or_else(|| Error::Network("a connection parameter was unreadable".into()))
        };
        Ok(webbluetooth_core::ConnectionParameters {
            interval: read(slots::ibluetooth_leconnection_parameters::CONNECTION_INTERVAL)?,
            latency: read(slots::ibluetooth_leconnection_parameters::CONNECTION_LATENCY)?,
            timeout: read(slots::ibluetooth_leconnection_parameters::LINK_TIMEOUT)?,
        })
    }

    /// Adopt a device by address.
    ///
    /// `connect` resolves the `BluetoothLEDevice` from the address anyway —
    /// WinRT has `FromBluetoothAddressAsync` and no notion of a device object
    /// that predates it — so adopting is recording the address and the grant.
    pub async fn adopt_device(
        &self,
        id: &str,
        allowed: webbluetooth_core::registry::Grant,
    ) -> Result<String> {
        let address = ble::parse_address(id)
            .ok_or_else(|| Error::NotFound(format!("{id:?} is not a Bluetooth address")))?;
        let name = self
            .sightings
            .lock()
            .unwrap()
            .get(id)
            .and_then(|s| s.name.clone());
        self.devices.insert(
            id,
            name,
            allowed,
            false,
            DeviceData {
                address,
                device: None,
                services: Vec::new(),
            },
        );
        Ok(id.to_owned())
    }

    async fn gatt_lock(&self, id: &str) -> Result<futures_util::lock::OwnedMutexGuard<()>> {
        self.devices.gatt_lock(id).await
    }

    /// Revoke a grant. Releasing the device object is what drops the link —
    /// Windows keeps it while any reference survives.
    pub fn forget(&self, id: &str) {
        let _ = self.devices.remove(id);
    }

    fn device(&self, id: &str) -> Result<ComPtr> {
        self.devices
            .get(id, |d| d.inner.device.clone())?
            .ok_or_else(|| Error::InvalidState("the device is not connected".into()))
    }

    // ── Attribute accessors ─────────────────────────────────────────────────

    pub fn service_is_primary(&self, _handle: &Handle) -> bool {
        // GetGattServicesAsync returns primary services only.
        true
    }

    pub fn characteristic_properties(&self, handle: &Handle) -> CharacteristicProperties {
        // GattCharacteristicProperties are the Bluetooth bit values.
        CharacteristicProperties(handle.0.properties)
    }

    pub fn cached_value(&self, handle: &Handle) -> Option<Vec<u8>> {
        handle.0.value.lock().unwrap().clone()
    }

    pub fn is_notifying(&self, handle: &Handle) -> bool {
        self.notifications
            .lock()
            .unwrap()
            .contains_key(&handle.object().key())
    }

    // ── GATT operations ─────────────────────────────────────────────────────

    pub async fn connect(self: &Arc<Self>, id: &str) -> Result<()> {
        self.require_powered_on().await?;
        if self.is_connected(id) {
            return Ok(());
        }
        let address = self.devices.get(id, |d| d.inner.address)?;

        let statics = ble::statics(
            classes::BLUETOOTH_LE_DEVICE,
            interfaces::I_BLUETOOTH_LE_DEVICE_STATICS,
        )
        .ok_or(Error::NotAvailable(Availability::Unsupported))?;

        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    u64,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = statics
                    .method(slots::ibluetooth_ledevice_statics::FROM_BLUETOOTH_ADDRESS_ASYNC);
                f(statics.as_raw(), address, &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
        .ok_or_else(|| Error::Network("FromBluetoothAddressAsync was refused".into()))?;

        let device = self
            .await_operation(operation, class_signature(classes::BLUETOOTH_LE_DEVICE))
            .await
            .map_err(|_| Error::Network(format!("no device at {id}")))?;

        // WinRT has no Connect; discovering services is what opens the link.
        let services = self.discover(&device).await?;

        self.attach_services_changed_handler(id, &device);

        self.devices.update(id, |d| {
            d.connected = true;
            d.inner.device = Some(device);
            d.inner.services = services;
        });
        // After the device is stored, because reading it goes back through the
        // registry. A failure here is not a failed connection: it costs a
        // larger MTU, not the link.
        self.read_max_pdu_size(id).await;
        Ok(())
    }

    /// Subscribe to `GattServicesChanged`, WinRT's account of the Service
    /// Changed indication.
    ///
    /// Windows subscribes to that characteristic itself and re-discovers, so
    /// what arrives is the result rather than the indication — the same shape
    /// as CoreBluetooth's `didModifyServices:` and BlueZ's GATT objects coming
    /// and going. Without it every handle stays valid on paper after the peer
    /// has rearranged its attribute table underneath them.
    fn attach_services_changed_handler(&self, id: &str, device: &ComPtr) {
        let handler_iid = Signature::Parameterized {
            generic: generics::TYPED_EVENT_HANDLER,
            arguments: vec![
                class_signature(classes::BLUETOOTH_LE_DEVICE),
                Signature::OBJECT,
            ],
        }
        .iid()
        .expect("a parameterised signature always has an IID");

        // Weak, so the handler the device owns does not own the engine back.
        // Once the engine is gone there is nobody left to tell.
        let engine = self.self_ref.clone();
        let id = id.to_owned();
        let watched = id.clone();
        let handler = Delegate::new(handler_iid, move |_sender, _args| {
            if let Some(engine) = engine.upgrade() {
                engine.devices.mark_services_changed(&watched);
            }
        });

        let mut token: i64 = 0;
        unsafe {
            let f: unsafe extern "system" fn(
                *mut std::ffi::c_void,
                *mut std::ffi::c_void,
                *mut i64,
            ) -> i32 = device.method(slots::ibluetooth_ledevice::GATT_SERVICES_CHANGED);
            f(device.as_raw(), handler.as_raw(), &mut token);
        }
        self.services_changed_handlers
            .lock()
            .unwrap()
            .insert(id, handler);
    }

    /// `GetGattServicesAsync`, unwrapped into `(uuid, service)` pairs.
    async fn discover(&self, device: &ComPtr) -> Result<Vec<(BluetoothUuid, ComPtr)>> {
        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = device.method(slots::ibluetooth_ledevice3::GET_GATT_SERVICES_ASYNC);
                f(device.as_raw(), &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
        .ok_or_else(|| Error::Network("GetGattServicesAsync was refused".into()))?;

        let result = self
            .await_operation(
                operation,
                class_signature(classes::GATT_DEVICE_SERVICES_RESULT),
            )
            .await?;

        let status =
            ble::get_i32(&result, slots::igatt_device_services_result::STATUS).unwrap_or(i32::MIN);
        if status != GATT_SUCCESS {
            return Err(gatt_error("service discovery", status));
        }

        let mut services = Vec::new();
        if let Some(vector) =
            ble::get_object(&result, slots::igatt_device_services_result::SERVICES)
        {
            ble::for_each(&vector, |service| {
                if let Some(uuid) = ble::get_guid(&service, slots::igatt_device_service::UUID) {
                    if let Ok(uuid) = BluetoothUuid::parse(&uuid.to_string()) {
                        services.push((uuid, service));
                    }
                }
            });
        }
        Ok(services)
    }

    pub fn disconnect(&self, id: &str) {
        // Releasing every reference is how Windows is told to drop the link.
        self.services_changed_handlers.lock().unwrap().remove(id);
        // Negotiated afresh on the next connection.
        self.pdu_sizes.lock().unwrap().remove(id);
        self.devices.update(id, |d| {
            d.inner.services.clear();
            d.inner.device = None;
        });
        self.devices.mark_disconnected(id);
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
            .filter(|(found, _)| uuid.is_none_or(|want| want == found))
            .map(|(found, object)| {
                Handle(Arc::new(HandleData {
                    object,
                    uuid: found,
                    properties: 0,
                    value: Mutex::new(None),
                }))
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

        // `IGattDeviceService3`, like the characteristic call — the original
        // `IGattDeviceService.GetIncludedServices` is synchronous and
        // deprecated.
        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = service
                    .object()
                    .method(slots::igatt_device_service3::GET_INCLUDED_SERVICES_ASYNC);
                f(service.object().as_raw(), &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
        .ok_or_else(|| Error::Network("GetIncludedServicesAsync was refused".into()))?;

        let result = self
            .await_operation(
                operation,
                class_signature(classes::GATT_DEVICE_SERVICES_RESULT),
            )
            .await?;
        let status =
            ble::get_i32(&result, slots::igatt_device_services_result::STATUS).unwrap_or(i32::MIN);
        if status != GATT_SUCCESS {
            return Err(gatt_error("included service discovery", status));
        }

        let mut out = Vec::new();
        if let Some(vector) =
            ble::get_object(&result, slots::igatt_device_services_result::SERVICES)
        {
            ble::for_each(&vector, |included| {
                let Some(found) = ble::get_guid(&included, slots::igatt_device_service::UUID)
                else {
                    return;
                };
                let Ok(found) = BluetoothUuid::parse(&found.to_string()) else {
                    return;
                };
                if uuid.is_some_and(|want| *want != found) {
                    return;
                }
                out.push(Handle(Arc::new(HandleData {
                    object: included,
                    uuid: found,
                    // A service declaration has no properties.
                    properties: 0,
                    value: Mutex::new(None),
                })));
            });
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

        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = service
                    .object()
                    .method(slots::igatt_device_service3::GET_CHARACTERISTICS_ASYNC);
                f(service.object().as_raw(), &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
        .ok_or_else(|| Error::Network("GetCharacteristicsAsync was refused".into()))?;

        let result = self
            .await_operation(
                operation,
                class_signature(classes::GATT_CHARACTERISTICS_RESULT),
            )
            .await?;
        let status =
            ble::get_i32(&result, slots::igatt_characteristics_result::STATUS).unwrap_or(i32::MIN);
        if status != GATT_SUCCESS {
            return Err(gatt_error("characteristic discovery", status));
        }

        let mut out = Vec::new();
        if let Some(vector) = ble::get_object(
            &result,
            slots::igatt_characteristics_result::CHARACTERISTICS,
        ) {
            ble::for_each(&vector, |characteristic| {
                let Some(found) = ble::get_guid(&characteristic, slots::igatt_characteristic::UUID)
                else {
                    return;
                };
                let Ok(found) = BluetoothUuid::parse(&found.to_string()) else {
                    return;
                };
                if uuid.is_some_and(|want| *want != found) {
                    return;
                }
                let properties = ble::get_i32(
                    &characteristic,
                    slots::igatt_characteristic::CHARACTERISTIC_PROPERTIES,
                )
                .unwrap_or(0) as u32;
                out.push(Handle(Arc::new(HandleData {
                    object: characteristic,
                    uuid: found,
                    properties,
                    value: Mutex::new(None),
                })));
            });
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

        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = characteristic
                    .object()
                    .method(slots::igatt_characteristic3::GET_DESCRIPTORS_ASYNC);
                f(characteristic.object().as_raw(), &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
        .ok_or_else(|| Error::Network("GetDescriptorsAsync was refused".into()))?;

        let result = self
            .await_operation(operation, class_signature(classes::GATT_DESCRIPTORS_RESULT))
            .await?;

        let mut out = Vec::new();
        if let Some(vector) = ble::get_object(&result, slots::igatt_descriptors_result::DESCRIPTORS)
        {
            ble::for_each(&vector, |descriptor| {
                let Some(found) = ble::get_guid(&descriptor, slots::igatt_descriptor::UUID) else {
                    return;
                };
                let Ok(found) = BluetoothUuid::parse(&found.to_string()) else {
                    return;
                };
                if uuid.is_some_and(|want| *want != found) {
                    return;
                }
                out.push(Handle(Arc::new(HandleData {
                    object: descriptor,
                    uuid: found,
                    properties: 0,
                    value: Mutex::new(None),
                })));
            });
        }
        Ok(out)
    }

    pub async fn read_characteristic(&self, id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let operation = invoke_async(
            handle.object(),
            slots::igatt_characteristic::READ_VALUE_ASYNC,
        )
        .ok_or_else(|| Error::Network("ReadValueAsync was refused".into()))?;
        let result = self
            .await_operation(operation, class_signature(classes::GATT_READ_RESULT))
            .await?;

        let status = ble::get_i32(&result, slots::igatt_read_result::STATUS).unwrap_or(i32::MIN);
        if status != GATT_SUCCESS {
            return Err(gatt_error("read", status));
        }
        let buffer = ble::get_object(&result, slots::igatt_read_result::VALUE)
            .ok_or_else(|| Error::Network("the read returned no buffer".into()))?;
        let bytes = ble::buffer_to_bytes(&buffer)
            .ok_or_else(|| Error::Network("the read buffer could not be copied".into()))?;
        *handle.0.value.lock().unwrap() = Some(bytes.clone());
        Ok(bytes)
    }

    pub async fn write_characteristic(
        &self,
        id: &str,
        handle: &Handle,
        value: &[u8],
        write_type: WriteType,
    ) -> Result<()> {
        // Only the acknowledged form takes the lock; see the note in the Apple
        // backend. WinRT queues writes internally and reports the outcome in
        // the operation's `GattCommunicationStatus`, so an unacknowledged one
        // needs no help from us to stay ordered.
        let _guard = match write_type {
            WriteType::WithResponse => Some(self.gatt_lock(id).await?),
            WriteType::WithoutResponse => None,
        };
        let buffer = ble::bytes_to_buffer(value)
            .ok_or_else(|| Error::Network("could not build a write buffer".into()))?;
        let option = match write_type {
            WriteType::WithResponse => WRITE_WITH_RESPONSE,
            WriteType::WithoutResponse => WRITE_WITHOUT_RESPONSE,
        };

        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *mut std::ffi::c_void,
                    i32,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = handle
                    .object()
                    .method(slots::igatt_characteristic::WRITE_VALUE_WITH_OPTION_ASYNC);
                f(
                    handle.object().as_raw(),
                    buffer.as_raw(),
                    option,
                    &mut operation,
                )
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
        .ok_or_else(|| Error::Network("WriteValueWithOptionAsync was refused".into()))?;

        // The operation yields a GattCommunicationStatus, an enum rather than a
        // class, so its signature is different in kind from the others here.
        let status = self
            .await_operation(
                operation,
                Signature::Enum {
                    name:
                        "Windows.Devices.Bluetooth.GenericAttributeProfile.GattCommunicationStatus",
                    signed: true,
                },
            )
            .await;
        match status {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn set_notify(&self, id: &str, handle: &Handle, enabled: bool) -> Result<bool> {
        let _guard = self.gatt_lock(id).await?;
        let properties = CharacteristicProperties(handle.0.properties);
        let value = match (enabled, properties.notify()) {
            (false, _) => CCCD_NONE,
            (true, true) => CCCD_NOTIFY,
            (true, false) => CCCD_INDICATE,
        };

        // Subscribing on Windows is a descriptor write like everywhere else,
        // but WinRT wraps it so the descriptor never has to be found.
        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    i32,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = handle.object().method(
                    slots::igatt_characteristic::WRITE_CLIENT_CHARACTERISTIC_CONFIGURATION_DESCRIPTOR_ASYNC,
                );
                f(handle.object().as_raw(), value, &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
            .ok_or_else(|| Error::Network("the subscription write was refused".into()))?;

        self.await_operation(
            operation,
            Signature::Enum {
                name: "Windows.Devices.Bluetooth.GenericAttributeProfile.GattCommunicationStatus",
                signed: true,
            },
        )
        .await?;

        if enabled {
            self.attach_value_handler(handle);
        } else {
            self.value_handlers
                .lock()
                .unwrap()
                .remove(&handle.object().key());
            self.notifications
                .lock()
                .unwrap()
                .remove(&handle.object().key());
        }
        Ok(enabled)
    }

    /// Route `ValueChanged` into the notification stream.
    fn attach_value_handler(&self, handle: &Handle) {
        let key = handle.object().key();
        if self.value_handlers.lock().unwrap().contains_key(&key) {
            return;
        }

        let handler_iid = Signature::Parameterized {
            generic: generics::TYPED_EVENT_HANDLER,
            arguments: vec![
                class_signature(classes::GATT_CHARACTERISTIC),
                class_signature(classes::GATT_VALUE_CHANGED_EVENT_ARGS),
            ],
        }
        .iid()
        .expect("a parameterised signature always has an IID");

        // The map is shared rather than the engine, so a live subscription does
        // not keep the whole backend alive.
        let subscribers = Arc::new(());
        let _ = subscribers;
        let notifications: *const Mutex<
            HashMap<usize, Vec<webbluetooth_core::backlog::Sender<Vec<u8>>>>,
        > = &self.notifications;
        let notifications = notifications as usize;
        // Weak, so the delegate the characteristic owns does not own the
        // characteristic back. A dead handle simply stops being updated.
        let value_slot = Arc::downgrade(&handle.0);

        let handler = Delegate::new(handler_iid, move |_sender, args| {
            if args.is_null() {
                return;
            }
            let args = std::mem::ManuallyDrop::new(unsafe { ComPtr::adopt(args) });
            let Some(args) = args.as_ref() else { return };
            let Some(buffer) = ble::get_object(
                args,
                slots::igatt_value_changed_event_args::CHARACTERISTIC_VALUE,
            ) else {
                return;
            };
            let Some(bytes) = ble::buffer_to_bytes(&buffer) else {
                return;
            };

            // SAFETY: the engine outlives every handler it attached — the
            // handler is dropped when `value_handlers` is, which the engine
            // owns.
            if let Some(slot) = value_slot.upgrade() {
                *slot.value.lock().unwrap() = Some(bytes.clone());
            }

            let map = unsafe {
                &*(notifications
                    as *const Mutex<
                        HashMap<usize, Vec<webbluetooth_core::backlog::Sender<Vec<u8>>>>,
                    >)
            };
            let mut guard = map.lock().unwrap();
            if let Some(list) = guard.get_mut(&key) {
                list.retain(|tx| tx.send(bytes.clone()).is_ok());
                if list.is_empty() {
                    guard.remove(&key);
                }
            }
        });

        let mut token: i64 = 0;
        unsafe {
            let f: unsafe extern "system" fn(
                *mut std::ffi::c_void,
                *mut std::ffi::c_void,
                *mut i64,
            ) -> i32 = handle
                .object()
                .method(slots::igatt_characteristic::VALUE_CHANGED);
            f(handle.object().as_raw(), handler.as_raw(), &mut token);
        }
        self.value_handlers.lock().unwrap().insert(key, handler);
    }

    pub fn subscribe(&self, handle: &Handle) -> webbluetooth_core::backlog::Receiver<Vec<u8>> {
        // Bounded: a notification arrives on a platform callback
        // thread that must return promptly, so there is nobody to
        // apply backpressure to. See `webbluetooth_core::backlog`.
        let (tx, rx) = webbluetooth_core::backlog::channel();
        self.notifications
            .lock()
            .unwrap()
            .entry(handle.object().key())
            .or_default()
            .push(tx);
        rx
    }

    pub async fn read_descriptor(&self, id: &str, handle: &Handle) -> Result<Vec<u8>> {
        let _guard = self.gatt_lock(id).await?;
        let operation = invoke_async(handle.object(), slots::igatt_descriptor::READ_VALUE_ASYNC)
            .ok_or_else(|| Error::Network("ReadValueAsync was refused".into()))?;
        let result = self
            .await_operation(operation, class_signature(classes::GATT_READ_RESULT))
            .await?;
        let buffer = ble::get_object(&result, slots::igatt_read_result::VALUE)
            .ok_or_else(|| Error::Network("the read returned no buffer".into()))?;
        let bytes = ble::buffer_to_bytes(&buffer)
            .ok_or_else(|| Error::Network("the read buffer could not be copied".into()))?;
        *handle.0.value.lock().unwrap() = Some(bytes.clone());
        Ok(bytes)
    }

    pub async fn write_descriptor(&self, id: &str, handle: &Handle, value: &[u8]) -> Result<()> {
        // A descriptor write is always an ATT Write Request, so it always
        // takes the lock — there is no unacknowledged form of it.
        let _guard = self.gatt_lock(id).await?;
        let buffer = ble::bytes_to_buffer(value)
            .ok_or_else(|| Error::Network("could not build a write buffer".into()))?;

        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *mut std::ffi::c_void,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = handle
                    .object()
                    .method(slots::igatt_descriptor::WRITE_VALUE_ASYNC);
                f(handle.object().as_raw(), buffer.as_raw(), &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }
        .ok_or_else(|| Error::Network("WriteValueAsync was refused".into()))?;

        self.await_operation(
            operation,
            Signature::Enum {
                name: "Windows.Devices.Bluetooth.GenericAttributeProfile.GattCommunicationStatus",
                signed: true,
            },
        )
        .await?;
        Ok(())
    }

    pub async fn read_rssi(&self, id: &str) -> Result<i32> {
        // WinRT has no equivalent of `readRSSI`: signal strength arrives only
        // on an advertisement. So this reports the last advertisement's
        // reading, which is what BlueZ's cached `RSSI` property amounts to as
        // well — but only while it is recent enough to mean anything, rather
        // than handing back a measurement from some earlier minute as though
        // it described the link now.
        const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30);

        let recent = self.sightings.lock().unwrap().get(id).and_then(|s| {
            (s.at.elapsed() <= MAX_AGE && s.rssi != webbluetooth_core::filter::UNAVAILABLE_RSSI)
                .then_some(s.rssi)
        });
        if let Some(rssi) = recent {
            return Ok(rssi);
        }
        Err(Error::NotSupported(
            "Windows reports RSSI only on an advertisement, and this device has \
             not been seen recently — scan, or use requestLEScan to track it"
                .into(),
        ))
    }

    /// `BluetoothLEDevice.RequestPreferredConnectionParameters`.
    ///
    /// The three presets WinRT offers line up with our three priorities, so
    /// nothing has to be expressed in connection intervals. `IBluetoothLEDevice6`
    /// arrived in Windows 10 2004, so an older build fails the cast — which is
    /// reported as unsupported rather than as an error.
    pub async fn request_connection_priority(
        &self,
        id: &str,
        priority: webbluetooth_core::ConnectionPriority,
    ) -> Result<()> {
        use webbluetooth_core::ConnectionPriority as P;
        let _guard = self.gatt_lock(id).await?;

        let device = self
            .devices
            .get(id, |d| d.inner.device.clone())?
            .ok_or_else(|| Error::InvalidState("the device is not connected".into()))?;
        let device6 = device
            .cast(interfaces::I_BLUETOOTH_LE_DEVICE6)
            .ok_or_else(|| {
                Error::NotSupported(
                    "IBluetoothLEDevice6 is unavailable; connection parameters need \
                     Windows 10 2004 or newer"
                        .into(),
                )
            })?;

        // The presets are static properties on the parameters class.
        let statics = ble::statics(
            classes::BLUETOOTH_LE_PREFERRED_CONNECTION_PARAMETERS,
            interfaces::I_BLUETOOTH_LE_PREFERRED_CONNECTION_PARAMETERS_STATICS,
        )
        .ok_or_else(|| {
            Error::NotSupported("BluetoothLEPreferredConnectionParameters is unavailable".into())
        })?;
        let slot = match priority {
            P::Balanced => slots::ibluetooth_lepreferred_connection_parameters_statics::BALANCED,
            P::High => {
                slots::ibluetooth_lepreferred_connection_parameters_statics::THROUGHPUT_OPTIMIZED
            }
            P::LowPower => {
                slots::ibluetooth_lepreferred_connection_parameters_statics::POWER_OPTIMIZED
            }
        };
        let preset = ble::get_object(&statics, slot)
            .ok_or_else(|| Error::NotSupported("the connection preset was refused".into()))?;

        // Synchronous, unlike most of WinRT: it returns the request object
        // rather than an operation to await.
        let mut request: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = unsafe {
            let f: unsafe extern "system" fn(
                *mut std::ffi::c_void,
                *mut std::ffi::c_void,
                *mut *mut std::ffi::c_void,
            ) -> i32 = device6
                .method(slots::ibluetooth_ledevice6::REQUEST_PREFERRED_CONNECTION_PARAMETERS);
            f(device6.as_raw(), preset.as_raw(), &mut request)
        };
        let request = (crate::com::succeeded(hr))
            .then(|| unsafe { ComPtr::adopt(request) })
            .flatten()
            .ok_or_else(|| {
                Error::Network("RequestPreferredConnectionParameters was refused".into())
            })?;

        // Status 0 is `Unspecified`; anything else means Windows took a view.
        // The peripheral still decides, so a successful request says only that
        // the ask was made.
        let _ = ble::get_i32(
            &request,
            slots::ibluetooth_lepreferred_connection_parameters_request::STATUS,
        );
        Ok(())
    }

    pub fn max_write_len(&self, id: &str, _write_type: WriteType) -> Result<usize> {
        // Three bytes of ATT header come off the negotiated PDU size. Falling
        // back to the guaranteed minimum rather than failing: a write that is
        // shorter than it could be still works, whereas refusing to write at
        // all because the session could not be read does not.
        Ok(self.max_pdu_size(id).unwrap_or(ATT_DEFAULT_MTU) as usize - 3)
    }

    /// `GattSession.MaxPduSize` — the negotiated ATT MTU.
    ///
    /// WinRT does not put this on the device: a `GattSession` has to be opened
    /// for it, which needs the `BluetoothDeviceId` off a later interface than
    /// the one carrying the rest of the device. Three hops for one number,
    /// which is why the result is cached by the caller rather than read per
    /// write.
    fn max_pdu_size(&self, id: &str) -> Option<u16> {
        self.pdu_sizes.lock().unwrap().get(id).copied()
    }

    /// Open a `GattSession` and remember what it says, once per connection.
    async fn read_max_pdu_size(&self, id: &str) -> Option<u16> {
        let device = self.device(id).ok()?;
        let device4 = device.cast(interfaces::I_BLUETOOTH_LE_DEVICE4)?;
        let device_id =
            ble::get_object(&device4, slots::ibluetooth_ledevice4::BLUETOOTH_DEVICE_ID)?;

        let statics = ble::statics(classes::GATT_SESSION, interfaces::I_GATT_SESSION_STATICS)?;
        // The raw out-parameter is confined to this block. A bare
        // `*mut c_void` alive across an await makes the whole future
        // `!Send` — `ComPtr` is `Send`, the pointer it is built from is not.
        let operation = {
            let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = unsafe {
                let f: unsafe extern "system" fn(
                    *mut std::ffi::c_void,
                    *mut std::ffi::c_void,
                    *mut *mut std::ffi::c_void,
                ) -> i32 = statics.method(slots::igatt_session_statics::FROM_DEVICE_ID_ASYNC);
                f(statics.as_raw(), device_id.as_raw(), &mut operation)
            };
            (crate::com::succeeded(hr))
                .then(|| unsafe { ComPtr::adopt(operation) })
                .flatten()
        }?;
        let session = self
            .await_operation(operation, class_signature(classes::GATT_SESSION))
            .await
            .ok()?;
        let size = ble::get_u16(&session, slots::igatt_session::MAX_PDU_SIZE)?;
        // The session is dropped here. It was opened to read one property, and
        // holding it would ask Windows to maintain the connection.
        self.pdu_sizes.lock().unwrap().insert(id.to_owned(), size);
        Some(size)
    }
}

/// Call a no-argument method that returns an `IAsyncOperation`.
fn invoke_async(object: &ComPtr, slot: usize) -> Option<ComPtr> {
    let mut operation: *mut std::ffi::c_void = std::ptr::null_mut();
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> i32 =
            object.method(slot);
        f(object.as_raw(), &mut operation)
    };
    crate::com::succeeded(hr)
        .then(|| unsafe { ComPtr::adopt(operation) })
        .flatten()
}

/// Read an `IVectorView<Guid>`, whose elements are values rather than objects.
fn read_guid_vector(vector: &ComPtr) -> Vec<Guid> {
    let mut size: u32 = 0;
    let ok = unsafe {
        let f: unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32) -> i32 =
            vector.method(slots::ivector_view::SIZE);
        crate::com::succeeded(f(vector.as_raw(), &mut size))
    };
    if !ok {
        return Vec::new();
    }
    (0..size)
        .filter_map(|index| {
            let mut guid = Guid::default();
            let hr = unsafe {
                let f: unsafe extern "system" fn(*mut std::ffi::c_void, u32, *mut Guid) -> i32 =
                    vector.method(slots::ivector_view::GET_AT);
                f(vector.as_raw(), index, &mut guid)
            };
            crate::com::succeeded(hr).then_some(guid)
        })
        .collect()
}

/// Turn a `GattCommunicationStatus` into the spec's vocabulary.
fn gatt_error(what: &str, status: i32) -> Error {
    match status {
        1 => Error::Network(format!("{what}: the device is unreachable")),
        2 => Error::Security(format!("{what}: the protocol refused it")),
        3 => Error::Security(format!("{what}: access is denied — is the device paired?")),
        _ => Error::Network(format!("{what} failed (status {status})")),
    }
}

/// Whether this process may use Bluetooth.
///
/// Windows has no per-application Bluetooth permission for a desktop process —
/// a packaged app declares a capability, and a plain one does not. Reaching the
/// stack at all is the honest answer.
pub fn authorization() -> webbluetooth_core::Authorization {
    match ble::statics(
        classes::BLUETOOTH_LE_DEVICE,
        interfaces::I_BLUETOOTH_LE_DEVICE_STATICS,
    ) {
        Some(_) => webbluetooth_core::Authorization::Allowed,
        None => webbluetooth_core::Authorization::Denied,
    }
}
