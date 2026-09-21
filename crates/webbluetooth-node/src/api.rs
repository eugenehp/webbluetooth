//! The Web Bluetooth object graph, built in Rust.
//!
//! `requestDevice` resolves to a `BluetoothDevice` with a `gatt`; the server
//! hands out services, a service hands out characteristics, a characteristic
//! reads and writes. That shape is the API — a caller writing against
//! `navigator.bluetooth` expects to walk it, not to thread identifiers through
//! flat calls.
//!
//! It is built here rather than reassembled in a JavaScript wrapper. A wrapper
//! works, but it puts the shape of the API in two places and only one of them
//! is checked by a compiler: rename a method on this side and the mistake
//! surfaces as `undefined is not a function` at the call site, at runtime, in
//! the user's program.
//!
//! Each object owns the Rust handle it stands for — see [`crate::object`] —
//! and every method is a promise that runs on the executor, so nothing here
//! occupies the runtime's thread while a peer is thinking.

use crate::emit::IntoJs;
use crate::napi::*;
use crate::object::{self, this_and_args, unwrap};
use crate::value::{self, Result, Thrown};
use crate::Instance;
use webbluetooth::{
    BluetoothDevice, RemoteGattCharacteristic, RemoteGattDescriptor, RemoteGattServer,
    RemoteGattService,
};

/// What each wrapped object carries.
///
/// The `Instance` is `'static` because it is leaked at registration and lives
/// as long as the module; the handles are cheap clones of `Arc`s.
struct DeviceHandle {
    device: BluetoothDevice,
}
struct ServerHandle {
    instance: &'static Instance,
    server: RemoteGattServer,
}
struct ServiceHandle {
    instance: &'static Instance,
    service: RemoteGattService,
}
struct CharacteristicHandle {
    instance: &'static Instance,
    characteristic: RemoteGattCharacteristic,
    /// `characteristicvaluechanged` listeners.
    ///
    /// Behind an `Arc` because the pump that feeds them outlives any one call
    /// and cannot borrow from the wrapped handle — the handle is freed when
    /// the JavaScript object is collected, and the pump must not be what keeps
    /// it alive.
    listeners: Listeners,
}

type Listeners = std::sync::Arc<std::sync::Mutex<Vec<std::sync::Arc<object::Held>>>>;
struct DescriptorHandle {
    instance: &'static Instance,
    descriptor: RemoteGattDescriptor,
}

/// Read `this`, unwrap it, and hand the body its handle and arguments.
///
/// Every method below has the same preamble, and writing it out each time is
/// how one of them ends up subtly different from the rest.
macro_rules! method {
    ($name:ident, $handle:ty, |$h:ident, $env:ident, $argv:ident| $body:block) => {
        unsafe extern "C" fn $name(env: napi_env, info: napi_callback_info) -> napi_value {
            let result = (|| -> Result<napi_value> {
                let (this, argv) = unsafe { this_and_args(env, info, 4) }?;
                let $h = unsafe { unwrap::<$handle>(env, this) }?;
                let ($env, $argv) = (env, argv);
                $body
            })();
            match result {
                Ok(value) => value,
                Err(e) => value::throw(env, &e.0),
            }
        }
    };
}

/// A required argument, as a string.
fn uuid_arg(env: napi_env, argv: &[napi_value], at: usize, what: &str) -> Result<String> {
    let value = argv
        .get(at)
        .copied()
        .filter(|v| !v.is_null())
        .ok_or_else(|| Thrown(format!("{what} is required")))?;
    if value::type_of(env, value)? != NAPI_STRING {
        return Err(Thrown(format!("{what} must be a string")));
    }
    value::as_string(env, value)
}

// ── Descriptor ──────────────────────────────────────────────────────────────

method!(descriptor_read, DescriptorHandle, |h, env, _argv| {
    let descriptor = h.descriptor.clone();
    h.instance
        .runtime
        .promise(env, async move { descriptor.read_value().await })
});

method!(descriptor_write, DescriptorHandle, |h, env, argv| {
    let value = value::as_bytes(
        env,
        argv.first()
            .copied()
            .filter(|v| !v.is_null())
            .ok_or_else(|| Thrown("a value is required".into()))?,
    )?;
    let descriptor = h.descriptor.clone();
    h.instance
        .runtime
        .promise(env, async move { descriptor.write_value(&value).await })
});

