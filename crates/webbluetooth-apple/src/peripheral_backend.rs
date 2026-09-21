//! The Apple peripheral engine: `CBPeripheralManager`.

use crate::{
    cb, peripheral as sys, ManagerState, PeripheralEvent, PeripheralEventSink, PeripheralHost,
    Retained,
};
use futures_channel::oneshot;
use futures_core::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use webbluetooth_core::error::{Availability, Error, Result};
use webbluetooth_core::peripheral::Advertising;
use webbluetooth_core::peripheral::{AttError, RestoredPeripheral, Service, Write};
use webbluetooth_core::uuid::BluetoothUuid;
use webbluetooth_core::uuid::IntoUuid;

/// Translate the shared error vocabulary into CoreBluetooth's.
fn to_sys(error: AttError) -> sys::AttError {
    use sys::AttError as S;
    match error {
        AttError::Success => S::Success,
        AttError::InvalidHandle => S::InvalidHandle,
        AttError::ReadNotPermitted => S::ReadNotPermitted,
        AttError::WriteNotPermitted => S::WriteNotPermitted,
        AttError::InvalidPdu => S::InvalidPdu,
        AttError::InsufficientAuthentication => S::InsufficientAuthentication,
        AttError::RequestNotSupported => S::RequestNotSupported,
        AttError::InvalidOffset => S::InvalidOffset,
        AttError::InsufficientAuthorization => S::InsufficientAuthorization,
        AttError::PrepareQueueFull => S::PrepareQueueFull,
        AttError::AttributeNotFound => S::AttributeNotFound,
        AttError::AttributeNotLong => S::AttributeNotLong,
        AttError::InsufficientEncryptionKeySize => S::InsufficientEncryptionKeySize,
        AttError::InvalidAttributeValueLength => S::InvalidAttributeValueLength,
        AttError::UnlikelyError => S::UnlikelyError,
        AttError::InsufficientEncryption => S::InsufficientEncryption,
        AttError::UnsupportedGroupType => S::UnsupportedGroupType,
        AttError::InsufficientResources => S::InsufficientResources,
    }
}

/// A central connected to this peripheral.
#[derive(Debug, Clone)]
pub struct RemoteCentral {
    handle: Retained,
    id: String,
}

impl RemoteCentral {
    /// The central's per-host identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The most bytes one notification to this central can carry.
    pub fn max_notification_length(&self) -> usize {
        unsafe { sys::central_max_update_length(self.handle.as_ptr()) }
    }
}

/// An inbound ATT read.
///
/// **Answer it.** A request that is dropped without a response is answered
/// [`webbluetooth_core::peripheral::AttError::RequestNotSupported`] automatically, because leaving it silent
/// stalls the central until the ATT timeout — but say what you mean.
pub struct ReadRequest {
    inner: Arc<Inner>,
    request: Retained,
    characteristic: BluetoothUuid,
    central: RemoteCentral,
    offset: usize,
    answered: bool,
}

impl ReadRequest {
    /// Which characteristic is being read.
    pub fn characteristic(&self) -> &BluetoothUuid {
        &self.characteristic
    }

    /// Who is reading.
    pub fn central(&self) -> &RemoteCentral {
        &self.central
    }

    /// Where in a long attribute this read starts. Non-zero when a central is
    /// paging through a value larger than the MTU.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Answer with a value.
    ///
    /// The offset is applied for you: pass the whole attribute value and the
    /// correct slice is sent.
    pub fn respond(mut self, value: &[u8]) -> Result<()> {
        if self.offset > value.len() {
            self.answer(AttError::InvalidOffset);
            return Err(Error::InvalidModification(format!(
                "read offset {} is past the end of a {}-byte value",
                self.offset,
                value.len()
            )));
        }
        unsafe { sys::request_set_value(self.request.as_ptr(), &value[self.offset..]) };
        self.answer(AttError::Success);
        Ok(())
    }

    /// Refuse, with a reason the central sees as an ATT error.
    pub fn reject(mut self, error: AttError) {
        self.answer(error);
    }

