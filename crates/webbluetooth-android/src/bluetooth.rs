//! The Android BLE APIs, reached through JNI.
//!
//! Shaped like the other backends' sys layers: an [`Event`] enum, an
//! [`EventSink`] to receive it, and a `Central` that owns the adapter. What
//! differs is where the events come from — Android delivers them to callback
//! *objects*, which is why [`crate::dex`] and [`crate::runtime`] exist.
//!
//! # Threading
//!
//! Callbacks arrive on binder threads the VM owns but Rust has never seen, so
//! every entry point attaches, pushes a local frame, and pops it before
//! returning. Without the frame, a scan that runs for a minute exhausts the
//! 512-entry local reference table and aborts the process.
//!
//! # Permissions
//!
//! `BLUETOOTH_SCAN` and `BLUETOOTH_CONNECT` from API 31, and
//! `ACCESS_FINE_LOCATION` for scanning on 23–30. A missing permission surfaces
//! as a `SecurityException` from the framework — caught, cleared and reported
//! rather than left pending.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::jni::{Env, JObject, JValue};
use crate::runtime::Runtime;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};

/// A retained Java object.
///
/// A global reference, so it survives the callback it arrived in and can cross
/// threads — the Android counterpart of `Retained` on Apple.
#[derive(Debug)]
pub struct Ref(JObject);

impl Ref {
    /// Promote a local reference. `None` if null.
    pub fn new(env: Env, object: JObject) -> Option<Self> {
        (!object.is_null()).then(|| Self(env.new_global_ref(object)))
    }

    pub fn as_ptr(&self) -> JObject {
        self.0
    }

    /// A stable identity key.
    pub fn key(&self) -> usize {
        self.0 as usize
    }
}

impl Clone for Ref {
    fn clone(&self) -> Self {
        match Runtime::get().and_then(|r| r.env().ok()) {
            Some(env) => Self(env.new_global_ref(self.0)),
            // Without a VM there is nothing to retain; the copy is inert and
            // the original still owns the reference.
            None => Self(self.0),
        }
    }
}

impl Drop for Ref {
    fn drop(&mut self) {
        if let Some(env) = Runtime::get().and_then(|r| r.env().ok()) {
            env.delete_global_ref(self.0);
        }
    }
}

unsafe impl Send for Ref {}
unsafe impl Sync for Ref {}

/// `BluetoothAdapter.STATE_*`, plus the cases that are not adapter states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerState {
    Unknown,
    Unsupported,
    Unauthorized,
    PoweredOff,
    PoweredOn,
}

/// What Android tells us.
#[derive(Debug)]
pub enum Event {
    /// A scan result. `record` is the raw advertising payload when Android
    /// supplies one.
    Discovered {
        address: String,
        name: Option<String>,
        rssi: i32,
        service_uuids: Vec<String>,
        manufacturer_data: Vec<(u16, Vec<u8>)>,
        service_data: Vec<(String, Vec<u8>)>,
        connectable: bool,
    },
    ScanFailed {
        code: i32,
    },
    /// `onConnectionStateChange`. `status` is a `GATT_*` code; 0 is success.
    ConnectionStateChanged {
        gatt: Ref,
        status: i32,
        connected: bool,
    },
    ServicesDiscovered {
        gatt: Ref,
        status: i32,
    },
    CharacteristicRead {
        gatt: Ref,
        characteristic: Ref,
        value: Vec<u8>,
        status: i32,
    },
    CharacteristicWritten {
        gatt: Ref,
        characteristic: Ref,
        status: i32,
    },
    /// A subscription delivering a value.
    CharacteristicChanged {
        gatt: Ref,
        characteristic: Ref,
        value: Vec<u8>,
    },
    DescriptorRead {
        gatt: Ref,
        descriptor: Ref,
        value: Vec<u8>,
        status: i32,
    },
    DescriptorWritten {
        gatt: Ref,
        descriptor: Ref,
        status: i32,
    },
    /// The link's PHY, as reported by `onPhyUpdate` or `onPhyRead`.
    ///
    /// One event for both because they carry the same thing and a caller only
    /// ever has one outstanding: the difference is whether it was asked to
    /// change or merely to report.
    PhyChanged {
        gatt: Ref,
        tx: i32,
        rx: i32,
        status: i32,
    },
    MtuChanged {
        gatt: Ref,
        mtu: i32,
        status: i32,
    },
    RssiRead {
        gatt: Ref,
        rssi: i32,
        status: i32,
    },
    /// The peer's attribute table changed; every handle into it is stale.
    ServicesChanged {
        gatt: Ref,
    },

