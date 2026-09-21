//! The Linux peripheral engine: a GATT application exported over D-Bus.
//!
//! BlueZ owns the controller, so a peripheral is not built by talking to
//! hardware but by *becoming a D-Bus service* and handing `bluetoothd` the
//! object tree it should publish:
//!
//! ```text
//! /io/webbluetooth/peripheral            org.freedesktop.DBus.ObjectManager
//!   service0                             org.bluez.GattService1
//!     service0/char0                     org.bluez.GattCharacteristic1
//!       service0/char0/desc0             org.bluez.GattDescriptor1
//! ```
//!
//! `GattManager1.RegisterApplication` points BlueZ at the root; it walks the
//! tree with `GetManagedObjects`, then calls back into our characteristics
//! whenever a central reads, writes or subscribes. Advertising is a second,
//! separate object registered with `LEAdvertisingManager1`.
//!
//! Two consequences worth knowing:
//!
//! * **Every read is a blocking D-Bus call.** BlueZ expects `ReadValue` to
//!   return the bytes, but this crate's API hands the application a
//!   [`ReadRequest`] to answer whenever it likes. The handler therefore parks
//!   until the application responds, with a deadline — which is why inbound
//!   method calls each get their own thread rather than running on the D-Bus
//!   reader.
//! * **Notifications are property changes.** There is no "notify" call; a
//!   subscribed central is sent a value by emitting `PropertiesChanged` for the
//!   characteristic's `Value`. BlueZ decides whether that becomes a
//!   notification or an indication from the characteristic's flags.

use crate::bluez::{interfaces, Bluez, Event, EventSink};
use crate::dbus::connection::ObjectHandler;
use crate::dbus::{Message, Value};
use futures_core::Stream;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use webbluetooth_core::error::{Availability, Error, Result};
use webbluetooth_core::gatt::CharacteristicProperties as Properties;
use webbluetooth_core::peripheral::{Advertising, AttError, RestoredPeripheral, Service, Write};
use webbluetooth_core::uuid::{BluetoothUuid, IntoUuid};

/// Where our objects live. Anything unique works; BlueZ only follows the path
/// it is given.
const ROOT: &str = "/io/webbluetooth/peripheral";
const ADVERTISEMENT: &str = "/io/webbluetooth/peripheral/advertisement0";

const DBUS_PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// How long a `ReadValue` or `WriteValue` waits for the application.
///
/// BlueZ gives a GATT operation rather longer than this, so timing out here
/// first means the central sees a proper ATT error instead of a stalled link.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// What a notification can carry before the MTU has been negotiated.
const DEFAULT_NOTIFY_LEN: usize = 20;

/// One `u64` out of a BlueZ options dictionary.
fn option_u64(options: Option<&Value>, key: &str) -> Option<u64> {
    options?.as_map().get(key)?.peel().as_u64()
}

/// The flag strings BlueZ expects, derived from our property bits.
fn flags_for(properties: Properties) -> Vec<String> {
    let mut flags = Vec::new();
    if properties.read() {
        flags.push("read".to_string());
    }
    if properties.write() {
        flags.push("write".to_string());
    }
    if properties.write_without_response() {
        flags.push("write-without-response".to_string());
    }
    if properties.notify() {
        flags.push("notify".to_string());
    }
    if properties.indicate() {
        flags.push("indicate".to_string());
    }
    flags
}

// ── Requests ────────────────────────────────────────────────────────────────

/// A central that is connected to us.
///
/// BlueZ identifies it by device object path; the address is the last element.
#[derive(Debug, Clone)]
pub struct RemoteCentral {
    id: String,
    mtu: usize,
}

