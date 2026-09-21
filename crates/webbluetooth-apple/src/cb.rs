//! Typed accessors for the CoreBluetooth classes.
//!
//! Thin, borrow-based wrappers: every function takes a borrowed `Id` and does
//! one message send. Ownership lives a layer up, in [`crate::objc::Retained`].

use crate::objc::{
    self, array_map, data_bytes, dict_get, global_nsstring, nsarray, nsdata, nsstring, number_bool,
    number_i64, require_class, to_string, Retained,
};
use crate::objc::{Id, NIL};

// ── Enums ───────────────────────────────────────────────────────────────────

/// `CBManagerState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerState {
    Unknown = 0,
    Resetting = 1,
    Unsupported = 2,
    Unauthorized = 3,
    PoweredOff = 4,
    PoweredOn = 5,
}

impl From<isize> for ManagerState {
    fn from(v: isize) -> Self {
        match v {
            1 => Self::Resetting,
            2 => Self::Unsupported,
            3 => Self::Unauthorized,
            4 => Self::PoweredOff,
            5 => Self::PoweredOn,
            _ => Self::Unknown,
        }
    }
}

/// `CBManagerAuthorization` — the process-wide Bluetooth TCC verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorization {
    NotDetermined = 0,
    Restricted = 1,
    Denied = 2,
    Allowed = 3,
}

impl From<isize> for Authorization {
    fn from(v: isize) -> Self {
        match v {
            1 => Self::Restricted,
            2 => Self::Denied,
            3 => Self::Allowed,
            _ => Self::NotDetermined,
        }
    }
}

/// `CBPeripheralState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeripheralState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Disconnecting = 3,
}

impl From<isize> for PeripheralState {
    fn from(v: isize) -> Self {
        match v {
            1 => Self::Connecting,
            2 => Self::Connected,
            3 => Self::Disconnecting,
            _ => Self::Disconnected,
        }
    }
}

/// `CBCharacteristicProperties`, a bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Properties(pub u32);

impl Properties {
    pub const BROADCAST: u32 = 0x01;
    pub const READ: u32 = 0x02;
    pub const WRITE_WITHOUT_RESPONSE: u32 = 0x04;
    pub const WRITE: u32 = 0x08;
    pub const NOTIFY: u32 = 0x10;
    pub const INDICATE: u32 = 0x20;
    pub const AUTHENTICATED_SIGNED_WRITES: u32 = 0x40;
    pub const EXTENDED_PROPERTIES: u32 = 0x80;
    pub const NOTIFY_ENCRYPTION_REQUIRED: u32 = 0x100;
    pub const INDICATE_ENCRYPTION_REQUIRED: u32 = 0x200;

    #[inline]
    pub fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
    #[inline]
    pub fn broadcast(self) -> bool {
        self.has(Self::BROADCAST)
    }
    #[inline]
    pub fn read(self) -> bool {
        self.has(Self::READ)
    }
    #[inline]
    pub fn write_without_response(self) -> bool {
        self.has(Self::WRITE_WITHOUT_RESPONSE)
    }
    #[inline]
    pub fn write(self) -> bool {
        self.has(Self::WRITE)
    }
    #[inline]
    pub fn notify(self) -> bool {
        self.has(Self::NOTIFY)
    }
    #[inline]
    pub fn indicate(self) -> bool {
        self.has(Self::INDICATE)
    }
    #[inline]
    pub fn authenticated_signed_writes(self) -> bool {
        self.has(Self::AUTHENTICATED_SIGNED_WRITES)
    }
    #[inline]
    pub fn reliable_write(self) -> bool {
        self.has(Self::EXTENDED_PROPERTIES)
    }
}

/// `CBCharacteristicWriteType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteType {
    WithResponse = 0,
    WithoutResponse = 1,
}

// ── CBManager ───────────────────────────────────────────────────────────────

/// `[CBManager authorization]` — the process-wide Bluetooth TCC verdict.
pub fn authorization() -> Authorization {
    unsafe { Authorization::from(msg_send_t![isize; require_class(c"CBManager"), authorization]) }
}

// ── CBUUID ──────────────────────────────────────────────────────────────────

