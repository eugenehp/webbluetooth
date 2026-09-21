//! The Windows peripheral engine: `GattServiceProvider`.
//!
//! Closer to CoreBluetooth than to Android: Windows manages the Client
//! Characteristic Configuration descriptor itself and reports subscriptions
//! directly, so none of the bookkeeping the Android engine needs applies here.
//!
//! Three things are particular to Windows:
//!
//! * **Advertising belongs to the service.** There is no separate publisher for
//!   a GATT server — `GattServiceProvider.StartAdvertising` advertises the
//!   service it was created for. Publishing two services means two providers.
//! * **A request must be held open with a deferral.** The event handler returns
//!   long before a caller has decided what to answer, and without a deferral
//!   the window closes and the central sees a timeout.
//! * **The local name is the system's.** Windows advertises the machine's
//!   Bluetooth name; nothing in the API sets a per-advertisement one.

use crate::ble;
use crate::com::{succeeded, ComPtr, Delegate};
use crate::guid::Signature;
use crate::iids::{classes, generics, interfaces, slots};
use crate::winrt;
use futures_channel::oneshot;
use futures_core::Stream;
use std::collections::HashMap;
use std::ffi::c_void;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use webbluetooth_core::error::{Availability, Error, Result};
use webbluetooth_core::peripheral::{Advertising, AttError, RestoredPeripheral, Service, Write};
use webbluetooth_core::uuid::{BluetoothUuid, IntoUuid};

/// `GattServiceProviderAdvertisementStatus.Started`.
const ADVERTISEMENT_STARTED: i32 = 2;

fn class_signature(class: classes::Class) -> Signature {
    Signature::Class {
        name: class.0,
        default_interface: class.1,
    }
}

/// Await a WinRT operation.
async fn await_operation(operation: ComPtr, result: Signature) -> Result<ComPtr> {
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
    let held = operation;
    let value = rx
        .await
        .map_err(|_| Error::Aborted("the operation was cancelled".into()))?;
    drop(held);
    value.ok_or_else(|| Error::Network("the operation reported no result".into()))
}

/// Call a no-argument method returning an `IAsyncOperation`.
fn invoke_async(object: &ComPtr, slot: usize) -> Option<ComPtr> {
    let mut operation: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
            object.method(slot);
        f(object.as_raw(), &mut operation)
    };
    succeeded(hr)
        .then(|| unsafe { ComPtr::adopt(operation) })
        .flatten()
}

// ── Inbound requests ────────────────────────────────────────────────────────

/// A central connected to this peripheral.
#[derive(Debug, Clone)]
pub struct RemoteCentral {
    client: Option<ComPtr>,
    id: String,
}

impl RemoteCentral {
    /// The central's session id.
    ///
    /// Windows identifies a subscriber by session rather than by address — it
    /// does not hand out the peer's address on this side at all.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The most bytes one notification to this central can carry.
    pub fn max_notification_length(&self) -> usize {
        self.client
            .as_ref()
            .and_then(|c| ble::get_i32(c, slots::igatt_subscribed_client::MAX_NOTIFICATION_SIZE))
            .filter(|n| *n > 0)
            .unwrap_or(20) as usize
    }
}

/// An inbound ATT read.
///
/// Held open by a deferral until it is answered; dropping it answers
/// [`webbluetooth_core::peripheral::AttError::RequestNotSupported`] rather than leaving the central waiting.
pub struct ReadRequest {
    request: ComPtr,
    deferral: Option<ComPtr>,
    central: RemoteCentral,
    characteristic: BluetoothUuid,
    offset: usize,
    answered: bool,
}

