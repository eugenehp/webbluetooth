//! Check the JNI vtable indices against a real JVM.
//!
//! [`crate::jni`](webbluetooth_android::jni) reaches JNI by *index* into the
//! function table, because that table's layout is fixed by the specification
//! and identical on every VM. That makes a wrong index silent memory
//! corruption rather than a compile error — so these tests start an actual JVM
//! through the Invocation API and exercise every slot the crate uses.
//!
//! JNI is JNI: a table that works against OpenJDK works against ART, because
//! both implement the same specification. This does not test Bluetooth, which
//! needs a device; it tests that the plumbing underneath it is wired correctly.
//!
//! Skipped when no `libjvm` can be found, so `cargo test` on a machine without
//! a JDK still passes. The Docker image has one.

use std::ffi::{c_char, c_void, CString};
use webbluetooth_android::jni::{Env, JValue, Vm, JNI_VERSION_1_6};

#[repr(C)]
struct JavaVmOption {
    option_string: *const c_char,
    extra_info: *mut c_void,
}

#[repr(C)]
struct JavaVmInitArgs {
    version: i32,
    n_options: i32,
    options: *mut JavaVmOption,
    ignore_unrecognized: u8,
}

unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: i32 = 2;

/// Find `libjvm` wherever this system keeps it.
fn load_libjvm() -> Option<*mut c_void> {
    let mut candidates: Vec<String> = vec![
        "libjvm.so".into(),
        "/usr/lib/jvm/default-java/lib/server/libjvm.so".into(),
    ];
    // Any JDK under /usr/lib/jvm will do.
    if let Ok(entries) = std::fs::read_dir("/usr/lib/jvm") {
        for entry in entries.flatten() {
            candidates.push(format!("{}/lib/server/libjvm.so", entry.path().display()));
        }
    }
    if let Ok(home) = std::env::var("JAVA_HOME") {
        candidates.push(format!("{home}/lib/server/libjvm.so"));
    }

    for path in candidates {
        let Ok(c) = CString::new(path) else { continue };
        let handle = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
        if !handle.is_null() {
            return Some(handle);
        }
    }
    None
}

/// Start a JVM, or `None` if there is none to start.
///
/// **A process may create exactly one.** These tests run in parallel threads,
/// so without a shared instance the second `JNI_CreateJavaVM` fails and its
/// test either skips silently or trips over a half-built VM. One `OnceLock`
/// removes the race and matches what the API actually allows.
fn jvm() -> Option<(Vm, Env)> {
    static VM: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let raw = (*VM.get_or_init(|| create_jvm().map(|vm| vm.0 as usize)))?;
    let vm = Vm(raw as *mut _);
    // Each thread needs its own environment even when the VM is shared.
    let env = vm.attach()?;
    Some((vm, env))
}

fn create_jvm() -> Option<Vm> {
    let handle = load_libjvm()?;
    let symbol = CString::new("JNI_CreateJavaVM").ok()?;
    let create = unsafe { dlsym(handle, symbol.as_ptr()) };
    if create.is_null() {
        return None;
    }
    type CreateJavaVm =
        unsafe extern "C" fn(*mut *mut c_void, *mut *mut c_void, *mut c_void) -> i32;
    let create: CreateJavaVm = unsafe { std::mem::transmute(create) };

    let mut args = JavaVmInitArgs {
        version: JNI_VERSION_1_6,
        n_options: 0,
        options: std::ptr::null_mut(),
        ignore_unrecognized: 1,
    };
    let (mut vm, mut env): (*mut c_void, *mut c_void) =
        (std::ptr::null_mut(), std::ptr::null_mut());
    let rc = unsafe { create(&mut vm, &mut env, &mut args as *mut _ as *mut c_void) };
    if rc != 0 || vm.is_null() || env.is_null() {
        return None;
    }
    let _ = env;
    Some(Vm(vm as *mut _))
}

