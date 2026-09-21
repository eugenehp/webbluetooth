//! The peripheral role: `CBPeripheralManager` and its runtime-built delegate.
//!
//! The mirror image of [`crate::delegate`]. Where a central discovers and reads
//! someone else's attribute table, a peripheral *publishes* one and answers
//! requests against it — so the delegate here carries inbound ATT reads and
//! writes rather than replies to outbound ones.
//!
//! The delegate class is synthesised the same way: `objc_allocateClassPair`
//! under `NSObject`, Rust `extern "C"` functions as method implementations,
//! `class_addProtocol` for `CBPeripheralManagerDelegate`, register. No
//! Objective-C or Swift source.
//!
//! # Rules CoreBluetooth enforces with exceptions
//!
//! Several misuses of `CBMutableCharacteristic` and `CBMutableDescriptor` raise
//! `NSInternalInconsistencyException`, and an Objective-C exception unwinding
//! through a Rust frame aborts the process. The offenders are checked in Rust
//! before the message is sent — see `validate_characteristic` and
//! `validate_descriptor`.

use crate::cb::{ManagerState, Properties};
use crate::objc::{
    array_map, data_bytes, error_message, global_nsstring, nsarray, nsdata, nsdictionary, nsstring,
    require_class, to_string, ClassBuilder, Retained,
};
use crate::objc::{Id, NIL};
use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

// ── Value types ─────────────────────────────────────────────────────────────

/// `CBAttributePermissions`, a bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions(pub u32);

impl Permissions {
    pub const READABLE: u32 = 0x01;
    pub const WRITEABLE: u32 = 0x02;
    pub const READ_ENCRYPTION_REQUIRED: u32 = 0x04;
    pub const WRITE_ENCRYPTION_REQUIRED: u32 = 0x08;

    #[inline]
    pub fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
    #[inline]
    pub fn readable(self) -> bool {
        self.has(Self::READABLE)
    }
    #[inline]
    pub fn writeable(self) -> bool {
        self.has(Self::WRITEABLE)
    }
    /// Any permission that would let a central write.
    #[inline]
    pub fn any_write(self) -> bool {
        self.has(Self::WRITEABLE) || self.has(Self::WRITE_ENCRYPTION_REQUIRED)
    }
}

/// `CBATTError` — the status a request is answered with.
///
/// These are the Bluetooth ATT error codes, so a central sees exactly the
/// failure the protocol defines rather than a generic one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(isize)]
pub enum AttError {
    Success = 0x00,
    InvalidHandle = 0x01,
    ReadNotPermitted = 0x02,
    WriteNotPermitted = 0x03,
    InvalidPdu = 0x04,
    InsufficientAuthentication = 0x05,
    RequestNotSupported = 0x06,
    InvalidOffset = 0x07,
    InsufficientAuthorization = 0x08,
    PrepareQueueFull = 0x09,
    AttributeNotFound = 0x0A,
    AttributeNotLong = 0x0B,
    InsufficientEncryptionKeySize = 0x0C,
    InvalidAttributeValueLength = 0x0D,
    UnlikelyError = 0x0E,
    InsufficientEncryption = 0x0F,
    UnsupportedGroupType = 0x10,
    InsufficientResources = 0x11,
}

// ── Events ──────────────────────────────────────────────────────────────────

/// What a published peripheral hears.
#[derive(Debug)]
pub enum PeripheralEvent {
    /// Always delivered once shortly after the manager is created. Nothing may
    /// be published before it.
    StateChanged(ManagerState),
    /// `startAdvertising:` finished — successfully if `error` is `None`.
    AdvertisingStarted { error: Option<(i64, String)> },
    /// A service was added to the local database.
    ServiceAdded {
        service: Retained,
        error: Option<(i64, String)>,
    },
    /// A central enabled notifications on one of our characteristics.
    Subscribed {
        central: Retained,
        characteristic: Retained,
    },
    Unsubscribed {
        central: Retained,
        characteristic: Retained,
    },
    /// An inbound ATT read. **Must** be answered with
    /// [`respond`] or the central's request times out.
    ReadRequest { request: Retained },
    /// One or more inbound ATT writes, to be applied atomically. Answer only
    /// the *first* request; that response covers the whole batch.
    WriteRequests { requests: Vec<Retained> },
    /// The notification queue drained after [`update_value`] returned `false`.
    ReadyToUpdateSubscribers,
    /// `publish_l2cap_channel` finished; `psm` is the assigned PSM.
    L2capPublished {
        psm: u16,
        error: Option<(i64, String)>,
    },
    /// `unpublish_l2cap_channel` finished.
    L2capUnpublished {
        psm: u16,
        error: Option<(i64, String)>,
    },
    /// A central opened an L2CAP channel to a PSM we published.
    L2capChannelOpened {
        channel: Option<Retained>,
        error: Option<(i64, String)>,
    },
    /// The system relaunched this process and is handing back what it preserved.
    ///
    /// Delivered **before** the first `StateChanged`, and only when the manager
    /// was created with a restore identifier.
    WillRestoreState(Box<RestoredPeripheralState>),
}