    fn answer(&mut self, result: AttError) {
        if self.answered {
            return;
        }
        self.answered = true;
        unsafe {
            sys::respond(
                self.inner.host.as_ptr(),
                self.request.as_ptr(),
                to_sys(result),
            )
        };
    }
}

impl Drop for ReadRequest {
    fn drop(&mut self) {
        // Responding twice raises an Objective-C exception, so `answer` is
        // idempotent; not responding at all stalls the central.
        self.answer(AttError::RequestNotSupported);
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

/// One or more inbound ATT writes, to be applied atomically.
///
/// CoreBluetooth delivers writes in a batch and expects a single answer for the
/// whole batch: either all of them take effect or none do. As with
/// [`ReadRequest`], dropping this answers [`webbluetooth_core::peripheral::AttError::RequestNotSupported`].
pub struct WriteRequest {
    inner: Arc<Inner>,
    /// Only the first is answered; that response covers the batch.
    first: Retained,
    writes: Vec<Write>,
    central: RemoteCentral,
    answered: bool,
}

impl WriteRequest {
    /// Every write in this batch, in order.
    pub fn writes(&self) -> &[Write] {
        &self.writes
    }

    /// Who is writing.
    pub fn central(&self) -> &RemoteCentral {
        &self.central
    }

    /// Accept the whole batch.
    pub fn accept(mut self) {
        self.answer(AttError::Success);
    }

    /// Refuse the whole batch.
    pub fn reject_all(mut self, error: AttError) {
        self.answer(error);
    }

    fn answer(&mut self, result: AttError) {
        if self.answered {
            return;
        }
        self.answered = true;
        unsafe {
            sys::respond(
                self.inner.host.as_ptr(),
                self.first.as_ptr(),
                to_sys(result),
            )
        };
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

/// The stream of [`Request`]s, returned once by [`Peripheral::new`].
/// How many requests from a central queue before one is dropped.
const REQUEST_BACKLOG: usize = 1024;

/// What centrals are asking of this peripheral, as a stream.
///
/// Handed back by [`Peripheral::new`] and consumed exactly once: a read or a
/// write is a question with one answer, so splitting the stream between two
/// consumers would mean two halves of an application each holding some of the
/// questions and neither able to answer all of them.
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

/// Something a connected central did.
#[derive(Debug)]
pub enum Request {
    /// A central is reading a characteristic with no fixed value.
    Read(ReadRequest),
    /// A central is writing.
    Write(WriteRequest),
    /// A central subscribed — start sending it notifications.
    Subscribed {
        central: RemoteCentral,
        characteristic: BluetoothUuid,
    },
    /// A central unsubscribed, or disconnected while subscribed.
    Unsubscribed {
        central: RemoteCentral,
        characteristic: BluetoothUuid,
    },
    /// The notification queue drained after [`PublishedCharacteristic::try_notify`]
    /// returned `false`.
    ReadyToNotify,
    /// A central opened an L2CAP channel to a PSM published with
    /// [`Peripheral::publish_l2cap_channel`].
    ChannelOpened(crate::l2cap::L2capChannel),
}

struct Inner {
    host: PeripheralHost,
    state: Mutex<ManagerState>,
    state_waiters: Mutex<Vec<oneshot::Sender<ManagerState>>>,
    advertising_waiters: Mutex<Vec<oneshot::Sender<Result<()>>>>,
    /// Keyed by the `CBMutableService` pointer we asked to add.
    service_waiters: Mutex<HashMap<usize, Vec<oneshot::Sender<Result<()>>>>>,
    ready_waiters: Mutex<Vec<oneshot::Sender<()>>>,
    /// Published characteristics, so an inbound request can be named.
    published: Mutex<HashMap<usize, BluetoothUuid>>,
    requests: Mutex<Option<webbluetooth_core::backlog::Sender<Request>>>,
    l2cap_publish_waiters: Mutex<Vec<oneshot::Sender<Result<u16>>>>,
    l2cap_unpublish_waiters: Mutex<Vec<oneshot::Sender<Result<()>>>>,
    restored: Mutex<Option<RestoredPeripheral>>,
}

struct Sink(Weak<Inner>);

impl PeripheralEventSink for Sink {
    fn emit(&self, event: PeripheralEvent) {
        if let Some(inner) = self.0.upgrade() {
            inner.handle(event);
        }
    }
}

impl Inner {
    fn handle(self: &Arc<Self>, event: PeripheralEvent) {
        match event {
            PeripheralEvent::StateChanged(state) => {
                *self.state.lock().unwrap() = state;
                for tx in self.state_waiters.lock().unwrap().drain(..) {
                    let _ = tx.send(state);
                }
            }

            PeripheralEvent::AdvertisingStarted { error } => {
                let result = match error {
                    Some((code, message)) => Err(Error::Network(format!(
                        "could not start advertising: {message} (CBError {code})"
                    ))),
                    None => Ok(()),
                };
                for tx in self.advertising_waiters.lock().unwrap().drain(..) {
                    let _ = tx.send(result.clone());
                }
            }

            PeripheralEvent::ServiceAdded { service, error } => {
                let result = match error {
                    Some((code, message)) => Err(Error::Network(format!(
                        "could not publish service: {message} (CBError {code})"
                    ))),
                    None => Ok(()),
                };
                let waiters = self.service_waiters.lock().unwrap().remove(&service.key());
                for tx in waiters.into_iter().flatten() {
                    let _ = tx.send(result.clone());
                }
            }

            PeripheralEvent::Subscribed {
                central,
                characteristic,
            } => {
                let uuid = self.name_of(&characteristic);
                self.send(Request::Subscribed {
                    central: self.wrap_central(central),
                    characteristic: uuid,
                });
            }

            PeripheralEvent::Unsubscribed {
                central,
                characteristic,
            } => {
                let uuid = self.name_of(&characteristic);
                self.send(Request::Unsubscribed {
                    central: self.wrap_central(central),
                    characteristic: uuid,
                });
            }

            PeripheralEvent::ReadRequest { request } => {
                let (characteristic, central, offset) = unsafe {
                    let c = sys::request_characteristic(request.as_ptr());
                    let uuid = Retained::retain(c)
                        .map(|r| self.name_of(&r))
                        .unwrap_or_else(|| BluetoothUuid::from_u16(0));
                    let central = Retained::retain(sys::request_central(request.as_ptr()));
                    (uuid, central, sys::request_offset(request.as_ptr()))
                };
                let Some(central) = central.map(|c| self.wrap_central(c)) else {
                    return;
                };
                let read = ReadRequest {
                    inner: self.clone(),
                    request,
                    characteristic,
                    central,
                    offset,
                    answered: false,
                };
                // If nobody is listening, `read` drops here and the central is
                // answered RequestNotSupported rather than left hanging.
                self.send(Request::Read(read));
            }

            PeripheralEvent::WriteRequests { requests } => {
                let Some(first) = requests.first().cloned() else {
                    return;
                };
                let central = unsafe { Retained::retain(sys::request_central(first.as_ptr())) };
                let Some(central) = central.map(|c| self.wrap_central(c)) else {
                    return;
                };

                let writes = requests
                    .iter()
                    .map(|r| unsafe {
                        let c = Retained::retain(sys::request_characteristic(r.as_ptr()));
                        Write {
                            characteristic: c
                                .map(|c| self.name_of(&c))
                                .unwrap_or_else(|| BluetoothUuid::from_u16(0)),
                            offset: sys::request_offset(r.as_ptr()),
                            value: sys::request_value(r.as_ptr()).unwrap_or_default(),
                        }
                    })
                    .collect();

                self.send(Request::Write(WriteRequest {
                    inner: self.clone(),
                    first,
                    writes,
                    central,
                    answered: false,
                }));
            }

            PeripheralEvent::L2capPublished { psm, error } => {
                let result = match error {
                    Some((code, message)) => Err(Error::Network(format!(
                        "could not publish an L2CAP channel: {message} (CBError {code})"
                    ))),
                    None => Ok(psm),
                };
                for tx in self.l2cap_publish_waiters.lock().unwrap().drain(..) {
                    let _ = tx.send(result.clone());
                }
            }

            PeripheralEvent::L2capUnpublished { psm, error } => {
                let result = match error {
                    Some((code, message)) => Err(Error::Network(format!(
                        "could not unpublish PSM {psm}: {message} (CBError {code})"
                    ))),
                    None => Ok(()),
                };
                for tx in self.l2cap_unpublish_waiters.lock().unwrap().drain(..) {
                    let _ = tx.send(result.clone());
                }
            }

            PeripheralEvent::L2capChannelOpened { channel, error } => {
                if error.is_some() {
                    return;
                }
                let Some(handle) = channel else { return };
                // If nobody is consuming requests the channel drops here, which
                // closes it — better than leaving a half-open pipe.
                // SAFETY: `handle` came from a `didOpenL2CAPChannel:` callback.
                if let Ok(channel) = unsafe { crate::l2cap::adopt(handle) } {
                    self.send(Request::ChannelOpened(channel));
                }
            }

            PeripheralEvent::WillRestoreState(restored) => {
                *self.restored.lock().unwrap() = Some(RestoredPeripheral {
                    advertised_local_name: restored.advertised_local_name.clone(),
                    advertised_services: restored
                        .advertised_services
                        .iter()
                        .filter_map(|s| BluetoothUuid::parse(s).ok())
                        .collect(),
                    service_count: restored.services.len(),
                });
            }

            PeripheralEvent::ReadyToUpdateSubscribers => {
                for tx in self.ready_waiters.lock().unwrap().drain(..) {
                    let _ = tx.send(());
                }
                self.send(Request::ReadyToNotify);
            }
        }
    }

    fn send(&self, request: Request) {
        let tx = self.requests.lock().unwrap().clone();
        if let Some(tx) = tx {
            if tx.send(request).is_err() {
                // The consumer is gone; stop trying.
                *self.requests.lock().unwrap() = None;
            }
        }
    }

    fn name_of(&self, characteristic: &Retained) -> BluetoothUuid {
        if let Some(uuid) = self.published.lock().unwrap().get(&characteristic.key()) {
            return *uuid;
        }
        // Not one we published, or published through a different object: read it
        // straight off the attribute.
        unsafe { cb::uuid_string(cb::attribute_uuid(characteristic.as_ptr())) }
            .and_then(|s| BluetoothUuid::parse(&s).ok())
            .unwrap_or_else(|| BluetoothUuid::from_u16(0))
    }

    fn wrap_central(&self, handle: Retained) -> RemoteCentral {
        let id = unsafe { sys::central_identifier(handle.as_ptr()) }.unwrap_or_default();
        RemoteCentral { handle, id }
    }

    async fn settled_state(&self) -> ManagerState {
        let mut state = *self.state.lock().unwrap();
        if state != ManagerState::Unknown {
            return state;
        }
        let rx = {
            let (tx, rx) = oneshot::channel();
            let current = *self.state.lock().unwrap();
            if current != ManagerState::Unknown {
                return current;
            }
            self.state_waiters.lock().unwrap().push(tx);
            rx
        };
        if let Ok(Ok(s)) =
            webbluetooth_core::timer::timeout(std::time::Duration::from_secs(5), rx).await
        {
            state = s;
        }
        state
    }

    async fn require_powered_on(&self) -> Result<()> {
        crate::backend::manager_state(self.settled_state().await).require_powered_on()
    }
}

/// A local GATT server — `CBPeripheralManager`.
#[derive(Clone)]
pub struct Peripheral {
    inner: Arc<Inner>,
}

impl Peripheral {
    /// Open the peripheral role, returning the handle and its request stream.
    ///
    /// The stream is single-consumer by construction: a [`ReadRequest`] must be
    /// answered exactly once, so it cannot be handed to two listeners.
    pub fn new() -> (Self, Requests) {
        Self::build(None)
    }

    /// Open the peripheral role with state preservation and restoration.
    ///
    /// Only the identifier is used here; see [`webbluetooth_core::Restoration`] for the
    /// platform caveat — in short, only iOS ever relaunches a process for this.
    pub fn with_restoration(restoration: webbluetooth_core::Restoration) -> (Self, Requests) {
        Self::build(Some(restoration))
    }

    fn build(restoration: Option<webbluetooth_core::Restoration>) -> (Self, Requests) {
        let identifier = restoration.map(|r| r.identifier);
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
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let sink = Arc::new(Sink(weak.clone()));
            Inner {
                host: PeripheralHost::with_restore_identifier(sink, false, identifier.as_deref()),
                state: Mutex::new(ManagerState::Unknown),
                state_waiters: Mutex::new(Vec::new()),
                advertising_waiters: Mutex::new(Vec::new()),
                service_waiters: Mutex::new(HashMap::new()),
                ready_waiters: Mutex::new(Vec::new()),
                published: Mutex::new(HashMap::new()),
                requests: Mutex::new(Some(tx)),
                l2cap_publish_waiters: Mutex::new(Vec::new()),
                l2cap_unpublish_waiters: Mutex::new(Vec::new()),
                restored: Mutex::new(None),
            }
        });
        // The first state callback can land before `new_cyclic` returns, when
        // the weak reference cannot be upgraded; reconcile from the manager.
        let actual = inner.host.state();
        if actual != ManagerState::Unknown {
            *inner.state.lock().unwrap() = actual;
        }
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

    /// Publish a service into the local GATT database.
    ///
    /// Resolves once CoreBluetooth confirms it. Definitions are validated first
    /// — see the module documentation for the two rules that would otherwise
    /// raise an Objective-C exception.
    pub async fn publish(&self, service: Service) -> Result<PublishedService> {
        service.validate()?;
        // CoreBluetooth's own rule on top of the portable ones: a
        // `CBMutableCharacteristic` with a cached value must be read-only,
        // because the value it caches can never change.
        for c in &service.characteristics {
            sys::validate_characteristic(
                crate::Properties(c.properties),
                crate::Permissions(c.permissions),
                c.value.as_deref(),
            )
            .map_err(|e| Error::NotSupported(format!("characteristic {}: {e}", c.uuid)))?;
        }
        self.inner.require_powered_on().await?;

        // Build the whole tree before adding it; nothing is published until
        // `addService:`, so a failure part-way leaves no trace.
        let service_uuid = service.uuid;
        let mut characteristic_handles: Vec<(BluetoothUuid, Retained)> = Vec::new();
        let mut descriptor_keepalive: Vec<Retained> = Vec::new();

        let cb_service = unsafe {
            let uuid = cb::uuid_from_string(service_uuid.as_str())
                .ok_or_else(|| Error::Security(format!("invalid service UUID {service_uuid}")))?;
            sys::mutable_service(uuid.as_ptr(), service.primary)
                .ok_or_else(|| Error::Aborted("could not allocate CBMutableService".into()))?
        };

        for definition in &service.characteristics {
            let handle = unsafe {
                let uuid = cb::uuid_from_string(definition.uuid.as_str()).ok_or_else(|| {
                    Error::Security(format!("invalid characteristic UUID {}", definition.uuid))
                })?;
                let c = sys::mutable_characteristic(
                    uuid.as_ptr(),
                    crate::Properties(definition.properties),
                    definition.value.as_deref(),
                    crate::Permissions(definition.permissions),
                )
                .ok_or_else(|| {
                    Error::Aborted("could not allocate CBMutableCharacteristic".into())
                })?;

                if !definition.descriptors.is_empty() {
                    let mut built = Vec::new();
                    for d in &definition.descriptors {
                        let duuid = cb::uuid_from_string(d.uuid.as_str()).ok_or_else(|| {
                            Error::Security(format!("invalid descriptor UUID {}", d.uuid))
                        })?;
                        let handle = sys::mutable_descriptor(duuid.as_ptr(), &d.value, d.is_string)
                            .ok_or_else(|| {
                                Error::Aborted("could not allocate CBMutableDescriptor".into())
                            })?;
                        built.push(handle);
                    }
                    let ptrs: Vec<_> = built.iter().map(|d| d.as_ptr()).collect();
                    sys::characteristic_set_descriptors(c.as_ptr(), &ptrs);
                    descriptor_keepalive.extend(built);
                }
                c
            };
            characteristic_handles.push((definition.uuid, handle));
        }

        let ptrs: Vec<_> = characteristic_handles
            .iter()
            .map(|(_, h)| h.as_ptr())
            .collect();
        unsafe { sys::service_set_characteristics(cb_service.as_ptr(), &ptrs) };

        // Register interest before adding — `didAddService:` can arrive on the
        // dispatch queue before this thread resumes.
        let rx = {
            let (tx, rx) = oneshot::channel();
            self.inner
                .service_waiters
                .lock()
                .unwrap()
                .entry(cb_service.key())
                .or_default()
                .push(tx);
            rx
        };
        unsafe { sys::add_service(self.inner.host.as_ptr(), cb_service.as_ptr()) };
        rx.await
            .map_err(|_| Error::Aborted("publishing was cancelled".into()))??;

        {
            let mut published = self.inner.published.lock().unwrap();
            for (uuid, handle) in &characteristic_handles {
                published.insert(handle.key(), *uuid);
            }
        }

        Ok(PublishedService {
            inner: self.inner.clone(),
            uuid: service_uuid,
            handle: cb_service,
            characteristics: characteristic_handles
                .into_iter()
                .map(|(uuid, handle)| PublishedCharacteristic {
                    inner: self.inner.clone(),
                    uuid,
                    handle,
                })
                .collect(),
            _descriptors: descriptor_keepalive,
        })
    }

    /// Remove every published service.
    pub fn unpublish_all(&self) {
        unsafe { sys::remove_all_services(self.inner.host.as_ptr()) };
        self.inner.published.lock().unwrap().clear();
    }

    /// Start advertising. Resolves once CoreBluetooth confirms it, which is
    /// also when a failure is reported.
    pub async fn start_advertising(&self, advertising: Advertising) -> Result<()> {
        self.inner.require_powered_on().await?;

        let uuids: Vec<Retained> = advertising
            .services
            .iter()
            .filter_map(|u| unsafe { cb::uuid_from_string(u.as_str()) })
            .collect();
        let ptrs: Vec<_> = uuids.iter().map(|u| u.as_ptr()).collect();

        let rx = {
            let (tx, rx) = oneshot::channel();
            self.inner.advertising_waiters.lock().unwrap().push(tx);
            rx
        };
        unsafe {
            sys::start_advertising(
                self.inner.host.as_ptr(),
                advertising.local_name.as_deref(),
                &ptrs,
            )
        };
        rx.await
            .map_err(|_| Error::Aborted("advertising was cancelled".into()))?
    }

    /// Publish an L2CAP channel and return the PSM the system assigned.
    ///
    /// Centrals connecting to it arrive as [`Request::ChannelOpened`]. With
    /// `encryption_required`, only bonded peers may connect.
    pub async fn publish_l2cap_channel(
        &self,
        encryption_required: bool,
    ) -> Result<webbluetooth_core::Psm> {
        self.inner.require_powered_on().await?;
        let rx = {
            let (tx, rx) = oneshot::channel();
            self.inner.l2cap_publish_waiters.lock().unwrap().push(tx);
            rx
        };
        unsafe { sys::publish_l2cap_channel(self.inner.host.as_ptr(), encryption_required) };
        rx.await
            .map_err(|_| Error::Aborted("publishing the L2CAP channel was cancelled".into()))?
    }

    /// Withdraw a previously published PSM.
    pub async fn unpublish_l2cap_channel(&self, psm: webbluetooth_core::Psm) -> Result<()> {
        let rx = {
            let (tx, rx) = oneshot::channel();
            self.inner.l2cap_unpublish_waiters.lock().unwrap().push(tx);
            rx
        };
        unsafe { sys::unpublish_l2cap_channel(self.inner.host.as_ptr(), psm) };
        rx.await
            .map_err(|_| Error::Aborted("unpublishing was cancelled".into()))?
    }

    /// What the system preserved across a relaunch, if this peripheral was
    /// created with a restoration identifier and was in fact restored.
    ///
    /// Waits for the adapter to settle first, because
    /// `peripheralManager:willRestoreState:` is delivered before the first
    /// state report.
    pub async fn restored_state(&self) -> Option<RestoredPeripheral> {
        let _ = self.inner.settled_state().await;
        self.inner.restored.lock().unwrap().clone()
    }

    /// Stop advertising.
    pub fn stop_advertising(&self) {
        unsafe { sys::stop_advertising(self.inner.host.as_ptr()) };
    }

    /// Whether the radio is currently advertising.
    pub fn is_advertising(&self) -> bool {
        self.inner.host.is_advertising()
    }
}

impl std::fmt::Debug for Peripheral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peripheral")
            .field("state", &*self.inner.state.lock().unwrap())
            .field("advertising", &self.is_advertising())
            .finish()
    }
}

/// A service that is live in the local GATT database.
///
/// Dropping this does **not** unpublish it — call
/// [`Peripheral::unpublish_all`]. It does release the characteristic handles,
/// so keep it for as long as you want to notify.
pub struct PublishedService {
    inner: Arc<Inner>,
    uuid: BluetoothUuid,
    handle: Retained,
    characteristics: Vec<PublishedCharacteristic>,
    /// Descriptors are owned by their characteristics once set, but holding
    /// them keeps the Rust-side references alive for the service's lifetime.
    _descriptors: Vec<Retained>,
}

impl PublishedService {
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.uuid
    }