fn descriptor_object(
    env: napi_env,
    instance: &'static Instance,
    descriptor: RemoteGattDescriptor,
) -> Result<napi_value> {
    let uuid = descriptor.uuid().to_string();
    let object = object::wrap(
        env,
        DescriptorHandle {
            instance,
            descriptor,
        },
        &[
            (c"readValue", descriptor_read as napi_callback),
            (c"writeValue", descriptor_write as napi_callback),
        ],
    )?;
    let uuid = value::string(env, &uuid)?;
    value::set(env, object, "uuid", uuid)?;
    Ok(object)
}

// ── Characteristic ──────────────────────────────────────────────────────────

method!(
    characteristic_read,
    CharacteristicHandle,
    |h, env, _argv| {
        let characteristic = h.characteristic.clone();
        h.instance
            .runtime
            .promise(env, async move { characteristic.read_value().await })
    }
);

method!(
    characteristic_write_with_response,
    CharacteristicHandle,
    |h, env, argv| {
        let value = value::as_bytes(
            env,
            argv.first()
                .copied()
                .filter(|v| !v.is_null())
                .ok_or_else(|| Thrown("a value is required".into()))?,
        )?;
        let characteristic = h.characteristic.clone();
        h.instance.runtime.promise(env, async move {
            characteristic.write_value_with_response(&value).await
        })
    }
);

method!(
    characteristic_write_without_response,
    CharacteristicHandle,
    |h, env, argv| {
        let value = value::as_bytes(
            env,
            argv.first()
                .copied()
                .filter(|v| !v.is_null())
                .ok_or_else(|| Thrown("a value is required".into()))?,
        )?;
        let characteristic = h.characteristic.clone();
        h.instance.runtime.promise(env, async move {
            characteristic.write_value_without_response(&value).await
        })
    }
);

method!(
    characteristic_descriptors,
    CharacteristicHandle,
    |h, env, _argv| {
        let characteristic = h.characteristic.clone();
        let instance = h.instance;
        h.instance.runtime.promise_with(
            env,
            async move { characteristic.get_descriptors().await },
            move |env, found: Vec<RemoteGattDescriptor>| {
                let items: Result<Vec<_>> = found
                    .into_iter()
                    .map(|d| descriptor_object(env, instance, d))
                    .collect();
                value::array(env, &items?)
            },
        )
    }
);

method!(
    characteristic_descriptor,
    CharacteristicHandle,
    |h, env, argv| {
        let uuid = uuid_arg(env, &argv, 0, "a descriptor UUID")?;
        let characteristic = h.characteristic.clone();
        let instance = h.instance;
        h.instance.runtime.promise_with(
            env,
            async move { characteristic.get_descriptor(uuid.as_str()).await },
            move |env, d| descriptor_object(env, instance, d),
        )
    }
);

method!(
    characteristic_add_listener,
    CharacteristicHandle,
    |h, env, argv| {
        let kind = uuid_arg(env, &argv, 0, "an event name")?;
        if kind != "characteristicvaluechanged" {
            return Err(Thrown(format!(
                "{kind:?} is not an event a characteristic reports; the only one \
                 is \"characteristicvaluechanged\""
            )));
        }
        let callback = argv
            .get(1)
            .copied()
            .filter(|v| !v.is_null())
            .ok_or_else(|| Thrown("a listener function is required".into()))?;
        if value::type_of(env, callback)? != NAPI_FUNCTION {
            return Err(Thrown("the listener must be a function".into()));
        }
        h.listeners
            .lock()
            .unwrap()
            .push(std::sync::Arc::new(object::Held::new(env, callback)?));
        value::undefined(env)
    }
);

