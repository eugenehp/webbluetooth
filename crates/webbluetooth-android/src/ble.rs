//! Calling the Android BLE APIs.
//!
//! Methods are resolved by name and signature at each call rather than cached.
//! BLE operates at human speed — a scan result every few hundred milliseconds,
//! a GATT read now and then — so the lookup costs nothing measurable, and it
//! removes a whole class of bug where a cached `jmethodID` outlives the class
//! it came from.

// As in `jni`: JNI references are raw pointers passed to the VM, not
// dereferenced here.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::bluetooth::{Event, EventSink, Ref};
use crate::jni::{Env, JObject, JValue};
use crate::runtime::{Error, Runtime};
use std::sync::Arc;

type Result<T> = std::result::Result<T, Error>;

/// `BluetoothDevice.TRANSPORT_LE`.
const TRANSPORT_LE: i32 = 2;
/// `BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT` / `_NO_RESPONSE`.
pub const WRITE_TYPE_DEFAULT: i32 = 2;
pub const WRITE_TYPE_NO_RESPONSE: i32 = 1;
/// `BluetoothGatt.GATT_SUCCESS`.
pub const GATT_SUCCESS: i32 = 0;

// ── Call helpers ────────────────────────────────────────────────────────────

fn missing(what: &str) -> Error {
    Error::MissingMethod(what.into())
}

pub fn call_object(
    env: Env,
    object: JObject,
    name: &str,
    signature: &str,
    args: &[JValue],
) -> Result<JObject> {
    let method = env
        .method_id(env.get_object_class(object), name, signature)
        .ok_or_else(|| missing(name))?;
    Ok(env.call_object(object, method, args))
}

pub fn call_bool(
    env: Env,
    object: JObject,
    name: &str,
    signature: &str,
    args: &[JValue],
) -> Result<bool> {
    let method = env
        .method_id(env.get_object_class(object), name, signature)
        .ok_or_else(|| missing(name))?;
    Ok(env.call_bool(object, method, args))
}

pub fn call_int(
    env: Env,
    object: JObject,
    name: &str,
    signature: &str,
    args: &[JValue],
) -> Result<i32> {
    let method = env
        .method_id(env.get_object_class(object), name, signature)
        .ok_or_else(|| missing(name))?;
    Ok(env.call_int(object, method, args))
}

pub fn call_void(
    env: Env,
    object: JObject,
    name: &str,
    signature: &str,
    args: &[JValue],
) -> Result<()> {
    let method = env
        .method_id(env.get_object_class(object), name, signature)
        .ok_or_else(|| missing(name))?;
    env.call_void(object, method, args);
    Ok(())
}

/// `java.util.UUID.fromString`.
pub fn uuid(env: Env, canonical: &str) -> Result<JObject> {
    let class = env
        .find_class("java/util/UUID")
        .ok_or_else(|| Error::MissingClass("java.util.UUID".into()))?;
    let from_string = env
        .static_method_id(class, "fromString", "(Ljava/lang/String;)Ljava/util/UUID;")
        .ok_or_else(|| missing("UUID.fromString"))?;
    let s = env.new_string(canonical);
    Ok(env.call_static_object(class, from_string, &[JValue::object(s)]))
}

/// The canonical string of a `java.util.UUID`, lowercase as the spec wants.
pub fn uuid_string(env: Env, object: JObject) -> Option<String> {
    if object.is_null() {
        return None;
    }
    let s = call_object(env, object, "toString", "()Ljava/lang/String;", &[]).ok()?;
    env.get_string(s).map(|s| s.to_ascii_lowercase())
}

/// Wrap a `java.util.UUID` in a `ParcelUuid`, which the scan APIs take.
fn parcel_uuid(env: Env, canonical: &str) -> Result<JObject> {
    let class = env
        .find_class("android/os/ParcelUuid")
        .ok_or_else(|| Error::MissingClass("android.os.ParcelUuid".into()))?;
    let ctor = env
        .method_id(class, "<init>", "(Ljava/util/UUID;)V")
        .ok_or_else(|| missing("ParcelUuid.<init>"))?;
    let inner = uuid(env, canonical)?;
    Ok(env.new_object(class, ctor, &[JValue::object(inner)]))
}

/// Walk a `java.util.List`, applying `f` to each element.
pub fn for_each(env: Env, list: JObject, mut f: impl FnMut(JObject)) {
    if list.is_null() {
        return;
    }
    let Ok(size) = call_int(env, list, "size", "()I", &[]) else {
        return;
    };
    for i in 0..size {
        if let Ok(item) = call_object(env, list, "get", "(I)Ljava/lang/Object;", &[JValue::int(i)])
        {
            f(item);
        }
    }
}

// ── Adapter ─────────────────────────────────────────────────────────────────

/// The Bluetooth adapter and its scanner.
pub struct Adapter {
    adapter: Ref,
}

