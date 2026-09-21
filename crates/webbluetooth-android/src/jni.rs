//! JNI, spoken directly.
//!
//! `JNIEnv` is a pointer to a pointer to a table of function pointers, and the
//! table's layout is fixed by the JNI specification and identical on every VM —
//! so it needs no binding crate, only the right indices.
//!
//! Indices are named as constants below rather than expressed as a `struct`
//! with two hundred reserved fields. Getting one wrong is silent memory
//! corruption rather than a compile error, so `tests/jvm.rs` calls a
//! representative set against a real JVM to check them.
//!
//! # Only the `A` call variants
//!
//! JNI offers three forms of every `Call*Method`: variadic, `va_list`, and an
//! array of `jvalue`. Only the last is used here. The variadic form cannot be
//! called correctly from Rust on AArch64 — arguments would be passed in
//! registers where the callee looks on the stack — which is the same hazard
//! `objc_msgSend` poses on Apple, and it gets the same answer.

// Every JNI reference is a raw pointer, and these methods hand them to the VM
// rather than dereferencing them in Rust — the VM is what validates them, and
// it does so far better than a null check here could. Marking each one `unsafe`
// would put an `unsafe` block around every BLE call in the crate without
// telling the reader anything the type does not already say: a `JObject` must
// be a live reference, obtained from JNI and not yet deleted.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_char, c_void};

/// A JNI local or global reference to a Java object.
///
/// Opaque: the pointer is the VM's, and dereferencing it is not a thing this
/// crate ever does.
pub type JObject = *mut c_void;
/// A reference to a `java.lang.Class`.
pub type JClass = JObject;
/// A reference to a `java.lang.String`.
pub type JString = JObject;
/// A reference to a Java array.
pub type JArray = JObject;
/// A resolved method, from `GetMethodID` or `GetStaticMethodID`.
///
/// Valid only while the class that produced it is loaded.
pub type JMethodId = *mut c_void;
/// A resolved field, from `GetFieldID` or `GetStaticFieldID`.
pub type JFieldId = *mut c_void;

/// A `jvalue`, the union JNI passes arguments in.
#[repr(C)]
#[derive(Clone, Copy)]
pub union JValue {
    pub z: u8,
    pub b: i8,
    pub c: u16,
    pub s: i16,
    pub i: i32,
    pub j: i64,
    pub f: f32,
    pub d: f64,
    pub l: JObject,
}

impl JValue {
    pub fn object(v: JObject) -> Self {
        Self { l: v }
    }
    pub fn int(v: i32) -> Self {
        Self { i: v }
    }
    pub fn long(v: i64) -> Self {
        Self { j: v }
    }
    pub fn bool(v: bool) -> Self {
        Self { z: u8::from(v) }
    }
    pub fn byte(v: i8) -> Self {
        Self { b: v }
    }
}

/// One entry in a `RegisterNatives` table.
#[repr(C)]
pub struct NativeMethod {
    pub name: *const c_char,
    pub signature: *const c_char,
    pub function: *const c_void,
}

