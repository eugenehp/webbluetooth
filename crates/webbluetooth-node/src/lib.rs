//! Web Bluetooth for Node.js and Deno.
//!
//! Neither runtime has `navigator.bluetooth` — that is a browser API, and a
//! server-side JavaScript runtime has no device picker and no permission
//! prompt to build one on. So unlike the WebAssembly backend, which *uses*
//! the browser's Web Bluetooth, this one *provides* it: the same
//! CoreBluetooth, BlueZ, WinRT and HCI implementations the Rust crate uses,
//! exposed to JavaScript.
//!
//! # Why not neon, or napi-rs
//!
//! Node-API is a C ABI with an explicit stability guarantee — that is the
//! entire reason it exists. A module built against it keeps working across
//! Node major versions without recompiling, and Deno and Bun implement the
//! same ABI. Declaring it is no more exotic than the JNI, COM and
//! Objective-C-runtime declarations elsewhere in this workspace, and it keeps
//! the dependency count where the rest of the project keeps it: zero.
//!
//! # One addon, both runtimes
//!
//! Deno implements Node-API, so the same `.node` file loads in both:
//!
//! ```js
//! // Node
//! const bluetooth = require('./webbluetooth.node');
//!
//! // Deno — needs --allow-ffi --unstable-ffi
//! import { createRequire } from 'node:module';
//! const bluetooth = createRequire(import.meta.url)('./webbluetooth.node');
//! ```
//!
//! # The consent problem
//!
//! In a browser `requestDevice` shows the user a picker. Here there is nobody
//! to show. The default is [`webbluetooth::chooser::FirstMatch`], which takes
//! the first device matching the filters — convenient, automatic, and a
//! removal of the consent step the Web Bluetooth security model is built on.
//! A caller that has a user to ask should pass a `chooser` callback and ask
//! them.

// Internals, not API. The crate's entire public surface is
// `napi_register_module_v1`, which the runtime looks up by name — everything
// else takes a `napi_env`, which is a raw pointer only valid on the runtime's
// own thread, and is nobody's business to call from outside.
pub(crate) mod api;
pub(crate) mod emit;
pub(crate) mod exec;
pub(crate) mod napi;
pub(crate) mod object;
pub(crate) mod value;

use napi::*;
use std::ffi::c_char;
use value::{Result, Thrown};

/// The runtime, and the state that outlives a single call.
///
/// One per module instance. Node can load an addon into several contexts
/// (worker threads, `vm` realms), and each gets its own.
pub struct Instance {
    /// The Bluetooth adapter this module is driving, opened on first use.
    ///
    /// Deliberately not opened at load. Constructing one touches the radio —
    /// on Apple it creates a `CBCentralManager`, and a process that does that
    /// without a Bluetooth usage description in *its own* image is terminated
    /// by the system. A host like `node` has no such description and cannot be
    /// given one by an addon: the embedded `__info_plist` belongs to this
    /// dylib, and TCC reads the running image's.
    ///
    /// So `require()` must not do it. Loading a module is not consent to use a
    /// radio, and a module that kills its host merely by being imported is
    /// wrong even where the system tolerates it.
    bluetooth: std::sync::OnceLock<webbluetooth::Bluetooth>,
    /// Somewhere to run futures. Node's loop is not ours to block, so the
    /// crate's futures run on a thread of their own and results come back
    /// through a threadsafe function.
    pub runtime: emit::Runtime,
}

impl Instance {
    fn new() -> Self {
        Self {
            bluetooth: std::sync::OnceLock::new(),
            runtime: emit::Runtime::new(),
        }
    }

    /// The adapter, opening it the first time something asks.
    pub fn bluetooth(&self) -> &webbluetooth::Bluetooth {
        self.bluetooth.get_or_init(webbluetooth::Bluetooth::new)
    }
}