/// What the system preserved for a `CBPeripheralManager` across a relaunch.
#[derive(Debug, Default)]
pub struct RestoredPeripheralState {
    /// Services that were published, as live `CBMutableService` objects.
    pub services: Vec<Retained>,
    /// The local name that was being advertised.
    pub advertised_local_name: Option<String>,
    /// The service UUIDs that were being advertised, as CBUUID strings.
    pub advertised_services: Vec<String>,
}

/// Parse the dictionary handed to `peripheralManager:willRestoreState:`.
///
/// # Safety
/// `dict` must be null or the `NSDictionary` CoreBluetooth supplied.
pub unsafe fn parse_restored_peripheral_state(dict: Id) -> RestoredPeripheralState {
    unsafe {
        let services = {
            let arr = crate::objc::dict_get(
                dict,
                global_nsstring(c"CBPeripheralManagerRestoredStateServicesKey"),
            );
            array_map(arr, |s| Retained::retain(s))
                .into_iter()
                .flatten()
                .collect()
        };
        let advertisement = crate::objc::dict_get(
            dict,
            global_nsstring(c"CBPeripheralManagerRestoredStateAdvertisementDataKey"),
        );
        let advertised_local_name = to_string(crate::objc::dict_get(
            advertisement,
            global_nsstring(c"CBAdvertisementDataLocalNameKey"),
        ));
        let advertised_services = {
            let arr = crate::objc::dict_get(
                advertisement,
                global_nsstring(c"CBAdvertisementDataServiceUUIDsKey"),
            );
            array_map(arr, |u| crate::cb::uuid_string(u))
                .into_iter()
                .flatten()
                .collect()
        };
        RestoredPeripheralState {
            services,
            advertised_local_name,
            advertised_services,
        }
    }
}

/// Where a peripheral delegate sends what it hears.
///
/// Called on the manager's dispatch queue, one event at a time.
pub trait PeripheralEventSink: Send + Sync {
    fn emit(&self, event: PeripheralEvent);
}

static SINKS: OnceLock<Mutex<HashMap<usize, Arc<dyn PeripheralEventSink>>>> = OnceLock::new();

fn sinks() -> &'static Mutex<HashMap<usize, Arc<dyn PeripheralEventSink>>> {
    SINKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn emit(delegate: Id, event: PeripheralEvent) {
    let sink = sinks()
        .lock()
        .ok()
        .and_then(|m| m.get(&(delegate as usize)).cloned());
    if let Some(sink) = sink {
        sink.emit(event);
    }
}

/// A live peripheral-manager delegate object.
#[derive(Debug)]
pub struct PeripheralDelegate {
    object: Retained,
}

impl PeripheralDelegate {
    pub fn new(sink: Arc<dyn PeripheralEventSink>) -> Self {
        let class = delegate_class();
        // SAFETY: `class` is registered and inherits NSObject's `init`.
        let object = unsafe {
            let obj: Id = crate::msg_send![class, alloc];
            let obj: Id = crate::msg_send![obj, init];
            Retained::adopt(obj).expect("peripheral delegate allocation failed")
        };
        sinks()
            .lock()
            .expect("sink registry poisoned")
            .insert(object.key(), sink);
        Self { object }
    }

    #[inline]
    pub fn as_ptr(&self) -> Id {
        self.object.as_ptr()
    }
}

impl Drop for PeripheralDelegate {
    fn drop(&mut self) {
        if let Ok(mut m) = sinks().lock() {
            m.remove(&self.object.key());
        }
    }
}

static CLASS: OnceLock<usize> = OnceLock::new();

fn delegate_class() -> Id {
    *CLASS.get_or_init(|| build_class() as usize) as Id
}