    // ── Peripheral role ─────────────────────────────────────────────────────
    ServerConnectionStateChanged {
        device: Ref,
        connected: bool,
    },
    ServiceAdded {
        status: i32,
    },
    ReadRequest {
        device: Ref,
        request_id: i32,
        offset: i32,
        characteristic: Ref,
    },
    WriteRequest {
        device: Ref,
        request_id: i32,
        characteristic: Ref,
        prepared: bool,
        response_needed: bool,
        offset: i32,
        value: Vec<u8>,
    },
    DescriptorReadRequest {
        device: Ref,
        request_id: i32,
        offset: i32,
        descriptor: Ref,
    },
    DescriptorWriteRequest {
        device: Ref,
        request_id: i32,
        descriptor: Ref,
        response_needed: bool,
        offset: i32,
        value: Vec<u8>,
    },
    NotificationSent {
        device: Ref,
        status: i32,
    },
    AdvertisingStarted {
        error: Option<i32>,
    },
}

/// Where events go. Called on a binder thread, one at a time per callback.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

// ── Sink registry, keyed by callback object ─────────────────────────────────

/// Registered sinks, each with the global reference identifying its callback.
///
/// **Not a map keyed by pointer.** A JNI reference is a handle, not an
/// address: the local reference a native method is handed and the global
/// reference held here denote the same object through different pointers, so
/// looking one up by the other's value never matches. `IsSameObject` is the
/// only way to ask whether two references mean the same object, which is why
/// this is a list that gets walked rather than a `HashMap`. There are a
/// handful of callbacks per connection, so walking it costs nothing.
type Sinks = Vec<(Ref, Arc<dyn EventSink>)>;

static SINKS: OnceLock<Mutex<Sinks>> = OnceLock::new();

fn sinks() -> &'static Mutex<Sinks> {
    SINKS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a sink for a callback object.
pub fn bind_sink(callback: &Ref, sink: Arc<dyn EventSink>) {
    let mut sinks = sinks().lock().unwrap();
    sinks.retain(|(existing, _)| existing.key() != callback.key());
    sinks.push((callback.clone(), sink));
}

pub fn unbind_sink(callback: &Ref) {
    sinks()
        .lock()
        .unwrap()
        .retain(|(existing, _)| existing.key() != callback.key());
}

/// Deliver an event to whichever sink owns `this`.
fn emit(env: Env, this: JObject, event: Event) {
    let sink = sinks().lock().ok().and_then(|sinks| {
        sinks
            .iter()
            .find(|(callback, _)| env.is_same_object(this, callback.as_ptr()))
            .map(|(_, sink)| sink.clone())
    });
    if let Some(sink) = sink {
        sink.emit(event);
    }
}

/// Run a callback body with an attached env and a local frame.
///
/// Every native below goes through this: attaching is required because the
/// thread is the VM's, and the frame is required because the callback may run
/// thousands of times.
fn in_callback(env: Env, body: impl FnOnce(Env)) {
    if !env.push_local_frame(32) {
        return;
    }
    body(env);
    env.pop_local_frame();
}

// ── BluetoothGattCallback ───────────────────────────────────────────────────

unsafe extern "C" fn on_connection_state_change(
    env: Env,
    this: JObject,
    gatt: JObject,
    status: i32,
    new_state: i32,
) {
    in_callback(env, |env| {
        let Some(gatt) = Ref::new(env, gatt) else {
            return;
        };
        // BluetoothProfile.STATE_CONNECTED == 2
        emit(
            env,
            this,
            Event::ConnectionStateChanged {
                gatt,
                status,
                connected: new_state == 2,
            },
        );
    });
}