impl ReadRequest {
    pub fn characteristic(&self) -> &BluetoothUuid {
        &self.characteristic
    }
    pub fn central(&self) -> &RemoteCentral {
        &self.central
    }
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Answer with a value. The offset is applied for you.
    pub fn respond(mut self, value: &[u8]) -> Result<()> {
        if self.offset > value.len() {
            self.fail(AttError::InvalidOffset);
            return Err(Error::InvalidModification(format!(
                "read offset {} is past the end of a {}-byte value",
                self.offset,
                value.len()
            )));
        }
        let buffer = ble::bytes_to_buffer(&value[self.offset..])
            .ok_or_else(|| Error::Network("could not build a response buffer".into()))?;
        let hr = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 = self
                .request
                .method(slots::igatt_read_request::RESPOND_WITH_VALUE);
            f(self.request.as_raw(), buffer.as_raw())
        };
        self.answered = true;
        self.complete();
        succeeded(hr)
            .then_some(())
            .ok_or_else(|| Error::Network("RespondWithValue was refused".into()))
    }

    /// Refuse, with a reason the central sees as an ATT error.
    pub fn reject(mut self, error: AttError) {
        self.fail(error);
    }

    fn fail(&mut self, error: AttError) {
        if self.answered {
            return;
        }
        self.answered = true;
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, u8) -> i32 = self
                .request
                .method(slots::igatt_read_request::RESPOND_WITH_PROTOCOL_ERROR);
            f(self.request.as_raw(), error as u8);
        }
        self.complete();
    }

    /// Release the deferral, which is what tells Windows the request is done.
    fn complete(&mut self) {
        if let Some(deferral) = self.deferral.take() {
            ble::call_void(&deferral, slots::ideferral::COMPLETE);
        }
    }
}

impl Drop for ReadRequest {
    fn drop(&mut self) {
        self.fail(AttError::RequestNotSupported);
    }
}

impl std::fmt::Debug for ReadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadRequest")
            .field("characteristic", &self.characteristic.as_str())
            .field("offset", &self.offset)
            .finish()
    }
}

/// One or more inbound ATT writes.
///
/// Windows delivers writes one at a time, so this carries exactly one — the
/// shape matches CoreBluetooth, which batches.
pub struct WriteRequest {
    request: ComPtr,
    deferral: Option<ComPtr>,
    central: RemoteCentral,
    writes: Vec<Write>,
    /// A write-without-response asks for no reply, and answering one is an
    /// error rather than a courtesy.
    response_needed: bool,
    answered: bool,
}

impl WriteRequest {
    pub fn writes(&self) -> &[Write] {
        &self.writes
    }
    pub fn central(&self) -> &RemoteCentral {
        &self.central
    }

    pub fn accept(mut self) {
        self.answer(None);
    }

    pub fn reject_all(mut self, error: AttError) {
        self.answer(Some(error));
    }

    fn answer(&mut self, error: Option<AttError>) {
        if self.answered {
            return;
        }
        self.answered = true;
        if self.response_needed {
            unsafe {
                match error {
                    None => {
                        let f: unsafe extern "system" fn(*mut c_void) -> i32 =
                            self.request.method(slots::igatt_write_request::RESPOND);
                        f(self.request.as_raw());
                    }
                    Some(error) => {
                        let f: unsafe extern "system" fn(*mut c_void, u8) -> i32 = self
                            .request
                            .method(slots::igatt_write_request::RESPOND_WITH_PROTOCOL_ERROR);
                        f(self.request.as_raw(), error as u8);
                    }
                }
            }
        }
        if let Some(deferral) = self.deferral.take() {
            ble::call_void(&deferral, slots::ideferral::COMPLETE);
        }
    }
}

impl Drop for WriteRequest {
    fn drop(&mut self) {
        self.answer(Some(AttError::RequestNotSupported));
    }
}

impl std::fmt::Debug for WriteRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteRequest")
            .field("writes", &self.writes.len())
            .finish()
    }
}

/// Something a connected central did.
#[derive(Debug)]
pub enum Request {
    Read(ReadRequest),
    Write(WriteRequest),
    Subscribed {
        central: RemoteCentral,
        characteristic: BluetoothUuid,
    },
    Unsubscribed {
        central: RemoteCentral,
        characteristic: BluetoothUuid,
    },
    /// The previous notification was delivered and another may be sent.
    ReadyToNotify,
}

/// The stream of [`Request`]s, returned once by [`Peripheral::new`].
/// How many requests from a central queue before one is dropped.
const REQUEST_BACKLOG: usize = 1024;

pub struct Requests {
    rx: webbluetooth_core::backlog::Receiver<Request>,
}