impl Adapter {
    /// `((BluetoothManager) context.getSystemService(BLUETOOTH_SERVICE)).getAdapter()`.
    pub fn open(runtime: &Runtime) -> Result<Self> {
        let env = runtime.env()?;

        let context_class = env
            .find_class("android/content/Context")
            .ok_or_else(|| Error::MissingClass("android.content.Context".into()))?;
        let field = env
            .static_field_id(context_class, "BLUETOOTH_SERVICE", "Ljava/lang/String;")
            .ok_or_else(|| missing("Context.BLUETOOTH_SERVICE"))?;
        let name = env.static_object_field(context_class, field);

        let manager = call_object(
            env,
            runtime.context(),
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::object(name)],
        )?;
        if manager.is_null() {
            return Err(Error::MissingClass(
                "BluetoothManager (no Bluetooth on this device)".into(),
            ));
        }

        let adapter = call_object(
            env,
            manager,
            "getAdapter",
            "()Landroid/bluetooth/BluetoothAdapter;",
            &[],
        )?;
        let adapter = Ref::new(env, adapter)
            .ok_or_else(|| Error::MissingClass("BluetoothAdapter (none present)".into()))?;
        Ok(Self { adapter })
    }

    pub fn as_ptr(&self) -> JObject {
        self.adapter.as_ptr()
    }

    /// Whether the adapter is on.
    pub fn is_enabled(&self, env: Env) -> bool {
        call_bool(env, self.as_ptr(), "isEnabled", "()Z", &[]).unwrap_or(false)
    }

    /// `adapter.getName()` — what this device advertises itself as.
    pub fn name(&self, env: Env) -> Option<String> {
        let name = call_object(env, self.as_ptr(), "getName", "()Ljava/lang/String;", &[]).ok()?;
        env.get_string(name)
    }

    /// `adapter.getAddress()` — the controller's own address.
    ///
    /// Since Android 6 this returns a fixed `02:00:00:00:00:00` to any app
    /// without `LOCAL_MAC_ADDRESS_PERMISSION`, which is a system permission an
    /// ordinary app cannot hold. The caller is expected to recognise that and
    /// report nothing rather than a placeholder every device shares.
    pub fn address(&self, env: Env) -> Option<String> {
        let address = call_object(
            env,
            self.as_ptr(),
            "getAddress",
            "()Ljava/lang/String;",
            &[],
        )
        .ok()?;
        env.get_string(address)
    }

    /// `adapter.getBluetoothLeScanner()` — null while Bluetooth is off.
    pub fn scanner(&self, env: Env) -> Result<JObject> {
        call_object(
            env,
            self.as_ptr(),
            "getBluetoothLeScanner",
            "()Landroid/bluetooth/le/BluetoothLeScanner;",
            &[],
        )
    }

    /// `adapter.getRemoteDevice(address)`.
    pub fn remote_device(&self, env: Env, address: &str) -> Result<JObject> {
        let address = env.new_string(address);
        call_object(
            env,
            self.as_ptr(),
            "getRemoteDevice",
            "(Ljava/lang/String;)Landroid/bluetooth/BluetoothDevice;",
            &[JValue::object(address)],
        )
    }

    /// `adapter.getBluetoothLeAdvertiser()` — null where advertising is
    /// unsupported, which is common on older or cheaper hardware.
    pub fn advertiser(&self, env: Env) -> Result<JObject> {
        call_object(
            env,
            self.as_ptr(),
            "getBluetoothLeAdvertiser",
            "()Landroid/bluetooth/le/BluetoothLeAdvertiser;",
            &[],
        )
    }
}

// ── Scanning ────────────────────────────────────────────────────────────────

/// Start a scan, filtering on service UUIDs where possible.
///
/// Android's filters run in the controller, so a filtered scan also survives
/// the background restrictions an unfiltered one does not.
pub fn start_scan(
    env: Env,
    scanner: JObject,
    callback: JObject,
    service_uuids: &[String],
) -> Result<()> {
    let filters = build_scan_filters(env, service_uuids)?;
    let settings = build_scan_settings(env)?;
    call_void(
        env,
        scanner,
        "startScan",
        "(Ljava/util/List;Landroid/bluetooth/le/ScanSettings;Landroid/bluetooth/le/ScanCallback;)V",
        &[
            JValue::object(filters),
            JValue::object(settings),
            JValue::object(callback),
        ],
    )
}

pub fn stop_scan(env: Env, scanner: JObject, callback: JObject) -> Result<()> {
    call_void(
        env,
        scanner,
        "stopScan",
        "(Landroid/bluetooth/le/ScanCallback;)V",
        &[JValue::object(callback)],
    )
}