fn build_class() -> Id {
    for attempt in 0..64u32 {
        let name = if attempt == 0 {
            c"WBRustCBPeripheralManagerDelegate".to_owned()
        } else {
            std::ffi::CString::new(format!("WBRustCBPeripheralManagerDelegate{attempt}"))
                .expect("class name")
        };
        // SAFETY: every IMP below matches the `v@:` encoding it is installed
        // with; all CBPeripheralManagerDelegate arguments are objects.
        let Some(builder) = (unsafe { ClassBuilder::new(c"NSObject", &name) }) else {
            continue;
        };
        return unsafe {
            builder
                .method(
                    c"peripheralManagerDidUpdateState:",
                    did_update_state as *const c_void,
                    c"v@:@",
                )
                .method(
                    c"peripheralManagerDidStartAdvertising:error:",
                    did_start_advertising as *const c_void,
                    c"v@:@@",
                )
                .method(
                    c"peripheralManager:didAddService:error:",
                    did_add_service as *const c_void,
                    c"v@:@@@",
                )
                .method(
                    c"peripheralManager:central:didSubscribeToCharacteristic:",
                    did_subscribe as *const c_void,
                    c"v@:@@@",
                )
                .method(
                    c"peripheralManager:central:didUnsubscribeFromCharacteristic:",
                    did_unsubscribe as *const c_void,
                    c"v@:@@@",
                )
                .method(
                    c"peripheralManager:didReceiveReadRequest:",
                    did_receive_read as *const c_void,
                    c"v@:@@",
                )
                .method(
                    c"peripheralManager:didReceiveWriteRequests:",
                    did_receive_writes as *const c_void,
                    c"v@:@@",
                )
                .method(
                    c"peripheralManagerIsReadyToUpdateSubscribers:",
                    is_ready_to_update as *const c_void,
                    c"v@:@",
                )
                // CBL2CAPPSM is a uint16_t, encoded "S" — not an object.
                .method(
                    c"peripheralManager:didPublishL2CAPChannel:error:",
                    did_publish_l2cap as *const c_void,
                    c"v@:@S@",
                )
                .method(
                    c"peripheralManager:didUnpublishL2CAPChannel:error:",
                    did_unpublish_l2cap as *const c_void,
                    c"v@:@S@",
                )
                .method(
                    c"peripheralManager:didOpenL2CAPChannel:error:",
                    did_open_l2cap as *const c_void,
                    c"v@:@@@",
                )
                .method(
                    c"peripheralManager:willRestoreState:",
                    will_restore_state as *const c_void,
                    c"v@:@@",
                )
                .conforms(c"CBPeripheralManagerDelegate")
                .register()
        };
    }
    panic!("could not register a peripheral delegate class after 64 attempts");
}

unsafe extern "C" fn did_update_state(this: Id, _cmd: *const c_void, manager: Id) {
    let state = unsafe { manager_state(manager) };
    emit(this, PeripheralEvent::StateChanged(state));
}

unsafe extern "C" fn did_start_advertising(this: Id, _cmd: *const c_void, _mgr: Id, error: Id) {
    emit(
        this,
        PeripheralEvent::AdvertisingStarted {
            error: unsafe { error_message(error) },
        },
    );
}