impl Requests {
    /// How many requests were dropped because this stream was not being read
    /// fast enough.
    ///
    /// A central can write as fast as the link allows and the callback
    /// delivering those writes cannot be made to wait, so a handler that falls
    /// behind loses the newest — the ones already queued are answered first.
    /// Anything treating these as an ordered command stream should check this
    /// rather than assume it saw them all.
    pub fn lost(&self) -> u64 {
        self.rx.lost()
    }
}

impl Stream for Requests {
    type Item = Request;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Request>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

// ── Engine ──────────────────────────────────────────────────────────────────

struct Inner {
    available: bool,
    /// One `GattServiceProvider` per published service — advertising belongs to
    /// the provider, so they cannot be shared.
    providers: Mutex<Vec<ComPtr>>,
    /// Handlers, held so Windows keeps calling them.
    handlers: Mutex<Vec<ComPtr>>,
    /// Who is subscribed to what, as Windows last reported it.
    subscribers: Mutex<HashMap<BluetoothUuid, Vec<RemoteCentral>>>,
    requests: Mutex<Option<webbluetooth_core::backlog::Sender<Request>>>,
    advertising: AtomicBool,
}

impl Inner {
    fn send(&self, request: Request) {
        let tx = self.requests.lock().unwrap().clone();
        if let Some(tx) = tx {
            if tx.send(request).is_err() {
                *self.requests.lock().unwrap() = None;
            }
        }
    }

    async fn require_powered_on(&self) -> Result<()> {
        if !self.available {
            return Err(Error::NotAvailable(Availability::Unsupported));
        }
        match ble::statics(
            classes::GATT_SERVICE_PROVIDER,
            interfaces::I_GATT_SERVICE_PROVIDER_STATICS,
        ) {
            Some(_) => Ok(()),
            None => Err(Error::NotAvailable(Availability::Unsupported)),
        }
    }
}

/// A local GATT server.
#[derive(Clone)]
pub struct Peripheral {
    inner: Arc<Inner>,
}

impl Peripheral {
    /// Open the peripheral role, returning the handle and its request stream.
    pub fn new() -> (Self, Requests) {
        // Requests from a connected central: reads and writes against the
        // GATT server this process publishes. Bounded, keeping the *oldest*,
        // because these are commands in order — a control point told to do
        // three things wants the first three, not the last three, and a
        // contiguous prefix with a known stopping point is something a
        // handler can reason about. `Requests::lost` reports the rest.
        let (tx, rx) = webbluetooth_core::backlog::bounded(
            REQUEST_BACKLOG,
            webbluetooth_core::backlog::Overflow::KeepOldest,
        );
        let inner = Arc::new(Inner {
            available: winrt::initialize(),
            providers: Mutex::new(Vec::new()),
            handlers: Mutex::new(Vec::new()),
            subscribers: Mutex::new(HashMap::new()),
            requests: Mutex::new(Some(tx)),
            advertising: AtomicBool::new(false),
        });
        (Self { inner }, Requests { rx })
    }

    /// Windows preserves nothing across a restart; the identifier is ignored.
    pub fn with_restoration(_restoration: webbluetooth_core::Restoration) -> (Self, Requests) {
        Self::new()
    }

    pub async fn availability(&self) -> std::result::Result<(), Availability> {
        match self.inner.require_powered_on().await {
            Ok(()) => Ok(()),
            Err(Error::NotAvailable(a)) => Err(a),
            Err(_) => Err(Availability::Unknown),
        }
    }