impl RemoteCentral {
    fn from_options(options: Option<&Value>, fallback_mtu: usize) -> Self {
        let device = options.map(|o| o.as_map()).and_then(|m| {
            m.get("device")
                .and_then(|v| v.peel().as_str().map(str::to_owned))
        });
        // `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF` — the address is the tail.
        let id = device
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .and_then(|last| last.strip_prefix("dev_"))
            .map(|a| a.replace('_', ":"))
            .or(device)
            .unwrap_or_else(|| "unknown".to_string());
        // The ATT MTU, of which a notification can use all but three bytes.
        let mtu = option_u64(options, "mtu")
            .map(|v| (v as usize).saturating_sub(3))
            .unwrap_or(fallback_mtu);
        Self { id, mtu }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The most bytes one notification to this central can carry.
    pub fn max_notification_length(&self) -> usize {
        self.mtu
    }
}

/// A central is reading a characteristic and is waiting for the bytes.
pub struct ReadRequest {
    characteristic: BluetoothUuid,
    central: RemoteCentral,
    offset: usize,
    answer: Option<Sender<std::result::Result<Vec<u8>, AttError>>>,
}

impl ReadRequest {
    pub fn characteristic(&self) -> &BluetoothUuid {
        &self.characteristic
    }

    pub fn central(&self) -> &RemoteCentral {
        &self.central
    }

    /// Where in the attribute the central is reading from, for long reads.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Answer with a value.
    pub fn respond(mut self, value: &[u8]) -> Result<()> {
        let Some(answer) = self.answer.take() else {
            return Err(Error::InvalidState("this read was already answered".into()));
        };
        answer
            .send(Ok(value.to_vec()))
            .map_err(|_| Error::InvalidState("the central stopped waiting".into()))
    }

    /// Refuse, with an ATT error the central will see.
    pub fn reject(mut self, error: AttError) {
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(Err(error));
        }
    }
}

impl Drop for ReadRequest {
    fn drop(&mut self) {
        // An unanswered read would hold the handler until its deadline and
        // stall the central for no reason. Dropping it is a refusal.
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(Err(AttError::UnlikelyError));
        }
    }
}

impl std::fmt::Debug for ReadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadRequest")
            .field("characteristic", &self.characteristic.as_str())
            .field("central", &self.central.id)
            .field("offset", &self.offset)
            .finish()
    }
}

/// A central has written. BlueZ delivers one write at a time.
pub struct WriteRequest {
    writes: Vec<Write>,
    central: RemoteCentral,
    answer: Option<Sender<std::result::Result<(), AttError>>>,
}

impl WriteRequest {
    pub fn writes(&self) -> &[Write] {
        &self.writes
    }

    pub fn central(&self) -> &RemoteCentral {
        &self.central
    }

    /// Accept the write.
    pub fn accept(mut self) {
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(Ok(()));
        }
    }

    /// Refuse it, with an ATT error the central will see.
    pub fn reject_all(mut self, error: AttError) {
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(Err(error));
        }
    }
}

impl Drop for WriteRequest {
    fn drop(&mut self) {
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(Err(AttError::UnlikelyError));
        }
    }
}

impl std::fmt::Debug for WriteRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteRequest")
            .field("writes", &self.writes.len())
            .field("central", &self.central.id)
            .finish()
    }
}

/// Something a central did that the application has to deal with.
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
    /// Never emitted on Linux.
    ///
    /// BlueZ has no equivalent of CoreBluetooth's
    /// `peripheralManagerIsReadyToUpdateSubscribers:` — a value change is
    /// emitted on the bus and BlueZ queues it — but the variant exists so code
    /// written against the other engines still compiles here.
    ReadyToNotify,
    /// A central connected to a published L2CAP channel.
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

/// One published characteristic's live state.
struct CharacteristicState {
    uuid: BluetoothUuid,
    path: String,
    flags: Vec<String>,
    /// The last value, which is what a `Value` property read returns and what
    /// a notification carries.
    value: Mutex<Vec<u8>>,
    /// Centrals BlueZ has told us are subscribed.
    subscribers: Mutex<Vec<RemoteCentral>>,
    descriptors: Vec<DescriptorState>,
}

struct DescriptorState {
    uuid: BluetoothUuid,
    path: String,
    value: Vec<u8>,
}

struct ServiceState {
    uuid: BluetoothUuid,
    path: String,
    primary: bool,
    characteristics: Vec<Arc<CharacteristicState>>,
}

struct Inner {
    bluez: Option<Arc<Bluez>>,
    to_app: webbluetooth_core::backlog::Sender<Request>,
    services: Mutex<Vec<Arc<ServiceState>>>,
    registered: AtomicBool,
    advertising: AtomicBool,
    next_index: AtomicUsize,
    /// The advertisement's `a{sv}` properties, which BlueZ reads back off the
    /// object rather than taking as call arguments.
    advertisement: Mutex<Vec<(Value, Value)>>,
}

