//! Node-API, declared rather than wrapped.
//!
//! Node-API is a C ABI with a stability guarantee: a module built against
//! version 8 keeps working across Node major versions, and Deno and Bun
//! implement the same ABI. That guarantee is what makes it reasonable to
//! declare here instead of taking a binding crate — the surface does not move,
//! and this workspace already speaks JNI, COM and the Objective-C runtime the
//! same way.
//!
//! The functions come from the host process, not from a library to link
//! against: Node exports them from the executable and resolves them when it
//! loads the addon. That is why there is no `#[link(name = ...)]` here, and why
//! the shared library has to be built to allow undefined symbols.

#![allow(non_camel_case_types)]
// The declarations the object graph needs are in place ahead of the callers
// that will use them; see `object.rs`.
#![allow(dead_code)]

use std::ffi::{c_char, c_void};

/// Opaque handles. Every one of these is a pointer the runtime owns.
pub type napi_env = *mut c_void;
pub type napi_value = *mut c_void;
pub type napi_deferred = *mut c_void;
pub type napi_ref = *mut c_void;
pub type napi_threadsafe_function = *mut c_void;

/// Every call returns one of these. Zero is success.
pub type napi_status = i32;
pub const NAPI_OK: napi_status = 0;

/// How a threadsafe function behaves when its queue is full. The queue here is
/// unbounded, so this is the only mode that can apply.
pub const NAPI_TSFN_NONBLOCKING: i32 = 0;

/// What `napi_typeof` reports.
pub type napi_valuetype = i32;
pub const NAPI_UNDEFINED: napi_valuetype = 0;
pub const NAPI_NULL: napi_valuetype = 1;
pub const NAPI_BOOLEAN: napi_valuetype = 2;
pub const NAPI_STRING: napi_valuetype = 4;
pub const NAPI_FUNCTION: napi_valuetype = 7;

/// A method to expose on the module's exports.
#[repr(C)]
pub struct napi_property_descriptor {
    pub utf8name: *const c_char,
    pub name: napi_value,
    pub method: Option<napi_callback>,
    pub getter: Option<napi_callback>,
    pub setter: Option<napi_callback>,
    pub value: napi_value,
    pub attributes: i32,
    pub data: *mut c_void,
}

pub type napi_callback = unsafe extern "C" fn(napi_env, napi_callback_info) -> napi_value;
pub type napi_callback_info = *mut c_void;
pub type napi_finalize = unsafe extern "C" fn(napi_env, *mut c_void, *mut c_void);
pub type napi_threadsafe_function_call_js =
    unsafe extern "C" fn(napi_env, napi_value, *mut c_void, *mut c_void);