/// `[CBUUID UUIDWithString:]`.
///
/// Accepts the 16-bit (`"180F"`), 32-bit, and 128-bit dashed forms.
///
/// # Safety
/// **`UUIDWithString:` raises an Objective-C exception on a malformed string**,
/// and an Objective-C exception unwinding through a Rust frame aborts the
/// process. The caller must have validated the string first — which is what
/// `webbluetooth::BluetoothUuid` exists to do.
pub unsafe fn uuid_from_string(s: &str) -> Option<Retained> {
    unsafe {
        let ns = Retained::adopt(nsstring(s))?;
        let u: Id = msg_send![require_class(c"CBUUID"), UUIDWithString: ns.as_ptr()];
        Retained::retain(u)
    }
}

/// `[cbuuid UUIDString]` — short form for 16/32-bit UUIDs, dashed for 128-bit.
///
/// # Safety
/// `uuid` must be a `CBUUID`.
pub unsafe fn uuid_string(uuid: Id) -> Option<String> {
    unsafe { to_string(msg_send![uuid, UUIDString]) }
}

// ── CBCentralManager ────────────────────────────────────────────────────────

/// `[[CBCentralManager alloc] initWithDelegate:queue:options:]`.
///
/// `options` may be nil. Returned at `+1`.
///
/// # Safety
/// `delegate` and `queue` must outlive the manager.
pub unsafe fn central_new(delegate: Id, queue: Id, options: Id) -> Option<Retained> {
    unsafe {
        let m: Id = msg_send![require_class(c"CBCentralManager"), alloc];
        let m: Id = msg_send![m, initWithDelegate: delegate, queue: queue, options: options];
        Retained::adopt(m)
    }
}

/// `CBCentralManagerOptionShowPowerAlertKey: @(show)` as an `NSDictionary`, `+1`.
///
/// # Safety
/// Caller owns the result.
pub unsafe fn central_options(
    show_power_alert: bool,
    restore_identifier: Option<&str>,
) -> Option<Retained> {
    unsafe {
        let mut keys: Vec<Id> = Vec::new();
        let mut values: Vec<Id> = Vec::new();
        let mut owned: Vec<Retained> = Vec::new();

        if show_power_alert {
            let key = global_nsstring(c"CBCentralManagerOptionShowPowerAlertKey");
            if !key.is_null() {
                keys.push(key);
                values.push(msg_send![require_class(c"NSNumber"), numberWithBool: true]);
            }
        }
        if let Some(id) = restore_identifier {
            let key = global_nsstring(c"CBCentralManagerOptionRestoreIdentifierKey");
            if let (false, Some(value)) = (key.is_null(), Retained::adopt(nsstring(id))) {
                keys.push(key);
                values.push(value.as_ptr());
                owned.push(value);
            }
        }
        if keys.is_empty() {
            return None;
        }
        objc::nsdictionary(&keys, &values)
    }
}

/// `openL2CAPChannel:` — ask the peer to open an L2CAP channel on `psm`.
///
/// The answer arrives as `peripheral:didOpenL2CAPChannel:error:`.
///
/// # Safety
/// `peripheral` must be a connected `CBPeripheral`.
pub unsafe fn peripheral_open_l2cap(peripheral: Id, psm: u16) {
    unsafe { msg_send_void![peripheral, openL2CAPChannel: psm] }
}

/// What the system preserved for a `CBCentralManager` across a relaunch.
#[derive(Debug, Default)]
pub struct RestoredCentralState {
    /// Peripherals that were connected or connecting.
    pub peripherals: Vec<Retained>,
    /// The services the interrupted scan was filtering on, as CBUUID strings.
    pub scan_services: Vec<String>,
    /// Whether that scan had `CBCentralManagerScanOptionAllowDuplicatesKey`.
    pub scan_allow_duplicates: bool,
}