impl Inner {
    fn adapter(&self) -> Result<(Arc<Bluez>, String)> {
        let bluez = self
            .bluez
            .clone()
            .ok_or(Error::NotAvailable(Availability::Unsupported))?;
        let adapter = bluez
            .adapter()
            .ok_or(Error::NotAvailable(Availability::Unsupported))?;
        Ok((bluez, adapter))
    }

    fn characteristic_at(&self, path: &str) -> Option<Arc<CharacteristicState>> {
        self.services
            .lock()
            .unwrap()
            .iter()
            .flat_map(|s| s.characteristics.clone())
            .find(|c| c.path == path)
    }

    fn descriptor_at(&self, path: &str) -> Option<(Arc<CharacteristicState>, Vec<u8>)> {
        for service in self.services.lock().unwrap().iter() {
            for characteristic in &service.characteristics {
                for descriptor in &characteristic.descriptors {
                    if descriptor.path == path {
                        return Some((characteristic.clone(), descriptor.value.clone()));
                    }
                }
            }
        }
        None
    }

    /// The whole tree, in the shape `GetManagedObjects` must return.
    fn managed_objects(&self) -> Value {
        let mut objects: Vec<(Value, Value)> = Vec::new();
        for service in self.services.lock().unwrap().iter() {
            objects.push((
                Value::ObjectPath(service.path.clone()),
                interfaces_of(vec![(
                    interfaces::SERVICE,
                    properties(vec![
                        ("UUID", Value::Str(service.uuid.as_str().to_owned())),
                        ("Primary", Value::Bool(service.primary)),
                    ]),
                )]),
            ));
            for characteristic in &service.characteristics {
                objects.push((
                    Value::ObjectPath(characteristic.path.clone()),
                    interfaces_of(vec![(
                        interfaces::CHARACTERISTIC,
                        properties(vec![
                            ("UUID", Value::Str(characteristic.uuid.as_str().to_owned())),
                            ("Service", Value::ObjectPath(service.path.clone())),
                            (
                                "Flags",
                                Value::string_array(characteristic.flags.iter().cloned()),
                            ),
                        ]),
                    )]),
                ));
                for descriptor in &characteristic.descriptors {
                    objects.push((
                        Value::ObjectPath(descriptor.path.clone()),
                        interfaces_of(vec![(
                            interfaces::DESCRIPTOR,
                            properties(vec![
                                ("UUID", Value::Str(descriptor.uuid.as_str().to_owned())),
                                (
                                    "Characteristic",
                                    Value::ObjectPath(characteristic.path.clone()),
                                ),
                                ("Flags", Value::string_array(["read".to_string()])),
                            ]),
                        )]),
                    ));
                }
            }
        }
        dict_of("{oa{sa{sv}}}", objects)
    }
}

fn dict_of(element: &str, entries: Vec<(Value, Value)>) -> Value {
    Value::Array {
        element: element.into(),
        items: entries
            .into_iter()
            .map(|(k, v)| Value::DictEntry(Box::new(k), Box::new(v)))
            .collect(),
    }
}

/// `a{sv}` — a property dictionary, whose values are variants.
fn properties(entries: Vec<(&str, Value)>) -> Value {
    dict_of(
        "{sv}",
        entries
            .into_iter()
            .map(|(name, value)| (Value::Str(name.to_owned()), Value::Variant(Box::new(value))))
            .collect(),
    )
}

/// `a{sa{sv}}` — one object's interfaces and their properties.
fn interfaces_of(entries: Vec<(&str, Value)>) -> Value {
    dict_of(
        "{sa{sv}}",
        entries
            .into_iter()
            .map(|(interface, props)| (Value::Str(interface.to_owned()), props))
            .collect(),
    )
}

/// Serves our whole object tree. One handler, because D-Bus dispatch matches by
/// path prefix and every object we export lives under [`ROOT`].
struct Application {
    inner: Arc<Inner>,
}