// Resolved by whichever runtime loads the addon — Node, Deno or Bun.
//
// On Unix there is nothing to link against: the host process already has these
// symbols and the loader resolves them, which `build.rs` tells the linker to
// allow. Windows has no such thing — every import must be named at link time —
// and the usual answer is an import library extracted from `node.exe`, which
// means a build step and a copy of Node on the build machine.
//
// `raw-dylib` removes that. It tells the compiler to synthesise the import
// stubs from these declarations, naming `node.exe` as the module to import
// from, so nothing has to be extracted and nothing has to be present. The
// addon can be cross-compiled for Windows from anywhere.
#[cfg_attr(windows, link(name = "node.exe", kind = "raw-dylib"))]
unsafe extern "C" {
    // ── Values out ──────────────────────────────────────────────────────────
    pub fn napi_get_undefined(env: napi_env, result: *mut napi_value) -> napi_status;
    pub fn napi_get_null(env: napi_env, result: *mut napi_value) -> napi_status;
    pub fn napi_get_boolean(env: napi_env, value: bool, result: *mut napi_value) -> napi_status;
    pub fn napi_create_double(env: napi_env, value: f64, result: *mut napi_value) -> napi_status;
    pub fn napi_create_string_utf8(
        env: napi_env,
        str_: *const c_char,
        length: usize,
        result: *mut napi_value,
    ) -> napi_status;
    pub fn napi_create_object(env: napi_env, result: *mut napi_value) -> napi_status;
    pub fn napi_create_array_with_length(
        env: napi_env,
        length: usize,
        result: *mut napi_value,
    ) -> napi_status;
    pub fn napi_create_arraybuffer(
        env: napi_env,
        byte_length: usize,
        data: *mut *mut c_void,
        result: *mut napi_value,
    ) -> napi_status;
    pub fn napi_create_error(
        env: napi_env,
        code: napi_value,
        msg: napi_value,
        result: *mut napi_value,
    ) -> napi_status;

    // ── Values in ───────────────────────────────────────────────────────────
    pub fn napi_typeof(
        env: napi_env,
        value: napi_value,
        result: *mut napi_valuetype,
    ) -> napi_status;
    pub fn napi_get_value_double(env: napi_env, value: napi_value, result: *mut f64)
        -> napi_status;
    pub fn napi_get_value_bool(env: napi_env, value: napi_value, result: *mut bool) -> napi_status;
    pub fn napi_get_value_string_utf8(
        env: napi_env,
        value: napi_value,
        buf: *mut c_char,
        bufsize: usize,
        result: *mut usize,
    ) -> napi_status;
    pub fn napi_get_arraybuffer_info(
        env: napi_env,
        arraybuffer: napi_value,
        data: *mut *mut c_void,
        byte_length: *mut usize,
    ) -> napi_status;
    pub fn napi_is_arraybuffer(env: napi_env, value: napi_value, result: *mut bool) -> napi_status;
    pub fn napi_get_typedarray_info(
        env: napi_env,
        typedarray: napi_value,
        type_: *mut i32,
        length: *mut usize,
        data: *mut *mut c_void,
        arraybuffer: *mut napi_value,
        byte_offset: *mut usize,
    ) -> napi_status;
    pub fn napi_is_typedarray(env: napi_env, value: napi_value, result: *mut bool) -> napi_status;

    // ── Objects ─────────────────────────────────────────────────────────────
    pub fn napi_set_named_property(
        env: napi_env,
        object: napi_value,
        utf8name: *const c_char,
        value: napi_value,
    ) -> napi_status;
    pub fn napi_get_named_property(
        env: napi_env,
        object: napi_value,
        utf8name: *const c_char,
        result: *mut napi_value,
    ) -> napi_status;
    pub fn napi_set_element(
        env: napi_env,
        object: napi_value,
        index: u32,
        value: napi_value,
    ) -> napi_status;
    pub fn napi_get_element(
        env: napi_env,
        object: napi_value,
        index: u32,
        result: *mut napi_value,
    ) -> napi_status;
    pub fn napi_get_array_length(env: napi_env, value: napi_value, result: *mut u32)
        -> napi_status;
    pub fn napi_has_named_property(
        env: napi_env,
        object: napi_value,
        utf8name: *const c_char,
        result: *mut bool,
    ) -> napi_status;
    pub fn napi_define_properties(
        env: napi_env,
        object: napi_value,
        property_count: usize,
        properties: *const napi_property_descriptor,
    ) -> napi_status;

    // ── Calls ───────────────────────────────────────────────────────────────
    pub fn napi_get_cb_info(
        env: napi_env,
        cbinfo: napi_callback_info,
        argc: *mut usize,
        argv: *mut napi_value,
        this_arg: *mut napi_value,
        data: *mut *mut c_void,
    ) -> napi_status;
    pub fn napi_throw(env: napi_env, error: napi_value) -> napi_status;

    // ── Objects that carry a Rust value ─────────────────────────────────────
    //
    // A `BluetoothDevice` handed to JavaScript is an ordinary object with an
    // owned Rust value attached. `napi_wrap` is what attaches it, and the
    // finalizer is what frees it when the object is collected.
    pub fn napi_wrap(
        env: napi_env,
        js_object: napi_value,
        native_object: *mut c_void,
        finalize_cb: Option<napi_finalize>,
        finalize_hint: *mut c_void,
        result: *mut napi_ref,
    ) -> napi_status;
    pub fn napi_unwrap(
        env: napi_env,
        js_object: napi_value,
        result: *mut *mut c_void,
    ) -> napi_status;

    /// A method, as a value — for putting on an instance rather than a class.
    pub fn napi_create_function(
        env: napi_env,
        utf8name: *const c_char,
        length: usize,
        cb: napi_callback,
        data: *mut c_void,
        result: *mut napi_value,
    ) -> napi_status;
    pub fn napi_call_function(
        env: napi_env,
        recv: napi_value,
        func: napi_value,
        argc: usize,
        argv: *const napi_value,
        result: *mut napi_value,
    ) -> napi_status;

    // ── Keeping a JavaScript value alive ────────────────────────────────────
    //
    // An event listener outlives the call that registered it, so it has to be
    // held by a reference rather than a scope.
    pub fn napi_create_reference(
        env: napi_env,
        value: napi_value,
        initial_refcount: u32,
        result: *mut napi_ref,
    ) -> napi_status;
    pub fn napi_get_reference_value(
        env: napi_env,
        ref_: napi_ref,
        result: *mut napi_value,
    ) -> napi_status;
    pub fn napi_delete_reference(env: napi_env, ref_: napi_ref) -> napi_status;

    // ── Promises ────────────────────────────────────────────────────────────
    pub fn napi_create_promise(
        env: napi_env,
        deferred: *mut napi_deferred,
        promise: *mut napi_value,
    ) -> napi_status;
    pub fn napi_resolve_deferred(
        env: napi_env,
        deferred: napi_deferred,
        resolution: napi_value,
    ) -> napi_status;
    pub fn napi_reject_deferred(
        env: napi_env,
        deferred: napi_deferred,
        rejection: napi_value,
    ) -> napi_status;

    // ── Crossing back from a Rust thread ────────────────────────────────────
    //
    // The backends call from their own threads — CoreBluetooth's dispatch
    // queue, the D-Bus reader, the ATT reader. A `napi_value` is only valid on
    // the runtime's thread, so anything arriving from elsewhere has to go
    // through one of these.
    pub fn napi_create_threadsafe_function(
        env: napi_env,
        func: napi_value,
        async_resource: napi_value,
        async_resource_name: napi_value,
        max_queue_size: usize,
        initial_thread_count: usize,
        thread_finalize_data: *mut c_void,
        thread_finalize_cb: Option<napi_finalize>,
        context: *mut c_void,
        call_js_cb: Option<napi_threadsafe_function_call_js>,
        result: *mut napi_threadsafe_function,
    ) -> napi_status;
    pub fn napi_call_threadsafe_function(
        func: napi_threadsafe_function,
        data: *mut c_void,
        is_blocking: i32,
    ) -> napi_status;
    pub fn napi_ref_threadsafe_function(
        env: napi_env,
        func: napi_threadsafe_function,
    ) -> napi_status;
    pub fn napi_unref_threadsafe_function(
        env: napi_env,
        func: napi_threadsafe_function,
    ) -> napi_status;

}