fn build_scan_filters(env: Env, service_uuids: &[String]) -> Result<JObject> {
    let list_class = env
        .find_class("java/util/ArrayList")
        .ok_or_else(|| Error::MissingClass("java.util.ArrayList".into()))?;
    let ctor = env
        .method_id(list_class, "<init>", "()V")
        .ok_or_else(|| missing("ArrayList"))?;
    let list = env.new_object(list_class, ctor, &[]);

    let filter_builder_class = env
        .find_class("android/bluetooth/le/ScanFilter$Builder")
        .ok_or_else(|| Error::MissingClass("ScanFilter.Builder".into()))?;
    let builder_ctor = env
        .method_id(filter_builder_class, "<init>", "()V")
        .ok_or_else(|| missing("ScanFilter.Builder.<init>"))?;

    for canonical in service_uuids {
        let builder = env.new_object(filter_builder_class, builder_ctor, &[]);
        let parcel = parcel_uuid(env, canonical)?;
        call_object(
            env,
            builder,
            "setServiceUuid",
            "(Landroid/os/ParcelUuid;)Landroid/bluetooth/le/ScanFilter$Builder;",
            &[JValue::object(parcel)],
        )?;
        let filter = call_object(
            env,
            builder,
            "build",
            "()Landroid/bluetooth/le/ScanFilter;",
            &[],
        )?;
        call_bool(
            env,
            list,
            "add",
            "(Ljava/lang/Object;)Z",
            &[JValue::object(filter)],
        )?;
    }
    Ok(list)
}

fn build_scan_settings(env: Env) -> Result<JObject> {
    let class = env
        .find_class("android/bluetooth/le/ScanSettings$Builder")
        .ok_or_else(|| Error::MissingClass("ScanSettings.Builder".into()))?;
    let ctor = env
        .method_id(class, "<init>", "()V")
        .ok_or_else(|| missing("ScanSettings.Builder.<init>"))?;
    let builder = env.new_object(class, ctor, &[]);

    // SCAN_MODE_LOW_LATENCY = 2. A chooser is waiting on this, so latency
    // matters more than power for the seconds it runs.
    call_object(
        env,
        builder,
        "setScanMode",
        "(I)Landroid/bluetooth/le/ScanSettings$Builder;",
        &[JValue::int(2)],
    )?;
    call_object(
        env,
        builder,
        "build",
        "()Landroid/bluetooth/le/ScanSettings;",
        &[],
    )
}

// ── GATT client ─────────────────────────────────────────────────────────────

/// `device.connectGatt(context, false, callback, TRANSPORT_LE)`.
///
/// `autoConnect = false` connects now and fails if the peer is not there;
/// `true` waits indefinitely, which is not what `connect()` promises.
/// `device.createBond()` — start the pairing ceremony.
///
/// `false` means it did not start at all; `true` means it started, which is
/// not the same as finished. Android reports the outcome later, through a
/// broadcast.
/// `gatt.setPreferredPhy(txMask, rxMask, options)`.
///
/// Masks rather than values: a caller states which PHYs it will accept and the
/// controller picks. `options` is the coded-PHY preference and is ignored
/// unless coded is in the mask.
pub fn set_preferred_phy(env: Env, gatt: JObject, tx: i32, rx: i32, options: i32) -> Result<()> {
    call_void(
        env,
        gatt,
        "setPreferredPhy",
        "(III)V",
        &[JValue::int(tx), JValue::int(rx), JValue::int(options)],
    )
}

/// `gatt.readPhy()` — the answer arrives in `onPhyRead`.
pub fn read_phy(env: Env, gatt: JObject) -> Result<()> {
    call_void(env, gatt, "readPhy", "()V", &[])
}

pub fn create_bond(env: Env, device: JObject) -> Result<bool> {
    call_bool(env, device, "createBond", "()Z", &[])
}

/// `device.getBondState()` — `BOND_NONE`, `BOND_BONDING` or `BOND_BONDED`.
pub fn bond_state(env: Env, device: JObject) -> Result<i32> {
    call_int(env, device, "getBondState", "()I", &[])
}

pub fn connect_gatt(
    env: Env,
    runtime: &Runtime,
    device: JObject,
    callback: JObject,
) -> Result<JObject> {
    call_object(
        env,
        device,
        "connectGatt",
        "(Landroid/content/Context;ZLandroid/bluetooth/BluetoothGattCallback;I)Landroid/bluetooth/BluetoothGatt;",
        &[
            JValue::object(runtime.context()),
            JValue::bool(false),
            JValue::object(callback),
            JValue::int(TRANSPORT_LE),
        ],
    )
}

pub fn discover_services(env: Env, gatt: JObject) -> Result<bool> {
    call_bool(env, gatt, "discoverServices", "()Z", &[])
}

pub fn services(env: Env, gatt: JObject) -> Result<JObject> {
    call_object(env, gatt, "getServices", "()Ljava/util/List;", &[])
}

pub fn service_characteristics(env: Env, service: JObject) -> Result<JObject> {
    call_object(
        env,
        service,
        "getCharacteristics",
        "()Ljava/util/List;",
        &[],
    )
}