impl ObjectHandler for Application {
    fn handle(&self, call: &Message) -> std::result::Result<Vec<Value>, (String, String)> {
        let path = call.path.clone().unwrap_or_default();
        let interface = call.interface.clone().unwrap_or_default();
        let member = call.member.clone().unwrap_or_default();

        match (interface.as_str(), member.as_str()) {
            (interfaces::OBJECT_MANAGER, "GetManagedObjects") => {
                Ok(vec![self.inner.managed_objects()])
            }

            (DBUS_PROPERTIES, "GetAll") => Ok(vec![self.properties_of(&path)]),

            (DBUS_PROPERTIES, "Get") => {
                let name = call.arg(1).and_then(Value::as_str).unwrap_or_default();
                self.property(&path, name)
                    .map(|v| vec![Value::Variant(Box::new(v))])
                    .ok_or_else(|| {
                        (
                            "org.freedesktop.DBus.Error.InvalidArgs".to_string(),
                            format!("no property {name} at {path}"),
                        )
                    })
            }

            (interfaces::CHARACTERISTIC, "ReadValue") => self.read(&path, call.arg(0)),
            (interfaces::CHARACTERISTIC, "WriteValue") => {
                self.write(&path, call.arg(0), call.arg(1))
            }
            (interfaces::CHARACTERISTIC, "StartNotify") => self.set_subscribed(&path, true),
            (interfaces::CHARACTERISTIC, "StopNotify") => self.set_subscribed(&path, false),

            // A descriptor's value is fixed at publication, so it needs no
            // round trip to the application.
            (interfaces::DESCRIPTOR, "ReadValue") => match self.inner.descriptor_at(&path) {
                Some((_, value)) => Ok(vec![Value::bytes(&value)]),
                None => Err((
                    "org.bluez.Error.Failed".to_string(),
                    format!("no descriptor at {path}"),
                )),
            },

            // Registered advertisements are released rather than unregistered
            // when BlueZ drops them.
            (interfaces::LE_ADVERTISEMENT, "Release") => Ok(Vec::new()),

            _ => Err((
                "org.freedesktop.DBus.Error.UnknownMethod".to_string(),
                format!("{interface}.{member} is not implemented"),
            )),
        }
    }
}

impl Application {
    fn properties_of(&self, path: &str) -> Value {
        if path == ADVERTISEMENT {
            // BlueZ reads the advertisement's properties rather than taking
            // them as arguments to RegisterAdvertisement.
            return dict_of("{sv}", self.inner.advertisement.lock().unwrap().clone());
        }
        if let Some(characteristic) = self.inner.characteristic_at(path) {
            return properties(vec![
                ("UUID", Value::Str(characteristic.uuid.as_str().to_owned())),
                (
                    "Flags",
                    Value::string_array(characteristic.flags.iter().cloned()),
                ),
                ("Value", Value::bytes(&characteristic.value.lock().unwrap())),
            ]);
        }
        properties(Vec::new())
    }

    fn property(&self, path: &str, name: &str) -> Option<Value> {
        let characteristic = self.inner.characteristic_at(path)?;
        match name {
            "UUID" => Some(Value::Str(characteristic.uuid.as_str().to_owned())),
            "Value" => Some(Value::bytes(&characteristic.value.lock().unwrap())),
            "Flags" => Some(Value::string_array(characteristic.flags.iter().cloned())),
            _ => None,
        }
    }

    /// Hand a read to the application and park until it answers.
    fn read(
        &self,
        path: &str,
        options: Option<&Value>,
    ) -> std::result::Result<Vec<Value>, (String, String)> {
        let Some(characteristic) = self.inner.characteristic_at(path) else {
            return Err((
                "org.bluez.Error.Failed".to_string(),
                format!("no characteristic at {path}"),
            ));
        };
        let central = RemoteCentral::from_options(options, DEFAULT_NOTIFY_LEN);
        let offset = option_u64(options, "offset").unwrap_or(0) as usize;

        let (tx, rx) = channel();
        let request = Request::Read(ReadRequest {
            characteristic: characteristic.uuid,
            central,
            offset,
            answer: Some(tx),
        });
        if self.inner.to_app.send(request).is_err() {
            // Nobody is listening, so the last value is the best answer there
            // is — which is also what a static characteristic would give.
            return Ok(vec![Value::bytes(&characteristic.value.lock().unwrap())]);
        }

        match rx.recv_timeout(ANSWER_TIMEOUT) {
            Ok(Ok(value)) => {
                *characteristic.value.lock().unwrap() = value.clone();
                Ok(vec![Value::bytes(&value)])
            }
            Ok(Err(error)) => Err((att_error_name(error), format!("read refused: {error:?}"))),
            Err(reason) => Err((
                "org.bluez.Error.Failed".to_string(),
                format!("the application did not answer the read: {reason}"),
            )),
        }
    }