method!(
    characteristic_start_notifications,
    CharacteristicHandle,
    |h, env, _argv| {
        let characteristic = h.characteristic.clone();
        let listeners = h.listeners.clone();
        let instance = h.instance;
        h.instance.runtime.promise(env, async move {
            let mut notifications = characteristic.start_notifications().await?;
            // The pump outlives this call: it runs until the peer stops, the
            // link drops, or `stopNotifications` ends the stream.
            //
            // It does not hold the event loop open. A subscription is a
            // standing interest, not outstanding work — the same way an idle
            // listener in a browser does not stop a page from being closed —
            // and a process that refuses to exit because something might
            // notify later is a worse default than one that exits. A caller
            // who wants to wait for notifications has an ordinary way to say
            // so: keep a handle, a timer, or a promise of their own alive.
            instance.runtime.spawn(async move {
                use futures_util::StreamExt;
                while let Some(value) = notifications.next().await {
                    crate::emit::deliver_to(&instance.runtime, listeners.clone(), value);
                }
            });
            Ok(())
        })
    }
);

method!(
    characteristic_stop_notifications,
    CharacteristicHandle,
    |h, env, _argv| {
        let characteristic = h.characteristic.clone();
        h.instance.runtime.promise(env, async move {
            characteristic.stop_notifications().await?;
            Ok(())
        })
    }
);

fn characteristic_object(
    env: napi_env,
    instance: &'static Instance,
    characteristic: RemoteGattCharacteristic,
) -> Result<napi_value> {
    let uuid = characteristic.uuid().to_string();
    let properties = characteristic.properties();
    let object = object::wrap(
        env,
        CharacteristicHandle {
            instance,
            characteristic,
            listeners: Listeners::default(),
        },
        &[
            (c"readValue", characteristic_read as napi_callback),
            (
                c"writeValueWithResponse",
                characteristic_write_with_response as napi_callback,
            ),
            (
                c"writeValueWithoutResponse",
                characteristic_write_without_response as napi_callback,
            ),
            (
                c"getDescriptors",
                characteristic_descriptors as napi_callback,
            ),
            (c"getDescriptor", characteristic_descriptor as napi_callback),
            (
                c"startNotifications",
                characteristic_start_notifications as napi_callback,
            ),
            (
                c"stopNotifications",
                characteristic_stop_notifications as napi_callback,
            ),
            (
                c"addEventListener",
                characteristic_add_listener as napi_callback,
            ),
        ],
    )?;

    let uuid = value::string(env, &uuid)?;
    value::set(env, object, "uuid", uuid)?;

    // `properties` is an object of booleans, as the specification defines it,
    // rather than the bitfield it is underneath.
    let bits = value::object(env)?;
    for (name, set) in [
        ("broadcast", properties.broadcast()),
        ("read", properties.read()),
        ("writeWithoutResponse", properties.write_without_response()),
        ("write", properties.write()),
        ("notify", properties.notify()),
        ("indicate", properties.indicate()),
        (
            "authenticatedSignedWrites",
            properties.authenticated_signed_writes(),
        ),
        ("reliableWrite", properties.reliable_write()),
        ("writableAuxiliaries", properties.writable_auxiliaries()),
    ] {
        let value = value::boolean(env, set)?;
        value::set(env, bits, name, value)?;
    }
    value::set(env, object, "properties", bits)?;
    Ok(object)
}

// ── Service ─────────────────────────────────────────────────────────────────

method!(service_characteristics, ServiceHandle, |h, env, _argv| {
    let service = h.service.clone();
    let instance = h.instance;
    h.instance.runtime.promise_with(
        env,
        async move { service.get_characteristics(None).await },
        move |env, found: Vec<RemoteGattCharacteristic>| {
            let items: Result<Vec<_>> = found
                .into_iter()
                .map(|c| characteristic_object(env, instance, c))
                .collect();
            value::array(env, &items?)
        },
    )
});

method!(service_characteristic, ServiceHandle, |h, env, argv| {
    let uuid = uuid_arg(env, &argv, 0, "a characteristic UUID")?;
    let service = h.service.clone();
    let instance = h.instance;
    h.instance.runtime.promise_with(
        env,
        async move { service.get_characteristic(uuid.as_str()).await },
        move |env, c| characteristic_object(env, instance, c),
    )
});

method!(service_included, ServiceHandle, |h, env, _argv| {
    let service = h.service.clone();
    let instance = h.instance;
    h.instance.runtime.promise_with(
        env,
        async move { service.get_included_services(None).await },
        move |env, found: Vec<RemoteGattService>| {
            let items: Result<Vec<_>> = found
                .into_iter()
                .map(|s| service_object(env, instance, s))
                .collect();
            value::array(env, &items?)
        },
    )
});