/// `BluetoothGattService.getIncludedServices()`.
///
/// Android populates this from the Include declarations it read during service
/// discovery, so no extra round trip is needed — unlike CoreBluetooth, which
/// has to be asked.
pub fn service_included_services(env: Env, service: JObject) -> Result<JObject> {
    call_object(
        env,
        service,
        "getIncludedServices",
        "()Ljava/util/List;",
        &[],
    )
}

pub fn characteristic_descriptors(env: Env, characteristic: JObject) -> Result<JObject> {
    call_object(
        env,
        characteristic,
        "getDescriptors",
        "()Ljava/util/List;",
        &[],
    )
}

pub fn attribute_uuid(env: Env, attribute: JObject) -> Option<String> {
    let uuid = call_object(env, attribute, "getUuid", "()Ljava/util/UUID;", &[]).ok()?;
    uuid_string(env, uuid)
}

pub fn characteristic_properties(env: Env, characteristic: JObject) -> i32 {
    call_int(env, characteristic, "getProperties", "()I", &[]).unwrap_or(0)
}

pub fn characteristic_value(env: Env, characteristic: JObject) -> Option<Vec<u8>> {
    let array = call_object(env, characteristic, "getValue", "()[B", &[]).ok()?;
    env.byte_array(array)
}

pub fn read_characteristic(env: Env, gatt: JObject, characteristic: JObject) -> Result<bool> {
    call_bool(
        env,
        gatt,
        "readCharacteristic",
        "(Landroid/bluetooth/BluetoothGattCharacteristic;)Z",
        &[JValue::object(characteristic)],
    )
}

/// Write a characteristic, using whichever API this device has.
///
/// API 33 replaced the set-then-write pair with a single call, and deprecated
/// the old one. Both are tried, newest first, because a library cannot pick at
/// build time what the device will offer at run time.
pub fn write_characteristic(
    env: Env,
    gatt: JObject,
    characteristic: JObject,
    value: &[u8],
    write_type: i32,
) -> Result<bool> {
    let bytes = env.new_byte_array(value);

    // API 33+: writeCharacteristic(characteristic, byte[], int) -> int status
    if let Some(method) = env.method_id(
        env.get_object_class(gatt),
        "writeCharacteristic",
        "(Landroid/bluetooth/BluetoothGattCharacteristic;[BI)I",
    ) {
        let status = env.call_int(
            gatt,
            method,
            &[
                JValue::object(characteristic),
                JValue::object(bytes),
                JValue::int(write_type),
            ],
        );
        return Ok(status == GATT_SUCCESS);
    }

    // Older: set the value on the characteristic, then write it.
    call_int(
        env,
        characteristic,
        "setWriteType",
        "(I)V",
        &[JValue::int(write_type)],
    )
    .ok();
    call_bool(
        env,
        characteristic,
        "setValue",
        "([B)Z",
        &[JValue::object(bytes)],
    )?;
    call_bool(
        env,
        gatt,
        "writeCharacteristic",
        "(Landroid/bluetooth/BluetoothGattCharacteristic;)Z",
        &[JValue::object(characteristic)],
    )
}

/// Subscribe.
///
/// Two steps, and missing the second is the most common Android BLE bug: local
/// delivery has to be enabled *and* the peer has to be told, by writing its
/// Client Characteristic Configuration descriptor.
pub fn set_notify(
    env: Env,
    gatt: JObject,
    characteristic: JObject,
    enable: bool,
    indicate: bool,
) -> Result<bool> {
    call_bool(
        env,
        gatt,
        "setCharacteristicNotification",
        "(Landroid/bluetooth/BluetoothGattCharacteristic;Z)Z",
        &[JValue::object(characteristic), JValue::bool(enable)],
    )?;

    let cccd = uuid(env, "00002902-0000-1000-8000-00805f9b34fb")?;
    let descriptor = call_object(
        env,
        characteristic,
        "getDescriptor",
        "(Ljava/util/UUID;)Landroid/bluetooth/BluetoothGattDescriptor;",
        &[JValue::object(cccd)],
    )?;
    if descriptor.is_null() {
        // No CCCD: local delivery is on, but the peer will send nothing.
        return Ok(false);
    }

    let value: &[u8] = match (enable, indicate) {
        (false, _) => &[0x00, 0x00],
        (true, false) => &[0x01, 0x00],
        (true, true) => &[0x02, 0x00],
    };
    write_descriptor(env, gatt, descriptor, value)
}

pub fn read_descriptor(env: Env, gatt: JObject, descriptor: JObject) -> Result<bool> {
    call_bool(
        env,
        gatt,
        "readDescriptor",
        "(Landroid/bluetooth/BluetoothGattDescriptor;)Z",
        &[JValue::object(descriptor)],
    )
}