unsafe extern "C" fn did_add_service(
    this: Id,
    _cmd: *const c_void,
    _mgr: Id,
    service: Id,
    error: Id,
) {
    unsafe {
        let Some(service) = Retained::retain(service) else {
            return;
        };
        emit(
            this,
            PeripheralEvent::ServiceAdded {
                service,
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn did_subscribe(
    this: Id,
    _cmd: *const c_void,
    _mgr: Id,
    central: Id,
    characteristic: Id,
) {
    unsafe {
        let (Some(central), Some(characteristic)) =
            (Retained::retain(central), Retained::retain(characteristic))
        else {
            return;
        };
        emit(
            this,
            PeripheralEvent::Subscribed {
                central,
                characteristic,
            },
        );
    }
}

unsafe extern "C" fn did_unsubscribe(
    this: Id,
    _cmd: *const c_void,
    _mgr: Id,
    central: Id,
    characteristic: Id,
) {
    unsafe {
        let (Some(central), Some(characteristic)) =
            (Retained::retain(central), Retained::retain(characteristic))
        else {
            return;
        };
        emit(
            this,
            PeripheralEvent::Unsubscribed {
                central,
                characteristic,
            },
        );
    }
}

unsafe extern "C" fn did_receive_read(this: Id, _cmd: *const c_void, _mgr: Id, request: Id) {
    let Some(request) = (unsafe { Retained::retain(request) }) else {
        return;
    };
    emit(this, PeripheralEvent::ReadRequest { request });
}

unsafe extern "C" fn did_receive_writes(this: Id, _cmd: *const c_void, _mgr: Id, requests: Id) {
    unsafe {
        let requests: Vec<Retained> = array_map(requests, |r| Retained::retain(r))
            .into_iter()
            .flatten()
            .collect();
        if requests.is_empty() {
            return;
        }
        emit(this, PeripheralEvent::WriteRequests { requests });
    }
}

unsafe extern "C" fn is_ready_to_update(this: Id, _cmd: *const c_void, _mgr: Id) {
    emit(this, PeripheralEvent::ReadyToUpdateSubscribers);
}

unsafe extern "C" fn did_publish_l2cap(
    this: Id,
    _cmd: *const c_void,
    _mgr: Id,
    psm: u16,
    error: Id,
) {
    emit(
        this,
        PeripheralEvent::L2capPublished {
            psm,
            error: unsafe { error_message(error) },
        },
    );
}

unsafe extern "C" fn did_unpublish_l2cap(
    this: Id,
    _cmd: *const c_void,
    _mgr: Id,
    psm: u16,
    error: Id,
) {
    emit(
        this,
        PeripheralEvent::L2capUnpublished {
            psm,
            error: unsafe { error_message(error) },
        },
    );
}

unsafe extern "C" fn did_open_l2cap(
    this: Id,
    _cmd: *const c_void,
    _mgr: Id,
    channel: Id,
    error: Id,
) {
    unsafe {
        emit(
            this,
            PeripheralEvent::L2capChannelOpened {
                channel: Retained::retain(channel),
                error: error_message(error),
            },
        );
    }
}

unsafe extern "C" fn will_restore_state(this: Id, _cmd: *const c_void, _mgr: Id, dict: Id) {
    let restored = unsafe { parse_restored_peripheral_state(dict) };
    emit(this, PeripheralEvent::WillRestoreState(Box::new(restored)));
}

// ── CBPeripheralManager ─────────────────────────────────────────────────────

/// `[manager state]`.
///
/// # Safety
/// `manager` must be a `CBPeripheralManager`.
pub unsafe fn manager_state(manager: Id) -> ManagerState {
    unsafe { ManagerState::from(crate::msg_send_t![isize; manager, state]) }
}

/// `[manager isAdvertising]`.
///
/// # Safety
/// `manager` must be a `CBPeripheralManager`.
pub unsafe fn is_advertising(manager: Id) -> bool {
    unsafe { crate::msg_send_t![bool; manager, isAdvertising] }
}

/// `startAdvertising:` with a local name and/or service UUIDs.
///
/// Those are the only two keys Apple honours in the peripheral role. The
/// advertisement has 28 bytes for everything; service UUIDs that do not fit go
/// to the overflow area, where only an Apple device scanning for them
/// explicitly will see them.
///
/// # Safety
/// `manager` must be a `CBPeripheralManager` and `services` all `CBUUID`.
pub unsafe fn start_advertising(manager: Id, local_name: Option<&str>, services: &[Id]) {
    unsafe {
        let mut keys: Vec<Id> = Vec::new();
        let mut values: Vec<Id> = Vec::new();
        // Kept alive until the dictionary is built.
        let mut owned: Vec<Retained> = Vec::new();

        if let Some(name) = local_name {
            let key = global_nsstring(c"CBAdvertisementDataLocalNameKey");
            if let (false, Some(value)) = (key.is_null(), Retained::adopt(nsstring(name))) {
                keys.push(key);
                values.push(value.as_ptr());
                owned.push(value);
            }
        }
        if !services.is_empty() {
            let key = global_nsstring(c"CBAdvertisementDataServiceUUIDsKey");
            if let (false, Some(value)) = (key.is_null(), Retained::adopt(nsarray(services))) {
                keys.push(key);
                values.push(value.as_ptr());
                owned.push(value);
            }
        }

        let dict = if keys.is_empty() {
            None
        } else {
            nsdictionary(&keys, &values)
        };
        crate::msg_send_void![
            manager,
            startAdvertising: dict.as_ref().map_or(NIL, |d| d.as_ptr())
        ];
    }
}

/// `[manager stopAdvertising]`.
///
/// # Safety
/// `manager` must be a `CBPeripheralManager`.
pub unsafe fn stop_advertising(manager: Id) {
    unsafe { crate::msg_send_void![manager, stopAdvertising] }
}

/// `addService:`.
///
/// # Safety
/// `service` must be a `CBMutableService` that has not been published.
pub unsafe fn add_service(manager: Id, service: Id) {
    unsafe { crate::msg_send_void![manager, addService: service] }
}

/// `removeService:`.
///
/// # Safety
/// `service` must be a `CBMutableService` published on `manager`.
pub unsafe fn remove_service(manager: Id, service: Id) {
    unsafe { crate::msg_send_void![manager, removeService: service] }
}

/// `[manager removeAllServices]`.
///
/// # Safety
/// `manager` must be a `CBPeripheralManager`.
pub unsafe fn remove_all_services(manager: Id) {
    unsafe { crate::msg_send_void![manager, removeAllServices] }
}

/// `respondToRequest:withResult:`.
///
/// # Safety
/// `request` must be a `CBATTRequest` from this manager's delegate that has
/// **not already been answered** — responding twice raises an Objective-C
/// exception, which aborts the process.
pub unsafe fn respond(manager: Id, request: Id, result: AttError) {
    unsafe {
        crate::msg_send_void![manager, respondToRequest: request, withResult: result as isize]
    }
}

/// `updateValue:forCharacteristic:onSubscribedCentrals:`.
///
/// `false` means the transmit queue is full; wait for
/// [`PeripheralEvent::ReadyToUpdateSubscribers`] and send the same value again.
/// An empty `centrals` slice means every subscriber.
///
/// # Safety
/// `characteristic` must be a published `CBMutableCharacteristic` and every
/// element of `centrals` a `CBCentral`.
pub unsafe fn update_value(manager: Id, value: &[u8], characteristic: Id, centrals: &[Id]) -> bool {
    unsafe {
        let Some(data) = Retained::adopt(nsdata(value)) else {
            return false;
        };
        let list = if centrals.is_empty() {
            None
        } else {
            Retained::adopt(nsarray(centrals))
        };
        crate::msg_send_t![
            bool;
            manager,
            updateValue: data.as_ptr(),
            forCharacteristic: characteristic,
            onSubscribedCentrals: list.as_ref().map_or(NIL, |l| l.as_ptr())
        ]
    }
}

/// `publishL2CAPChannelWithEncryption:` — open a listening PSM.
///
/// The assigned PSM arrives as [`PeripheralEvent::L2capPublished`]; a central
/// connecting to it arrives as [`PeripheralEvent::L2capChannelOpened`].
///
/// # Safety
/// `manager` must be a powered-on `CBPeripheralManager`.
pub unsafe fn publish_l2cap_channel(manager: Id, encryption_required: bool) {
    unsafe {
        crate::msg_send_void![manager, publishL2CAPChannelWithEncryption: encryption_required]
    }
}

/// `unpublishL2CAPChannel:`.
///
/// # Safety
/// `manager` must be a `CBPeripheralManager` that published `psm`.
pub unsafe fn unpublish_l2cap_channel(manager: Id, psm: u16) {
    unsafe { crate::msg_send_void![manager, unpublishL2CAPChannel: psm] }
}

// ── Mutable attributes ──────────────────────────────────────────────────────

/// Why a characteristic definition is one CoreBluetooth would reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidAttribute {
    /// A cached value forces read-only. CoreBluetooth documents that a
    /// characteristic created with a value "will be cached and marked
    /// `CBCharacteristicPropertyRead` and `CBAttributePermissionsReadable`";
    /// anything else raises an exception.
    CachedValueMustBeReadOnly,
    /// Only Characteristic User Description (`0x2901`) and Characteristic
    /// Presentation Format (`0x2904`) may be created. The Client Characteristic
    /// Configuration and Extended Properties descriptors are created
    /// automatically from the characteristic's own properties.
    UnsupportedDescriptor,
    /// A descriptor's value is required and cannot be null.
    DescriptorValueRequired,
}

impl std::fmt::Display for InvalidAttribute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CachedValueMustBeReadOnly => f.write_str(
                "a characteristic with a fixed value must be read-only; leave the value unset \
                 for anything writeable or changing, and answer read requests on demand",
            ),
            Self::UnsupportedDescriptor => f.write_str(
                "only the user description (0x2901) and presentation format (0x2904) descriptors \
                 can be created; the client characteristic configuration descriptor is added \
                 automatically when a characteristic can notify",
            ),
            Self::DescriptorValueRequired => f.write_str("a descriptor must have a value"),
        }
    }
}