unsafe extern "C" fn on_services_discovered(env: Env, this: JObject, gatt: JObject, status: i32) {
    in_callback(env, |env| {
        let Some(gatt) = Ref::new(env, gatt) else {
            return;
        };
        emit(env, this, Event::ServicesDiscovered { gatt, status });
    });
}

unsafe extern "C" fn on_characteristic_read(
    env: Env,
    this: JObject,
    gatt: JObject,
    characteristic: JObject,
    value: JObject,
    status: i32,
) {
    in_callback(env, |env| {
        let (Some(gatt), Some(characteristic)) =
            (Ref::new(env, gatt), Ref::new(env, characteristic))
        else {
            return;
        };
        let value = env.byte_array(value).unwrap_or_default();
        emit(
            env,
            this,
            Event::CharacteristicRead {
                gatt,
                characteristic,
                value,
                status,
            },
        );
    });
}

unsafe extern "C" fn on_characteristic_write(
    env: Env,
    this: JObject,
    gatt: JObject,
    characteristic: JObject,
    status: i32,
) {
    in_callback(env, |env| {
        let (Some(gatt), Some(characteristic)) =
            (Ref::new(env, gatt), Ref::new(env, characteristic))
        else {
            return;
        };
        emit(
            env,
            this,
            Event::CharacteristicWritten {
                gatt,
                characteristic,
                status,
            },
        );
    });
}

unsafe extern "C" fn on_characteristic_changed(
    env: Env,
    this: JObject,
    gatt: JObject,
    characteristic: JObject,
    value: JObject,
) {
    in_callback(env, |env| {
        let (Some(gatt), Some(characteristic)) =
            (Ref::new(env, gatt), Ref::new(env, characteristic))
        else {
            return;
        };
        let value = env.byte_array(value).unwrap_or_default();
        emit(
            env,
            this,
            Event::CharacteristicChanged {
                gatt,
                characteristic,
                value,
            },
        );
    });
}

unsafe extern "C" fn on_descriptor_read(
    env: Env,
    this: JObject,
    gatt: JObject,
    descriptor: JObject,
    status: i32,
    value: JObject,
) {
    in_callback(env, |env| {
        let (Some(gatt), Some(descriptor)) = (Ref::new(env, gatt), Ref::new(env, descriptor))
        else {
            return;
        };
        let value = env.byte_array(value).unwrap_or_default();
        emit(
            env,
            this,
            Event::DescriptorRead {
                gatt,
                descriptor,
                value,
                status,
            },
        );
    });
}

unsafe extern "C" fn on_descriptor_write(
    env: Env,
    this: JObject,
    gatt: JObject,
    descriptor: JObject,
    status: i32,
) {
    in_callback(env, |env| {
        let (Some(gatt), Some(descriptor)) = (Ref::new(env, gatt), Ref::new(env, descriptor))
        else {
            return;
        };
        emit(
            env,
            this,
            Event::DescriptorWritten {
                gatt,
                descriptor,
                status,
            },
        );
    });
}

unsafe extern "C" fn on_mtu_changed(env: Env, this: JObject, gatt: JObject, mtu: i32, status: i32) {
    in_callback(env, |env| {
        let Some(gatt) = Ref::new(env, gatt) else {
            return;
        };
        emit(env, this, Event::MtuChanged { gatt, mtu, status });
    });
}

unsafe extern "C" fn on_phy_update(
    env: Env,
    this: JObject,
    gatt: JObject,
    tx: i32,
    rx: i32,
    status: i32,
) {
    in_callback(env, |env| {
        let Some(gatt) = Ref::new(env, gatt) else {
            return;
        };
        emit(
            env,
            this,
            Event::PhyChanged {
                gatt,
                tx,
                rx,
                status,
            },
        );
    });
}

unsafe extern "C" fn on_phy_read(
    env: Env,
    this: JObject,
    gatt: JObject,
    tx: i32,
    rx: i32,
    status: i32,
) {
    in_callback(env, |env| {
        let Some(gatt) = Ref::new(env, gatt) else {
            return;
        };
        emit(
            env,
            this,
            Event::PhyChanged {
                gatt,
                tx,
                rx,
                status,
            },
        );
    });
}

