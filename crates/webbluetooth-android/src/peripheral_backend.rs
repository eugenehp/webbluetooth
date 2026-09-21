//! The Android peripheral engine: `BluetoothGattServer` and
//! `BluetoothLeAdvertiser`.
//!
//! Two differences from CoreBluetooth are handled here so that the API behaves
//! the same on both:
//!
//! * **Android does not create the Client Characteristic Configuration
//!   descriptor.** CoreBluetooth adds one to any characteristic that can
//!   notify; Android publishes exactly what you hand it, so a notify
//!   characteristic without a CCCD is one no central can ever subscribe to.
//!   [`Peripheral::publish`] adds it.
//! * **Android does not tell you who is subscribed.** There is no
//!   `subscribedCentrals`; a subscription *is* a write to that descriptor, so
//!   the engine watches for it and keeps the list itself. Those writes surface
//!   as [`Request::Subscribed`] rather than as a raw write, which is what a
//!   caller expects and what the Apple engine reports.
//!
//! Android is also the more permissive of the two about what it will publish:
//! a characteristic may carry a value *and* be writable, and any descriptor
//! UUID is accepted. Only the CCCD is refused, because the engine manages it.

use crate::ble;
use crate::bluetooth::{Event, EventSink, Ref};
use crate::runtime::Runtime;
use futures_channel::oneshot;
use futures_core::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use webbluetooth_core::error::{Availability, Error, Result};
use webbluetooth_core::gatt::CharacteristicProperties as Properties;
use webbluetooth_core::peripheral::{
    cccd_uuid, Advertising, AttError, Permissions, RestoredPeripheral, Service, Write,
};
use webbluetooth_core::uuid::{BluetoothUuid, IntoUuid};

// ── Inbound requests ────────────────────────────────────────────────────────

/// A central connected to this peripheral.
#[derive(Debug, Clone)]
pub struct RemoteCentral {
    device: Ref,
    id: String,
}

impl RemoteCentral {
    fn new(device: Ref) -> Self {
        let id = Runtime::get()
            .and_then(|r| r.env().ok())
            .and_then(|env| ble::device_address(env, device.as_ptr()))
            .unwrap_or_default();
        Self { device, id }
    }

    /// The central's Bluetooth address.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The most bytes one notification can carry.
    ///
    /// Android does not expose the negotiated MTU per central from the server
    /// side, so this is the value every link is guaranteed to accept.
    pub fn max_notification_length(&self) -> usize {
        20
    }
}

/// An inbound ATT read.
///
/// **Answer it.** A request dropped without a response is answered
/// [`webbluetooth_core::peripheral::AttError::RequestNotSupported`] automatically, because silence stalls the
/// central until the ATT timeout.
pub struct ReadRequest {
    inner: Arc<Inner>,
    central: RemoteCentral,
    request_id: i32,
    offset: usize,
    characteristic: BluetoothUuid,
    answered: bool,
}

impl ReadRequest {
    pub fn characteristic(&self) -> &BluetoothUuid {
        &self.characteristic
    }
    pub fn central(&self) -> &RemoteCentral {
        &self.central
    }
    /// Where in a long attribute this read starts.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Answer with a value. The offset is applied for you.
    pub fn respond(mut self, value: &[u8]) -> Result<()> {
        if self.offset > value.len() {
            self.answer(AttError::InvalidOffset, &[]);
            return Err(Error::InvalidModification(format!(
                "read offset {} is past the end of a {}-byte value",
                self.offset,
                value.len()
            )));
        }
        self.answer(AttError::Success, &value[self.offset..]);
        Ok(())
    }

    /// Refuse, with a reason the central sees as an ATT error.
    pub fn reject(mut self, error: AttError) {
        self.answer(error, &[]);
    }

    fn answer(&mut self, result: AttError, value: &[u8]) {
        if self.answered {
            return;
        }
        self.answered = true;
        self.inner.respond(
            &self.central.device,
            self.request_id,
            result,
            self.offset,
            value,
        );
    }
}