/// Check a characteristic definition against the rules CoreBluetooth enforces
/// with exceptions.
///
/// Called before constructing the object, because an Objective-C exception
/// unwinding through a Rust frame aborts the process rather than returning an
/// error.
pub fn validate_characteristic(
    properties: Properties,
    permissions: Permissions,
    value: Option<&[u8]>,
) -> Result<(), InvalidAttribute> {
    if value.is_some() {
        let read_only_properties = properties.0 == Properties::READ;
        let read_only_permissions = !permissions.any_write();
        if !read_only_properties || !read_only_permissions {
            return Err(InvalidAttribute::CachedValueMustBeReadOnly);
        }
    }
    Ok(())
}

/// The two descriptor types `CBMutableDescriptor` accepts.
const USER_DESCRIPTION_UUID: u16 = 0x2901;
const PRESENTATION_FORMAT_UUID: u16 = 0x2904;

/// Check a descriptor definition. `uuid16` is the 16-bit assigned number, or
/// `None` for a vendor UUID (which is never supported here).
pub fn validate_descriptor(uuid16: Option<u16>, has_value: bool) -> Result<(), InvalidAttribute> {
    match uuid16 {
        Some(USER_DESCRIPTION_UUID) | Some(PRESENTATION_FORMAT_UUID) => {}
        _ => return Err(InvalidAttribute::UnsupportedDescriptor),
    }
    if !has_value {
        return Err(InvalidAttribute::DescriptorValueRequired);
    }
    Ok(())
}