pub fn write_descriptor(
    env: Env,
    gatt: JObject,
    descriptor: JObject,
    value: &[u8],
) -> Result<bool> {
    let bytes = env.new_byte_array(value);

    // API 33+ takes the value directly.
    if let Some(method) = env.method_id(
        env.get_object_class(gatt),
        "writeDescriptor",
        "(Landroid/bluetooth/BluetoothGattDescriptor;[B)I",
    ) {
        let status = env.call_int(
            gatt,
            method,
            &[JValue::object(descriptor), JValue::object(bytes)],
        );
        return Ok(status == GATT_SUCCESS);
    }

    call_bool(
        env,
        descriptor,
        "setValue",
        "([B)Z",
        &[JValue::object(bytes)],
    )?;
    call_bool(
        env,
        gatt,
        "writeDescriptor",
        "(Landroid/bluetooth/BluetoothGattDescriptor;)Z",
        &[JValue::object(descriptor)],
    )
}

pub fn request_mtu(env: Env, gatt: JObject, mtu: i32) -> Result<bool> {
    call_bool(env, gatt, "requestMtu", "(I)Z", &[JValue::int(mtu)])
}

pub fn read_remote_rssi(env: Env, gatt: JObject) -> Result<bool> {
    call_bool(env, gatt, "readRemoteRssi", "()Z", &[])
}

/// `BluetoothGatt.requestConnectionPriority(int)`.
///
/// A hint: Android asks the controller to re-negotiate the connection
/// interval, and the peripheral has the final say. `false` means the request
/// was not even sent — usually because the argument was out of range or the
/// link is gone.
pub fn request_connection_priority(env: Env, gatt: JObject, priority: i32) -> Result<bool> {
    call_bool(
        env,
        gatt,
        "requestConnectionPriority",
        "(I)Z",
        &[JValue::int(priority)],
    )
}

pub fn disconnect(env: Env, gatt: JObject) -> Result<()> {
    call_void(env, gatt, "disconnect", "()V", &[])
}

/// Release the connection's resources.
///
/// Android allows a small number of concurrent GATT clients — historically 32,
/// far fewer in practice — and leaking them is how an app stops being able to
/// connect at all until it is restarted.
pub fn close(env: Env, gatt: JObject) -> Result<()> {
    call_void(env, gatt, "close", "()V", &[])
}

// ── L2CAP ───────────────────────────────────────────────────────────────────

/// `device.createL2capChannel(psm)` — API 29+.
pub fn create_l2cap_channel(env: Env, device: JObject, psm: i32) -> Result<JObject> {
    call_object(
        env,
        device,
        "createL2capChannel",
        "(I)Landroid/bluetooth/BluetoothSocket;",
        &[JValue::int(psm)],
    )
}

pub fn socket_connect(env: Env, socket: JObject) -> Result<()> {
    call_void(env, socket, "connect", "()V", &[])
}

pub fn socket_close(env: Env, socket: JObject) -> Result<()> {
    call_void(env, socket, "close", "()V", &[])
}

pub fn socket_input_stream(env: Env, socket: JObject) -> Result<JObject> {
    call_object(
        env,
        socket,
        "getInputStream",
        "()Ljava/io/InputStream;",
        &[],
    )
}

pub fn socket_output_stream(env: Env, socket: JObject) -> Result<JObject> {
    call_object(
        env,
        socket,
        "getOutputStream",
        "()Ljava/io/OutputStream;",
        &[],
    )
}

/// Read once from a `java.io.InputStream`. `Ok(None)` at end of stream.
pub fn stream_read(env: Env, stream: JObject, buffer: usize) -> Result<Option<Vec<u8>>> {
    let array = env.new_byte_array(&vec![0u8; buffer]);
    let n = call_int(env, stream, "read", "([B)I", &[JValue::object(array)])?;
    if n < 0 {
        return Ok(None);
    }
    let mut bytes = env.byte_array(array).unwrap_or_default();
    bytes.truncate(n as usize);
    Ok(Some(bytes))
}

pub fn stream_write(env: Env, stream: JObject, data: &[u8]) -> Result<()> {
    let array = env.new_byte_array(data);
    call_void(env, stream, "write", "([B)V", &[JValue::object(array)])?;
    call_void(env, stream, "flush", "()V", &[])
}

// ── Callback objects ────────────────────────────────────────────────────────

/// A generated callback class, instantiated and bound to a sink.
pub struct Callback {
    object: Ref,
}

impl Callback {
    /// Define the class if needed, make one, and route its events to `sink`.
    pub fn new(
        runtime: &Runtime,
        binary_name: &str,
        dex: Vec<u8>,
        natives: &[(&str, &str, *const std::ffi::c_void)],
        sink: Arc<dyn EventSink>,
    ) -> Result<Self> {
        let class = runtime.define_class(binary_name, dex, natives)?;
        let object = runtime.new_instance(class)?;
        let env = runtime.env()?;
        let object = Ref::new(env, object)
            .ok_or_else(|| Error::ClassLoad(format!("{binary_name} produced a null instance")))?;
        crate::bluetooth::bind_sink(&object, sink);
        Ok(Self { object })
    }

    pub fn as_ptr(&self) -> JObject {
        self.object.as_ptr()
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        crate::bluetooth::unbind_sink(&self.object);
    }
}

/// Discard events — useful where a callback is required but nothing listens.
pub struct Discard;