    fn write(
        &self,
        path: &str,
        value: Option<&Value>,
        options: Option<&Value>,
    ) -> std::result::Result<Vec<Value>, (String, String)> {
        let Some(characteristic) = self.inner.characteristic_at(path) else {
            return Err((
                "org.bluez.Error.Failed".to_string(),
                format!("no characteristic at {path}"),
            ));
        };
        let written: Vec<u8> = value.and_then(Value::as_bytes).unwrap_or_default();
        let central = RemoteCentral::from_options(options, DEFAULT_NOTIFY_LEN);
        let offset = option_u64(options, "offset").unwrap_or(0) as usize;

        let (tx, rx) = channel();
        let request = Request::Write(WriteRequest {
            writes: vec![Write {
                characteristic: characteristic.uuid,
                offset,
                value: written.clone(),
            }],
            central,
            answer: Some(tx),
        });
        if self.inner.to_app.send(request).is_err() {
            *characteristic.value.lock().unwrap() = written;
            return Ok(Vec::new());
        }

        match rx.recv_timeout(ANSWER_TIMEOUT) {
            Ok(Ok(())) => {
                *characteristic.value.lock().unwrap() = written;
                Ok(Vec::new())
            }
            Ok(Err(error)) => Err((att_error_name(error), format!("write refused: {error:?}"))),
            Err(reason) => Err((
                "org.bluez.Error.Failed".to_string(),
                format!("the application did not answer the write: {reason}"),
            )),
        }
    }

    /// `StartNotify` / `StopNotify`. BlueZ does not say which central, so the
    /// subscription is recorded against the connected device it knows about.
    fn set_subscribed(
        &self,
        path: &str,
        subscribed: bool,
    ) -> std::result::Result<Vec<Value>, (String, String)> {
        let Some(characteristic) = self.inner.characteristic_at(path) else {
            return Err((
                "org.bluez.Error.Failed".to_string(),
                format!("no characteristic at {path}"),
            ));
        };
        // BlueZ passes no options to StartNotify, so there is no MTU or device
        // to read: the subscriber is described as far as it can be.
        let central = RemoteCentral {
            id: "subscribed".to_string(),
            mtu: DEFAULT_NOTIFY_LEN,
        };
        {
            let mut subscribers = characteristic.subscribers.lock().unwrap();
            if subscribed {
                subscribers.push(central.clone());
            } else {
                subscribers.clear();
            }
        }
        let request = if subscribed {
            Request::Subscribed {
                central,
                characteristic: characteristic.uuid,
            }
        } else {
            Request::Unsubscribed {
                central,
                characteristic: characteristic.uuid,
            }
        };
        let _ = self.inner.to_app.send(request);
        Ok(Vec::new())
    }
}

/// The ATT error names BlueZ recognises on a GATT reply.
fn att_error_name(error: AttError) -> String {
    // BlueZ maps its own error names back onto ATT codes, so a refusal reaches
    // the central as the error the application asked for rather than a generic
    // failure. Anything without a name falls back to `Failed`, which BlueZ
    // sends as `UnlikelyError`.
    match error {
        AttError::ReadNotPermitted | AttError::WriteNotPermitted => "org.bluez.Error.NotPermitted",
        AttError::InsufficientAuthentication | AttError::InsufficientEncryption => {
            "org.bluez.Error.NotPaired"
        }
        AttError::InsufficientAuthorization => "org.bluez.Error.NotAuthorized",
        AttError::RequestNotSupported => "org.bluez.Error.NotSupported",
        AttError::InvalidOffset => "org.bluez.Error.InvalidOffset",
        AttError::InvalidAttributeValueLength => "org.bluez.Error.InvalidValueLength",
        _ => "org.bluez.Error.Failed",
    }
    .to_string()
}