// JNI 1.6 has 233 function-pointer slots. Naming the ones used, with the index
// each occupies in `JNINativeInterface_`.
mod slot {
    pub const GET_VERSION: usize = 4;
    pub const FIND_CLASS: usize = 6;
    pub const THROW_NEW: usize = 14;
    pub const EXCEPTION_OCCURRED: usize = 15;
    pub const EXCEPTION_DESCRIBE: usize = 16;
    pub const EXCEPTION_CLEAR: usize = 17;
    pub const PUSH_LOCAL_FRAME: usize = 19;
    pub const POP_LOCAL_FRAME: usize = 20;
    pub const NEW_GLOBAL_REF: usize = 21;
    pub const DELETE_GLOBAL_REF: usize = 22;
    pub const DELETE_LOCAL_REF: usize = 23;
    pub const IS_SAME_OBJECT: usize = 24;
    pub const GET_OBJECT_CLASS: usize = 31;
    pub const GET_METHOD_ID: usize = 33;
    pub const CALL_OBJECT_METHOD_A: usize = 36;
    pub const CALL_BOOLEAN_METHOD_A: usize = 39;
    pub const CALL_INT_METHOD_A: usize = 51;
    pub const CALL_LONG_METHOD_A: usize = 54;
    pub const CALL_VOID_METHOD_A: usize = 63;
    pub const GET_STATIC_METHOD_ID: usize = 113;
    pub const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
    pub const CALL_STATIC_INT_METHOD_A: usize = 131;
    pub const CALL_STATIC_VOID_METHOD_A: usize = 143;
    pub const GET_STATIC_FIELD_ID: usize = 144;
    pub const GET_STATIC_OBJECT_FIELD: usize = 145;
    pub const GET_STATIC_INT_FIELD: usize = 150;
    pub const NEW_STRING_UTF: usize = 167;
    pub const GET_STRING_UTF_CHARS: usize = 169;
    pub const RELEASE_STRING_UTF_CHARS: usize = 170;
    pub const GET_ARRAY_LENGTH: usize = 171;
    pub const NEW_OBJECT_ARRAY: usize = 172;
    pub const GET_OBJECT_ARRAY_ELEMENT: usize = 173;
    pub const SET_OBJECT_ARRAY_ELEMENT: usize = 174;
    pub const NEW_BYTE_ARRAY: usize = 176;
    pub const GET_BYTE_ARRAY_ELEMENTS: usize = 184;
    pub const RELEASE_BYTE_ARRAY_ELEMENTS: usize = 192;
    pub const REGISTER_NATIVES: usize = 215;
    pub const NEW_DIRECT_BYTE_BUFFER: usize = 229;
    pub const NEW_OBJECT_A: usize = 30;
    pub const ALLOC_OBJECT: usize = 27;
}

/// A JNI environment pointer, valid only on the thread that produced it.
///
/// `repr(transparent)` because the VM passes this as a bare `JNIEnv*` to every
/// native method — it must have exactly a pointer's layout.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Env(pub *mut *const [*const c_void; 234]);

impl Env {
    #[inline]
    fn f<T: Copy>(&self, index: usize) -> T {
        debug_assert!(!self.0.is_null(), "null JNIEnv");
        // SAFETY: the table is owned by the VM and outlives every use; the
        // caller has picked an index whose signature matches `T`.
        unsafe {
            let table = *self.0;
            core::mem::transmute_copy(&(*table)[index])
        }
    }

    pub fn version(&self) -> i32 {
        let f: unsafe extern "C" fn(Env) -> i32 = self.f(slot::GET_VERSION);
        unsafe { f(*self) }
    }

    /// `FindClass`, with an internal name like `android/bluetooth/BluetoothAdapter`.
    ///
    /// Returns a **local** reference; promote it with
    /// [`Env::new_global_ref`] to keep it past the current frame.
    pub fn find_class(&self, name: &str) -> Option<JClass> {
        let name = cstring(name);
        let f: unsafe extern "C" fn(Env, *const c_char) -> JClass = self.f(slot::FIND_CLASS);
        let class = unsafe { f(*self, name.as_ptr()) };
        self.check_exception();
        (!class.is_null()).then_some(class)
    }

    pub fn get_object_class(&self, object: JObject) -> JClass {
        let f: unsafe extern "C" fn(Env, JObject) -> JClass = self.f(slot::GET_OBJECT_CLASS);
        unsafe { f(*self, object) }
    }

    pub fn method_id(&self, class: JClass, name: &str, signature: &str) -> Option<JMethodId> {
        let (name, signature) = (cstring(name), cstring(signature));
        let f: unsafe extern "C" fn(Env, JClass, *const c_char, *const c_char) -> JMethodId =
            self.f(slot::GET_METHOD_ID);
        let id = unsafe { f(*self, class, name.as_ptr(), signature.as_ptr()) };
        self.check_exception();
        (!id.is_null()).then_some(id)
    }