    /// Every published characteristic.
    ///
    /// Owned rather than borrowed, because BlueZ cannot lend them: a Linux
    /// `PublishedService` holds `Arc<CharacteristicState>` and pairs each with
    /// the manager on the way out, and storing the paired values instead would
    /// point a service at the manager that owns it. Cloning one is two
    /// refcount bumps, so the three platforms that *could* lend a slice match
    /// the one that cannot, rather than the API changing shape per platform.
    pub fn characteristics(&self) -> Vec<PublishedCharacteristic> {
        self.characteristics.to_vec()
    }

    /// The published characteristic with this UUID.
    pub fn characteristic(&self, uuid: impl IntoUuid) -> Option<PublishedCharacteristic> {
        let uuid = uuid.into_uuid().ok()?;
        self.characteristics
            .iter()
            .find(|c| c.uuid == uuid)
            .cloned()
    }

    /// Remove just this service from the database.
    pub fn unpublish(self) {
        unsafe { sys::remove_service(self.inner.host.as_ptr(), self.handle.as_ptr()) };
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
    handle: Retained,
}

impl PublishedCharacteristic {
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.uuid
    }

    /// The centrals currently subscribed to this characteristic.
    pub fn subscribers(&self) -> Vec<RemoteCentral> {
        unsafe { sys::subscribed_centrals(self.handle.as_ptr()) }
            .into_iter()
            .map(|handle| {
                let id = unsafe { sys::central_identifier(handle.as_ptr()) }.unwrap_or_default();
                RemoteCentral { handle, id }
            })
            .collect()
    }