impl EventSink for Discard {
    fn emit(&self, _: Event) {}
}

// ── Peripheral role ─────────────────────────────────────────────────────────

/// `BluetoothGattService.SERVICE_TYPE_PRIMARY` / `_SECONDARY`.
const SERVICE_TYPE_PRIMARY: i32 = 0;
const SERVICE_TYPE_SECONDARY: i32 = 1;

/// `BluetoothGattServer.GATT_SUCCESS` and the ATT codes it forwards.
pub const ATT_SUCCESS: i32 = 0;

/// Map the shared permission bits onto Android's.
///
/// Android's constants are not the Bluetooth ones: read is `1` but write is
/// `16`, with the encrypted variants interleaved between them.
pub fn permissions_to_android(permissions: u32) -> i32 {
    const READABLE: u32 = 0x01;
    const WRITEABLE: u32 = 0x02;
    const READ_ENCRYPTED: u32 = 0x04;
    const WRITE_ENCRYPTED: u32 = 0x08;

    let mut out = 0;
    if permissions & READABLE != 0 {
        out |= 1; // PERMISSION_READ
    }
    if permissions & READ_ENCRYPTED != 0 {
        out |= 2; // PERMISSION_READ_ENCRYPTED
    }
    if permissions & WRITEABLE != 0 {
        out |= 16; // PERMISSION_WRITE
    }
    if permissions & WRITE_ENCRYPTED != 0 {
        out |= 32; // PERMISSION_WRITE_ENCRYPTED
    }
    out
}

/// `manager.openGattServer(context, callback)`.
pub fn open_gatt_server(env: Env, runtime: &Runtime, callback: JObject) -> Result<JObject> {
    let context_class = env
        .find_class("android/content/Context")
        .ok_or_else(|| Error::MissingClass("android.content.Context".into()))?;
    let field = env
        .static_field_id(context_class, "BLUETOOTH_SERVICE", "Ljava/lang/String;")
        .ok_or_else(|| missing("Context.BLUETOOTH_SERVICE"))?;
    let name = env.static_object_field(context_class, field);
    let manager = call_object(
        env,
        runtime.context(),
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue::object(name)],
    )?;
    call_object(
        env,
        manager,
        "openGattServer",
        "(Landroid/content/Context;Landroid/bluetooth/BluetoothGattServerCallback;)Landroid/bluetooth/BluetoothGattServer;",
        &[JValue::object(runtime.context()), JValue::object(callback)],
    )
}

/// `new BluetoothGattService(uuid, primary ? PRIMARY : SECONDARY)`.
pub fn new_service(env: Env, canonical: &str, primary: bool) -> Result<JObject> {
    let class = env
        .find_class("android/bluetooth/BluetoothGattService")
        .ok_or_else(|| Error::MissingClass("BluetoothGattService".into()))?;
    let ctor = env
        .method_id(class, "<init>", "(Ljava/util/UUID;I)V")
        .ok_or_else(|| missing("BluetoothGattService.<init>"))?;
    let uuid = uuid(env, canonical)?;
    let kind = if primary {
        SERVICE_TYPE_PRIMARY
    } else {
        SERVICE_TYPE_SECONDARY
    };
    Ok(env.new_object(class, ctor, &[JValue::object(uuid), JValue::int(kind)]))
}

/// `new BluetoothGattCharacteristic(uuid, properties, permissions)`.
pub fn new_characteristic(
    env: Env,
    canonical: &str,
    properties: u32,
    permissions: u32,
) -> Result<JObject> {
    let class = env
        .find_class("android/bluetooth/BluetoothGattCharacteristic")
        .ok_or_else(|| Error::MissingClass("BluetoothGattCharacteristic".into()))?;
    let ctor = env
        .method_id(class, "<init>", "(Ljava/util/UUID;II)V")
        .ok_or_else(|| missing("BluetoothGattCharacteristic.<init>"))?;
    let uuid = uuid(env, canonical)?;
    Ok(env.new_object(
        class,
        ctor,
        &[
            JValue::object(uuid),
            // Android's PROPERTY_* are the Bluetooth bits, unlike its
            // permissions.
            JValue::int(properties as i32),
            JValue::int(permissions_to_android(permissions)),
        ],
    ))
}

/// `new BluetoothGattDescriptor(uuid, permissions)`.
pub fn new_descriptor(env: Env, canonical: &str, permissions: u32) -> Result<JObject> {
    let class = env
        .find_class("android/bluetooth/BluetoothGattDescriptor")
        .ok_or_else(|| Error::MissingClass("BluetoothGattDescriptor".into()))?;
    let ctor = env
        .method_id(class, "<init>", "(Ljava/util/UUID;I)V")
        .ok_or_else(|| missing("BluetoothGattDescriptor.<init>"))?;
    let uuid = uuid(env, canonical)?;
    Ok(env.new_object(
        class,
        ctor,
        &[
            JValue::object(uuid),
            JValue::int(permissions_to_android(permissions)),
        ],
    ))
}