    pub fn static_method_id(
        &self,
        class: JClass,
        name: &str,
        signature: &str,
    ) -> Option<JMethodId> {
        let (name, signature) = (cstring(name), cstring(signature));
        let f: unsafe extern "C" fn(Env, JClass, *const c_char, *const c_char) -> JMethodId =
            self.f(slot::GET_STATIC_METHOD_ID);
        let id = unsafe { f(*self, class, name.as_ptr(), signature.as_ptr()) };
        self.check_exception();
        (!id.is_null()).then_some(id)
    }

    pub fn static_field_id(&self, class: JClass, name: &str, signature: &str) -> Option<JFieldId> {
        let (name, signature) = (cstring(name), cstring(signature));
        let f: unsafe extern "C" fn(Env, JClass, *const c_char, *const c_char) -> JFieldId =
            self.f(slot::GET_STATIC_FIELD_ID);
        let id = unsafe { f(*self, class, name.as_ptr(), signature.as_ptr()) };
        self.check_exception();
        (!id.is_null()).then_some(id)
    }

    pub fn static_object_field(&self, class: JClass, field: JFieldId) -> JObject {
        let f: unsafe extern "C" fn(Env, JClass, JFieldId) -> JObject =
            self.f(slot::GET_STATIC_OBJECT_FIELD);
        unsafe { f(*self, class, field) }
    }

    pub fn static_int_field(&self, class: JClass, field: JFieldId) -> i32 {
        let f: unsafe extern "C" fn(Env, JClass, JFieldId) -> i32 =
            self.f(slot::GET_STATIC_INT_FIELD);
        unsafe { f(*self, class, field) }
    }

    // ── Calls ───────────────────────────────────────────────────────────────

    pub fn call_object(&self, object: JObject, method: JMethodId, args: &[JValue]) -> JObject {
        let f: unsafe extern "C" fn(Env, JObject, JMethodId, *const JValue) -> JObject =
            self.f(slot::CALL_OBJECT_METHOD_A);
        let out = unsafe { f(*self, object, method, args.as_ptr()) };
        self.check_exception();
        out
    }

    pub fn call_bool(&self, object: JObject, method: JMethodId, args: &[JValue]) -> bool {
        let f: unsafe extern "C" fn(Env, JObject, JMethodId, *const JValue) -> u8 =
            self.f(slot::CALL_BOOLEAN_METHOD_A);
        let out = unsafe { f(*self, object, method, args.as_ptr()) } != 0;
        self.check_exception();
        out
    }

    pub fn call_int(&self, object: JObject, method: JMethodId, args: &[JValue]) -> i32 {
        let f: unsafe extern "C" fn(Env, JObject, JMethodId, *const JValue) -> i32 =
            self.f(slot::CALL_INT_METHOD_A);
        let out = unsafe { f(*self, object, method, args.as_ptr()) };
        self.check_exception();
        out
    }

    pub fn call_long(&self, object: JObject, method: JMethodId, args: &[JValue]) -> i64 {
        let f: unsafe extern "C" fn(Env, JObject, JMethodId, *const JValue) -> i64 =
            self.f(slot::CALL_LONG_METHOD_A);
        let out = unsafe { f(*self, object, method, args.as_ptr()) };
        self.check_exception();
        out
    }

    pub fn call_void(&self, object: JObject, method: JMethodId, args: &[JValue]) {
        let f: unsafe extern "C" fn(Env, JObject, JMethodId, *const JValue) =
            self.f(slot::CALL_VOID_METHOD_A);
        unsafe { f(*self, object, method, args.as_ptr()) };
        self.check_exception();
    }

    pub fn call_static_object(&self, class: JClass, method: JMethodId, args: &[JValue]) -> JObject {
        let f: unsafe extern "C" fn(Env, JClass, JMethodId, *const JValue) -> JObject =
            self.f(slot::CALL_STATIC_OBJECT_METHOD_A);
        let out = unsafe { f(*self, class, method, args.as_ptr()) };
        self.check_exception();
        out
    }