/// Parse the dictionary handed to `centralManager:willRestoreState:`.
///
/// # Safety
/// `dict` must be null or the `NSDictionary` CoreBluetooth supplied.
pub unsafe fn parse_restored_central_state(dict: Id) -> RestoredCentralState {
    unsafe {
        let peripherals = {
            let arr = dict_get(
                dict,
                global_nsstring(c"CBCentralManagerRestoredStatePeripheralsKey"),
            );
            array_map(arr, |p| Retained::retain(p))
                .into_iter()
                .flatten()
                .collect()
        };
        let scan_services = {
            let arr = dict_get(
                dict,
                global_nsstring(c"CBCentralManagerRestoredStateScanServicesKey"),
            );
            array_map(arr, |u| uuid_string(u))
                .into_iter()
                .flatten()
                .collect()
        };
        let scan_allow_duplicates = {
            let opts = dict_get(
                dict,
                global_nsstring(c"CBCentralManagerRestoredStateScanOptionsKey"),
            );
            let key = global_nsstring(c"CBCentralManagerScanOptionAllowDuplicatesKey");
            number_bool(dict_get(opts, key)).unwrap_or(false)
        };
        RestoredCentralState {
            peripherals,
            scan_services,
            scan_allow_duplicates,
        }
    }
}

/// `[central state]`.
///
/// # Safety
/// `central` must be a `CBCentralManager`.
pub unsafe fn central_state(central: Id) -> ManagerState {
    unsafe { ManagerState::from(msg_send_t![isize; central, state]) }
}

/// `[central isScanning]`.
///
/// # Safety
/// `central` must be a `CBCentralManager`.
pub unsafe fn central_is_scanning(central: Id) -> bool {
    unsafe { msg_send_t![bool; central, isScanning] }
}

/// `scanForPeripheralsWithServices:options:`. An empty slice scans for all.
///
/// `allow_duplicates` maps to `CBCentralManagerScanOptionAllowDuplicatesKey`,
/// which the Web Bluetooth scanning path wants so that RSSI keeps updating
/// while a chooser is deciding.
///
/// # Safety
/// `central` must be a `CBCentralManager` and every element of `services` a
/// `CBUUID`.
pub unsafe fn central_scan(central: Id, services: &[Id], allow_duplicates: bool) {
    unsafe {
        let arr = if services.is_empty() {
            None
        } else {
            Retained::adopt(nsarray(services))
        };
        let opts = if allow_duplicates {
            let key = global_nsstring(c"CBCentralManagerScanOptionAllowDuplicatesKey");
            if key.is_null() {
                None
            } else {
                let val: Id = msg_send![require_class(c"NSNumber"), numberWithBool: true];
                let keys = Retained::adopt(nsarray(&[key]));
                let vals = Retained::adopt(nsarray(&[val]));
                match (keys, vals) {
                    (Some(k), Some(v)) => {
                        let d: Id = msg_send![
                            require_class(c"NSDictionary"),
                            dictionaryWithObjects: v.as_ptr(),
                            forKeys: k.as_ptr()
                        ];
                        Retained::retain(d)
                    }
                    _ => None,
                }
            }
        } else {
            None
        };
        msg_send_void![
            central,
            scanForPeripheralsWithServices: arr.as_ref().map_or(NIL, |a| a.as_ptr()),
            options: opts.as_ref().map_or(NIL, |o| o.as_ptr())
        ];
    }
}

/// `[central stopScan]`.
///
/// # Safety
/// `central` must be a `CBCentralManager`.
pub unsafe fn central_stop_scan(central: Id) {
    unsafe { msg_send_void![central, stopScan] }
}

/// `connectPeripheral:options:`.
///
/// # Safety
/// `central` must be a `CBCentralManager` and `peripheral` a `CBPeripheral`.
pub unsafe fn central_connect(central: Id, peripheral: Id) {
    unsafe { msg_send_void![central, connectPeripheral: peripheral, options: NIL] }
}

/// `cancelPeripheralConnection:`.
///
/// # Safety
/// `central` must be a `CBCentralManager` and `peripheral` a `CBPeripheral`.
pub unsafe fn central_disconnect(central: Id, peripheral: Id) {
    unsafe { msg_send_void![central, cancelPeripheralConnection: peripheral] }
}