    /// Publish a service.
    ///
    /// Each service gets its own `GattServiceProvider`, because on Windows the
    /// provider is also what advertises it.
    pub async fn publish(&self, service: Service) -> Result<PublishedService> {
        service.validate()?;
        self.inner.require_powered_on().await?;

        let statics = ble::statics(
            classes::GATT_SERVICE_PROVIDER,
            interfaces::I_GATT_SERVICE_PROVIDER_STATICS,
        )
        .ok_or(Error::NotAvailable(Availability::Unsupported))?;

        // GattServiceProvider.CreateAsync(uuid)
        let uuid = crate::guid::Guid::parse(service.uuid.as_str());
        let mut operation: *mut c_void = std::ptr::null_mut();
        let hr = unsafe {
            let f: unsafe extern "system" fn(
                *mut c_void,
                crate::guid::Guid,
                *mut *mut c_void,
            ) -> i32 = statics.method(slots::igatt_service_provider_statics::CREATE_ASYNC);
            f(statics.as_raw(), uuid, &mut operation)
        };
        let operation = succeeded(hr)
            .then(|| unsafe { ComPtr::adopt(operation) })
            .flatten()
            .ok_or_else(|| Error::Network("GattServiceProvider.CreateAsync was refused".into()))?;

        let result = await_operation(
            operation,
            class_signature(classes::GATT_SERVICE_PROVIDER_RESULT),
        )
        .await?;
        let provider = ble::get_object(
            &result,
            slots::igatt_service_provider_result::SERVICE_PROVIDER,
        )
        .ok_or_else(|| Error::Network("no service provider was created".into()))?;
        let local_service = ble::get_object(&provider, slots::igatt_service_provider::SERVICE)
            .ok_or_else(|| Error::Network("the provider published no service".into()))?;

        let mut characteristics = Vec::new();
        for definition in &service.characteristics {
            let handle = self
                .create_characteristic(&local_service, definition)
                .await?;
            characteristics.push(handle);
        }

        self.inner.providers.lock().unwrap().push(provider.clone());
        Ok(PublishedService {
            inner: self.inner.clone(),
            uuid: service.uuid,
            provider,
            characteristics,
        })
    }

    /// Build one characteristic and wire its request handlers.
    async fn create_characteristic(
        &self,
        local_service: &ComPtr,
        definition: &webbluetooth_core::peripheral::Characteristic,
    ) -> Result<PublishedCharacteristic> {
        let parameters = ble::activate(classes::GATT_LOCAL_CHARACTERISTIC_PARAMETERS)
            .ok_or_else(|| Error::Network("could not build characteristic parameters".into()))?;

        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, i32) -> i32 = parameters.method(
                slots::igatt_local_characteristic_parameters::SET_CHARACTERISTIC_PROPERTIES,
            );
            f(parameters.as_raw(), definition.properties as i32);
        }
        if let Some(value) = &definition.value {
            if let Some(buffer) = ble::bytes_to_buffer(value) {
                unsafe {
                    let f: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 = parameters
                        .method(slots::igatt_local_characteristic_parameters::SET_STATIC_VALUE);
                    f(parameters.as_raw(), buffer.as_raw());
                }
            }
        }

        let uuid = crate::guid::Guid::parse(definition.uuid.as_str());
        let mut operation: *mut c_void = std::ptr::null_mut();
        let hr = unsafe {
            let f: unsafe extern "system" fn(
                *mut c_void,
                crate::guid::Guid,
                *mut c_void,
                *mut *mut c_void,
            ) -> i32 =
                local_service.method(slots::igatt_local_service::CREATE_CHARACTERISTIC_ASYNC);
            f(
                local_service.as_raw(),
                uuid,
                parameters.as_raw(),
                &mut operation,
            )
        };
        let operation = succeeded(hr)
            .then(|| unsafe { ComPtr::adopt(operation) })
            .flatten()
            .ok_or_else(|| Error::Network("CreateCharacteristicAsync was refused".into()))?;

        let result = await_operation(
            operation,
            class_signature(classes::GATT_LOCAL_CHARACTERISTIC_RESULT),
        )
        .await?;
        let characteristic = ble::get_object(
            &result,
            slots::igatt_local_characteristic_result::CHARACTERISTIC,
        )
        .ok_or_else(|| Error::Network("no characteristic was created".into()))?;

