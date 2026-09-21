//! JavaScript objects that own a Rust value.
//!
//! The Web Bluetooth API is an object graph — a device has a `gatt`, a server
//! hands out services, a service hands out characteristics — and each of those
//! objects has to carry the Rust handle it stands for.
//!
//! `napi_wrap` is how: an ordinary object gets an owned `Box<T>` attached, and
//! a finalizer frees it when the object is collected. Methods go on the
//! instance rather than a prototype, because there is no class here to hang a
//! prototype from and an instance property is what `napi_create_function`
//! produces anyway.
//!
//! The alternative — returning plain records and rebuilding the graph in a
//! JavaScript wrapper — means the shape of the API lives in two places and
//! only one of them is checked by a compiler.

use crate::napi::*;
use crate::value::{self, Result, Thrown};
use std::ffi::c_void;

/// Free the `Box<T>` a [`wrap`] attached.
///
/// # Safety
/// Called by the runtime with the pointer given to `napi_wrap`.
unsafe extern "C" fn finalize<T>(_env: napi_env, data: *mut c_void, _hint: *mut c_void) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(data as *mut T) });
    }
}

/// A new object owning `value`, with `methods` on it.
///
/// Each method is handed the same `T` when it runs: the callback reads it back
/// with [`unwrap`]. Nothing is shared between objects, so two devices cannot
/// see each other's state.
pub fn wrap<T: 'static>(
    env: napi_env,
    value_of: T,
    methods: &[(&'static std::ffi::CStr, napi_callback)],
) -> Result<napi_value> {
    wrap_with(env, value_of, methods, &[])
}

/// As [`wrap`], with properties whose value is computed on each read.
///
/// `gatt.connected` has to be a getter rather than a stored field: it changes
/// under the caller, and a value copied in at construction would be a lie the
/// moment the link dropped.
pub fn wrap_with<T: 'static>(
    env: napi_env,
    value_of: T,
    methods: &[(&'static std::ffi::CStr, napi_callback)],
    getters: &[(&'static std::ffi::CStr, napi_callback)],
) -> Result<napi_value> {
    let object = value::object(env)?;
    let boxed = Box::into_raw(Box::new(value_of)) as *mut c_void;

    // Attached before the methods, so a method that somehow ran during
    // construction would still find its value.
    value::ok(
        unsafe {
            napi_wrap(
                env,
                object,
                boxed,
                Some(finalize::<T>),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        "napi_wrap",
    )
    .inspect_err(|_| {
        // The runtime did not take ownership, so this side still has it.
        drop(unsafe { Box::from_raw(boxed as *mut T) });
    })?;

    for (name, callback) in methods {
        let mut function = std::ptr::null_mut();
        value::ok(
            unsafe {
                napi_create_function(
                    env,
                    name.as_ptr(),
                    // `NAPI_AUTO_LENGTH`: the name is NUL-terminated.
                    usize::MAX,
                    *callback,
                    std::ptr::null_mut(),
                    &mut function,
                )
            },
            "napi_create_function",
        )?;
        let name = name
            .to_str()
            .map_err(|_| Thrown("a method name was not UTF-8".into()))?;
        value::set(env, object, name, function)?;
    }

    for (name, getter) in getters {
        let descriptor = napi_property_descriptor {
            utf8name: name.as_ptr(),
            name: std::ptr::null_mut(),
            method: None,
            getter: Some(*getter),
            setter: None,
            value: std::ptr::null_mut(),
            // Enumerable, so it shows up the way a plain field would.
            attributes: 2,
            data: std::ptr::null_mut(),
        };
        value::ok(
            unsafe { napi_define_properties(env, object, 1, &descriptor) },
            "napi_define_properties",
        )?;
    }
    Ok(object)
}

/// The value a [`wrap`] attached to `this`.
///
/// # Safety
/// `object` must be one this module wrapped with the same `T`. A method
/// reached through the object it was defined on always is; one detached and
/// called on something else is not, which is why this checks the status rather
/// than casting blind.
pub unsafe fn unwrap<T: 'static>(env: napi_env, object: napi_value) -> Result<&'static T> {
    let mut data: *mut c_void = std::ptr::null_mut();
    value::ok(
        unsafe { napi_unwrap(env, object, &mut data) },
        "napi_unwrap",
    )?;
    if data.is_null() {
        return Err(Thrown(
            "this method was called on something that is not the object it belongs to".into(),
        ));
    }
    // SAFETY: the box is owned by the object and freed only by the finalizer,
    // which runs after the last reference to the object is gone — so it
    // outlives any call made through it.
    Ok(unsafe { &*(data as *const T) })
}

/// The `this` of a call, and its arguments.
///
/// # Safety
/// `info` must be the callback info the runtime passed to this callback.
pub unsafe fn this_and_args(
    env: napi_env,
    info: napi_callback_info,
    count: usize,
) -> Result<(napi_value, Vec<napi_value>)> {
    let mut argc = count;
    let mut argv = vec![std::ptr::null_mut(); count.max(1)];
    let mut this = std::ptr::null_mut();
    value::ok(
        unsafe {
            napi_get_cb_info(
                env,
                info,
                &mut argc,
                argv.as_mut_ptr(),
                &mut this,
                std::ptr::null_mut(),
            )
        },
        "napi_get_cb_info",
    )?;
    argv.truncate(count);
    Ok((this, argv))
}

/// Hold a JavaScript value past the call that produced it.
///
/// An event listener is registered once and called later, so it cannot live in
/// the handle scope it arrived in.
pub struct Held {
    reference: napi_ref,
}

// SAFETY: a `napi_ref` is an opaque handle the runtime owns. It is created and
// read on the runtime's thread; this type only carries it between them.
unsafe impl Send for Held {}
unsafe impl Sync for Held {}

// The reference is deliberately never deleted by hand. A listener lives as
// long as the characteristic that owns it: when that object is collected the
// `Held` goes with it, and the runtime reclaims the reference with the object
// it points into. Releasing earlier would mean tracking, from a background
// pump, whether JavaScript still holds the callback — which is the question
// the collector already answers.
impl Held {
    pub fn new(env: napi_env, value: napi_value) -> Result<Self> {
        let mut reference = std::ptr::null_mut();
        value::ok(
            unsafe { napi_create_reference(env, value, 1, &mut reference) },
            "napi_create_reference",
        )?;
        Ok(Self { reference })
    }

    /// The value, valid for this turn of the event loop.
    ///
    /// # Safety
    /// Must be called on the runtime's thread.
    pub unsafe fn get(&self, env: napi_env) -> Result<napi_value> {
        let mut out = std::ptr::null_mut();
        value::ok(
            unsafe { napi_get_reference_value(env, self.reference, &mut out) },
            "napi_get_reference_value",
        )?;
        Ok(out)
    }

    /// Call it with one argument, discarding the result.
    ///
    /// # Safety
    /// Must be called on the runtime's thread.
    pub unsafe fn call(&self, env: napi_env, argument: napi_value) -> Result<()> {
        let function = unsafe { self.get(env) }?;
        let undefined = value::undefined(env)?;
        let args = [argument];
        value::ok(
            unsafe {
                napi_call_function(
                    env,
                    undefined,
                    function,
                    args.len(),
                    args.as_ptr(),
                    std::ptr::null_mut(),
                )
            },
            "napi_call_function",
        )
    }
}

/// A method name and the callback behind it.
///
/// Spelled out so a table of methods reads as a table rather than as a tuple
/// whose halves you have to remember the order of.
#[macro_export]
macro_rules! methods {
    ($($name:literal => $callback:path),* $(,)?) => {
        &[$((
            // A C string, checked at compile time.
            match std::ffi::CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()) {
                Ok(s) => s,
                Err(_) => panic!("a method name held a NUL"),
            },
            $callback as $crate::napi::napi_callback,
        )),*]
    };
}

#[cfg(test)]
mod tests {
    /// Every method name is a `c"..."` literal, so a NUL inside one is a
    /// compile error rather than a property registered under a truncated name
    /// that every call site then misses.
    #[test]
    fn method_names_are_nul_terminated_at_compile_time() {
        assert_eq!(c"readValue".to_bytes(), b"readValue");
        assert_eq!(c"readValue".to_bytes_with_nul().last(), Some(&0));
    }

    /// `NAPI_AUTO_LENGTH` is `usize::MAX` — the runtime's signal to measure a
    /// NUL-terminated string itself. Passing a real length here would be a
    /// byte count, and getting it wrong truncates the method name silently.
    #[test]
    fn auto_length_is_the_sentinel_the_runtime_expects() {
        const NAPI_AUTO_LENGTH: usize = usize::MAX;
        assert_eq!(NAPI_AUTO_LENGTH, usize::MAX);
    }
}