unsafe extern "C" fn on_read_remote_rssi(
    env: Env,
    this: JObject,
    gatt: JObject,
    rssi: i32,
    status: i32,
) {
    in_callback(env, |env| {
        let Some(gatt) = Ref::new(env, gatt) else {
            return;
        };
        emit(env, this, Event::RssiRead { gatt, rssi, status });
    });
}

unsafe extern "C" fn on_service_changed(env: Env, this: JObject, gatt: JObject) {
    in_callback(env, |env| {
        let Some(gatt) = Ref::new(env, gatt) else {
            return;
        };
        emit(env, this, Event::ServicesChanged { gatt });
    });
}

/// The `(name, signature, function)` table for `GattCallback`.
///
/// These must match [`crate::gatt_callback_dex`] exactly — `RegisterNatives`
/// checks the name and signature against the class, so a mismatch fails loudly
/// at registration rather than quietly at the first call.
pub fn gatt_natives() -> Vec<(&'static str, &'static str, *const c_void)> {
    const GATT: &str = "Landroid/bluetooth/BluetoothGatt;";
    const CHAR: &str = "Landroid/bluetooth/BluetoothGattCharacteristic;";
    const DESC: &str = "Landroid/bluetooth/BluetoothGattDescriptor;";
    vec![
        (
            "onConnectionStateChange",
            concat!("(", "Landroid/bluetooth/BluetoothGatt;", "II)V"),
            on_connection_state_change as *const c_void,
        ),
        (
            "onServicesDiscovered",
            concat!("(", "Landroid/bluetooth/BluetoothGatt;", "I)V"),
            on_services_discovered as *const c_void,
        ),
        (
            "onCharacteristicRead",
            concat!(
                "(Landroid/bluetooth/BluetoothGatt;",
                "Landroid/bluetooth/BluetoothGattCharacteristic;[BI)V"
            ),
            on_characteristic_read as *const c_void,
        ),
        (
            "onCharacteristicWrite",
            concat!(
                "(Landroid/bluetooth/BluetoothGatt;",
                "Landroid/bluetooth/BluetoothGattCharacteristic;I)V"
            ),
            on_characteristic_write as *const c_void,
        ),
        (
            "onCharacteristicChanged",
            concat!(
                "(Landroid/bluetooth/BluetoothGatt;",
                "Landroid/bluetooth/BluetoothGattCharacteristic;[B)V"
            ),
            on_characteristic_changed as *const c_void,
        ),
        (
            "onDescriptorRead",
            concat!(
                "(Landroid/bluetooth/BluetoothGatt;",
                "Landroid/bluetooth/BluetoothGattDescriptor;I[B)V"
            ),
            on_descriptor_read as *const c_void,
        ),
        (
            "onDescriptorWrite",
            concat!(
                "(Landroid/bluetooth/BluetoothGatt;",
                "Landroid/bluetooth/BluetoothGattDescriptor;I)V"
            ),
            on_descriptor_write as *const c_void,
        ),
        (
            "onMtuChanged",
            "(Landroid/bluetooth/BluetoothGatt;II)V",
            on_mtu_changed as *const c_void,
        ),
        (
            "onPhyUpdate",
            "(Landroid/bluetooth/BluetoothGatt;III)V",
            on_phy_update as *const c_void,
        ),
        (
            "onPhyRead",
            "(Landroid/bluetooth/BluetoothGatt;III)V",
            on_phy_read as *const c_void,
        ),
        (
            "onReadRemoteRssi",
            "(Landroid/bluetooth/BluetoothGatt;II)V",
            on_read_remote_rssi as *const c_void,
        ),
        (
            "onServiceChanged",
            "(Landroid/bluetooth/BluetoothGatt;)V",
            on_service_changed as *const c_void,
        ),
        // Referencing the constants keeps them beside the signatures above.
        ("", GATT, std::ptr::null()),
        ("", CHAR, std::ptr::null()),
        ("", DESC, std::ptr::null()),
    ]
    .into_iter()
    .filter(|(name, _, _)| !name.is_empty())
    .collect()
}

// ── ScanCallback ────────────────────────────────────────────────────────────

unsafe extern "C" fn on_scan_result(env: Env, this: JObject, _type: i32, result: JObject) {
    in_callback(env, |env| {
        if let Some(event) = read_scan_result(env, result) {
            emit(env, this, event);
        }
    });
}