/// `[[CBMutableService alloc] initWithType:primary:]`, `+1`.
///
/// # Safety
/// `uuid` must be a `CBUUID`.
pub unsafe fn mutable_service(uuid: Id, primary: bool) -> Option<Retained> {
    unsafe {
        let s: Id = crate::msg_send![require_class(c"CBMutableService"), alloc];
        let s: Id = crate::msg_send![s, initWithType: uuid, primary: primary];
        Retained::adopt(s)
    }
}

/// `[service setCharacteristics:]`.
///
/// # Safety
/// `service` must be an unpublished `CBMutableService`.
pub unsafe fn service_set_characteristics(service: Id, characteristics: &[Id]) {
    unsafe {
        let arr = Retained::adopt(nsarray(characteristics));
        crate::msg_send_void![
            service,
            setCharacteristics: arr.as_ref().map_or(NIL, |a| a.as_ptr())
        ];
    }
}

/// `[service setIncludedServices:]`.
///
/// # Safety
/// `service` must be an unpublished `CBMutableService`.
pub unsafe fn service_set_included(service: Id, included: &[Id]) {
    unsafe {
        let arr = Retained::adopt(nsarray(included));
        crate::msg_send_void![
            service,
            setIncludedServices: arr.as_ref().map_or(NIL, |a| a.as_ptr())
        ];
    }
}

/// `[[CBMutableCharacteristic alloc] initWithType:properties:value:permissions:]`, `+1`.
///
/// # Safety
/// `uuid` must be a `CBUUID`, and the combination must have passed
/// `validate_characteristic` — otherwise CoreBluetooth raises.
pub unsafe fn mutable_characteristic(
    uuid: Id,
    properties: Properties,
    value: Option<&[u8]>,
    permissions: Permissions,
) -> Option<Retained> {
    unsafe {
        let data = value.and_then(|v| Retained::adopt(nsdata(v)));
        let c: Id = crate::msg_send![require_class(c"CBMutableCharacteristic"), alloc];
        let c: Id = crate::msg_send![
            c,
            initWithType: uuid,
            properties: properties.0 as usize,
            value: data.as_ref().map_or(NIL, |d| d.as_ptr()),
            permissions: permissions.0 as usize
        ];
        Retained::adopt(c)
    }
}

/// `[characteristic setDescriptors:]`.
///
/// # Safety
/// `characteristic` must be an unpublished `CBMutableCharacteristic`.
pub unsafe fn characteristic_set_descriptors(characteristic: Id, descriptors: &[Id]) {
    unsafe {
        let arr = Retained::adopt(nsarray(descriptors));
        crate::msg_send_void![
            characteristic,
            setDescriptors: arr.as_ref().map_or(NIL, |a| a.as_ptr())
        ];
    }
}

/// `[characteristic subscribedCentrals]`, retained.
///
/// # Safety
/// `characteristic` must be a published `CBMutableCharacteristic`.
pub unsafe fn subscribed_centrals(characteristic: Id) -> Vec<Retained> {
    unsafe {
        let arr: Id = crate::msg_send![characteristic, subscribedCentrals];
        array_map(arr, |c| Retained::retain(c))
            .into_iter()
            .flatten()
            .collect()
    }
}