pub fn service_add_characteristic(
    env: Env,
    service: JObject,
    characteristic: JObject,
) -> Result<bool> {
    call_bool(
        env,
        service,
        "addCharacteristic",
        "(Landroid/bluetooth/BluetoothGattCharacteristic;)Z",
        &[JValue::object(characteristic)],
    )
}

pub fn characteristic_add_descriptor(
    env: Env,
    characteristic: JObject,
    descriptor: JObject,
) -> Result<bool> {
    call_bool(
        env,
        characteristic,
        "addDescriptor",
        "(Landroid/bluetooth/BluetoothGattDescriptor;)Z",
        &[JValue::object(descriptor)],
    )
}

pub fn set_value(env: Env, attribute: JObject, value: &[u8]) -> Result<bool> {
    let bytes = env.new_byte_array(value);
    call_bool(
        env,
        attribute,
        "setValue",
        "([B)Z",
        &[JValue::object(bytes)],
    )
}

pub fn server_add_service(env: Env, server: JObject, service: JObject) -> Result<bool> {
    call_bool(
        env,
        server,
        "addService",
        "(Landroid/bluetooth/BluetoothGattService;)Z",
        &[JValue::object(service)],
    )
}

pub fn server_remove_service(env: Env, server: JObject, service: JObject) -> Result<bool> {
    call_bool(
        env,
        server,
        "removeService",
        "(Landroid/bluetooth/BluetoothGattService;)Z",
        &[JValue::object(service)],
    )
}

pub fn server_clear_services(env: Env, server: JObject) -> Result<()> {
    call_void(env, server, "clearServices", "()V", &[])
}

pub fn server_close(env: Env, server: JObject) -> Result<()> {
    call_void(env, server, "close", "()V", &[])
}

/// `server.sendResponse(device, requestId, status, offset, value)`.
///
/// Every request that asked for a response must get exactly one, or the central
/// stalls until the ATT timeout.
pub fn server_send_response(
    env: Env,
    server: JObject,
    device: JObject,
    request_id: i32,
    status: i32,
    offset: i32,
    value: &[u8],
) -> Result<bool> {
    let bytes = env.new_byte_array(value);
    call_bool(
        env,
        server,
        "sendResponse",
        "(Landroid/bluetooth/BluetoothDevice;III[B)Z",
        &[
            JValue::object(device),
            JValue::int(request_id),
            JValue::int(status),
            JValue::int(offset),
            JValue::object(bytes),
        ],
    )
}

/// Push a value to one subscriber.
///
/// API 33 takes the bytes directly and returns a status; older releases want
/// the value set on the characteristic first. Both are tried, newest first.
pub fn server_notify(
    env: Env,
    server: JObject,
    device: JObject,
    characteristic: JObject,
    confirm: bool,
    value: &[u8],
) -> Result<bool> {
    let bytes = env.new_byte_array(value);

    if let Some(method) = env.method_id(
        env.get_object_class(server),
        "notifyCharacteristicChanged",
        "(Landroid/bluetooth/BluetoothDevice;Landroid/bluetooth/BluetoothGattCharacteristic;Z[B)I",
    ) {
        let status = env.call_int(
            server,
            method,
            &[
                JValue::object(device),
                JValue::object(characteristic),
                JValue::bool(confirm),
                JValue::object(bytes),
            ],
        );
        return Ok(status == ATT_SUCCESS);
    }

    set_value(env, characteristic, value)?;
    call_bool(
        env,
        server,
        "notifyCharacteristicChanged",
        "(Landroid/bluetooth/BluetoothDevice;Landroid/bluetooth/BluetoothGattCharacteristic;Z)Z",
        &[
            JValue::object(device),
            JValue::object(characteristic),
            JValue::bool(confirm),
        ],
    )
}

/// The centrals connected to the GATT server.
pub fn server_connected_devices(env: Env, runtime: &Runtime) -> Result<JObject> {
    let context_class = env
        .find_class("android/content/Context")
        .ok_or_else(|| Error::MissingClass("android.content.Context".into()))?;
    let field = env
        .static_field_id(context_class, "BLUETOOTH_SERVICE", "Ljava/lang/String;")
        .ok_or_else(|| missing("Context.BLUETOOTH_SERVICE"))?;
    let name = env.static_object_field(context_class, field);
    let manager = call_object(
        env,
        runtime.context(),
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue::object(name)],
    )?;
    // BluetoothProfile.GATT_SERVER == 8
    call_object(
        env,
        manager,
        "getConnectedDevices",
        "(I)Ljava/util/List;",
        &[JValue::int(8)],
    )
}

pub fn device_address(env: Env, device: JObject) -> Option<String> {
    let s = call_object(env, device, "getAddress", "()Ljava/lang/String;", &[]).ok()?;
    env.get_string(s)
}

// ── Advertising ─────────────────────────────────────────────────────────────