    /// Send a value to every subscriber without waiting.
    ///
    /// `false` means the transmit queue is full and nothing was sent; wait for
    /// [`Request::ReadyToNotify`] and try the same value again, or use
    /// [`PublishedCharacteristic::notify`], which does that for you.
    pub fn try_notify(&self, value: &[u8]) -> bool {
        unsafe { sys::update_value(self.inner.host.as_ptr(), value, self.handle.as_ptr(), &[]) }
    }

    /// Send a value to specific subscribers without waiting.
    pub fn try_notify_centrals(&self, value: &[u8], centrals: &[RemoteCentral]) -> bool {
        let ptrs: Vec<_> = centrals.iter().map(|c| c.handle.as_ptr()).collect();
        unsafe { sys::update_value(self.inner.host.as_ptr(), value, self.handle.as_ptr(), &ptrs) }
    }

    /// Send a value to every subscriber, waiting for queue space if needed.
    ///
    /// Retries when CoreBluetooth reports the queue has drained. With no
    /// subscribers this returns immediately — there is nowhere to send.
    pub async fn notify(&self, value: &[u8]) -> Result<()> {
        if self.subscribers().is_empty() {
            return Ok(());
        }
        loop {
            if self.try_notify(value) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;
    use webbluetooth_core::gatt::CharacteristicProperties as Properties;
    use webbluetooth_core::peripheral::{Characteristic, Descriptor, Permissions, Service};
    use webbluetooth_core::uuid::{characteristics, services};

    #[test]
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn a_fixed_value_cannot_also_be_writeable() {
        let service = Service::new(services::BATTERY_SERVICE)
            .unwrap()
            .characteristic(
                Characteristic::new(characteristics::BATTERY_LEVEL)
                    .unwrap()
                    .read()
                    .write()
                    .value([50]),
            );
        let (peripheral, _events) = Peripheral::new();
        let err = block_on(peripheral.publish(service)).unwrap_err();
        assert!(matches!(err, Error::NotSupported(_)), "got {err:?}");
        assert!(
            err.to_string().contains("read-only"),
            "unhelpful message: {err}"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn a_fixed_value_cannot_also_notify() {
        let service = Service::new(services::BATTERY_SERVICE)
            .unwrap()
            .characteristic(
                Characteristic::new(characteristics::BATTERY_LEVEL)
                    .unwrap()
                    .read()
                    .notify()
                    .value([50]),
            );
        let (peripheral, _events) = Peripheral::new();
        assert!(block_on(peripheral.publish(service)).is_err());
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn a_characteristic_needs_at_least_one_property() {
        let service = Service::new(services::BATTERY_SERVICE)
            .unwrap()
            .characteristic(Characteristic::new(characteristics::BATTERY_LEVEL).unwrap());
        let (peripheral, _events) = Peripheral::new();
        let err = block_on(peripheral.publish(service)).unwrap_err();
        assert!(err.to_string().contains("no properties"), "got {err}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn a_service_needs_at_least_one_characteristic() {
        let (peripheral, _events) = Peripheral::new();
        let err = block_on(peripheral.publish(Service::new(services::BATTERY_SERVICE).unwrap()))
            .unwrap_err();
        assert!(err.to_string().contains("no characteristics"), "got {err}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn the_cccd_cannot_be_declared_by_hand() {
        // CoreBluetooth creates it from the notify property; declaring it would
        // raise an exception.
        let service = Service::new(services::BATTERY_SERVICE)
            .unwrap()
            .characteristic(
                Characteristic::new(characteristics::BATTERY_LEVEL)
                    .unwrap()
                    .read()
                    .notify()
                    .descriptor(Descriptor {
                        uuid: BluetoothUuid::from_u16(0x2902),
                        value: vec![0, 0],
                        is_string: false,
                    }),
            );
        let (peripheral, _events) = Peripheral::new();
        let err = block_on(peripheral.publish(service)).unwrap_err();
        assert!(err.to_string().contains("automatically"), "got {err}");
    }

    #[test]
    fn a_user_description_descriptor_is_accepted() {
        let d = Descriptor::user_description("Battery level");
        assert!(d.validate().is_ok());
        assert!(d.is_string);
    }

    #[test]
    fn definitions_build_the_expected_bits() {
        let c = Characteristic::new(characteristics::BATTERY_LEVEL)
            .unwrap()
            .read()
            .notify();
        assert!(Properties(c.properties).read());
        assert!(Properties(c.properties).notify());
        assert!(!Properties(c.properties).write());
        assert!(Permissions(c.permissions).readable());
        assert!(!Permissions(c.permissions).any_write());

        let w = Characteristic::new(characteristics::HEART_RATE_CONTROL_POINT)
            .unwrap()
            .write()
            .require_encryption();
        assert!(Permissions(w.permissions).has(Permissions::WRITE_ENCRYPTION_REQUIRED));
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn the_manager_reports_a_state() {
        let (peripheral, _events) = Peripheral::new();
        let availability = block_on(peripheral.availability());
        // Whatever it says, it must not still be Unknown.
        assert!(!matches!(availability, Err(Availability::Unknown)));
        assert!(!peripheral.is_advertising());
    }
}