/// `[[CBMutableDescriptor alloc] initWithType:value:]`, `+1`.
///
/// The user description descriptor takes an `NSString`; the presentation
/// format descriptor takes `NSData`. `is_string` picks which.
///
/// # Safety
/// `uuid` must be a `CBUUID` that passed `validate_descriptor`.
pub unsafe fn mutable_descriptor(uuid: Id, value: &[u8], is_string: bool) -> Option<Retained> {
    unsafe {
        let boxed = if is_string {
            Retained::adopt(nsstring(&String::from_utf8_lossy(value)))
        } else {
            Retained::adopt(nsdata(value))
        }?;
        let d: Id = crate::msg_send![require_class(c"CBMutableDescriptor"), alloc];
        let d: Id = crate::msg_send![d, initWithType: uuid, value: boxed.as_ptr()];
        Retained::adopt(d)
    }
}

// ── CBATTRequest / CBCentral ────────────────────────────────────────────────

/// `[request central]`, borrowed.
///
/// # Safety
/// `request` must be a `CBATTRequest`.
pub unsafe fn request_central(request: Id) -> Id {
    unsafe { crate::msg_send![request, central] }
}

/// `[request characteristic]`, borrowed.
///
/// # Safety
/// `request` must be a `CBATTRequest`.
pub unsafe fn request_characteristic(request: Id) -> Id {
    unsafe { crate::msg_send![request, characteristic] }
}

/// `[request offset]` — where in a long attribute this read or write starts.
///
/// # Safety
/// `request` must be a `CBATTRequest`.
pub unsafe fn request_offset(request: Id) -> usize {
    unsafe { crate::msg_send_t![usize; request, offset] }
}

/// `[request value]` — the bytes a central wrote, or `None` for a read.
///
/// # Safety
/// `request` must be a `CBATTRequest`.
pub unsafe fn request_value(request: Id) -> Option<Vec<u8>> {
    unsafe { data_bytes(crate::msg_send![request, value]) }
}

/// `[request setValue:]` — the bytes to answer a read with.
///
/// # Safety
/// `request` must be a `CBATTRequest` awaiting a response.
pub unsafe fn request_set_value(request: Id, value: &[u8]) {
    unsafe {
        let Some(data) = Retained::adopt(nsdata(value)) else {
            return;
        };
        crate::msg_send_void![request, setValue: data.as_ptr()];
    }
}

/// `[[central identifier] UUIDString]`.
///
/// # Safety
/// `central` must be a `CBCentral`.
pub unsafe fn central_identifier(central: Id) -> Option<String> {
    unsafe {
        to_string(crate::msg_send![
            crate::msg_send![central, identifier],
            UUIDString
        ])
    }
}

/// `[central maximumUpdateValueLength]` — the most one notification can carry.
///
/// # Safety
/// `central` must be a `CBCentral`.
pub unsafe fn central_max_update_length(central: Id) -> usize {
    unsafe { crate::msg_send_t![usize; central, maximumUpdateValueLength] }
}

// ── Host ────────────────────────────────────────────────────────────────────

/// A `CBPeripheralManager` with a runtime-built delegate on a private queue.
#[derive(Debug)]
pub struct PeripheralHost {
    manager: Retained,
    /// `CBPeripheralManager` holds its delegate weakly.
    _delegate: PeripheralDelegate,
    _queue: crate::dispatch::Queue,
}

impl PeripheralHost {
    /// Create a peripheral manager delivering events to `sink`.
    pub fn new(sink: Arc<dyn PeripheralEventSink>, show_power_alert: bool) -> Self {
        Self::with_restore_identifier(sink, show_power_alert, None)
    }