// ── Stubs, for tests only ───────────────────────────────────────────────────
//
// Generated to match the declarations above one for one. Each reports failure
// rather than success: a test that reaches one has strayed into code that
// needs a real runtime, and should fail loudly instead of reading an
// uninitialised out-parameter as if it had been filled in.
// Each stub is `unsafe fn` only to match the declaration it stands in for;
// none of them dereferences anything, and the contract they nominally carry is
// the one documented on the block above. Repeating a `# Safety` section forty
// times to say "this one does nothing" would bury the one place it matters.
#[cfg(test)]
mod tests {
    use super::*;

    /// The descriptor is passed by pointer to the runtime, which reads it as
    /// C. A layout mismatch would be read as garbage rather than rejected.
    #[test]
    fn the_property_descriptor_is_c_shaped() {
        assert_eq!(
            std::mem::size_of::<napi_property_descriptor>(),
            std::mem::size_of::<*const c_char>()
                + std::mem::size_of::<napi_value>()
                + std::mem::size_of::<Option<napi_callback>>() * 3
                + std::mem::size_of::<napi_value>()
                + std::mem::size_of::<i32>()
                + std::mem::size_of::<*mut c_void>()
                // `attributes` is an i32 followed by a pointer, so the struct
                // carries padding to realign.
                + std::mem::size_of::<*mut c_void>()
                - std::mem::size_of::<i32>(),
        );
    }

    /// A nullable function pointer must be pointer-sized, or `None` would not
    /// be the null the runtime checks for.
    #[test]
    fn an_absent_callback_is_a_null_pointer() {
        assert_eq!(
            std::mem::size_of::<Option<napi_callback>>(),
            std::mem::size_of::<*const c_void>()
        );
    }

    #[test]
    fn success_is_zero() {
        assert_eq!(NAPI_OK, 0);
    }
}