impl Drop for ReadRequest {
    fn drop(&mut self) {
        self.answer(AttError::RequestNotSupported, &[]);
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
/// Android delivers writes one at a time rather than in a batch, so this
/// usually carries exactly one — but the shape matches CoreBluetooth, which
/// batches.
pub struct WriteRequest {
    inner: Arc<Inner>,
    central: RemoteCentral,
    request_id: i32,
    writes: Vec<Write>,
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
        self.answer(AttError::Success);
    }

    pub fn reject_all(mut self, error: AttError) {
        self.answer(error);
    }

    fn answer(&mut self, result: AttError) {
        if self.answered {
            return;
        }
        self.answered = true;
        // A write-without-response asks for no reply, and sending one anyway
        // is an error on Android.
        if !self.response_needed {
            return;
        }
        let offset = self.writes.first().map(|w| w.offset).unwrap_or(0);
        self.inner
            .respond(&self.central.device, self.request_id, result, offset, &[]);
    }
}

impl Drop for WriteRequest {
    fn drop(&mut self) {
        self.answer(AttError::RequestNotSupported);
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
    /// A central subscribed — start sending it notifications.
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
    /// A central opened an L2CAP channel to a published PSM.
    ChannelOpened(crate::l2cap::L2capChannel),
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
    server: Mutex<Option<Ref>>,
    /// Held so the callback objects outlive the server using them.
    server_callback: Mutex<Option<ble::Callback>>,
    advertise_callback: Mutex<Option<ble::Callback>>,
    service_waiters: Mutex<Vec<oneshot::Sender<Result<()>>>>,
    advertising_waiters: Mutex<Vec<oneshot::Sender<Result<()>>>>,
    ready_waiters: Mutex<Vec<oneshot::Sender<()>>>,
    /// Who is subscribed to what. Android keeps no such list, so this is it.
    subscribers: Mutex<HashMap<BluetoothUuid, Vec<RemoteCentral>>>,
    /// Published characteristics, so an inbound request can be named.
    published: Mutex<HashMap<usize, BluetoothUuid>>,
    /// Which characteristic each CCCD belongs to.
    cccds: Mutex<HashMap<usize, BluetoothUuid>>,
    requests: Mutex<Option<webbluetooth_core::backlog::Sender<Request>>>,
    advertising: AtomicBool,
    /// Listening L2CAP sockets, by PSM.
    l2cap: Mutex<HashMap<u16, (Ref, Arc<AtomicBool>)>>,
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
    fn handle(self: &Arc<Self>, event: Event) {
        match event {
            Event::ServiceAdded { status } => {
                let result = if status == ble::GATT_SUCCESS {
                    Ok(())
                } else {
                    Err(Error::Network(format!(
                        "could not publish service (status {status})"
                    )))
                };
                for tx in self.service_waiters.lock().unwrap().drain(..) {
                    let _ = tx.send(result.clone());
                }
            }

            Event::AdvertisingStarted { error } => {
                let result = match error {
                    None => {
                        self.advertising.store(true, Ordering::Release);
                        Ok(())
                    }
                    Some(code) => Err(Error::Network(format!(
                        "could not start advertising: {}",
                        describe_advertise_error(code)
                    ))),
                };
                for tx in self.advertising_waiters.lock().unwrap().drain(..) {
                    let _ = tx.send(result.clone());
                }
            }

            Event::ReadRequest {
                device,
                request_id,
                offset,
                characteristic,
            } => {
                let uuid = self.name_of(&characteristic);
                let central = RemoteCentral::new(device);
                self.send(Request::Read(ReadRequest {
                    inner: self.clone(),
                    central,
                    request_id,
                    offset: offset.max(0) as usize,
                    characteristic: uuid,
                    answered: false,
                }));
            }

            Event::WriteRequest {
                device,
                request_id,
                characteristic,
                response_needed,
                offset,
                value,
                ..
            } => {
                let uuid = self.name_of(&characteristic);
                let central = RemoteCentral::new(device);
                self.send(Request::Write(WriteRequest {
                    inner: self.clone(),
                    central,
                    request_id,
                    writes: vec![Write {
                        characteristic: uuid,
                        offset: offset.max(0) as usize,
                        value,
                    }],
                    response_needed,
                    answered: false,
                }));
            }

            Event::DescriptorReadRequest {
                device,
                request_id,
                offset,
                descriptor,
            } => {
                // The only descriptor the engine publishes is the CCCD, and its
                // value is the subscription state — answer it here rather than
                // troubling the caller with it.
                let central = RemoteCentral::new(device);
                let is_subscribed = self
                    .cccds
                    .lock()
                    .unwrap()
                    .get(&descriptor.key())
                    .map(|uuid| self.is_subscribed(uuid, &central))
                    .unwrap_or(false);
                let value: &[u8] = if is_subscribed {
                    &[0x01, 0x00]
                } else {
                    &[0x00, 0x00]
                };
                self.respond(
                    &central.device,
                    request_id,
                    AttError::Success,
                    offset.max(0) as usize,
                    value,
                );
            }

            Event::DescriptorWriteRequest {
                device,
                request_id,
                descriptor,
                response_needed,
                offset,
                value,
            } => {
                let central = RemoteCentral::new(device);
                let owner = self.cccds.lock().unwrap().get(&descriptor.key()).cloned();

                if response_needed {
                    self.respond(
                        &central.device,
                        request_id,
                        AttError::Success,
                        offset.max(0) as usize,
                        &[],
                    );
                }

                // A subscription *is* this write. Anything non-zero enables.
                let Some(characteristic) = owner else { return };
                let enabling = value.first().is_some_and(|b| *b != 0);
                let mut subscribers = self.subscribers.lock().unwrap();
                let list = subscribers.entry(characteristic).or_default();
                if enabling {
                    if !list.iter().any(|c| c.id == central.id) {
                        list.push(central.clone());
                    }
                } else {
                    list.retain(|c| c.id != central.id);
                }
                drop(subscribers);

                self.send(if enabling {
                    Request::Subscribed {
                        central,
                        characteristic,
                    }
                } else {
                    Request::Unsubscribed {
                        central,
                        characteristic,
                    }
                });
            }

            Event::NotificationSent { .. } => {
                for tx in self.ready_waiters.lock().unwrap().drain(..) {
                    let _ = tx.send(());
                }
                self.send(Request::ReadyToNotify);
            }

            Event::ServerConnectionStateChanged { device, connected } => {
                if connected {
                    return;
                }
                // A central that drops the link is unsubscribed from everything.
                let central = RemoteCentral::new(device);
                let mut subscribers = self.subscribers.lock().unwrap();
                let dropped: Vec<BluetoothUuid> = subscribers
                    .iter()
                    .filter(|(_, list)| list.iter().any(|c| c.id == central.id))
                    .map(|(uuid, _)| *uuid)
                    .collect();
                for uuid in &dropped {
                    if let Some(list) = subscribers.get_mut(uuid) {
                        list.retain(|c| c.id != central.id);
                    }
                }
                drop(subscribers);
                for characteristic in dropped {
                    self.send(Request::Unsubscribed {
                        central: central.clone(),
                        characteristic,
                    });
                }
            }

            _ => {}
        }
    }

    fn send(&self, request: Request) {
        let tx = self.requests.lock().unwrap().clone();
        if let Some(tx) = tx {
            if tx.send(request).is_err() {
                *self.requests.lock().unwrap() = None;
            }
        }
    }

    fn name_of(&self, characteristic: &Ref) -> BluetoothUuid {
        if let Some(uuid) = self.published.lock().unwrap().get(&characteristic.key()) {
            return *uuid;
        }
        Runtime::get()
            .and_then(|r| r.env().ok())
            .and_then(|env| ble::attribute_uuid(env, characteristic.as_ptr()))
            .and_then(|s| BluetoothUuid::parse(&s).ok())
            .unwrap_or_else(|| BluetoothUuid::from_u16(0))
    }

    fn is_subscribed(&self, characteristic: &BluetoothUuid, central: &RemoteCentral) -> bool {
        self.subscribers
            .lock()
            .unwrap()
            .get(characteristic)
            .is_some_and(|list| list.iter().any(|c| c.id == central.id))
    }

    fn respond(
        &self,
        device: &Ref,
        request_id: i32,
        status: AttError,
        offset: usize,
        value: &[u8],
    ) {
        let (Some(runtime), Some(server)) = (Runtime::get(), self.server.lock().unwrap().clone())
        else {
            return;
        };
        let Ok(env) = runtime.env() else { return };
        let _ = ble::server_send_response(
            env,
            server.as_ptr(),
            device.as_ptr(),
            request_id,
            status as i32,
            offset as i32,
            value,
        );
    }

    async fn require_powered_on(&self) -> Result<()> {
        let runtime = Runtime::get().ok_or_else(|| {
            Error::NotSupported(
                "the Android backend is not started — call webbluetooth::android::init(vm, context)"
                    .into(),
            )
        })?;
        let env = runtime.env().map_err(|e| Error::Network(e.to_string()))?;
        let adapter = ble::Adapter::open(runtime)
            .map_err(|_| Error::NotAvailable(Availability::Unsupported))?;
        if adapter.is_enabled(env) {
            Ok(())
        } else {
            Err(Error::NotAvailable(Availability::PoweredOff))
        }
    }
}

/// Android's `AdvertiseCallback.ADVERTISE_FAILED_*`.
fn describe_advertise_error(code: i32) -> &'static str {
    match code {
        1 => "the advertising payload is larger than 31 bytes",
        2 => "too many advertisers are already running",
        3 => "this peripheral is already advertising",
        4 => "an internal error",
        5 => "this device does not support advertising",
        _ => "an unknown error",
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
        Self::build(None)
    }