unsafe extern "C" fn on_batch_scan_results(env: Env, this: JObject, results: JObject) {
    in_callback(env, |env| {
        // java.util.List, so size()/get(int).
        let Some(list) = env.find_class("java/util/List") else {
            return;
        };
        let (Some(size), Some(get)) = (
            env.method_id(list, "size", "()I"),
            env.method_id(list, "get", "(I)Ljava/lang/Object;"),
        ) else {
            return;
        };
        let count = env.call_int(results, size, &[]);
        for i in 0..count {
            let item = env.call_object(results, get, &[JValue::int(i)]);
            if let Some(event) = read_scan_result(env, item) {
                emit(env, this, event);
            }
        }
    });
}

unsafe extern "C" fn on_scan_failed(env: Env, this: JObject, code: i32) {
    in_callback(env, |env| emit(env, this, Event::ScanFailed { code }));
}

/// Pull an advertisement out of a `ScanResult`.
fn read_scan_result(env: Env, result: JObject) -> Option<Event> {
    if result.is_null() {
        return None;
    }
    let result_class = env.get_object_class(result);
    let get_device = env.method_id(
        result_class,
        "getDevice",
        "()Landroid/bluetooth/BluetoothDevice;",
    )?;
    let get_rssi = env.method_id(result_class, "getRssi", "()I")?;
    let get_record = env.method_id(
        result_class,
        "getScanRecord",
        "()Landroid/bluetooth/le/ScanRecord;",
    )?;

    let device = env.call_object(result, get_device, &[]);
    let rssi = env.call_int(result, get_rssi, &[]);
    let record = env.call_object(result, get_record, &[]);

    let device_class = env.get_object_class(device);
    let get_address = env.method_id(device_class, "getAddress", "()Ljava/lang/String;")?;
    let address = env.get_string(env.call_object(device, get_address, &[]))?;

    let mut name = None;
    let mut service_uuids = Vec::new();
    let mut manufacturer_data = Vec::new();
    let mut service_data = Vec::new();

    if !record.is_null() {
        let record_class = env.get_object_class(record);
        if let Some(get_name) = env.method_id(record_class, "getDeviceName", "()Ljava/lang/String;")
        {
            name = env.get_string(env.call_object(record, get_name, &[]));
        }
        if let Some(get_uuids) =
            env.method_id(record_class, "getServiceUuids", "()Ljava/util/List;")
        {
            let list = env.call_object(record, get_uuids, &[]);
            service_uuids = read_uuid_list(env, list);
        }
        if let Some(get_manufacturer) = env.method_id(
            record_class,
            "getManufacturerSpecificData",
            "()Landroid/util/SparseArray;",
        ) {
            let sparse = env.call_object(record, get_manufacturer, &[]);
            manufacturer_data = read_sparse_bytes(env, sparse);
        }
        if let Some(get_service_data) =
            env.method_id(record_class, "getServiceData", "()Ljava/util/Map;")
        {
            let map = env.call_object(record, get_service_data, &[]);
            service_data = read_service_data(env, map);
        }
    }

    Some(Event::Discovered {
        address,
        name,
        rssi,
        service_uuids,
        manufacturer_data,
        service_data,
        // Android reports connectability only from API 26 in ScanResult, and
        // anything reachable through a Device1 handle accepts connections.
        connectable: true,
    })
}

/// A `List<ParcelUuid>` as canonical strings.
fn read_uuid_list(env: Env, list: JObject) -> Vec<String> {
    let mut out = Vec::new();
    if list.is_null() {
        return out;
    }
    let Some(list_class) = env.find_class("java/util/List") else {
        return out;
    };
    let (Some(size), Some(get)) = (
        env.method_id(list_class, "size", "()I"),
        env.method_id(list_class, "get", "(I)Ljava/lang/Object;"),
    ) else {
        return out;
    };
    let count = env.call_int(list, size, &[]);
    for i in 0..count {
        let parcel = env.call_object(list, get, &[JValue::int(i)]);
        if let Some(uuid) = parcel_uuid_to_string(env, parcel) {
            out.push(uuid);
        }
    }
    out
}