fn service_object(
    env: napi_env,
    instance: &'static Instance,
    service: RemoteGattService,
) -> Result<napi_value> {
    let uuid = service.uuid().to_string();
    let primary = service.is_primary();
    let object = object::wrap(
        env,
        ServiceHandle { instance, service },
        &[
            (
                c"getCharacteristics",
                service_characteristics as napi_callback,
            ),
            (
                c"getCharacteristic",
                service_characteristic as napi_callback,
            ),
            (c"getIncludedServices", service_included as napi_callback),
        ],
    )?;
    let uuid = value::string(env, &uuid)?;
    value::set(env, object, "uuid", uuid)?;
    let primary = value::boolean(env, primary)?;
    value::set(env, object, "isPrimary", primary)?;
    Ok(object)
}

// ── GATT server ─────────────────────────────────────────────────────────────

method!(server_connect, ServerHandle, |h, env, _argv| {
    let server = h.server.clone();
    h.instance.runtime.promise(env, async move {
        server.connect().await?;
        Ok(())
    })
});

method!(server_disconnect, ServerHandle, |h, env, _argv| {
    h.server.disconnect();
    value::undefined(env)
});

method!(server_services, ServerHandle, |h, env, _argv| {
    let server = h.server.clone();
    let instance = h.instance;
    h.instance.runtime.promise_with(
        env,
        async move { server.get_primary_services(None).await },
        move |env, found: Vec<RemoteGattService>| {
            let items: Result<Vec<_>> = found
                .into_iter()
                .map(|s| service_object(env, instance, s))
                .collect();
            value::array(env, &items?)
        },
    )
});

method!(server_service, ServerHandle, |h, env, argv| {
    let uuid = uuid_arg(env, &argv, 0, "a service UUID")?;
    let server = h.server.clone();
    let instance = h.instance;
    h.instance.runtime.promise_with(
        env,
        async move { server.get_primary_service(uuid.as_str()).await },
        move |env, s| service_object(env, instance, s),
    )
});

/// `gatt.connected` — a getter, because it changes under the caller.
unsafe extern "C" fn server_connected(env: napi_env, info: napi_callback_info) -> napi_value {
    let result = (|| -> Result<napi_value> {
        let (this, _) = unsafe { this_and_args(env, info, 0) }?;
        let handle = unsafe { unwrap::<ServerHandle>(env, this) }?;
        value::boolean(env, handle.server.connected())
    })();
    match result {
        Ok(value) => value,
        Err(e) => value::throw(env, &e.0),
    }
}

fn server_object(
    env: napi_env,
    instance: &'static Instance,
    server: RemoteGattServer,
) -> Result<napi_value> {
    object::wrap_with(
        env,
        ServerHandle { instance, server },
        &[
            (c"connect", server_connect as napi_callback),
            (c"disconnect", server_disconnect as napi_callback),
            (c"getPrimaryServices", server_services as napi_callback),
            (c"getPrimaryService", server_service as napi_callback),
        ],
        &[(c"connected", server_connected as napi_callback)],
    )
}

// ── Device ──────────────────────────────────────────────────────────────────

method!(device_forget, DeviceHandle, |h, env, _argv| {
    h.device.clone().forget();
    value::undefined(env)
});

/// Build the `BluetoothDevice` a caller receives.
pub fn device_object(
    env: napi_env,
    instance: &'static Instance,
    device: BluetoothDevice,
) -> Result<napi_value> {
    let id = device.id().to_owned();
    let name = device.name();
    let address = device.address();
    let server = server_object(env, instance, device.gatt())?;

    let object = object::wrap(
        env,
        DeviceHandle { device },
        &[(c"forget", device_forget as napi_callback)],
    )?;

    let id = value::string(env, &id)?;
    value::set(env, object, "id", id)?;
    let name = name.into_js(env)?;
    value::set(env, object, "name", name)?;
    // Not in the specification — a page never learns an address — but this is
    // not a page, and everywhere but Apple the platform gives one.
    let address = address.into_js(env)?;
    value::set(env, object, "address", address)?;
    value::set(env, object, "gatt", server)?;
    Ok(object)
}