    /// Android has no state preservation, so the identifier is accepted and
    /// ignored — nothing here relaunches a process for Bluetooth.
    pub fn with_restoration(_restoration: webbluetooth_core::Restoration) -> (Self, Requests) {
        Self::build(None)
    }

    fn build(_identifier: Option<String>) -> (Self, Requests) {
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
        let inner = Arc::new_cyclic(|_weak: &Weak<Inner>| Inner {
            server: Mutex::new(None),
            server_callback: Mutex::new(None),
            advertise_callback: Mutex::new(None),
            service_waiters: Mutex::new(Vec::new()),
            advertising_waiters: Mutex::new(Vec::new()),
            ready_waiters: Mutex::new(Vec::new()),
            subscribers: Mutex::new(HashMap::new()),
            published: Mutex::new(HashMap::new()),
            cccds: Mutex::new(HashMap::new()),
            requests: Mutex::new(Some(tx)),
            advertising: AtomicBool::new(false),
            l2cap: Mutex::new(HashMap::new()),
        });
        (Self { inner }, Requests { rx })
    }

    /// Why the peripheral role is not usable, if it is not.
    pub async fn availability(&self) -> std::result::Result<(), Availability> {
        match self.inner.require_powered_on().await {
            Ok(()) => Ok(()),
            Err(Error::NotAvailable(a)) => Err(a),
            Err(_) => Err(Availability::Unknown),
        }
    }