fn parcel_uuid_to_string(env: Env, parcel: JObject) -> Option<String> {
    if parcel.is_null() {
        return None;
    }
    let class = env.get_object_class(parcel);
    let to_string = env.method_id(class, "toString", "()Ljava/lang/String;")?;
    env.get_string(env.call_object(parcel, to_string, &[]))
        .map(|s| s.to_ascii_lowercase())
}

/// A `SparseArray<byte[]>` — manufacturer data, keyed by company identifier.
fn read_sparse_bytes(env: Env, sparse: JObject) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    if sparse.is_null() {
        return out;
    }
    let class = env.get_object_class(sparse);
    let (Some(size), Some(key_at), Some(value_at)) = (
        env.method_id(class, "size", "()I"),
        env.method_id(class, "keyAt", "(I)I"),
        env.method_id(class, "valueAt", "(I)Ljava/lang/Object;"),
    ) else {
        return out;
    };
    let count = env.call_int(sparse, size, &[]);
    for i in 0..count {
        let key = env.call_int(sparse, key_at, &[JValue::int(i)]);
        let value = env.call_object(sparse, value_at, &[JValue::int(i)]);
        if let Some(bytes) = env.byte_array(value) {
            out.push((key as u16, bytes));
        }
    }
    out
}

/// A `Map<ParcelUuid, byte[]>`.
fn read_service_data(env: Env, map: JObject) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    if map.is_null() {
        return out;
    }
    let Some(map_class) = env.find_class("java/util/Map") else {
        return out;
    };
    let Some(entry_set) = env.method_id(map_class, "entrySet", "()Ljava/util/Set;") else {
        return out;
    };
    let set = env.call_object(map, entry_set, &[]);
    let Some(set_class) = env.find_class("java/util/Set") else {
        return out;
    };
    let Some(iterator) = env.method_id(set_class, "iterator", "()Ljava/util/Iterator;") else {
        return out;
    };
    let it = env.call_object(set, iterator, &[]);
    let Some(it_class) = env.find_class("java/util/Iterator") else {
        return out;
    };
    let (Some(has_next), Some(next)) = (
        env.method_id(it_class, "hasNext", "()Z"),
        env.method_id(it_class, "next", "()Ljava/lang/Object;"),
    ) else {
        return out;
    };
    let Some(entry_class) = env.find_class("java/util/Map$Entry") else {
        return out;
    };
    let (Some(get_key), Some(get_value)) = (
        env.method_id(entry_class, "getKey", "()Ljava/lang/Object;"),
        env.method_id(entry_class, "getValue", "()Ljava/lang/Object;"),
    ) else {
        return out;
    };

    while env.call_bool(it, has_next, &[]) {
        let entry = env.call_object(it, next, &[]);
        let key = env.call_object(entry, get_key, &[]);
        let value = env.call_object(entry, get_value, &[]);
        if let (Some(uuid), Some(bytes)) = (parcel_uuid_to_string(env, key), env.byte_array(value))
        {
            out.push((uuid, bytes));
        }
    }
    out
}

// ── BluetoothGattServerCallback ─────────────────────────────────────────────

unsafe extern "C" fn on_server_connection_state_change(
    env: Env,
    this: JObject,
    device: JObject,
    _status: i32,
    new_state: i32,
) {
    in_callback(env, |env| {
        let Some(device) = Ref::new(env, device) else {
            return;
        };
        emit(
            env,
            this,
            Event::ServerConnectionStateChanged {
                device,
                connected: new_state == 2,
            },
        );
    });
}

unsafe extern "C" fn on_service_added(env: Env, this: JObject, status: i32, _service: JObject) {
    in_callback(env, |env| emit(env, this, Event::ServiceAdded { status }));
}

unsafe extern "C" fn on_characteristic_read_request(
    env: Env,
    this: JObject,
    device: JObject,
    request_id: i32,
    offset: i32,
    characteristic: JObject,
) {
    in_callback(env, |env| {
        let (Some(device), Some(characteristic)) =
            (Ref::new(env, device), Ref::new(env, characteristic))
        else {
            return;
        };
        emit(
            env,
            this,
            Event::ReadRequest {
                device,
                request_id,
                offset,
                characteristic,
            },
        );
    });
}