        self.attach_handlers(&characteristic, definition.uuid);
        Ok(PublishedCharacteristic {
            inner: self.inner.clone(),
            uuid: definition.uuid,
            handle: characteristic,
        })
    }

    /// Route `ReadRequested`, `WriteRequested` and `SubscribedClientsChanged`.
    fn attach_handlers(&self, characteristic: &ComPtr, uuid: BluetoothUuid) {
        use slots::igatt_local_characteristic as c;

        let read_iid = typed_handler_iid(classes::GATT_READ_REQUESTED_EVENT_ARGS);
        let write_iid = typed_handler_iid(classes::GATT_WRITE_REQUESTED_EVENT_ARGS);
        let subscribed_iid = Signature::Parameterized {
            generic: generics::TYPED_EVENT_HANDLER,
            arguments: vec![
                class_signature(classes::GATT_LOCAL_CHARACTERISTIC),
                Signature::OBJECT,
            ],
        }
        .iid()
        .expect("a parameterised signature always has an IID");

        let engine = Arc::downgrade(&self.inner);
        let for_read = uuid;
        let read = Delegate::new(read_iid, move |_sender, args| {
            let Some(engine) = engine.upgrade() else {
                return;
            };
            on_read_requested(&engine, args, for_read);
        });

        let engine = Arc::downgrade(&self.inner);
        let for_write = uuid;
        let write = Delegate::new(write_iid, move |_sender, args| {
            let Some(engine) = engine.upgrade() else {
                return;
            };
            on_write_requested(&engine, args, for_write);
        });

        let engine = Arc::downgrade(&self.inner);
        let for_subscribe = uuid;
        let subscribed = Delegate::new(subscribed_iid, move |sender, _args| {
            let Some(engine) = engine.upgrade() else {
                return;
            };
            on_subscribers_changed(&engine, sender, for_subscribe);
        });

        for (slot, handler) in [
            (c::READ_REQUESTED, &read),
            (c::WRITE_REQUESTED, &write),
            (c::SUBSCRIBED_CLIENTS_CHANGED, &subscribed),
        ] {
            let mut token: i64 = 0;
            unsafe {
                let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i64) -> i32 =
                    characteristic.method(slot);
                f(characteristic.as_raw(), handler.as_raw(), &mut token);
            }
        }
        let mut held = self.inner.handlers.lock().unwrap();
        held.push(read);
        held.push(write);
        held.push(subscribed);
    }

    /// Remove every published service.
    pub fn unpublish_all(&self) {
        for provider in self.inner.providers.lock().unwrap().drain(..) {
            ble::call_void(&provider, slots::igatt_service_provider::STOP_ADVERTISING);
        }
        self.inner.handlers.lock().unwrap().clear();
        self.inner.subscribers.lock().unwrap().clear();
        self.inner.advertising.store(false, Ordering::Release);
    }

    /// Start advertising every published service.
    ///
    /// Windows advertises the machine's Bluetooth name, so
    /// [`Advertising::local_name`](webbluetooth_core::peripheral::Advertising::local_name) has no
    /// effect here; the service UUIDs come from what was published.
    pub async fn start_advertising(&self, _advertising: Advertising) -> Result<()> {
        self.inner.require_powered_on().await?;
        let providers = self.inner.providers.lock().unwrap().clone();
        if providers.is_empty() {
            return Err(Error::InvalidState(
                "publish a service before advertising — Windows advertises services, not names"
                    .into(),
            ));
        }

        let parameters = ble::activate(classes::GATT_SERVICE_PROVIDER_ADVERTISING_PARAMETERS)
            .ok_or_else(|| Error::Network("could not build advertising parameters".into()))?;
        use slots::igatt_service_provider_advertising_parameters as ap;
        for (slot, value) in [
            (ap::SET_IS_CONNECTABLE, true),
            (ap::SET_IS_DISCOVERABLE, true),
        ] {
            unsafe {
                let f: unsafe extern "system" fn(*mut c_void, u8) -> i32 = parameters.method(slot);
                f(parameters.as_raw(), u8::from(value));
            }
        }

        for provider in &providers {
            let hr = unsafe {
                let f: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 = provider
                    .method(slots::igatt_service_provider::START_ADVERTISING_WITH_PARAMETERS);
                f(provider.as_raw(), parameters.as_raw())
            };
            if !succeeded(hr) {
                return Err(Error::Network("StartAdvertising was refused".into()));
            }
        }
        self.inner.advertising.store(true, Ordering::Release);
        Ok(())
    }

    pub fn stop_advertising(&self) {
        for provider in self.inner.providers.lock().unwrap().iter() {
            ble::call_void(
                &provider.clone(),
                slots::igatt_service_provider::STOP_ADVERTISING,
            );
        }
        self.inner.advertising.store(false, Ordering::Release);
    }

    /// Whether the radio is currently advertising.
    pub fn is_advertising(&self) -> bool {
        // Ask the provider rather than trusting the flag: Windows stops
        // advertising on its own when the radio is turned off.
        let providers = self.inner.providers.lock().unwrap();
        let live = providers.iter().any(|p| {
            ble::get_i32(p, slots::igatt_service_provider::ADVERTISEMENT_STATUS)
                == Some(ADVERTISEMENT_STARTED)
        });
        live || (providers.is_empty() && self.inner.advertising.load(Ordering::Acquire))
    }

    /// Not available: Windows exposes no L2CAP channel API at all.
    pub async fn publish_l2cap_channel(
        &self,
        _encryption_required: bool,
    ) -> Result<webbluetooth_core::Psm> {
        Err(Error::NotSupported(
            "Windows exposes no L2CAP connection-oriented channel API".into(),
        ))
    }

    pub async fn unpublish_l2cap_channel(&self, _psm: webbluetooth_core::Psm) -> Result<()> {
        Ok(())
    }

    /// Always `None`: Windows preserves nothing across a restart.
    pub async fn restored_state(&self) -> Option<RestoredPeripheral> {
        None
    }
}