    pub fn call_static_int(&self, class: JClass, method: JMethodId, args: &[JValue]) -> i32 {
        let f: unsafe extern "C" fn(Env, JClass, JMethodId, *const JValue) -> i32 =
            self.f(slot::CALL_STATIC_INT_METHOD_A);
        let out = unsafe { f(*self, class, method, args.as_ptr()) };
        self.check_exception();
        out
    }

    pub fn call_static_void(&self, class: JClass, method: JMethodId, args: &[JValue]) {
        let f: unsafe extern "C" fn(Env, JClass, JMethodId, *const JValue) =
            self.f(slot::CALL_STATIC_VOID_METHOD_A);
        unsafe { f(*self, class, method, args.as_ptr()) };
        self.check_exception();
    }

    pub fn new_object(&self, class: JClass, ctor: JMethodId, args: &[JValue]) -> JObject {
        let f: unsafe extern "C" fn(Env, JClass, JMethodId, *const JValue) -> JObject =
            self.f(slot::NEW_OBJECT_A);
        let out = unsafe { f(*self, class, ctor, args.as_ptr()) };
        self.check_exception();
        out
    }

    /// Allocate without running a constructor.
    pub fn alloc_object(&self, class: JClass) -> JObject {
        let f: unsafe extern "C" fn(Env, JClass) -> JObject = self.f(slot::ALLOC_OBJECT);
        let out = unsafe { f(*self, class) };
        self.check_exception();
        out
    }

    // ── References ──────────────────────────────────────────────────────────

    pub fn new_global_ref(&self, object: JObject) -> JObject {
        let f: unsafe extern "C" fn(Env, JObject) -> JObject = self.f(slot::NEW_GLOBAL_REF);
        unsafe { f(*self, object) }
    }

    pub fn delete_global_ref(&self, object: JObject) {
        let f: unsafe extern "C" fn(Env, JObject) = self.f(slot::DELETE_GLOBAL_REF);
        unsafe { f(*self, object) };
    }

    pub fn delete_local_ref(&self, object: JObject) {
        let f: unsafe extern "C" fn(Env, JObject) = self.f(slot::DELETE_LOCAL_REF);
        unsafe { f(*self, object) };
    }

    pub fn is_same_object(&self, a: JObject, b: JObject) -> bool {
        let f: unsafe extern "C" fn(Env, JObject, JObject) -> u8 = self.f(slot::IS_SAME_OBJECT);
        unsafe { f(*self, a, b) != 0 }
    }

    /// Push a local frame, so a burst of locals is released together.
    ///
    /// The default local-reference table is small — 512 entries on Android —
    /// and a scan callback that leaks a few per advertisement exhausts it.
    pub fn push_local_frame(&self, capacity: i32) -> bool {
        let f: unsafe extern "C" fn(Env, i32) -> i32 = self.f(slot::PUSH_LOCAL_FRAME);
        unsafe { f(*self, capacity) == 0 }
    }

    pub fn pop_local_frame(&self) {
        let f: unsafe extern "C" fn(Env, JObject) -> JObject = self.f(slot::POP_LOCAL_FRAME);
        unsafe { f(*self, core::ptr::null_mut()) };
    }

    // ── Strings and arrays ──────────────────────────────────────────────────

    pub fn new_string(&self, s: &str) -> JString {
        let s = cstring(s);
        let f: unsafe extern "C" fn(Env, *const c_char) -> JString = self.f(slot::NEW_STRING_UTF);
        unsafe { f(*self, s.as_ptr()) }
    }

    pub fn get_string(&self, string: JString) -> Option<String> {
        if string.is_null() {
            return None;
        }
        let get: unsafe extern "C" fn(Env, JString, *mut u8) -> *const c_char =
            self.f(slot::GET_STRING_UTF_CHARS);
        let release: unsafe extern "C" fn(Env, JString, *const c_char) =
            self.f(slot::RELEASE_STRING_UTF_CHARS);
        unsafe {
            let chars = get(*self, string, core::ptr::null_mut());
            if chars.is_null() {
                return None;
            }
            let out = core::ffi::CStr::from_ptr(chars)
                .to_string_lossy()
                .into_owned();
            release(*self, string, chars);
            Some(out)
        }
    }