/// Discards BlueZ's central-role events; a peripheral has no use for them.
struct Discard;
impl EventSink for Discard {
    fn emit(&self, _event: Event) {}
}

// ── Peripheral ──────────────────────────────────────────────────────────────

/// A local GATT server, published through `bluetoothd`.
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
        // three things wants the first three, not the last three.
        let (to_app, rx) = webbluetooth_core::backlog::bounded(
            REQUEST_BACKLOG,
            webbluetooth_core::backlog::Overflow::KeepOldest,
        );
        let bluez = Bluez::new(Arc::new(Discard)).ok().map(Arc::new);
        let inner = Arc::new(Inner {
            bluez,
            to_app,
            services: Mutex::new(Vec::new()),
            registered: AtomicBool::new(false),
            advertising: AtomicBool::new(false),
            next_index: AtomicUsize::new(0),
            advertisement: Mutex::new(Vec::new()),
        });
        if let Some(bluez) = &inner.bluez {
            bluez.export(
                ROOT,
                Arc::new(Application {
                    inner: inner.clone(),
                }),
            );
        }
        (Self { inner }, Requests { rx })
    }

    /// Linux has no state restoration; this is [`Peripheral::new`].
    pub fn with_restoration(_restoration: webbluetooth_core::Restoration) -> (Self, Requests) {
        Self::new()
    }

    /// Why the peripheral role is not usable, if it is not.
    pub async fn availability(&self) -> std::result::Result<(), Availability> {
        let Some(bluez) = &self.inner.bluez else {
            return Err(Availability::Unauthorized);
        };
        if bluez.adapter().is_none() {
            return Err(Availability::Unsupported);
        }
        if !bluez.is_powered() {
            return Err(Availability::PoweredOff);
        }
        Ok(())
    }

    /// Publish a service into the local GATT database.
    ///
    /// The first call registers the application with BlueZ; later ones
    /// re-register it, because `GattManager1` has no way to add a service to an
    /// application it has already walked.
    pub async fn publish(&self, service: Service) -> Result<PublishedService> {
        service.validate()?;
        let (bluez, adapter) = self.inner.adapter()?;
        if !bluez.is_powered() {
            return Err(Error::NotAvailable(Availability::PoweredOff));
        }

        let index = self.inner.next_index.fetch_add(1, Ordering::Relaxed);
        let service_path = format!("{ROOT}/service{index}");
        let mut characteristics = Vec::new();

        for (i, definition) in service.characteristics.iter().enumerate() {
            let path = format!("{service_path}/char{i}");
            let descriptors = definition
                .descriptors
                .iter()
                .enumerate()
                .map(|(j, d)| DescriptorState {
                    uuid: d.uuid,
                    path: format!("{path}/desc{j}"),
                    value: d.value.clone(),
                })
                .collect();
            characteristics.push(Arc::new(CharacteristicState {
                uuid: definition.uuid,
                path,
                flags: flags_for(Properties(definition.properties)),
                value: Mutex::new(definition.value.clone().unwrap_or_default()),
                subscribers: Mutex::new(Vec::new()),
                descriptors,
            }));
        }

        let state = Arc::new(ServiceState {
            uuid: service.uuid,
            path: service_path,
            primary: service.primary,
            characteristics,
        });
        self.inner.services.lock().unwrap().push(state.clone());

        // Re-register so BlueZ walks the tree again and sees the addition.
        if self.inner.registered.swap(true, Ordering::SeqCst) {
            let _ = call(
                &bluez,
                &adapter,
                interfaces::GATT_MANAGER,
                "UnregisterApplication",
                vec![Value::ObjectPath(ROOT.to_string())],
            );
        }
        call(
            &bluez,
            &adapter,
            interfaces::GATT_MANAGER,
            "RegisterApplication",
            vec![Value::ObjectPath(ROOT.to_string()), properties(Vec::new())],
        )
        .inspect_err(|_| {
            // A service BlueZ refused must not stay in the tree, or the next
            // registration fails the same way.
            self.inner
                .services
                .lock()
                .unwrap()
                .retain(|s| s.path != state.path);
            self.inner.registered.store(false, Ordering::SeqCst);
        })?;

        Ok(PublishedService {
            inner: self.inner.clone(),
            state,
        })
    }

    /// Withdraw every published service.
    pub fn unpublish_all(&self) {
        self.inner.services.lock().unwrap().clear();
        if self.inner.registered.swap(false, Ordering::SeqCst) {
            if let Ok((bluez, adapter)) = self.inner.adapter() {
                let _ = call(
                    &bluez,
                    &adapter,
                    interfaces::GATT_MANAGER,
                    "UnregisterApplication",
                    vec![Value::ObjectPath(ROOT.to_string())],
                );
            }
        }
    }

    /// Start advertising.
    pub async fn start_advertising(&self, advertising: Advertising) -> Result<()> {
        let (bluez, adapter) = self.inner.adapter()?;
        if self.inner.advertising.load(Ordering::SeqCst) {
            self.stop_advertising();
        }

        // The advertisement is its own object, and BlueZ reads its properties
        // rather than taking them as call arguments.
        let mut entries: Vec<(Value, Value)> = vec![(
            Value::Str("Type".into()),
            Value::Variant(Box::new(Value::Str("peripheral".into()))),
        )];
        if !advertising.services.is_empty() {
            entries.push((
                Value::Str("ServiceUUIDs".into()),
                Value::Variant(Box::new(Value::string_array(
                    advertising.services.iter().map(|u| u.as_str().to_owned()),
                ))),
            ));
        }
        if let Some(name) = &advertising.local_name {
            entries.push((
                Value::Str("LocalName".into()),
                Value::Variant(Box::new(Value::Str(name.clone()))),
            ));
        }
        *self.inner.advertisement.lock().unwrap() = entries;

        bluez.export(
            ADVERTISEMENT,
            Arc::new(Application {
                inner: self.inner.clone(),
            }),
        );
        call(
            &bluez,
            &adapter,
            interfaces::LE_ADVERTISING_MANAGER,
            "RegisterAdvertisement",
            vec![
                Value::ObjectPath(ADVERTISEMENT.to_string()),
                properties(Vec::new()),
            ],
        )?;
        self.inner.advertising.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Stop advertising.
    pub fn stop_advertising(&self) {
        if !self.inner.advertising.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Ok((bluez, adapter)) = self.inner.adapter() {
            let _ = call(
                &bluez,
                &adapter,
                interfaces::LE_ADVERTISING_MANAGER,
                "UnregisterAdvertisement",
                vec![Value::ObjectPath(ADVERTISEMENT.to_string())],
            );
        }
    }

    pub fn is_advertising(&self) -> bool {
        self.inner.advertising.load(Ordering::SeqCst)
    }

    /// Not available through BlueZ's GATT manager.
    pub async fn publish_l2cap_channel(
        &self,
        _encryption_required: bool,
    ) -> Result<webbluetooth_core::Psm> {
        Err(Error::NotSupported(
            "BlueZ has no D-Bus API for publishing an L2CAP channel; a peripheral \
             would have to listen on an L2CAP socket itself"
                .into(),
        ))
    }

    pub async fn unpublish_l2cap_channel(&self, _psm: webbluetooth_core::Psm) -> Result<()> {
        Err(Error::NotSupported(
            "no L2CAP channel can be published on this platform".into(),
        ))
    }

    /// Linux preserves nothing across a relaunch.
    pub async fn restored_state(&self) -> Option<RestoredPeripheral> {
        None
    }
}

