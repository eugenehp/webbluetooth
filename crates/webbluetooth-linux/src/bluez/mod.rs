//! The BlueZ object model.
//!
//! BlueZ publishes the whole Bluetooth world as a D-Bus object tree and expects
//! clients to track it through `ObjectManager`:
//!
//! ```text
//!   /org/bluez                                        org.bluez.AgentManager1
//!   /org/bluez/hci0                                   org.bluez.Adapter1
//!                                                     org.bluez.LEAdvertisingManager1
//!                                                     org.bluez.GattManager1
//!   /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF             org.bluez.Device1
//!   …/dev_AA_BB_CC_DD_EE_FF/service0010               org.bluez.GattService1
//!   …/service0010/char0011                            org.bluez.GattCharacteristic1
//!   …/char0011/desc0013                               org.bluez.GattDescriptor1
//! ```
//!
//! So an object *path* is the handle, and the hierarchy is encoded in it: a
//! characteristic's service is its parent path. That is the opposite of
//! CoreBluetooth, where handles are opaque pointers and the tree is walked by
//! asking. It is also why this layer keeps a cached mirror of the tree — every
//! lookup would otherwise be a round trip.
//!
//! # Where this differs from CoreBluetooth
//!
//! * **Discovery is not a request/reply.** `StartDiscovery` returns immediately
//!   and devices appear as `InterfacesAdded` signals. Same as CoreBluetooth.
//! * **GATT reads are.** `ReadValue` returns the bytes in its method reply, so
//!   there is no read-versus-notification ambiguity to resolve — the problem
//!   [`crate::dbus`]'s Apple counterpart needs a FIFO for does not exist here.
//! * **Notifications are property changes.** `StartNotify` then
//!   `PropertiesChanged` on `Value`.
//! * **Services are not discovered explicitly.** Connecting resolves them and
//!   sets `ServicesResolved`; the objects simply appear in the tree.