/// Start advertising.
///
/// Android will not put an arbitrary name in the payload: `AdvertiseData` only
/// offers `setIncludeDeviceName`, which uses the *adapter's* name. Changing
/// that means `BluetoothAdapter.setName`, which is global to the device — so
/// [`Advertising::local_name`] is honoured by including the device name, not by
/// overriding it.
///
/// [`Advertising::local_name`]: crate::ble
pub fn start_advertising(
    env: Env,
    advertiser: JObject,
    callback: JObject,
    include_name: bool,
    service_uuids: &[String],
    connectable: bool,
) -> Result<()> {
    let settings = build_advertise_settings(env, connectable)?;
    let data = build_advertise_data(env, include_name, service_uuids)?;
    call_void(
        env,
        advertiser,
        "startAdvertising",
        concat!(
            "(Landroid/bluetooth/le/AdvertiseSettings;",
            "Landroid/bluetooth/le/AdvertiseData;",
            "Landroid/bluetooth/le/AdvertiseCallback;)V"
        ),
        &[
            JValue::object(settings),
            JValue::object(data),
            JValue::object(callback),
        ],
    )
}

pub fn stop_advertising(env: Env, advertiser: JObject, callback: JObject) -> Result<()> {
    call_void(
        env,
        advertiser,
        "stopAdvertising",
        "(Landroid/bluetooth/le/AdvertiseCallback;)V",
        &[JValue::object(callback)],
    )
}

fn build_advertise_settings(env: Env, connectable: bool) -> Result<JObject> {
    let class = env
        .find_class("android/bluetooth/le/AdvertiseSettings$Builder")
        .ok_or_else(|| Error::MissingClass("AdvertiseSettings.Builder".into()))?;
    let ctor = env
        .method_id(class, "<init>", "()V")
        .ok_or_else(|| missing("AdvertiseSettings.Builder.<init>"))?;
    let builder = env.new_object(class, ctor, &[]);

    // ADVERTISE_MODE_LOW_LATENCY = 2: a peripheral is meant to be found.
    call_object(
        env,
        builder,
        "setAdvertiseMode",
        "(I)Landroid/bluetooth/le/AdvertiseSettings$Builder;",
        &[JValue::int(2)],
    )?;
    call_object(
        env,
        builder,
        "setConnectable",
        "(Z)Landroid/bluetooth/le/AdvertiseSettings$Builder;",
        &[JValue::bool(connectable)],
    )?;
    call_object(
        env,
        builder,
        "build",
        "()Landroid/bluetooth/le/AdvertiseSettings;",
        &[],
    )
}

fn build_advertise_data(env: Env, include_name: bool, service_uuids: &[String]) -> Result<JObject> {
    let class = env
        .find_class("android/bluetooth/le/AdvertiseData$Builder")
        .ok_or_else(|| Error::MissingClass("AdvertiseData.Builder".into()))?;
    let ctor = env
        .method_id(class, "<init>", "()V")
        .ok_or_else(|| missing("AdvertiseData.Builder.<init>"))?;
    let builder = env.new_object(class, ctor, &[]);

    call_object(
        env,
        builder,
        "setIncludeDeviceName",
        "(Z)Landroid/bluetooth/le/AdvertiseData$Builder;",
        &[JValue::bool(include_name)],
    )?;
    for canonical in service_uuids {
        let parcel = parcel_uuid(env, canonical)?;
        call_object(
            env,
            builder,
            "addServiceUuid",
            "(Landroid/os/ParcelUuid;)Landroid/bluetooth/le/AdvertiseData$Builder;",
            &[JValue::object(parcel)],
        )?;
    }
    call_object(
        env,
        builder,
        "build",
        "()Landroid/bluetooth/le/AdvertiseData;",
        &[],
    )
}

// ── L2CAP, listening side ───────────────────────────────────────────────────

/// `adapter.listenUsingInsecureL2capChannel()` — API 29+.
///
/// Returns a `BluetoothServerSocket`; its PSM is assigned by the system and
/// read back with `getPsm()`.
pub fn listen_l2cap(env: Env, adapter: JObject, secure: bool) -> Result<JObject> {
    let name = if secure {
        "listenUsingL2capChannel"
    } else {
        "listenUsingInsecureL2capChannel"
    };
    call_object(
        env,
        adapter,
        name,
        "()Landroid/bluetooth/BluetoothServerSocket;",
        &[],
    )
}

pub fn server_socket_psm(env: Env, socket: JObject) -> Result<i32> {
    call_int(env, socket, "getPsm", "()I", &[])
}

/// Block until a central connects, then return the accepted socket.
pub fn server_socket_accept(env: Env, socket: JObject) -> Result<JObject> {
    call_object(
        env,
        socket,
        "accept",
        "()Landroid/bluetooth/BluetoothSocket;",
        &[],
    )
}

pub fn server_socket_close(env: Env, socket: JObject) -> Result<()> {
    call_void(env, socket, "close", "()V", &[])
}
