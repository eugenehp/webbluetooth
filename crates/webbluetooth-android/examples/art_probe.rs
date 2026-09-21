//! Load the generated classes and bind their natives, inside real ART.
//!
//! ```sh
//! ./scripts/android/art.sh    # builds, pushes and runs this
//! ```
//!
//! `dexdump` parses a DEX and a desktop JVM has no `android.bluetooth` to
//! inherit from, so neither exercises the part of this crate that actually
//! runs on a device: wrapping the DEX in a direct `ByteBuffer`, handing it to
//! `InMemoryDexClassLoader`, and binding each `native` method with
//! `RegisterNatives`. That is where a wrong JNI slot or a malformed DEX would
//! show up, and none of it had ever been executed.
//!
//! This is loaded by `dalvikvm`, which has no `Context`. The `Context` is only
//! needed to reach the Bluetooth system services, which this deliberately does
//! not touch, so a plain `java.lang.Object` stands in for it.

use std::ffi::c_void;
use std::sync::OnceLock;
use webbluetooth_android::jni::{Env, JClass, JObject, JString, Vm, JNI_VERSION_1_6};
use webbluetooth_android::runtime::Runtime;

static VM: OnceLock<usize> = OnceLock::new();

/// ART calls this on `System.load`, and it is the only place a native library
/// is handed the `JavaVM` — an `Env` cannot produce one.
#[no_mangle]
pub extern "C" fn JNI_OnLoad(vm: Vm, _reserved: *mut c_void) -> i32 {
    let _ = VM.set(vm.0 as usize);
    JNI_VERSION_1_6
}

/// A native method the probe registers by hand, so that calling it proves
/// `RegisterNatives` bound something real.
extern "C" fn touched(_env: Env, _this: JObject, _a: JObject, _b: i32, _c: i32) {}

#[no_mangle]
pub extern "C" fn Java_Probe_run(env: Env, _class: JClass) -> JString {
    let mut report = String::new();
    let Some(raw) = VM.get() else {
        return env.new_string("JNI_OnLoad never ran");
    };
    let vm = Vm(*raw as *mut _);

    // Stand-in Context: non-null so `init` does not go looking for an
    // application that does not exist in this process.
    let Some(object_class) = env.find_class("java/lang/Object") else {
        return env.new_string("no java.lang.Object");
    };
    let Some(ctor) = env.method_id(object_class, "<init>", "()V") else {
        return env.new_string("no Object.<init>");
    };
    let stand_in = env.new_object(object_class, ctor, &[]);

    let runtime = match Runtime::init(vm, stand_in) {
        Ok(r) => r,
        Err(e) => return env.new_string(&format!("Runtime::init failed: {e}")),
    };

    for (name, dex) in webbluetooth_android::dex_classes() {
        let binary = format!("dev.webbluetooth.{}", class_of(name));

        // `define_class` registers the natives as part of loading, so binding
        // one real method here is what puts `RegisterNatives` on the path. The
        // signature has to match the DEX exactly or the call fails — which is
        // the point: a wrong slot index or a malformed method table shows up
        // as a failure to bind, not as a crash later.
        let natives: Vec<(&str, &str, *const c_void)> = match name {
            "gatt" => vec![(
                "onConnectionStateChange",
                "(Landroid/bluetooth/BluetoothGatt;II)V",
                touched as *const c_void,
            )],
            _ => vec![],
        };
        let bound = natives.len();

        match runtime.define_class(&binary, dex, &natives) {
            Ok(class) => {
                let instance = runtime.new_instance(class);
                report.push_str(&format!(
                    "  {binary}: loaded, instantiated={}, natives bound={bound}\n",
                    instance.map(|o| !o.is_null()).unwrap_or(false)
                ));
            }
            Err(e) => report.push_str(&format!("  {binary}: FAILED {e}\n")),
        }
    }

    env.new_string(&report)
}

/// `gatt` → `GattCallback`, matching what `dex_classes` names each blob.
fn class_of(name: &str) -> &'static str {
    match name {
        "gatt" => "GattCallback",
        "scan" => "ScanCallback",
        "server" => "GattServerCallback",
        "advertise" => "AdvertiseCallback",
        _ => "Unknown",
    }
}