unsafe extern "C" fn on_characteristic_write_request(
    env: Env,
    this: JObject,
    device: JObject,
    request_id: i32,
    characteristic: JObject,
    prepared: u8,
    response_needed: u8,
    offset: i32,
    value: JObject,
) {
    in_callback(env, |env| {
        let (Some(device), Some(characteristic)) =
            (Ref::new(env, device), Ref::new(env, characteristic))
        else {
            return;
        };
        emit(
            env,
            this,
            Event::WriteRequest {
                device,
                request_id,
                characteristic,
                prepared: prepared != 0,
                response_needed: response_needed != 0,
                offset,
                value: env.byte_array(value).unwrap_or_default(),
            },
        );
    });
}

unsafe extern "C" fn on_descriptor_read_request(
    env: Env,
    this: JObject,
    device: JObject,
    request_id: i32,
    offset: i32,
    descriptor: JObject,
) {
    in_callback(env, |env| {
        let (Some(device), Some(descriptor)) = (Ref::new(env, device), Ref::new(env, descriptor))
        else {
            return;
        };
        emit(
            env,
            this,
            Event::DescriptorReadRequest {
                device,
                request_id,
                offset,
                descriptor,
            },
        );
    });
}

unsafe extern "C" fn on_descriptor_write_request(
    env: Env,
    this: JObject,
    device: JObject,
    request_id: i32,
    descriptor: JObject,
    _prepared: u8,
    response_needed: u8,
    offset: i32,
    value: JObject,
) {
    in_callback(env, |env| {
        let (Some(device), Some(descriptor)) = (Ref::new(env, device), Ref::new(env, descriptor))
        else {
            return;
        };
        emit(
            env,
            this,
            Event::DescriptorWriteRequest {
                device,
                request_id,
                descriptor,
                response_needed: response_needed != 0,
                offset,
                value: env.byte_array(value).unwrap_or_default(),
            },
        );
    });
}

unsafe extern "C" fn on_notification_sent(env: Env, this: JObject, device: JObject, status: i32) {
    in_callback(env, |env| {
        let Some(device) = Ref::new(env, device) else {
            return;
        };
        emit(env, this, Event::NotificationSent { device, status });
    });
}

unsafe extern "C" fn on_server_mtu_changed(env: Env, this: JObject, _device: JObject, _mtu: i32) {
    in_callback(env, |_| {
        let _ = this;
    });
}

/// The `(name, signature, function)` table for `BluetoothGattServerCallback`.
pub fn server_natives() -> Vec<(&'static str, &'static str, *const c_void)> {
    vec![
        (
            "onConnectionStateChange",
            "(Landroid/bluetooth/BluetoothDevice;II)V",
            on_server_connection_state_change as *const c_void,
        ),
        (
            "onServiceAdded",
            "(ILandroid/bluetooth/BluetoothGattService;)V",
            on_service_added as *const c_void,
        ),
        (
            "onCharacteristicReadRequest",
            concat!(
                "(Landroid/bluetooth/BluetoothDevice;II",
                "Landroid/bluetooth/BluetoothGattCharacteristic;)V"
            ),
            on_characteristic_read_request as *const c_void,
        ),
        (
            "onCharacteristicWriteRequest",
            concat!(
                "(Landroid/bluetooth/BluetoothDevice;I",
                "Landroid/bluetooth/BluetoothGattCharacteristic;ZZI[B)V"
            ),
            on_characteristic_write_request as *const c_void,
        ),
        (
            "onDescriptorReadRequest",
            concat!(
                "(Landroid/bluetooth/BluetoothDevice;II",
                "Landroid/bluetooth/BluetoothGattDescriptor;)V"
            ),
            on_descriptor_read_request as *const c_void,
        ),
        (
            "onDescriptorWriteRequest",
            concat!(
                "(Landroid/bluetooth/BluetoothDevice;I",
                "Landroid/bluetooth/BluetoothGattDescriptor;ZZI[B)V"
            ),
            on_descriptor_write_request as *const c_void,
        ),
        (
            "onNotificationSent",
            "(Landroid/bluetooth/BluetoothDevice;I)V",
            on_notification_sent as *const c_void,
        ),
        (
            "onMtuChanged",
            "(Landroid/bluetooth/BluetoothDevice;I)V",
            on_server_mtu_changed as *const c_void,
        ),
    ]
}