/// `retrieveConnectedPeripheralsWithServices:` — peripherals *another* process
/// on this machine already has connected.
///
/// # Safety
/// `central` must be a `CBCentralManager`, `services` all `CBUUID`.
pub unsafe fn central_retrieve_connected(central: Id, services: &[Id]) -> Vec<Retained> {
    unsafe {
        let Some(arr) = Retained::adopt(nsarray(services)) else {
            return Vec::new();
        };
        let res: Id = msg_send![central, retrieveConnectedPeripheralsWithServices: arr.as_ptr()];
        array_map(res, |p| Retained::retain(p))
            .into_iter()
            .flatten()
            .collect()
    }
}

/// `retrievePeripheralsWithIdentifiers:` — re-find a peripheral across process
/// restarts from its `NSUUID` identifier string.
///
/// # Safety
/// `central` must be a `CBCentralManager`.
pub unsafe fn central_retrieve_by_identifier(central: Id, identifiers: &[&str]) -> Vec<Retained> {
    unsafe {
        let mut nsuuids = Vec::with_capacity(identifiers.len());
        for id in identifiers {
            let Some(s) = Retained::adopt(nsstring(id)) else {
                continue;
            };
            let u: Id = msg_send![require_class(c"NSUUID"), alloc];
            let u: Id = msg_send![u, initWithUUIDString: s.as_ptr()];
            if let Some(r) = Retained::adopt(u) {
                nsuuids.push(r);
            }
        }
        let ptrs: Vec<Id> = nsuuids.iter().map(|u| u.as_ptr()).collect();
        let Some(arr) = Retained::adopt(nsarray(&ptrs)) else {
            return Vec::new();
        };
        let res: Id = msg_send![central, retrievePeripheralsWithIdentifiers: arr.as_ptr()];
        array_map(res, |p| Retained::retain(p))
            .into_iter()
            .flatten()
            .collect()
    }
}

// ── CBPeripheral ────────────────────────────────────────────────────────────

/// `[peripheral name]`.
///
/// # Safety
/// `peripheral` must be a `CBPeripheral`.
pub unsafe fn peripheral_name(peripheral: Id) -> Option<String> {
    unsafe { to_string(msg_send![peripheral, name]) }
}

/// `[[peripheral identifier] UUIDString]` — stable per machine, per peripheral.
///
/// This is what Web Bluetooth's `BluetoothDevice.id` maps to. Note that Apple
/// rotates it per-host, so it is not the BD_ADDR and is not comparable across
/// machines.
///
/// # Safety
/// `peripheral` must be a `CBPeripheral`.
pub unsafe fn peripheral_identifier(peripheral: Id) -> Option<String> {
    unsafe { to_string(msg_send![msg_send![peripheral, identifier], UUIDString]) }
}

/// `[peripheral state]`.
///
/// # Safety
/// `peripheral` must be a `CBPeripheral`.
pub unsafe fn peripheral_state(peripheral: Id) -> PeripheralState {
    unsafe { PeripheralState::from(msg_send_t![isize; peripheral, state]) }
}

/// `[peripheral setDelegate:]`.
///
/// # Safety
/// `delegate` must outlive the peripheral's use.
pub unsafe fn peripheral_set_delegate(peripheral: Id, delegate: Id) {
    unsafe { msg_send_void![peripheral, setDelegate: delegate] }
}

/// `discoverServices:`. An empty slice discovers everything.
///
/// # Safety
/// `peripheral` must be a connected `CBPeripheral`.
pub unsafe fn peripheral_discover_services(peripheral: Id, services: &[Id]) {
    unsafe {
        let arr = if services.is_empty() {
            None
        } else {
            Retained::adopt(nsarray(services))
        };
        msg_send_void![peripheral, discoverServices: arr.as_ref().map_or(NIL, |a| a.as_ptr())];
    }
}

/// `[peripheral services]`, retained.
///
/// # Safety
/// `peripheral` must be a `CBPeripheral`.
pub unsafe fn peripheral_services(peripheral: Id) -> Vec<Retained> {
    unsafe {
        let arr: Id = msg_send![peripheral, services];
        array_map(arr, |s| Retained::retain(s))
            .into_iter()
            .flatten()
            .collect()
    }
}