use crate::dbus::connection::{Error as DbusError, ObjectHandler};
use crate::dbus::{Connection, Message, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub mod interfaces {
    pub const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
    pub const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
    pub const ADAPTER: &str = "org.bluez.Adapter1";
    pub const DEVICE: &str = "org.bluez.Device1";
    pub const SERVICE: &str = "org.bluez.GattService1";
    pub const CHARACTERISTIC: &str = "org.bluez.GattCharacteristic1";
    pub const DESCRIPTOR: &str = "org.bluez.GattDescriptor1";
    pub const GATT_MANAGER: &str = "org.bluez.GattManager1";
    pub const LE_ADVERTISING_MANAGER: &str = "org.bluez.LEAdvertisingManager1";
    pub const LE_ADVERTISEMENT: &str = "org.bluez.LEAdvertisement1";
}

pub const SERVICE_NAME: &str = "org.bluez";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub type Result<T> = std::result::Result<T, DbusError>;

/// A D-Bus object path — the Linux equivalent of a CoreBluetooth handle.
///
/// Cheap to clone and hash, and unlike a pointer it stays meaningful across a
/// disconnect, which is why the layer above still needs a generation counter to
/// know when a path has gone stale.
pub type Path = Arc<str>;

pub fn path(s: &str) -> Path {
    Arc::from(s)
}

/// One object in BlueZ's tree: its interfaces and their properties.
#[derive(Debug, Clone, Default)]
pub struct Object {
    pub interfaces: HashMap<String, HashMap<String, Value>>,
}

impl Object {
    pub fn has(&self, interface: &str) -> bool {
        self.interfaces.contains_key(interface)
    }

    pub fn property(&self, interface: &str, name: &str) -> Option<&Value> {
        self.interfaces.get(interface)?.get(name)
    }

    pub fn string(&self, interface: &str, name: &str) -> Option<String> {
        self.property(interface, name)?.as_str().map(str::to_owned)
    }

    pub fn bool(&self, interface: &str, name: &str) -> Option<bool> {
        self.property(interface, name)?.as_bool()
    }

    pub fn int(&self, interface: &str, name: &str) -> Option<i64> {
        self.property(interface, name)?.as_i64()
    }
}

/// What the tree tells us happened.
#[derive(Debug, Clone)]
pub enum Event {
    /// The adapter's `Powered` changed, or it appeared/vanished.
    AdapterChanged { powered: bool, present: bool },
    /// A device object appeared, or re-advertised with new properties.
    DeviceSeen {
        path: Path,
        properties: HashMap<String, Value>,
    },
    /// A device object vanished — BlueZ removes stale ones after a scan.
    DeviceRemoved { path: Path },
    /// `Connected` became true.
    Connected { path: Path },
    /// `Connected` became false.
    Disconnected { path: Path },
    /// `ServicesResolved` became true: the GATT tree is now populated.
    ServicesResolved { path: Path },
    /// A characteristic's `Value` changed — a notification or indication.
    CharacteristicValue { path: Path, value: Vec<u8> },
    /// A `GattService1` object appeared under or vanished from a device.
    ///
    /// This is the tree's account of the Service Changed indication. BlueZ
    /// subscribes to that characteristic itself and re-discovers, so what
    /// reaches us is the result rather than the indication: services arriving
    /// and leaving under the device's path. `path` is the device, not the
    /// service.
    ServicesChanged { path: Path },
    /// The whole bus went away.
    Disconnected_,
}

/// Where tree changes are delivered. Called on the D-Bus reader thread.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

/// A connection to BlueZ, with a cached mirror of its object tree.
pub struct Bluez {
    connection: Arc<Connection>,
    tree: Arc<Mutex<HashMap<String, Object>>>,
    adapter: Mutex<Option<String>>,
    /// Held so the signal handler's clone is not the only reference.
    _sink: Arc<dyn EventSink>,
}

impl Bluez {
    /// Connect to the system bus, subscribe to the tree, and load it.
    pub fn new(sink: Arc<dyn EventSink>) -> Result<Self> {
        let connection = Arc::new(Connection::system()?);
        let tree = Arc::new(Mutex::new(HashMap::new()));

        let bluez = Self {
            connection: connection.clone(),
            tree: tree.clone(),
            adapter: Mutex::new(None),
            _sink: sink.clone(),
        };

        // Subscribe *before* the initial load, so nothing that happens during
        // it is missed. Duplicate events are harmless; a lost one is not.
        let handler_tree = tree.clone();
        let handler_sink = sink.clone();
        connection.add_match(
            "type='signal',sender='org.bluez'",
            Arc::new(move |m: &Message| {
                on_signal(&handler_tree, &handler_sink, m);
            }),
        )?;

        bluez.reload()?;
        Ok(bluez)
    }

    /// The underlying bus, for the peripheral role's object exports.
    pub fn connection(&self) -> &Arc<Connection> {
        &self.connection
    }

    /// Re-read the whole tree with `GetManagedObjects`.
    pub fn reload(&self) -> Result<()> {
        let reply = self.connection.call_blocking(
            Message::call(
                SERVICE_NAME,
                "/",
                interfaces::OBJECT_MANAGER,
                "GetManagedObjects",
            ),
            DEFAULT_TIMEOUT,
        )?;

        let mut tree = HashMap::new();
        if let Some(objects) = reply.arg(0) {
            for (object_path, interfaces) in objects.as_map() {
                tree.insert(
                    object_path,
                    Object {
                        interfaces: to_interfaces(&interfaces),
                    },
                );
            }
        }
        *self.tree.lock().unwrap() = tree;

        // Keep whichever adapter was chosen, if it is still there. Re-reading
        // the tree must not silently move an established session to a
        // different controller.
        let adapters = self.adapters();
        let current = self.adapter.lock().unwrap().clone();
        let keep = current.filter(|c| adapters.contains(c));

        *self.adapter.lock().unwrap() = keep.or_else(|| {
            // `WEBBLUETOOTH_ADAPTER` names one explicitly — "hci1" or a full
            // object path — which is how a machine with several controllers
            // picks, and how a two-adapter test puts each end on its own.
            if let Ok(want) = std::env::var("WEBBLUETOOTH_ADAPTER") {
                let wanted = Self::adapter_path(&want);
                if let Some(found) = adapters.iter().find(|p| **p == wanted) {
                    return Some(found.clone());
                }
            }
            // Otherwise the first, which sorts to hci0 — what every other tool
            // defaults to.
            adapters.into_iter().next()
        });
        Ok(())
    }

    /// `hci1` or `/org/bluez/hci1` both name the same adapter.
    fn adapter_path(name: &str) -> String {
        if name.starts_with('/') {
            name.to_owned()
        } else {
            format!("/org/bluez/{name}")
        }
    }

    /// Every adapter BlueZ is managing, sorted, so `hci0` comes first.
    pub fn adapters(&self) -> Vec<String> {
        let mut adapters: Vec<String> = self
            .tree
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, o)| o.has(interfaces::ADAPTER))
            .map(|(p, _)| p.clone())
            .collect();
        adapters.sort();
        adapters
    }

    /// Use a named adapter for everything from here on.
    ///
    /// Takes `hci1` or a full object path. Fails if BlueZ is not managing it,
    /// rather than silently carrying on with a different one.
    pub fn select_adapter(&self, name: &str) -> Result<()> {
        let wanted = Self::adapter_path(name);
        let adapters = self.adapters();
        if !adapters.contains(&wanted) {
            return Err(DbusError::Call {
                name: "org.bluez.Error.NoAdapter".into(),
                message: format!("no adapter at {wanted}; BlueZ has {}", adapters.join(", ")),
            });
        }
        *self.adapter.lock().unwrap() = Some(wanted);
        Ok(())
    }

    /// The local adapter's own Bluetooth address, as BlueZ reports it.
    pub fn adapter_address(&self) -> Option<String> {
        let adapter = self.adapter()?;
        self.object(&adapter)?
            .string(interfaces::ADAPTER, "Address")
    }

    /// The adapter object path, if one exists.
    pub fn adapter(&self) -> Option<String> {
        self.adapter.lock().unwrap().clone()
    }

    /// Whether an adapter exists and is powered.
    pub fn is_powered(&self) -> bool {
        let Some(adapter) = self.adapter() else {
            return false;
        };
        self.object(&adapter)
            .and_then(|o| o.bool(interfaces::ADAPTER, "Powered"))
            .unwrap_or(false)
    }

    /// Turn the adapter on.
    pub fn set_powered(&self, on: bool) -> Result<()> {
        let Some(adapter) = self.adapter() else {
            return Err(DbusError::Call {
                name: "org.bluez.Error.NoAdapter".into(),
                message: "no Bluetooth adapter present".into(),
            });
        };
        self.set_property(&adapter, interfaces::ADAPTER, "Powered", Value::Bool(on))
    }

    /// A snapshot of one object.
    pub fn object(&self, path: &str) -> Option<Object> {
        self.tree.lock().unwrap().get(path).cloned()
    }

    /// Every object path carrying `interface`.
    pub fn paths_with(&self, interface: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .tree
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, o)| o.has(interface))
            .map(|(p, _)| p.clone())
            .collect();
        out.sort();
        out
    }

    /// Children of `parent` carrying `interface` — how the GATT tree is walked,
    /// since BlueZ encodes the hierarchy in the path.
    pub fn children_with(&self, parent: &str, interface: &str) -> Vec<String> {
        let prefix = format!("{parent}/");
        let mut out: Vec<String> = self
            .tree
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, o)| p.starts_with(&prefix) && o.has(interface))
            .map(|(p, _)| p.clone())
            .collect();
        out.sort();
        out
    }

    /// Read a property live, bypassing the cache.
    pub fn get_property(&self, path: &str, interface: &str, name: &str) -> Result<Value> {
        let reply = self.connection.call_blocking(
            Message::call(SERVICE_NAME, path, interfaces::PROPERTIES, "Get")
                .with_body(vec![Value::Str(interface.into()), Value::Str(name.into())]),
            DEFAULT_TIMEOUT,
        )?;
        Ok(reply.arg(0).cloned().unwrap_or(Value::Byte(0)))
    }

    /// Write a property.
    pub fn set_property(
        &self,
        path: &str,
        interface: &str,
        name: &str,
        value: Value,
    ) -> Result<()> {
        self.connection.call_blocking(
            Message::call(SERVICE_NAME, path, interfaces::PROPERTIES, "Set").with_body(vec![
                Value::Str(interface.into()),
                Value::Str(name.into()),
                Value::Variant(Box::new(value)),
            ]),
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    /// Call a BlueZ method, delivering the reply to a callback.
    ///
    /// Callback-based rather than blocking, so the async layer can wrap it
    /// without a thread per request — the same shape as the Apple backend.
    pub fn call_async(
        &self,
        path: &str,
        interface: &str,
        member: &str,
        args: Vec<Value>,
        then: impl FnOnce(Result<Message>) + Send + 'static,
    ) {
        self.connection.call_async(
            Message::call(SERVICE_NAME, path, interface, member).with_body(args),
            Box::new(then),
        );
    }

    /// As [`Bluez::call_async`], keeping any descriptors the reply carried.
    ///
    /// For `AcquireWrite` and `AcquireNotify`, whose answer is a socket. The
    /// body holds only an index; the descriptor travelled beside the message.
    pub fn call_async_with_fds(
        &self,
        path: &str,
        interface: &str,
        member: &str,
        args: Vec<Value>,
        then: impl FnOnce(Result<(Vec<Value>, Vec<std::os::fd::RawFd>)>) + Send + 'static,
    ) {
        let connection = self.connection.clone();
        self.connection.call_async(
            Message::call(SERVICE_NAME, path, interface, member).with_body(args),
            Box::new(move |reply| {
                then(reply.map(|m| {
                    // Claimed by the serial the reply answers, which is what
                    // the reader filed them under.
                    let fds = m
                        .reply_serial
                        .map(|s| connection.take_fds(s))
                        .unwrap_or_default();
                    (m.body, fds)
                }))
            }),
        );
    }

    /// Call a BlueZ method and wait.
    pub fn call(
        &self,
        path: &str,
        interface: &str,
        member: &str,
        args: Vec<Value>,
    ) -> Result<Message> {
        self.connection.call_blocking(
            Message::call(SERVICE_NAME, path, interface, member).with_body(args),
            DEFAULT_TIMEOUT,
        )
    }

    /// Export an object so BlueZ can call into it — the peripheral role.
    pub fn export(&self, path: &str, handler: Arc<dyn ObjectHandler>) {
        self.connection.export(path, handler);
    }
}

/// Convert an `a{sa{sv}}` interfaces dictionary into a plain map.
fn to_interfaces(value: &Value) -> HashMap<String, HashMap<String, Value>> {
    value
        .as_map()
        .into_iter()
        .map(|(name, props)| (name, props.as_map()))
        .collect()
}

/// The device object path a GATT service hangs off.
///
/// BlueZ nests the tree in the path itself — a service is
/// `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/service001c` — so the owner is the
/// parent. Returns `None` for a path with no parent rather than guessing.
fn device_of(service_path: &str) -> Option<&str> {
    let (device, _) = service_path.rsplit_once('/')?;
    (!device.is_empty()).then_some(device)
}

/// Apply a signal to the cached tree and tell the sink what changed.
fn on_signal(
    tree: &Arc<Mutex<HashMap<String, Object>>>,
    sink: &Arc<dyn EventSink>,
    message: &Message,
) {
    match (message.interface.as_deref(), message.member.as_deref()) {
        (Some(interfaces::OBJECT_MANAGER), Some("InterfacesAdded")) => {
            let (Some(object_path), Some(interfaces)) = (
                message.arg(0).and_then(Value::as_str).map(str::to_owned),
                message.arg(1),
            ) else {
                return;
            };
            let added = to_interfaces(interfaces);
            {
                let mut tree = tree.lock().unwrap();
                let entry = tree.entry(object_path.clone()).or_default();
                for (name, props) in &added {
                    entry.interfaces.insert(name.clone(), props.clone());
                }
            }
            if let Some(props) = added.get(interfaces::DEVICE) {
                sink.emit(Event::DeviceSeen {
                    path: path(&object_path),
                    properties: props.clone(),
                });
            }
            if added.contains_key(interfaces::SERVICE) {
                if let Some(device) = device_of(&object_path) {
                    sink.emit(Event::ServicesChanged { path: path(device) });
                }
            }
        }

        (Some(interfaces::OBJECT_MANAGER), Some("InterfacesRemoved")) => {
            let (Some(object_path), Some(removed)) = (
                message.arg(0).and_then(Value::as_str).map(str::to_owned),
                message.arg(1),
            ) else {
                return;
            };
            let names = removed.as_strings();
            let now_empty = {
                let mut tree = tree.lock().unwrap();
                if let Some(entry) = tree.get_mut(&object_path) {
                    for n in &names {
                        entry.interfaces.remove(n);
                    }
                    if entry.interfaces.is_empty() {
                        tree.remove(&object_path);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if names.iter().any(|n| n == interfaces::SERVICE) {
                if let Some(device) = device_of(&object_path) {
                    sink.emit(Event::ServicesChanged { path: path(device) });
                }
            }
            if now_empty || names.iter().any(|n| n == interfaces::DEVICE) {
                sink.emit(Event::DeviceRemoved {
                    path: path(&object_path),
                });
            }
        }

        (Some(interfaces::PROPERTIES), Some("PropertiesChanged")) => {
            let Some(object_path) = message.path.clone() else {
                return;
            };
            let Some(interface) = message.arg(0).and_then(Value::as_str).map(str::to_owned) else {
                return;
            };
            let changed = message.arg(1).map(Value::as_map).unwrap_or_default();
            let invalidated = message.arg(2).map(Value::as_strings).unwrap_or_default();

            {
                let mut tree = tree.lock().unwrap();
                let entry = tree.entry(object_path.clone()).or_default();
                let props = entry.interfaces.entry(interface.clone()).or_default();
                for (k, v) in &changed {
                    props.insert(k.clone(), v.clone());
                }
                for k in &invalidated {
                    props.remove(k);
                }
            }

            let handle = path(&object_path);
            match interface.as_str() {
                interfaces::DEVICE => {
                    if let Some(connected) = changed.get("Connected").and_then(Value::as_bool) {
                        sink.emit(if connected {
                            Event::Connected {
                                path: handle.clone(),
                            }
                        } else {
                            Event::Disconnected {
                                path: handle.clone(),
                            }
                        });
                    }
                    if changed.get("ServicesResolved").and_then(Value::as_bool) == Some(true) {
                        sink.emit(Event::ServicesResolved {
                            path: handle.clone(),
                        });
                    }
                    // RSSI updates are how a scan reports a device it already
                    // knows, so they count as sightings.
                    if changed.contains_key("RSSI") || changed.contains_key("ManufacturerData") {
                        sink.emit(Event::DeviceSeen {
                            path: handle,
                            properties: changed,
                        });
                    }
                }
                interfaces::CHARACTERISTIC => {
                    if let Some(bytes) = changed.get("Value").and_then(Value::as_bytes) {
                        sink.emit(Event::CharacteristicValue {
                            path: handle,
                            value: bytes,
                        });
                    }
                }
                interfaces::ADAPTER => {
                    if let Some(powered) = changed.get("Powered").and_then(Value::as_bool) {
                        sink.emit(Event::AdapterChanged {
                            powered,
                            present: true,
                        });
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_services_owner_is_its_parent_path() {
        assert_eq!(
            device_of("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/service001c"),
            Some("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF")
        );
        // A path with no parent is reported as such rather than guessed at,
        // so a malformed signal cannot be attributed to some other device.
        assert_eq!(device_of("/service001c"), None);
        assert_eq!(device_of("service001c"), None);
        assert_eq!(device_of(""), None);
    }

    #[test]
    fn object_paths_encode_the_gatt_hierarchy() {
        // BlueZ has no "parent" property; the path *is* the relationship.
        let device = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
        let service = format!("{device}/service0010");
        let characteristic = format!("{service}/char0011");
        assert!(characteristic.starts_with(&format!("{service}/")));
        assert!(service.starts_with(&format!("{device}/")));
    }

    #[test]
    fn interfaces_dictionaries_flatten() {
        let raw = Value::Array {
            element: "{sa{sv}}".into(),
            items: vec![Value::DictEntry(
                Box::new(Value::Str(interfaces::DEVICE.into())),
                Box::new(Value::dict([
                    ("Name".into(), Value::Str("Pixel".into())),
                    ("Connected".into(), Value::Bool(false)),
                ])),
            )],
        };
        let flat = to_interfaces(&raw);
        let device = flat.get(interfaces::DEVICE).expect("Device1 missing");
        assert_eq!(device.get("Name").and_then(Value::as_str), Some("Pixel"));
        assert_eq!(
            device.get("Connected").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn object_accessors_read_typed_properties() {
        let mut interfaces_map = HashMap::new();
        interfaces_map.insert(
            interfaces::DEVICE.to_string(),
            HashMap::from([
                (
                    "Name".to_string(),
                    Value::Variant(Box::new(Value::Str("Pixel".into()))),
                ),
                (
                    "RSSI".to_string(),
                    Value::Variant(Box::new(Value::Int16(-55))),
                ),
                (
                    "Connected".to_string(),
                    Value::Variant(Box::new(Value::Bool(true))),
                ),
            ]),
        );
        let object = Object {
            interfaces: interfaces_map,
        };

        assert!(object.has(interfaces::DEVICE));
        assert!(!object.has(interfaces::ADAPTER));
        assert_eq!(
            object.string(interfaces::DEVICE, "Name").as_deref(),
            Some("Pixel")
        );
        assert_eq!(object.int(interfaces::DEVICE, "RSSI"), Some(-55));
        assert_eq!(object.bool(interfaces::DEVICE, "Connected"), Some(true));
        assert_eq!(object.string(interfaces::DEVICE, "Missing"), None);
    }
}