    /// Open the GATT server, once, and keep its callback alive.
    fn server(&self) -> Result<Ref> {
        if let Some(server) = self.inner.server.lock().unwrap().clone() {
            return Ok(server);
        }
        let runtime = Runtime::get()
            .ok_or_else(|| Error::NotSupported("the Android backend is not started".into()))?;
        let env = runtime.env().map_err(|e| Error::Network(e.to_string()))?;

        let callback = ble::Callback::new(
            runtime,
            "dev.webbluetooth.GattServerCallback",
            crate::gatt_server_callback_dex(),
            &crate::bluetooth::server_natives(),
            Arc::new(Sink(Arc::downgrade(&self.inner))),
        )
        .map_err(|e| Error::Network(e.to_string()))?;

        let server = ble::open_gatt_server(env, runtime, callback.as_ptr())
            .map_err(|e| Error::Network(e.to_string()))?;
        let server = Ref::new(env, server).ok_or(Error::NotAvailable(Availability::Unsupported))?;

        *self.inner.server_callback.lock().unwrap() = Some(callback);
        *self.inner.server.lock().unwrap() = Some(server.clone());
        Ok(server)
    }

    /// Publish a service into the local GATT database.
    ///
    /// A characteristic that can notify gets a Client Characteristic
    /// Configuration descriptor added automatically — Android does not, and
    /// without one no central can subscribe.
    pub async fn publish(&self, service: Service) -> Result<PublishedService> {
        service.validate()?;
        self.inner.require_powered_on().await?;
        let runtime = Runtime::get()
            .ok_or_else(|| Error::NotSupported("the Android backend is not started".into()))?;
        let env = runtime.env().map_err(|e| Error::Network(e.to_string()))?;
        let server = self.server()?;

        let java_service = ble::new_service(env, service.uuid.as_str(), service.primary)
            .map_err(|e| Error::Network(e.to_string()))?;
        let mut published = Vec::new();

        for definition in &service.characteristics {
            let characteristic = ble::new_characteristic(
                env,
                definition.uuid.as_str(),
                definition.properties,
                definition.permissions,
            )
            .map_err(|e| Error::Network(e.to_string()))?;

            if let Some(value) = &definition.value {
                ble::set_value(env, characteristic, value)
                    .map_err(|e| Error::Network(e.to_string()))?;
            }

            for d in &definition.descriptors {
                let descriptor = ble::new_descriptor(env, d.uuid.as_str(), Permissions::READABLE)
                    .map_err(|e| Error::Network(e.to_string()))?;
                ble::set_value(env, descriptor, &d.value)
                    .map_err(|e| Error::Network(e.to_string()))?;
                ble::characteristic_add_descriptor(env, characteristic, descriptor)
                    .map_err(|e| Error::Network(e.to_string()))?;
            }

            // The subscription descriptor, which Android will not add for us.
            let properties = Properties(definition.properties);
            if properties.notify() || properties.indicate() {
                let cccd = ble::new_descriptor(
                    env,
                    cccd_uuid().as_str(),
                    Permissions::READABLE | Permissions::WRITEABLE,
                )
                .map_err(|e| Error::Network(e.to_string()))?;
                ble::set_value(env, cccd, &[0x00, 0x00])
                    .map_err(|e| Error::Network(e.to_string()))?;
                ble::characteristic_add_descriptor(env, characteristic, cccd)
                    .map_err(|e| Error::Network(e.to_string()))?;
                if let Some(cccd) = Ref::new(env, cccd) {
                    self.inner
                        .cccds
                        .lock()
                        .unwrap()
                        .insert(cccd.key(), definition.uuid);
                    // Held for the life of the service.
                    std::mem::forget(cccd);
                }
            }

            ble::service_add_characteristic(env, java_service, characteristic)
                .map_err(|e| Error::Network(e.to_string()))?;

            if let Some(handle) = Ref::new(env, characteristic) {
                self.inner
                    .published
                    .lock()
                    .unwrap()
                    .insert(handle.key(), definition.uuid);
                published.push(PublishedCharacteristic {
                    inner: self.inner.clone(),
                    uuid: definition.uuid,
                    handle,
                });
            }
        }

        // `addService` is asynchronous; the answer is `onServiceAdded`.
        let rx = {
            let (tx, rx) = oneshot::channel();
            self.inner.service_waiters.lock().unwrap().push(tx);
            rx
        };
        ble::server_add_service(env, server.as_ptr(), java_service)
            .map_err(|e| Error::Network(e.to_string()))?;
        rx.await
            .map_err(|_| Error::Aborted("publishing was cancelled".into()))??;

        let handle = Ref::new(env, java_service)
            .ok_or_else(|| Error::Aborted("the published service vanished".into()))?;
        Ok(PublishedService {
            inner: self.inner.clone(),
            uuid: service.uuid,
            handle,
            characteristics: published,
        })
    }