/// `discoverCharacteristics:forService:`.
///
/// # Safety
/// `peripheral` must own `service`.
pub unsafe fn peripheral_discover_characteristics(peripheral: Id, uuids: &[Id], service: Id) {
    unsafe {
        let arr = if uuids.is_empty() {
            None
        } else {
            Retained::adopt(nsarray(uuids))
        };
        msg_send_void![
            peripheral,
            discoverCharacteristics: arr.as_ref().map_or(NIL, |a| a.as_ptr()),
            forService: service
        ];
    }
}

/// `discoverDescriptorsForCharacteristic:`.
///
/// # Safety
/// `peripheral` must own `characteristic`.
pub unsafe fn peripheral_discover_descriptors(peripheral: Id, characteristic: Id) {
    unsafe { msg_send_void![peripheral, discoverDescriptorsForCharacteristic: characteristic] }
}

/// `readValueForCharacteristic:`.
///
/// # Safety
/// `peripheral` must own `characteristic`.
pub unsafe fn peripheral_read_characteristic(peripheral: Id, characteristic: Id) {
    unsafe { msg_send_void![peripheral, readValueForCharacteristic: characteristic] }
}

/// `writeValue:forCharacteristic:type:`.
///
/// # Safety
/// `peripheral` must own `characteristic`.
pub unsafe fn peripheral_write_characteristic(
    peripheral: Id,
    characteristic: Id,
    value: &[u8],
    write_type: WriteType,
) {
    unsafe {
        let Some(data) = Retained::adopt(nsdata(value)) else {
            return;
        };
        msg_send_void![
            peripheral,
            writeValue: data.as_ptr(),
            forCharacteristic: characteristic,
            type: write_type as isize
        ];
    }
}

/// `setNotifyValue:forCharacteristic:`.
///
/// # Safety
/// `peripheral` must own `characteristic`.
pub unsafe fn peripheral_set_notify(peripheral: Id, characteristic: Id, enabled: bool) {
    unsafe {
        msg_send_void![peripheral, setNotifyValue: enabled, forCharacteristic: characteristic]
    }
}

/// `readValueForDescriptor:`.
///
/// # Safety
/// `peripheral` must own `descriptor`.
pub unsafe fn peripheral_read_descriptor(peripheral: Id, descriptor: Id) {
    unsafe { msg_send_void![peripheral, readValueForDescriptor: descriptor] }
}

/// `writeValue:forDescriptor:`.
///
/// # Safety
/// `peripheral` must own `descriptor`.
pub unsafe fn peripheral_write_descriptor(peripheral: Id, descriptor: Id, value: &[u8]) {
    unsafe {
        let Some(data) = Retained::adopt(nsdata(value)) else {
            return;
        };
        msg_send_void![peripheral, writeValue: data.as_ptr(), forDescriptor: descriptor];
    }
}

/// `readRSSI`.
///
/// # Safety
/// `peripheral` must be a connected `CBPeripheral`.
pub unsafe fn peripheral_read_rssi(peripheral: Id) {
    unsafe { msg_send_void![peripheral, readRSSI] }
}

/// `maximumWriteValueLengthForType:` — the ATT MTU payload budget.
///
/// # Safety
/// `peripheral` must be a connected `CBPeripheral`.
pub unsafe fn peripheral_max_write_len(peripheral: Id, write_type: WriteType) -> usize {
    unsafe { msg_send_t![usize; peripheral, maximumWriteValueLengthForType: write_type as isize] }
}

/// `canSendWriteWithoutResponse`.
///
/// # Safety
/// `peripheral` must be a `CBPeripheral`.
pub unsafe fn peripheral_can_write_without_response(peripheral: Id) -> bool {
    unsafe { msg_send_t![bool; peripheral, canSendWriteWithoutResponse] }
}

// ── CBService / CBCharacteristic / CBDescriptor ─────────────────────────────

/// `[attribute UUID]`, borrowed.
///
/// # Safety
/// `attribute` must be a `CBAttribute` subclass.
pub unsafe fn attribute_uuid(attribute: Id) -> Id {
    unsafe { msg_send![attribute, UUID] }
}