impl std::fmt::Debug for Peripheral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peripheral")
            .field("services", &self.inner.services.lock().unwrap().len())
            .field("advertising", &self.is_advertising())
            .finish()
    }
}

fn call(
    bluez: &Arc<Bluez>,
    path: &str,
    interface: &str,
    member: &str,
    body: Vec<Value>,
) -> Result<Message> {
    bluez
        .call(path, interface, member, body)
        .map_err(|e| Error::Network(format!("{interface}.{member} failed: {e}")))
}

// ── Published objects ───────────────────────────────────────────────────────

/// A service that is live in the local GATT database.
pub struct PublishedService {
    inner: Arc<Inner>,
    state: Arc<ServiceState>,
}

impl PublishedService {
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.state.uuid
    }

    pub fn characteristics(&self) -> Vec<PublishedCharacteristic> {
        self.state
            .characteristics
            .iter()
            .map(|c| PublishedCharacteristic {
                inner: self.inner.clone(),
                state: c.clone(),
            })
            .collect()
    }

    pub fn characteristic(&self, uuid: impl IntoUuid) -> Option<PublishedCharacteristic> {
        let uuid = uuid.into_uuid().ok()?;
        self.state
            .characteristics
            .iter()
            .find(|c| c.uuid == uuid)
            .map(|c| PublishedCharacteristic {
                inner: self.inner.clone(),
                state: c.clone(),
            })
    }

    /// Remove just this service.
    ///
    /// BlueZ has no way to withdraw one service from a registered application,
    /// so the application is re-registered without it.
    pub fn unpublish(self) {
        self.inner
            .services
            .lock()
            .unwrap()
            .retain(|s| s.path != self.state.path);
        if let Ok((bluez, adapter)) = self.inner.adapter() {
            let _ = call(
                &bluez,
                &adapter,
                interfaces::GATT_MANAGER,
                "UnregisterApplication",
                vec![Value::ObjectPath(ROOT.to_string())],
            );
            let empty = self.inner.services.lock().unwrap().is_empty();
            if empty {
                self.inner.registered.store(false, Ordering::SeqCst);
            } else {
                let _ = call(
                    &bluez,
                    &adapter,
                    interfaces::GATT_MANAGER,
                    "RegisterApplication",
                    vec![Value::ObjectPath(ROOT.to_string()), properties(Vec::new())],
                );
            }
        }
    }
}