    /// Remove every published service.
    pub fn unpublish_all(&self) {
        if let (Some(runtime), Some(server)) =
            (Runtime::get(), self.inner.server.lock().unwrap().clone())
        {
            if let Ok(env) = runtime.env() {
                let _ = ble::server_clear_services(env, server.as_ptr());
            }
        }
        self.inner.published.lock().unwrap().clear();
        self.inner.cccds.lock().unwrap().clear();
        self.inner.subscribers.lock().unwrap().clear();
    }

    /// Start advertising.
    ///
    /// Android will not put an arbitrary name in the payload — `AdvertiseData`
    /// only offers "include the device name", and the device name is the
    /// adapter's, shared by every app. So
    /// [`Advertising::local_name`](webbluetooth_core::peripheral::Advertising::local_name) turns that
    /// flag on rather than setting a name.
    pub async fn start_advertising(&self, advertising: Advertising) -> Result<()> {
        self.inner.require_powered_on().await?;
        let runtime = Runtime::get()
            .ok_or_else(|| Error::NotSupported("the Android backend is not started".into()))?;
        let env = runtime.env().map_err(|e| Error::Network(e.to_string()))?;
        let adapter = ble::Adapter::open(runtime)
            .map_err(|_| Error::NotAvailable(Availability::Unsupported))?;
        let advertiser = adapter
            .advertiser(env)
            .map_err(|e| Error::Network(e.to_string()))?;
        if advertiser.is_null() {
            return Err(Error::NotSupported(
                "this device cannot advertise — no BluetoothLeAdvertiser".into(),
            ));
        }

        let callback = ble::Callback::new(
            runtime,
            "dev.webbluetooth.AdvertiseCallback",
            crate::advertise_callback_dex(),
            &crate::bluetooth::advertise_natives(),
            Arc::new(Sink(Arc::downgrade(&self.inner))),
        )
        .map_err(|e| Error::Network(e.to_string()))?;

        let services: Vec<String> = advertising
            .services
            .iter()
            .map(|u| u.as_str().to_owned())
            .collect();

        let rx = {
            let (tx, rx) = oneshot::channel();
            self.inner.advertising_waiters.lock().unwrap().push(tx);
            rx
        };
        ble::start_advertising(
            env,
            advertiser,
            callback.as_ptr(),
            advertising.local_name.is_some(),
            &services,
            true,
        )
        .map_err(|e| Error::Network(e.to_string()))?;
        *self.inner.advertise_callback.lock().unwrap() = Some(callback);

        rx.await
            .map_err(|_| Error::Aborted("advertising was cancelled".into()))?
    }