/// `[service isPrimary]`.
///
/// # Safety
/// `service` must be a `CBService`.
pub unsafe fn service_is_primary(service: Id) -> bool {
    unsafe { msg_send_t![bool; service, isPrimary] }
}

/// `[service characteristics]`, retained.
///
/// # Safety
/// `service` must be a `CBService`.
pub unsafe fn service_characteristics(service: Id) -> Vec<Retained> {
    unsafe {
        let arr: Id = msg_send![service, characteristics];
        array_map(arr, |c| Retained::retain(c))
            .into_iter()
            .flatten()
            .collect()
    }
}

/// `[peripheral discoverIncludedServices:forService:]`.
///
/// Answered by `peripheral:didDiscoverIncludedServicesForService:error:`, after
/// which `service_included` returns the results. An empty `services` filter
/// means "every included service".
///
/// # Safety
/// `peripheral` must be a `CBPeripheral`, `service` one of its `CBService`s,
/// and every entry of `services` a `CBUUID`.
pub unsafe fn peripheral_discover_included_services(peripheral: Id, services: &[Id], service: Id) {
    unsafe {
        let filter = if services.is_empty() {
            NIL
        } else {
            nsarray(services)
        };
        msg_send_void![
            peripheral,
            discoverIncludedServices: filter,
            forService: service
        ];
    }
}

/// `[service includedServices]`, retained.
///
/// # Safety
/// `service` must be a `CBService`.
pub unsafe fn service_included(service: Id) -> Vec<Retained> {
    unsafe {
        let arr: Id = msg_send![service, includedServices];
        array_map(arr, |s| Retained::retain(s))
            .into_iter()
            .flatten()
            .collect()
    }
}

/// `[characteristic properties]`.
///
/// # Safety
/// `characteristic` must be a `CBCharacteristic`.
pub unsafe fn characteristic_properties(characteristic: Id) -> Properties {
    unsafe { Properties(msg_send_t![usize; characteristic, properties] as u32) }
}

/// `[characteristic value]` — the last value read or notified.
///
/// # Safety
/// `characteristic` must be a `CBCharacteristic`.
pub unsafe fn characteristic_value(characteristic: Id) -> Option<Vec<u8>> {
    unsafe { data_bytes(msg_send![characteristic, value]) }
}

/// `[characteristic isNotifying]`.
///
/// # Safety
/// `characteristic` must be a `CBCharacteristic`.
pub unsafe fn characteristic_is_notifying(characteristic: Id) -> bool {
    unsafe { msg_send_t![bool; characteristic, isNotifying] }
}

/// `[characteristic service]`, borrowed. Nil once the service is released.
///
/// # Safety
/// `characteristic` must be a `CBCharacteristic`.
pub unsafe fn characteristic_service(characteristic: Id) -> Id {
    unsafe { msg_send![characteristic, service] }
}

/// `[characteristic descriptors]`, retained.
///
/// # Safety
/// `characteristic` must be a `CBCharacteristic`.
pub unsafe fn characteristic_descriptors(characteristic: Id) -> Vec<Retained> {
    unsafe {
        let arr: Id = msg_send![characteristic, descriptors];
        array_map(arr, |d| Retained::retain(d))
            .into_iter()
            .flatten()
            .collect()
    }
}

/// `[descriptor value]`.
///
/// CoreBluetooth types a descriptor's value per descriptor UUID — `NSNumber`
/// for the client configuration bits, `NSString` for a user description — so
/// this normalises to bytes the way the ATT layer would have carried them.
///
/// # Safety
/// `descriptor` must be a `CBDescriptor`.
pub unsafe fn descriptor_value(descriptor: Id) -> Option<Vec<u8>> {
    unsafe {
        let v: Id = msg_send![descriptor, value];
        if v.is_null() {
            return None;
        }
        if msg_send_t![bool; v, isKindOfClass: require_class(c"NSData")] {
            return data_bytes(v);
        }
        if msg_send_t![bool; v, isKindOfClass: require_class(c"NSNumber")] {
            let n = number_i64(v).unwrap_or(0);
            return Some((n as u16).to_le_bytes().to_vec());
        }
        if msg_send_t![bool; v, isKindOfClass: require_class(c"NSString")] {
            return to_string(v).map(String::into_bytes);
        }
        None
    }
}