/// Read the arguments of a call.
///
/// # Safety
/// `info` must be the callback info the runtime passed to this callback.
unsafe fn args(env: napi_env, info: napi_callback_info, count: usize) -> Result<Vec<napi_value>> {
    let mut argc = count;
    let mut argv = vec![std::ptr::null_mut(); count];
    value::ok(
        unsafe {
            napi_get_cb_info(
                env,
                info,
                &mut argc,
                argv.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        "napi_get_cb_info",
    )?;
    // Missing arguments come back as undefined rather than short, so the
    // vector is always `count` long and each callback checks what it needs.
    Ok(argv)
}

/// `getAvailability()` — resolves to a boolean.
unsafe extern "C" fn get_availability(env: napi_env, info: napi_callback_info) -> napi_value {
    let instance = match unsafe { instance_from(env, info) } {
        Ok(i) => i,
        Err(e) => return value::throw(env, &e.0),
    };
    let bluetooth = instance.bluetooth().clone();
    match instance
        .runtime
        .promise(env, async move { Ok(bluetooth.get_availability().await) })
    {
        Ok(promise) => promise,
        Err(e) => value::throw(env, &e.0),
    }
}

/// One argument, or an error naming which was missing.
fn required(argv: &[napi_value], at: usize, what: &str) -> Result<napi_value> {
    argv.get(at)
        .copied()
        .filter(|v| !v.is_null())
        .ok_or_else(|| Thrown(format!("argument {} ({what}) is required", at + 1)))
}

/// Read a `RequestDeviceOptions` from the object a caller passed.
///
/// The shape is Web Bluetooth's, so a page's options object works unchanged:
/// `{ filters: [{ services, name, namePrefix }], optionalServices, acceptAllDevices }`.
unsafe fn request_options(
    env: napi_env,
    value: napi_value,
) -> Result<webbluetooth::RequestDeviceOptions> {
    use webbluetooth::{DeviceFilter, RequestDeviceOptions};

    let bad = |e: webbluetooth::Error| Thrown(e.to_string());
    let mut options = RequestDeviceOptions::new();

    let uuids = |env, list: napi_value| -> Result<Vec<String>> {
        let mut len = 0;
        value::ok(
            unsafe { napi_get_array_length(env, list, &mut len) },
            "array length",
        )?;
        let mut out = Vec::new();
        for i in 0..len {
            let mut item = std::ptr::null_mut();
            value::ok(
                unsafe { napi_get_element(env, list, i, &mut item) },
                "array element",
            )?;
            out.push(value::as_string(env, item)?);
        }
        Ok(out)
    };

    if let Some(filters) = value::get(env, value, "filters")? {
        let mut count = 0;
        value::ok(
            unsafe { napi_get_array_length(env, filters, &mut count) },
            "filters length",
        )?;
        for i in 0..count {
            let mut entry = std::ptr::null_mut();
            value::ok(
                unsafe { napi_get_element(env, filters, i, &mut entry) },
                "filter",
            )?;
            let mut filter = DeviceFilter::new();
            if let Some(services) = value::get(env, entry, "services")? {
                filter = filter.services(uuids(env, services)?).map_err(bad)?;
            }
            if let Some(name) = value::get(env, entry, "name")? {
                filter = filter.name(value::as_string(env, name)?);
            }
            if let Some(prefix) = value::get(env, entry, "namePrefix")? {
                filter = filter.name_prefix(value::as_string(env, prefix)?);
            }
            options = options.filter(filter);
        }
    }
    if let Some(optional) = value::get(env, value, "optionalServices")? {
        options = options
            .optional_services(uuids(env, optional)?)
            .map_err(bad)?;
    }
    if let Some(all) = value::get(env, value, "acceptAllDevices")? {
        if value::as_bool(env, all)? {
            options = options.accept_all_devices();
        }
    }
    Ok(options)
}

/// `requestDevice(options)` — resolves to a `BluetoothDevice`.
unsafe extern "C" fn request_device(env: napi_env, info: napi_callback_info) -> napi_value {
    let result = (|| -> Result<napi_value> {
        let instance = unsafe { instance_from(env, info) }?;
        let argv = unsafe { args(env, info, 1) }?;
        let options = unsafe { request_options(env, required(&argv, 0, "options")?) }?;

        let bluetooth = instance.bluetooth().clone();
        // The whole object graph, built on the runtime's thread once the
        // device exists — `gatt`, and everything reachable from it.
        instance.runtime.promise_with(
            env,
            async move { bluetooth.request_device(options).await },
            move |env, device| api::device_object(env, instance, device),
        )
    })();
    match result {
        Ok(promise) => promise,
        Err(e) => value::throw(env, &e.0),
    }
}

/// The module's state, reached from a callback.
///
/// # Safety
/// `info` must be this callback's info; the data pointer is the `Instance`
/// installed by [`napi_register_module_v1`], which lives as long as the module.
unsafe fn instance_from(env: napi_env, info: napi_callback_info) -> Result<&'static Instance> {
    let mut argc = 0usize;
    let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
    value::ok(
        unsafe {
            napi_get_cb_info(
                env,
                info,
                &mut argc,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut data,
            )
        },
        "napi_get_cb_info",
    )?;
    if data.is_null() {
        return Err(Thrown("the module state is missing".into()));
    }
    // SAFETY: the pointer was leaked at registration and is never freed, so it
    // outlives every callback that can observe it.
    Ok(unsafe { &*(data as *const Instance) })
}

/// Describe one exported function.
fn method(
    name: &'static std::ffi::CStr,
    callback: napi_callback,
    data: *mut std::ffi::c_void,
) -> napi_property_descriptor {
    napi_property_descriptor {
        utf8name: name.as_ptr() as *const c_char,
        name: std::ptr::null_mut(),
        method: Some(callback),
        getter: None,
        setter: None,
        value: std::ptr::null_mut(),
        // writable | enumerable | configurable.
        //
        // `napi_default` is none of those, which loads fine and then behaves
        // oddly: the methods work but `Object.keys(module)` is empty and
        // `const { requestDevice } = require(...)` finds nothing, because a
        // non-enumerable property is invisible to both. Module exports are
        // expected to be ordinary properties.
        attributes: 1 | 2 | 4,
        data,
    }
}

/// What Node calls when it loads the addon.
///
/// The symbol name is the ABI: the runtime looks it up by string, which is why
/// it is `#[no_mangle]` and why the version suffix is part of the name.
///
/// # Safety
/// Called once per module instance by the runtime.
#[no_mangle]
pub unsafe extern "C" fn napi_register_module_v1(env: napi_env, exports: napi_value) -> napi_value {
    // Leaked deliberately: the runtime has no hook that reliably runs at
    // unload on every platform, and the alternative — a finalizer that may or
    // may not fire — would be a use-after-free rather than a leak.
    let instance: &'static Instance = Box::leak(Box::new(Instance::new()));
    let data = instance as *const Instance as *mut std::ffi::c_void;

    // The module's own surface is small on purpose: everything else hangs off
    // the device these return, as an object graph rather than a set of flat
    // calls a wrapper has to reassemble.
    let methods = [
        method(c"getAvailability", get_availability, data),
        method(c"requestDevice", request_device, data),
    ];

    if value::ok(
        unsafe { napi_define_properties(env, exports, methods.len(), methods.as_ptr()) },
        "napi_define_properties",
    )
    .is_err()
    {
        return value::throw(env, "could not install the module's exports");
    }
    exports
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime finds the entry point by name. A typo here is a module that
    /// loads and then reports "is not a valid Node.js addon", with nothing to
    /// point at.
    #[test]
    fn the_registration_symbol_is_the_one_node_looks_up() {
        // The coercion is the assertion: it only compiles if a function of
        // exactly this name and signature exists. A typo here is a module that
        // loads and then reports "is not a valid Node.js addon", with nothing
        // to point at.
        let _entry: unsafe extern "C" fn(napi_env, napi_value) -> napi_value =
            napi_register_module_v1;
    }

    /// Exports have to be enumerable, or destructuring a `require()` result
    /// silently yields `undefined` for every name.
    #[test]
    fn an_export_is_an_ordinary_enumerable_property() {
        let d = method(c"getAvailability", get_availability, std::ptr::null_mut());
        const ENUMERABLE: i32 = 2;
        assert_eq!(
            d.attributes & ENUMERABLE,
            ENUMERABLE,
            "exports must show up in Object.keys()"
        );
    }

    #[test]
    fn a_method_descriptor_names_its_callback() {
        let d = method(c"getAvailability", get_availability, std::ptr::null_mut());
        assert!(d.method.is_some());
        assert!(d.getter.is_none() && d.setter.is_none());
        assert!(!d.utf8name.is_null());
    }
}