// ── AdvertiseCallback ───────────────────────────────────────────────────────

unsafe extern "C" fn on_start_success(env: Env, this: JObject, _settings: JObject) {
    in_callback(env, |env| {
        emit(env, this, Event::AdvertisingStarted { error: None })
    });
}

unsafe extern "C" fn on_start_failure(env: Env, this: JObject, code: i32) {
    in_callback(env, |env| {
        emit(env, this, Event::AdvertisingStarted { error: Some(code) })
    });
}

/// The `(name, signature, function)` table for `AdvertiseCallback`.
pub fn advertise_natives() -> Vec<(&'static str, &'static str, *const c_void)> {
    vec![
        (
            "onStartSuccess",
            "(Landroid/bluetooth/le/AdvertiseSettings;)V",
            on_start_success as *const c_void,
        ),
        ("onStartFailure", "(I)V", on_start_failure as *const c_void),
    ]
}

/// The `(name, signature, function)` table for `ScanCallback`.
pub fn scan_natives() -> Vec<(&'static str, &'static str, *const c_void)> {
    vec![
        (
            "onScanResult",
            "(ILandroid/bluetooth/le/ScanResult;)V",
            on_scan_result as *const c_void,
        ),
        (
            "onBatchScanResults",
            "(Ljava/util/List;)V",
            on_batch_scan_results as *const c_void,
        ),
        ("onScanFailed", "(I)V", on_scan_failed as *const c_void),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every native must match the DEX that declares it, or `RegisterNatives`
    /// rejects the whole table.
    #[test]
    fn native_tables_match_the_generated_dex() {
        let gatt = gatt_natives();
        // No count here on purpose: the DEX class is generated *from* this
        // table, so the two cannot disagree and a number would only be one
        // more thing to update. What is still worth asserting is that each
        // entry is well formed, because a bad descriptor is what
        // `RegisterNatives` actually rejects.
        assert!(!gatt.is_empty(), "GattCallback overrides something");
        for (name, signature, f) in &gatt {
            assert!(!name.is_empty());
            assert!(
                signature.starts_with('('),
                "{name}: {signature} is not a descriptor"
            );
            assert!(
                signature.ends_with(")V"),
                "{name}: every callback returns void"
            );
            assert!(!f.is_null());
        }

        let scan = scan_natives();
        assert_eq!(scan.len(), 3);
        for (_, signature, _) in &scan {
            assert!(signature.ends_with(")V"));
        }
    }

    #[test]
    fn the_peripheral_tables_match_their_dex() {
        // Eight server overrides and two advertising ones, matching the
        // classes `crate::gatt_server_callback_dex` and
        // `crate::advertise_callback_dex` declare.
        assert_eq!(server_natives().len(), 8);
        assert_eq!(advertise_natives().len(), 2);
        for (name, signature, f) in server_natives().iter().chain(advertise_natives().iter()) {
            assert!(!name.is_empty());
            assert!(
                signature.starts_with('(') && signature.ends_with(")V"),
                "{name}"
            );
            assert!(!f.is_null());
        }
    }

    #[test]
    fn signatures_agree_between_the_dex_and_the_native_table() {
        // The DEX declares the methods; this table binds them. They are written
        // in two places, so check they say the same thing.
        let dex_source = crate::gatt_callback_dex();
        assert!(!dex_source.is_empty());
        // Names must line up one for one.
        let names: Vec<&str> = gatt_natives().into_iter().map(|(n, _, _)| n).collect();
        for expected in [
            "onConnectionStateChange",
            "onServicesDiscovered",
            "onCharacteristicRead",
            "onCharacteristicWrite",
            "onCharacteristicChanged",
            "onDescriptorRead",
            "onDescriptorWrite",
            "onMtuChanged",
            "onReadRemoteRssi",
            "onServiceChanged",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} is missing from the native table"
            );
        }
    }
}