/// `[descriptor characteristic]`, borrowed.
///
/// # Safety
/// `descriptor` must be a `CBDescriptor`.
pub unsafe fn descriptor_characteristic(descriptor: Id) -> Id {
    unsafe { msg_send![descriptor, characteristic] }
}

// ── Advertisement data ──────────────────────────────────────────────────────

/// One advertising packet, as CoreBluetooth surfaces it.
///
/// CoreBluetooth does not expose raw AD structures — it hands over a parsed
/// `NSDictionary` — so this is what a scan can honestly report. Notably absent
/// versus the Web Bluetooth spec: there is no way to see the peripheral's
/// Bluetooth address, and `appearance` is not advertised through this API.
#[derive(Debug, Clone, Default)]
pub struct Advertisement {
    /// `CBAdvertisementDataLocalNameKey` — the advertised name, which may differ
    /// from `CBPeripheral.name` (that one is the cached GAP name).
    pub local_name: Option<String>,
    /// `CBAdvertisementDataTxPowerLevelKey`, in dBm.
    pub tx_power: Option<i16>,
    /// `CBAdvertisementDataIsConnectable`.
    pub is_connectable: Option<bool>,
    /// `CBAdvertisementDataServiceUUIDsKey`, as CBUUID strings.
    pub service_uuids: Vec<String>,
    /// `CBAdvertisementDataOverflowServiceUUIDsKey` — services that did not fit
    /// in the primary advertisement, visible only to an iOS/macOS scanner.
    pub overflow_service_uuids: Vec<String>,
    /// `CBAdvertisementDataSolicitedServiceUUIDsKey`.
    pub solicited_service_uuids: Vec<String>,
    /// `CBAdvertisementDataManufacturerDataKey`, including the leading
    /// little-endian company identifier.
    pub manufacturer_data: Option<Vec<u8>>,
    /// `CBAdvertisementDataServiceDataKey`, keyed by CBUUID string.
    pub service_data: Vec<(String, Vec<u8>)>,
}

/// Parse the `advertisementData` dictionary from `didDiscoverPeripheral`.
///
/// # Safety
/// `dict` must be null or the `NSDictionary` CoreBluetooth supplied.
pub unsafe fn parse_advertisement(dict: Id) -> Advertisement {
    unsafe {
        let uuids_at = |sym: &core::ffi::CStr| -> Vec<String> {
            let arr = dict_get(dict, global_nsstring(sym));
            array_map(arr, |u| uuid_string(u))
                .into_iter()
                .flatten()
                .collect()
        };

        let service_data = {
            let sd = dict_get(dict, global_nsstring(c"CBAdvertisementDataServiceDataKey"));
            let keys = objc::dict_keys(sd);
            array_map(keys, |k| {
                let uuid = uuid_string(k)?;
                let bytes = data_bytes(dict_get(sd, k))?;
                Some((uuid, bytes))
            })
            .into_iter()
            .flatten()
            .collect()
        };

        Advertisement {
            local_name: to_string(dict_get(
                dict,
                global_nsstring(c"CBAdvertisementDataLocalNameKey"),
            )),
            tx_power: number_i64(dict_get(
                dict,
                global_nsstring(c"CBAdvertisementDataTxPowerLevelKey"),
            ))
            .map(|v| v as i16),
            is_connectable: number_bool(dict_get(
                dict,
                global_nsstring(c"CBAdvertisementDataIsConnectable"),
            )),
            service_uuids: uuids_at(c"CBAdvertisementDataServiceUUIDsKey"),
            overflow_service_uuids: uuids_at(c"CBAdvertisementDataOverflowServiceUUIDsKey"),
            solicited_service_uuids: uuids_at(c"CBAdvertisementDataSolicitedServiceUUIDsKey"),
            manufacturer_data: data_bytes(dict_get(
                dict,
                global_nsstring(c"CBAdvertisementDataManufacturerDataKey"),
            )),
            service_data,
        }
    }
}