    /// As [`PeripheralHost::new`], opting into state preservation and
    /// restoration. `restore_identifier` must be identical on every launch.
    pub fn with_restore_identifier(
        sink: Arc<dyn PeripheralEventSink>,
        show_power_alert: bool,
        restore_identifier: Option<&str>,
    ) -> Self {
        let queue = crate::dispatch::Queue::serial(c"dev.webbluetooth.peripheral");
        let delegate = PeripheralDelegate::new(sink);
        // SAFETY: the queue and delegate are stored alongside the manager, so
        // both outlive it.
        let manager = unsafe {
            let mut keys: Vec<Id> = Vec::new();
            let mut values: Vec<Id> = Vec::new();
            let mut owned: Vec<Retained> = Vec::new();
            if show_power_alert {
                let key = global_nsstring(c"CBPeripheralManagerOptionShowPowerAlertKey");
                if !key.is_null() {
                    keys.push(key);
                    values.push(crate::msg_send![
                        require_class(c"NSNumber"),
                        numberWithBool: true
                    ]);
                }
            }
            if let Some(id) = restore_identifier {
                let key = global_nsstring(c"CBPeripheralManagerOptionRestoreIdentifierKey");
                if let (false, Some(value)) = (key.is_null(), Retained::adopt(nsstring(id))) {
                    keys.push(key);
                    values.push(value.as_ptr());
                    owned.push(value);
                }
            }
            let options = if keys.is_empty() {
                None
            } else {
                nsdictionary(&keys, &values)
            };
            let m: Id = crate::msg_send![require_class(c"CBPeripheralManager"), alloc];
            let m: Id = crate::msg_send![
                m,
                initWithDelegate: delegate.as_ptr(),
                queue: queue.as_ptr(),
                options: options.as_ref().map_or(NIL, |o| o.as_ptr())
            ];
            Retained::adopt(m).expect("CBPeripheralManager allocation failed")
        };
        Self {
            manager,
            _delegate: delegate,
            _queue: queue,
        }
    }

    #[inline]
    pub fn as_ptr(&self) -> Id {
        self.manager.as_ptr()
    }

    pub fn state(&self) -> ManagerState {
        unsafe { manager_state(self.as_ptr()) }
    }

    pub fn is_advertising(&self) -> bool {
        unsafe { is_advertising(self.as_ptr()) }
    }
}

unsafe impl Send for PeripheralHost {}
unsafe impl Sync for PeripheralHost {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct Collect(mpsc::Sender<ManagerState>);
    impl PeripheralEventSink for Collect {
        fn emit(&self, event: PeripheralEvent) {
            if let PeripheralEvent::StateChanged(s) = event {
                let _ = self.0.send(s);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn peripheral_delegate_class_registers_and_receives_state() {
        let (tx, rx) = mpsc::channel();
        let host = PeripheralHost::new(Arc::new(Collect(tx)), false);
        let state = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("peripheralManagerDidUpdateState: never arrived");
        assert_eq!(state, host.state());
        assert!(!host.is_advertising());
    }

    #[test]
    fn a_fixed_value_forces_read_only() {
        let read = Properties(Properties::READ);
        let readable = Permissions(Permissions::READABLE);
        assert!(validate_characteristic(read, readable, Some(&[1, 2, 3])).is_ok());

        // Writeable with a fixed value would raise inside CoreBluetooth.
        assert_eq!(
            validate_characteristic(
                Properties(Properties::READ | Properties::WRITE),
                readable,
                Some(&[1])
            ),
            Err(InvalidAttribute::CachedValueMustBeReadOnly)
        );
        assert_eq!(
            validate_characteristic(
                read,
                Permissions(Permissions::READABLE | Permissions::WRITEABLE),
                Some(&[1])
            ),
            Err(InvalidAttribute::CachedValueMustBeReadOnly)
        );
        // Notify with a fixed value is the same mistake.
        assert_eq!(
            validate_characteristic(
                Properties(Properties::READ | Properties::NOTIFY),
                readable,
                Some(&[1])
            ),
            Err(InvalidAttribute::CachedValueMustBeReadOnly)
        );
    }

    #[test]
    fn a_dynamic_value_may_be_anything() {
        // No cached value: every combination is legal.
        assert!(validate_characteristic(
            Properties(Properties::READ | Properties::WRITE | Properties::NOTIFY),
            Permissions(Permissions::READABLE | Permissions::WRITEABLE),
            None
        )
        .is_ok());
    }

    #[test]
    fn only_two_descriptor_types_can_be_created() {
        assert!(validate_descriptor(Some(0x2901), true).is_ok());
        assert!(validate_descriptor(Some(0x2904), true).is_ok());
        // The CCCD is created by CoreBluetooth itself.
        assert_eq!(
            validate_descriptor(Some(0x2902), true),
            Err(InvalidAttribute::UnsupportedDescriptor)
        );
        assert_eq!(
            validate_descriptor(None, true),
            Err(InvalidAttribute::UnsupportedDescriptor)
        );
        assert_eq!(
            validate_descriptor(Some(0x2901), false),
            Err(InvalidAttribute::DescriptorValueRequired)
        );
    }
}