impl std::fmt::Debug for PublishedService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishedService")
            .field("uuid", &self.state.uuid.as_str())
            .field("characteristics", &self.state.characteristics.len())
            .finish()
    }
}

/// A characteristic that is live in the local GATT database.
#[derive(Clone)]
pub struct PublishedCharacteristic {
    inner: Arc<Inner>,
    state: Arc<CharacteristicState>,
}

impl PublishedCharacteristic {
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.state.uuid
    }

    /// The centrals BlueZ has told us are subscribed.
    ///
    /// BlueZ reports that *someone* subscribed, not who: `StartNotify` carries
    /// no device. The entries here therefore stand for the subscription rather
    /// than identifying a peer.
    pub fn subscribers(&self) -> Vec<RemoteCentral> {
        self.state.subscribers.lock().unwrap().clone()
    }

    /// Push a value to whoever is subscribed, without waiting.
    ///
    /// There is no "notify" call in BlueZ's GATT API. A subscribed central is
    /// sent a value by emitting `PropertiesChanged` for this characteristic's
    /// `Value`; BlueZ decides from the flags whether that goes out as a
    /// notification or an indication.
    pub fn try_notify(&self, value: &[u8]) -> bool {
        *self.state.value.lock().unwrap() = value.to_vec();
        if self.state.subscribers.lock().unwrap().is_empty() {
            return false;
        }
        let Some(bluez) = &self.inner.bluez else {
            return false;
        };
        let signal = Message::signal(&self.state.path, DBUS_PROPERTIES, "PropertiesChanged")
            .with_body(vec![
                Value::Str(interfaces::CHARACTERISTIC.to_string()),
                properties(vec![("Value", Value::bytes(value))]),
                Value::string_array(Vec::<String>::new()),
            ]);
        bluez.connection().send(signal).is_ok()
    }

    /// Push a value to whoever is subscribed.
    pub async fn notify(&self, value: &[u8]) -> Result<()> {
        if self.try_notify(value) {
            return Ok(());
        }
        if self.state.subscribers.lock().unwrap().is_empty() {
            return Err(Error::InvalidState(format!(
                "nothing is subscribed to {}",
                self.state.uuid
            )));
        }
        Err(Error::Network(
            "could not emit the value change on the bus".into(),
        ))
    }
}

impl std::fmt::Debug for PublishedCharacteristic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishedCharacteristic")
            .field("uuid", &self.state.uuid.as_str())
            .field("subscribers", &self.subscribers().len())
            .finish()
    }
}