    pub fn array_length(&self, array: JArray) -> i32 {
        if array.is_null() {
            return 0;
        }
        let f: unsafe extern "C" fn(Env, JArray) -> i32 = self.f(slot::GET_ARRAY_LENGTH);
        unsafe { f(*self, array) }
    }

    pub fn object_array_element(&self, array: JArray, index: i32) -> JObject {
        let f: unsafe extern "C" fn(Env, JArray, i32) -> JObject =
            self.f(slot::GET_OBJECT_ARRAY_ELEMENT);
        unsafe { f(*self, array, index) }
    }

    pub fn new_object_array(&self, length: i32, class: JClass, initial: JObject) -> JArray {
        let f: unsafe extern "C" fn(Env, i32, JClass, JObject) -> JArray =
            self.f(slot::NEW_OBJECT_ARRAY);
        unsafe { f(*self, length, class, initial) }
    }

    pub fn set_object_array_element(&self, array: JArray, index: i32, value: JObject) {
        let f: unsafe extern "C" fn(Env, JArray, i32, JObject) =
            self.f(slot::SET_OBJECT_ARRAY_ELEMENT);
        unsafe { f(*self, array, index, value) };
    }

    /// Copy a `byte[]` out of the VM.
    pub fn byte_array(&self, array: JArray) -> Option<Vec<u8>> {
        if array.is_null() {
            return None;
        }
        let length = self.array_length(array) as usize;
        let get: unsafe extern "C" fn(Env, JArray, *mut u8) -> *mut i8 =
            self.f(slot::GET_BYTE_ARRAY_ELEMENTS);
        let release: unsafe extern "C" fn(Env, JArray, *mut i8, i32) =
            self.f(slot::RELEASE_BYTE_ARRAY_ELEMENTS);
        unsafe {
            let elements = get(*self, array, core::ptr::null_mut());
            if elements.is_null() {
                return None;
            }
            let out = core::slice::from_raw_parts(elements as *const u8, length).to_vec();
            // Mode 2 = JNI_ABORT: release without copying back, since we only read.
            release(*self, array, elements, 2);
            Some(out)
        }
    }

    /// Make a `byte[]` the VM owns.
    pub fn new_byte_array(&self, data: &[u8]) -> JArray {
        let new: unsafe extern "C" fn(Env, i32) -> JArray = self.f(slot::NEW_BYTE_ARRAY);
        let array = unsafe { new(*self, data.len() as i32) };
        if array.is_null() || data.is_empty() {
            return array;
        }
        let get: unsafe extern "C" fn(Env, JArray, *mut u8) -> *mut i8 =
            self.f(slot::GET_BYTE_ARRAY_ELEMENTS);
        let release: unsafe extern "C" fn(Env, JArray, *mut i8, i32) =
            self.f(slot::RELEASE_BYTE_ARRAY_ELEMENTS);
        unsafe {
            let elements = get(*self, array, core::ptr::null_mut());
            if !elements.is_null() {
                core::ptr::copy_nonoverlapping(data.as_ptr(), elements as *mut u8, data.len());
                // Mode 0: commit and free.
                release(*self, array, elements, 0);
            }
        }
        array
    }

    // ── Natives and exceptions ──────────────────────────────────────────────

    /// Bind Rust functions to a class's `native` methods.
    pub fn register_natives(&self, class: JClass, methods: &[NativeMethod]) -> bool {
        let f: unsafe extern "C" fn(Env, JClass, *const NativeMethod, i32) -> i32 =
            self.f(slot::REGISTER_NATIVES);
        let rc = unsafe { f(*self, class, methods.as_ptr(), methods.len() as i32) };
        self.check_exception();
        rc == 0
    }