impl std::fmt::Debug for Peripheral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peripheral")
            .field("advertising", &self.is_advertising())
            .finish()
    }
}

/// `TypedEventHandler<GattLocalCharacteristic, TArgs>`.
fn typed_handler_iid(args: classes::Class) -> crate::guid::Guid {
    Signature::Parameterized {
        generic: generics::TYPED_EVENT_HANDLER,
        arguments: vec![
            class_signature(classes::GATT_LOCAL_CHARACTERISTIC),
            class_signature(args),
        ],
    }
    .iid()
    .expect("a parameterised signature always has an IID")
}

/// The central behind a request, identified by its session.
fn central_from_session(session: Option<ComPtr>) -> RemoteCentral {
    let id = session
        .as_ref()
        .and_then(|s| ble::get_object(s, slots::igatt_session::DEVICE_ID))
        .and_then(|d| ble::get_string(&d, slots::igatt_session::DEVICE_ID))
        .unwrap_or_default();
    RemoteCentral { client: None, id }
}

fn on_read_requested(engine: &Arc<Inner>, args: *mut c_void, characteristic: BluetoothUuid) {
    if args.is_null() {
        return;
    }
    let args = std::mem::ManuallyDrop::new(unsafe { ComPtr::adopt(args) });
    let Some(args) = args.as_ref() else { return };

    // Take the deferral first: without it the request window closes as soon as
    // this handler returns, long before a caller has answered.
    let deferral = invoke_deferral(args, slots::igatt_read_requested_event_args::GET_DEFERRAL);
    let central = central_from_session(ble::get_object(
        args,
        slots::igatt_read_requested_event_args::SESSION,
    ));

    let Some(operation) = invoke_async(
        args,
        slots::igatt_read_requested_event_args::GET_REQUEST_ASYNC,
    ) else {
        return;
    };
    let engine = engine.clone();
    let result_signature = class_signature(classes::GATT_READ_REQUEST);
    let once = Mutex::new(Some((deferral, central, characteristic)));

    ble::on_completed(&operation, result_signature, move |request| {
        let Some(request) = request else { return };
        let Ok(mut guard) = once.lock() else { return };
        let Some((deferral, central, characteristic)) = guard.take() else {
            return;
        };
        let offset = ble::get_i32(&request, slots::igatt_read_request::OFFSET)
            .unwrap_or(0)
            .max(0) as usize;
        engine.send(Request::Read(ReadRequest {
            request,
            deferral,
            central,
            characteristic,
            offset,
            answered: false,
        }));
    });
    // The operation must outlive its handler; Windows holds a reference.
    std::mem::forget(operation);
}