    /// Stop advertising.
    pub fn stop_advertising(&self) {
        let Some(callback) = self.inner.advertise_callback.lock().unwrap().take() else {
            return;
        };
        if let Some(runtime) = Runtime::get() {
            if let Ok(env) = runtime.env() {
                if let Ok(adapter) = ble::Adapter::open(runtime) {
                    if let Ok(advertiser) = adapter.advertiser(env) {
                        let _ = ble::stop_advertising(env, advertiser, callback.as_ptr());
                    }
                }
            }
        }
        self.inner.advertising.store(false, Ordering::Release);
    }

    /// Whether the radio is currently advertising.
    pub fn is_advertising(&self) -> bool {
        self.inner.advertising.load(Ordering::Acquire)
    }

    /// Publish an L2CAP channel and return the PSM the system assigned.
    ///
    /// Needs API 29. A thread accepts connections and reports each as
    /// [`Request::ChannelOpened`], because `accept()` blocks.
    pub async fn publish_l2cap_channel(
        &self,
        encryption_required: bool,
    ) -> Result<webbluetooth_core::Psm> {
        self.inner.require_powered_on().await?;
        let runtime = Runtime::get()
            .ok_or_else(|| Error::NotSupported("the Android backend is not started".into()))?;
        let env = runtime.env().map_err(|e| Error::Network(e.to_string()))?;
        let adapter = ble::Adapter::open(runtime)
            .map_err(|_| Error::NotAvailable(Availability::Unsupported))?;

        let socket = ble::listen_l2cap(env, adapter.as_ptr(), encryption_required)
            .map_err(|e| Error::Network(e.to_string()))?;
        if socket.is_null() {
            return Err(Error::NotSupported(
                "listenUsingL2capChannel is unavailable — it needs API 29 or newer".into(),
            ));
        }
        let psm =
            ble::server_socket_psm(env, socket).map_err(|e| Error::Network(e.to_string()))? as u16;
        let Some(socket) = Ref::new(env, socket) else {
            return Err(Error::Network("the listening socket vanished".into()));
        };

        let stop = Arc::new(AtomicBool::new(false));
        self.inner
            .l2cap
            .lock()
            .unwrap()
            .insert(psm, (socket.clone(), stop.clone()));

        // `accept()` blocks, so it gets its own thread.
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name(format!("webbluetooth-l2cap-accept-{psm}"))
            .spawn(move || {
                let Some(runtime) = Runtime::get() else {
                    return;
                };
                let Ok(env) = runtime.env() else { return };
                while !stop.load(Ordering::Acquire) {
                    let Ok(accepted) = ble::server_socket_accept(env, socket.as_ptr()) else {
                        break;
                    };
                    if accepted.is_null() {
                        break;
                    }
                    if let Ok(channel) = crate::l2cap::from_accepted(accepted, psm) {
                        inner.send(Request::ChannelOpened(channel));
                    }
                }
                runtime.vm().detach();
            })
            .map_err(|e| Error::Network(format!("could not start the accept thread: {e}")))?;

        Ok(psm)
    }

    /// Withdraw a previously published PSM.
    pub async fn unpublish_l2cap_channel(&self, psm: webbluetooth_core::Psm) -> Result<()> {
        let Some((socket, stop)) = self.inner.l2cap.lock().unwrap().remove(&psm) else {
            return Ok(());
        };
        stop.store(true, Ordering::Release);
        if let Some(runtime) = Runtime::get() {
            if let Ok(env) = runtime.env() {
                // Closing is what wakes the accept thread.
                let _ = ble::server_socket_close(env, socket.as_ptr());
            }
        }
        Ok(())
    }