    /// Wrap memory we own as a `java.nio.ByteBuffer` the VM can read.
    ///
    /// No copy is made, so the memory must outlive every use of the buffer —
    /// which for a DEX means the life of the class loader built from it.
    pub fn new_direct_byte_buffer(&self, data: *mut u8, len: usize) -> JObject {
        let f: unsafe extern "C" fn(Env, *mut c_void, i64) -> JObject =
            self.f(slot::NEW_DIRECT_BYTE_BUFFER);
        unsafe { f(*self, data as *mut c_void, len as i64) }
    }

    pub fn exception_occurred(&self) -> JObject {
        let f: unsafe extern "C" fn(Env) -> JObject = self.f(slot::EXCEPTION_OCCURRED);
        unsafe { f(*self) }
    }

    pub fn exception_clear(&self) {
        let f: unsafe extern "C" fn(Env) = self.f(slot::EXCEPTION_CLEAR);
        unsafe { f(*self) };
    }

    pub fn exception_describe(&self) {
        let f: unsafe extern "C" fn(Env) = self.f(slot::EXCEPTION_DESCRIBE);
        unsafe { f(*self) };
    }

    pub fn throw_new(&self, class: JClass, message: &str) {
        let message = cstring(message);
        let f: unsafe extern "C" fn(Env, JClass, *const c_char) -> i32 = self.f(slot::THROW_NEW);
        unsafe { f(*self, class, message.as_ptr()) };
    }

    /// Clear any pending exception.
    ///
    /// **A pending exception makes almost every later JNI call undefined**, so
    /// every wrapper above clears before returning. The alternative — checking
    /// at each call site — is the same work done less reliably.
    pub fn check_exception(&self) -> bool {
        let thrown = self.exception_occurred();
        if thrown.is_null() {
            return false;
        }
        // Printing the stack trace is invaluable when wiring a new call and
        // noise everywhere else, so it is opt-in.
        if std::env::var_os("WEBBLUETOOTH_JNI_TRACE").is_some() {
            self.exception_describe();
        }
        self.exception_clear();
        self.delete_local_ref(thrown);
        true
    }
}

/// A `JavaVM`, which unlike [`Env`] is valid on every thread.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Vm(pub *mut *const [*const c_void; 8]);

unsafe impl Send for Vm {}
unsafe impl Sync for Vm {}

mod vm_slot {
    pub const DESTROY_JAVA_VM: usize = 3;
    pub const ATTACH_CURRENT_THREAD: usize = 4;
    pub const DETACH_CURRENT_THREAD: usize = 5;
    pub const GET_ENV: usize = 6;
    pub const ATTACH_CURRENT_THREAD_AS_DAEMON: usize = 7;
}

/// `JNI_VERSION_1_6`.
pub const JNI_VERSION_1_6: i32 = 0x0001_0006;

impl Vm {
    #[inline]
    fn f<T: Copy>(&self, index: usize) -> T {
        unsafe {
            let table = *self.0;
            core::mem::transmute_copy(&(*table)[index])
        }
    }

    /// The environment for this thread, if it is already attached.
    pub fn env(&self) -> Option<Env> {
        let f: unsafe extern "C" fn(Vm, *mut *mut c_void, i32) -> i32 = self.f(vm_slot::GET_ENV);
        let mut env: *mut c_void = core::ptr::null_mut();
        let rc = unsafe { f(*self, &mut env, JNI_VERSION_1_6) };
        (rc == 0 && !env.is_null()).then_some(Env(env as *mut _))
    }

    /// Attach this thread, as a daemon so it never blocks VM shutdown.
    pub fn attach(&self) -> Option<Env> {
        if let Some(env) = self.env() {
            return Some(env);
        }
        let f: unsafe extern "C" fn(Vm, *mut *mut c_void, *mut c_void) -> i32 =
            self.f(vm_slot::ATTACH_CURRENT_THREAD_AS_DAEMON);
        let mut env: *mut c_void = core::ptr::null_mut();
        let rc = unsafe { f(*self, &mut env, core::ptr::null_mut()) };
        (rc == 0 && !env.is_null()).then_some(Env(env as *mut _))
    }