fn on_write_requested(engine: &Arc<Inner>, args: *mut c_void, characteristic: BluetoothUuid) {
    if args.is_null() {
        return;
    }
    let args = std::mem::ManuallyDrop::new(unsafe { ComPtr::adopt(args) });
    let Some(args) = args.as_ref() else { return };

    let deferral = invoke_deferral(args, slots::igatt_write_requested_event_args::GET_DEFERRAL);
    let central = central_from_session(ble::get_object(
        args,
        slots::igatt_write_requested_event_args::SESSION,
    ));

    let Some(operation) = invoke_async(
        args,
        slots::igatt_write_requested_event_args::GET_REQUEST_ASYNC,
    ) else {
        return;
    };
    let engine = engine.clone();
    let result_signature = class_signature(classes::GATT_WRITE_REQUEST);
    let once = Mutex::new(Some((deferral, central, characteristic)));

    ble::on_completed(&operation, result_signature, move |request| {
        let Some(request) = request else { return };
        let Ok(mut guard) = once.lock() else { return };
        let Some((deferral, central, characteristic)) = guard.take() else {
            return;
        };

        let offset = ble::get_i32(&request, slots::igatt_write_request::OFFSET)
            .unwrap_or(0)
            .max(0) as usize;
        let value = ble::get_object(&request, slots::igatt_write_request::VALUE)
            .and_then(|b| ble::buffer_to_bytes(&b))
            .unwrap_or_default();
        // GattWriteOption::WriteWithResponse == 0.
        let response_needed =
            ble::get_i32(&request, slots::igatt_write_request::OPTION).unwrap_or(0) == 0;

        engine.send(Request::Write(WriteRequest {
            request,
            deferral,
            central,
            writes: vec![Write {
                characteristic,
                offset,
                value,
            }],
            response_needed,
            answered: false,
        }));
    });
    std::mem::forget(operation);
}

fn on_subscribers_changed(engine: &Arc<Inner>, sender: *mut c_void, characteristic: BluetoothUuid) {
    if sender.is_null() {
        return;
    }
    let sender = std::mem::ManuallyDrop::new(unsafe { ComPtr::adopt(sender) });
    let Some(sender) = sender.as_ref() else {
        return;
    };

    let mut current = Vec::new();
    if let Some(vector) = ble::get_object(
        sender,
        slots::igatt_local_characteristic::SUBSCRIBED_CLIENTS,
    ) {
        ble::for_each(&vector, |client| {
            let id = ble::get_object(&client, slots::igatt_subscribed_client::SESSION)
                .and_then(|s| ble::get_string(&s, slots::igatt_session::DEVICE_ID))
                .unwrap_or_default();
            current.push(RemoteCentral {
                client: Some(client),
                id,
            });
        });
    }

    // Windows reports the whole list, so what changed is the difference.
    let mut subscribers = engine.subscribers.lock().unwrap();
    let previous = subscribers.remove(&characteristic).unwrap_or_default();
    subscribers.insert(characteristic, current.clone());
    drop(subscribers);

    for central in &current {
        if !previous.iter().any(|p| p.id == central.id) {
            engine.send(Request::Subscribed {
                central: central.clone(),
                characteristic,
            });
        }
    }
    for central in &previous {
        if !current.iter().any(|c| c.id == central.id) {
            engine.send(Request::Unsubscribed {
                central: central.clone(),
                characteristic,
            });
        }
    }
}

/// `GetDeferral()`, which holds a request open past the handler.
fn invoke_deferral(args: &ComPtr, slot: usize) -> Option<ComPtr> {
    let mut deferral: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 = args.method(slot);
        f(args.as_raw(), &mut deferral)
    };
    succeeded(hr)
        .then(|| unsafe { ComPtr::adopt(deferral) })
        .flatten()
}

/// A service that is live in the local GATT database.
pub struct PublishedService {
    inner: Arc<Inner>,
    uuid: BluetoothUuid,
    provider: ComPtr,
    characteristics: Vec<PublishedCharacteristic>,
}