    /// Always `None`: Android preserves nothing across a relaunch.
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

/// A service that is live in the local GATT database.
pub struct PublishedService {
    inner: Arc<Inner>,
    uuid: BluetoothUuid,
    handle: Ref,
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

    /// Remove just this service from the database.
    pub fn unpublish(self) {
        if let (Some(runtime), Some(server)) =
            (Runtime::get(), self.inner.server.lock().unwrap().clone())
        {
            if let Ok(env) = runtime.env() {
                let _ = ble::server_remove_service(env, server.as_ptr(), self.handle.as_ptr());
            }
        }
        let mut published = self.inner.published.lock().unwrap();
        for c in &self.characteristics {
            published.remove(&c.handle.key());
        }
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
    handle: Ref,
}

impl PublishedCharacteristic {
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.uuid
    }

    /// The centrals currently subscribed.
    ///
    /// Tracked here rather than asked of Android, which keeps no such list.
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
    ///
    /// `false` if any send was refused — Android takes one notification at a
    /// time per connection and rejects the next until `onNotificationSent`.
    pub fn try_notify(&self, value: &[u8]) -> bool {
        self.try_notify_centrals(value, &self.subscribers())
    }

    /// Send a value to specific subscribers without waiting.
    pub fn try_notify_centrals(&self, value: &[u8], centrals: &[RemoteCentral]) -> bool {
        let (Some(runtime), Some(server)) =
            (Runtime::get(), self.inner.server.lock().unwrap().clone())
        else {
            return false;
        };
        let Ok(env) = runtime.env() else { return false };

        let indicate = false;
        let mut all = true;
        for central in centrals {
            all &= ble::server_notify(
                env,
                server.as_ptr(),
                central.device.as_ptr(),
                self.handle.as_ptr(),
                indicate,
                value,
            )
            .unwrap_or(false);
        }
        all
    }

    /// Send a value to every subscriber, waiting for the radio if needed.
    pub async fn notify(&self, value: &[u8]) -> Result<()> {
        let centrals = self.subscribers();
        if centrals.is_empty() {
            return Ok(());
        }
        loop {
            if self.try_notify_centrals(value, &centrals) {
                return Ok(());
            }
            let rx = {
                let (tx, rx) = oneshot::channel();
                self.inner.ready_waiters.lock().unwrap().push(tx);
                rx
            };
            rx.await
                .map_err(|_| Error::Aborted("notification was cancelled".into()))?;
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

/// Android publishes whatever it is given, so these check far less than the
/// Apple engine does — and that difference is the point.
#[cfg(test)]
mod tests {
    use super::*;
    use webbluetooth_core::peripheral::{Characteristic, Descriptor};
    use webbluetooth_core::uuid::characteristics;

    #[test]
    fn a_fixed_value_may_also_be_writable() {
        // CoreBluetooth forbids this and applies that rule where it publishes;
        // the portable checks do not, and Android adds none of its own.
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
    fn the_cccd_cannot_be_declared_by_hand() {
        // The engine adds it for any notify characteristic; a second would be
        // published alongside.
        let err = Descriptor {
            uuid: cccd_uuid(),
            value: vec![0, 0],
            is_string: false,
        }
        .validate()
        .unwrap_err();
        assert!(err.to_string().contains("automatically"), "got {err}");
    }

    #[test]
    fn any_other_descriptor_is_accepted() {
        // Unlike CoreBluetooth, which allows only 0x2901 and 0x2904.
        assert!(Descriptor::user_description("x").validate().is_ok());
        assert!(Descriptor::presentation_format([0u8; 7]).validate().is_ok());
        assert!(Descriptor {
            uuid: BluetoothUuid::from_u16(0x2908),
            value: vec![0],
            is_string: false,
        }
        .validate()
        .is_ok());
        // A descriptor with no value is refused everywhere.
        assert!(Descriptor {
            uuid: BluetoothUuid::from_u16(0x2901),
            value: Vec::new(),
            is_string: true,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn advertise_errors_are_explained() {
        assert!(describe_advertise_error(1).contains("31 bytes"));
        assert!(describe_advertise_error(5).contains("does not support"));
    }
}