    pub fn detach(&self) {
        let f: unsafe extern "C" fn(Vm) -> i32 = self.f(vm_slot::DETACH_CURRENT_THREAD);
        unsafe { f(*self) };
        let _ = (vm_slot::DESTROY_JAVA_VM, vm_slot::ATTACH_CURRENT_THREAD);
    }
}

/// A NUL-terminated copy, since JNI takes C strings throughout.
fn cstring(s: &str) -> Vec<c_char> {
    let mut out: Vec<c_char> = s.bytes().map(|b| b as c_char).collect();
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jvalue_is_a_machine_word() {
        // JNI passes arguments as an array of these; a wrong size shifts every
        // argument after the first.
        assert_eq!(core::mem::size_of::<JValue>(), 8);
        assert_eq!(core::mem::align_of::<JValue>(), 8);
    }

    #[test]
    fn native_method_struct_is_three_pointers() {
        // struct JNINativeMethod { char* name; char* signature; void* fnPtr; }
        assert_eq!(core::mem::size_of::<NativeMethod>(), 24);
    }

    #[test]
    fn cstrings_are_nul_terminated() {
        let s = cstring("abc");
        assert_eq!(s.len(), 4);
        assert_eq!(s[3], 0);
        assert_eq!(cstring("")[0], 0);
    }

    #[test]
    fn slot_indices_are_distinct() {
        // A duplicate index means two names for one function — and one of them
        // is wrong.
        let indices = [
            slot::GET_VERSION,
            slot::FIND_CLASS,
            slot::THROW_NEW,
            slot::EXCEPTION_OCCURRED,
            slot::EXCEPTION_DESCRIBE,
            slot::EXCEPTION_CLEAR,
            slot::PUSH_LOCAL_FRAME,
            slot::POP_LOCAL_FRAME,
            slot::NEW_GLOBAL_REF,
            slot::DELETE_GLOBAL_REF,
            slot::DELETE_LOCAL_REF,
            slot::IS_SAME_OBJECT,
            slot::ALLOC_OBJECT,
            slot::NEW_OBJECT_A,
            slot::GET_OBJECT_CLASS,
            slot::GET_METHOD_ID,
            slot::CALL_OBJECT_METHOD_A,
            slot::CALL_BOOLEAN_METHOD_A,
            slot::CALL_INT_METHOD_A,
            slot::CALL_LONG_METHOD_A,
            slot::CALL_VOID_METHOD_A,
            slot::GET_STATIC_METHOD_ID,
            slot::CALL_STATIC_OBJECT_METHOD_A,
            slot::CALL_STATIC_INT_METHOD_A,
            slot::CALL_STATIC_VOID_METHOD_A,
            slot::GET_STATIC_FIELD_ID,
            slot::GET_STATIC_OBJECT_FIELD,
            slot::GET_STATIC_INT_FIELD,
            slot::NEW_STRING_UTF,
            slot::GET_STRING_UTF_CHARS,
            slot::RELEASE_STRING_UTF_CHARS,
            slot::GET_ARRAY_LENGTH,
            slot::NEW_OBJECT_ARRAY,
            slot::GET_OBJECT_ARRAY_ELEMENT,
            slot::SET_OBJECT_ARRAY_ELEMENT,
            slot::NEW_BYTE_ARRAY,
            slot::GET_BYTE_ARRAY_ELEMENTS,
            slot::RELEASE_BYTE_ARRAY_ELEMENTS,
            slot::REGISTER_NATIVES,
            slot::NEW_DIRECT_BYTE_BUFFER,
        ];
        let mut seen = std::collections::HashSet::new();
        for i in indices {
            assert!(i < 234, "slot {i} is past the end of JNINativeInterface_");
            assert!(seen.insert(i), "slot {i} is named twice");
        }
    }
}