impl PublishedService {
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.uuid
    }

    pub fn characteristics(&self) -> Vec<PublishedCharacteristic> {
        self.characteristics.to_vec()
    }

    pub fn characteristic(&self, uuid: impl IntoUuid) -> Option<PublishedCharacteristic> {
        let uuid = uuid.into_uuid().ok()?;
        self.characteristics
            .iter()
            .find(|c| c.uuid == uuid)
            .cloned()
    }

    /// Remove just this service.
    pub fn unpublish(self) {
        ble::call_void(
            &self.provider,
            slots::igatt_service_provider::STOP_ADVERTISING,
        );
        let mut providers = self.inner.providers.lock().unwrap();
        providers.retain(|p| p.key() != self.provider.key());
    }
}

impl std::fmt::Debug for PublishedService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishedService")
            .field("uuid", &self.uuid.as_str())
            .field("characteristics", &self.characteristics.len())
            .finish()
    }
}

/// A characteristic that is live in the local GATT database.
#[derive(Clone)]
pub struct PublishedCharacteristic {
    inner: Arc<Inner>,
    uuid: BluetoothUuid,
    handle: ComPtr,
}

impl PublishedCharacteristic {
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.uuid
    }

    /// The centrals currently subscribed, as Windows last reported them.
    pub fn subscribers(&self) -> Vec<RemoteCentral> {
        self.inner
            .subscribers
            .lock()
            .unwrap()
            .get(&self.uuid)
            .cloned()
            .unwrap_or_default()
    }

    /// Send a value to every subscriber without waiting.
    pub fn try_notify(&self, value: &[u8]) -> bool {
        let Some(buffer) = ble::bytes_to_buffer(value) else {
            return false;
        };
        let mut operation: *mut c_void = std::ptr::null_mut();
        let hr = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> i32 =
                self.handle
                    .method(slots::igatt_local_characteristic::NOTIFY_VALUE_ASYNC);
            f(self.handle.as_raw(), buffer.as_raw(), &mut operation)
        };
        // Fire and forget: the per-client results are not awaited here.
        if succeeded(hr) {
            if let Some(operation) = unsafe { ComPtr::adopt(operation) } {
                std::mem::forget(operation);
            }
            true
        } else {
            false
        }
    }

    /// Send a value to specific subscribers.
    ///
    /// Windows notifies every subscriber or none — there is no per-client
    /// variant — so this notifies all of them when `centrals` is non-empty.
    pub fn try_notify_centrals(&self, value: &[u8], centrals: &[RemoteCentral]) -> bool {
        !centrals.is_empty() && self.try_notify(value)
    }

    /// Send a value to every subscriber.
    pub async fn notify(&self, value: &[u8]) -> Result<()> {
        if self.subscribers().is_empty() {
            return Ok(());
        }
        if self.try_notify(value) {
            Ok(())
        } else {
            Err(Error::Network("NotifyValueAsync was refused".into()))
        }
    }
}

impl std::fmt::Debug for PublishedCharacteristic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishedCharacteristic")
            .field("uuid", &self.uuid.as_str())
            .field("subscribers", &self.subscribers().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webbluetooth_core::peripheral::{cccd_uuid, Characteristic, Descriptor};
    use webbluetooth_core::uuid::characteristics;

    #[test]
    fn a_static_value_may_also_be_writable() {
        // CoreBluetooth forbids this and applies that rule where it publishes;
        // the portable checks do not, and Windows adds none of its own.
        assert!(Characteristic::new(characteristics::BATTERY_LEVEL)
            .unwrap()
            .read()
            .write()
            .value([50])
            .validate()
            .is_ok());
    }

    #[test]
    fn a_characteristic_still_needs_a_property() {
        assert!(Characteristic::new(characteristics::BATTERY_LEVEL)
            .unwrap()
            .validate()
            .is_err());
    }

    #[test]
    fn windows_manages_the_cccd_itself() {
        let err = Descriptor {
            uuid: cccd_uuid(),
            value: vec![0, 0],
            is_string: false,
        }
        .validate()
        .unwrap_err();
        assert!(err.to_string().contains("manages it"), "got {err}");
        assert!(Descriptor::user_description("x").validate().is_ok());
        assert!(Descriptor {
            uuid: BluetoothUuid::from_u16(0x2901),
            value: Vec::new(),
            is_string: true,
        }
        .validate()
        .is_err());
    }
}