#[test]
fn every_jni_slot_we_use_behaves() {
    let Some((vm, env)) = jvm() else {
        eprintln!("no libjvm — skipping");
        return;
    };

    // ── GetVersion ──────────────────────────────────────────────────────────
    let version = env.version();
    assert!(
        version >= JNI_VERSION_1_6,
        "JNI version {version:#x} looks wrong"
    );

    // ── FindClass, GetMethodID, NewObject ───────────────────────────────────
    let string_class = env
        .find_class("java/lang/String")
        .expect("no java/lang/String");
    let object_class = env
        .find_class("java/lang/Object")
        .expect("no java/lang/Object");
    assert!(
        env.find_class("does/not/Exist").is_none(),
        "a missing class should be None"
    );

    // ── NewStringUTF / GetStringUTFChars ────────────────────────────────────
    let hello = env.new_string("héllo ☕");
    assert!(!hello.is_null());
    assert_eq!(
        env.get_string(hello).as_deref(),
        Some("héllo ☕"),
        "string round trip"
    );
    assert_eq!(env.get_string(std::ptr::null_mut()), None);

    // ── CallIntMethodA ──────────────────────────────────────────────────────
    let length = env
        .method_id(string_class, "length", "()I")
        .expect("String.length");
    assert_eq!(
        env.call_int(hello, length, &[]),
        7,
        "UTF-16 length of 'héllo ☕'"
    );

    // ── CallObjectMethodA, with an argument ─────────────────────────────────
    let concat = env
        .method_id(
            string_class,
            "concat",
            "(Ljava/lang/String;)Ljava/lang/String;",
        )
        .expect("String.concat");
    let world = env.new_string("!");
    let joined = env.call_object(hello, concat, &[JValue::object(world)]);
    assert_eq!(env.get_string(joined).as_deref(), Some("héllo ☕!"));

    // ── CallBooleanMethodA ──────────────────────────────────────────────────
    let equals = env
        .method_id(object_class, "equals", "(Ljava/lang/Object;)Z")
        .expect("equals");
    assert!(env.call_bool(hello, equals, &[JValue::object(hello)]));
    assert!(!env.call_bool(hello, equals, &[JValue::object(world)]));

    // ── GetStaticMethodID / CallStaticObjectMethodA ─────────────────────────
    let integer = env
        .find_class("java/lang/Integer")
        .expect("java/lang/Integer");
    let to_string = env
        .static_method_id(integer, "toString", "(I)Ljava/lang/String;")
        .expect("Integer.toString");
    let n = env.call_static_object(integer, to_string, &[JValue::int(-42)]);
    assert_eq!(env.get_string(n).as_deref(), Some("-42"));

    // ── CallStaticIntMethodA ────────────────────────────────────────────────
    let parse = env
        .static_method_id(integer, "parseInt", "(Ljava/lang/String;)I")
        .expect("parseInt");
    let numeric = env.new_string("1234");
    assert_eq!(
        env.call_static_int(integer, parse, &[JValue::object(numeric)]),
        1234
    );

    // ── GetStaticFieldID / GetStaticIntField ────────────────────────────────
    let max = env
        .static_field_id(integer, "MAX_VALUE", "I")
        .expect("Integer.MAX_VALUE");
    assert_eq!(env.static_int_field(integer, max), i32::MAX);

    // ── CallLongMethodA ─────────────────────────────────────────────────────
    let long_class = env.find_class("java/lang/Long").expect("java/lang/Long");
    let value_of = env
        .static_method_id(long_class, "valueOf", "(J)Ljava/lang/Long;")
        .expect("valueOf");
    let boxed = env.call_static_object(long_class, value_of, &[JValue::long(1 << 40)]);
    let long_value = env
        .method_id(long_class, "longValue", "()J")
        .expect("longValue");
    assert_eq!(env.call_long(boxed, long_value, &[]), 1 << 40);

    // ── byte[] round trip ───────────────────────────────────────────────────
    let bytes = env.new_byte_array(&[0xDE, 0xAD, 0xBE, 0xEF]);
    assert_eq!(env.array_length(bytes), 4);
    assert_eq!(env.byte_array(bytes), Some(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    assert_eq!(env.byte_array(env.new_byte_array(&[])), Some(vec![]));
    assert_eq!(env.byte_array(std::ptr::null_mut()), None);

    // ── Object arrays ───────────────────────────────────────────────────────
    let array = env.new_object_array(2, string_class, std::ptr::null_mut());
    env.set_object_array_element(array, 0, hello);
    env.set_object_array_element(array, 1, world);
    assert_eq!(env.array_length(array), 2);
    assert_eq!(
        env.get_string(env.object_array_element(array, 1))
            .as_deref(),
        Some("!")
    );

    // ── GetObjectClass / IsSameObject ───────────────────────────────────────
    let of_hello = env.get_object_class(hello);
    assert!(
        env.is_same_object(of_hello, string_class),
        "getClass should be String"
    );
    assert!(!env.is_same_object(hello, world));

    // ── Global references outlive the frame ─────────────────────────────────
    let global = env.new_global_ref(hello);
    assert!(env.is_same_object(global, hello));
    env.delete_global_ref(global);

    // ── Local frames ────────────────────────────────────────────────────────
    assert!(env.push_local_frame(16), "PushLocalFrame failed");
    let scoped = env.new_string("temporary");
    assert_eq!(env.get_string(scoped).as_deref(), Some("temporary"));
    env.pop_local_frame();

    // ── Exceptions are caught and cleared, not left pending ─────────────────
    // parseInt("nope") throws; every wrapper must return with a clean state.
    let bad = env.new_string("nope");
    let _ = env.call_static_int(integer, parse, &[JValue::object(bad)]);
    assert!(
        env.exception_occurred().is_null(),
        "an exception was left pending — every later JNI call would be undefined"
    );
    // …and the environment still works afterwards.
    assert_eq!(
        env.call_static_int(integer, parse, &[JValue::object(numeric)]),
        1234
    );

    // ── RegisterNatives ─────────────────────────────────────────────────────
    // The whole Android approach rests on this slot. Registering a method that
    // does not exist must *fail cleanly* — if the index were wrong we would be
    // calling SetDoubleArrayRegion instead, and corrupt memory rather than
    // return an error.
    let name = CString::new("noSuchNative").unwrap();
    let signature = CString::new("()V").unwrap();
    extern "C" fn never_called() {}
    let bogus = [webbluetooth_android::jni::NativeMethod {
        name: name.as_ptr(),
        signature: signature.as_ptr(),
        function: never_called as *const c_void,
    }];
    assert!(
        !env.register_natives(string_class, &bogus),
        "registering a method String does not declare should fail"
    );
    assert!(
        env.exception_occurred().is_null(),
        "the failure left an exception pending"
    );

    // ── The VM hands back the same env on an attached thread ────────────────
    let from_vm = vm.env().expect("GetEnv on an attached thread");
    assert_eq!(from_vm.version(), version);
}

/// A local and a global reference to the same object are different pointers.
///
/// The sink registry used to be a `HashMap` keyed by the global reference's
/// value, looked up with the *local* reference JNI hands a native method. It
/// never matched, so every callback event was dropped: no scan results, no
/// connection changes, no notifications. Nothing failed — the app simply waited
/// forever.
///
/// `IsSameObject` is the only way to ask whether two references denote the same
/// object, and this pins down why.
#[test]
fn a_local_and_a_global_reference_differ_in_value() {
    let Some((_vm, env)) = jvm() else {
        eprintln!("no libjvm — skipping");
        return;
    };
    let class = env.find_class("java/lang/Object").expect("Object");
    let ctor = env
        .method_id(class, "<init>", "()V")
        .expect("Object.<init>");
    let local = env.new_object(class, ctor, &[]);
    assert!(!local.is_null());

    let global = env.new_global_ref(local);
    assert!(!global.is_null());

    assert_ne!(
        local as usize, global as usize,
        "if these were equal the old pointer-keyed lookup would have worked, \
         and this test would not be needed"
    );
    assert!(
        env.is_same_object(local, global),
        "different values, same object — which is the whole point"
    );

    // And a reference to a *different* object must not be mistaken for it.
    let other = env.new_object(class, ctor, &[]);
    assert!(!env.is_same_object(other, global));

    env.delete_global_ref(global);
}

#[test]
fn attaching_a_thread_gives_a_working_env() {
    let Some((vm, _)) = jvm() else {
        eprintln!("no libjvm — skipping");
        return;
    };
    // Every Bluetooth callback arrives on a VM thread, but the crate's own
    // threads have to attach themselves.
    let handle = std::thread::spawn(move || {
        let env = vm.attach().expect("AttachCurrentThreadAsDaemon failed");
        let class = env
            .find_class("java/lang/String")
            .expect("String on an attached thread");
        assert!(!class.is_null());
        let s = env.new_string("from another thread");
        assert_eq!(env.get_string(s).as_deref(), Some("from another thread"));
        vm.detach();
    });
    handle.join().expect("the attached thread panicked");
}
