//! Calling WinRT's Bluetooth APIs.
//!
//! Everything here is vtable slots from [`crate::iids::slots`] and IIDs from
//! [`crate::iids`], both generated from Windows metadata. Nothing is counted or
//! remembered by hand, because a slot off by one is a jump into a different
//! function and there is no runtime here that would catch it.
//!
//! # Async
//!
//! Every WinRT call that can block returns an `IAsyncOperation<T>`, which is
//! completed by handing it a delegate. The delegate's IID is *computed* from
//! the result type — see [`crate::guid::Signature`] — so each call site names
//! the signature of what it expects back.
//!
//! # Buffers
//!
//! GATT values are `IBuffer`, which has no public accessor. `DataReader` and
//! `DataWriter` convert both ways without needing the `IBufferByteAccess` COM
//! escape hatch, at the cost of an allocation nobody will notice.

use crate::com::{succeeded, ComPtr, Hresult, FIRST_METHOD};
use crate::guid::{Guid, Signature};
use crate::iids::{classes, generics, interfaces, slots};
use crate::winrt::{activation_factory, HString, WinRtString};
use std::ffi::c_void;
use std::sync::Mutex;

/// `IActivationFactory`.
///
/// One of the fundamental COM interfaces rather than a WinRT one, so it is not
/// in the generated metadata. `ActivateInstance` is its only method, and sits
/// at the first slot after `IInspectable`.
const IID_IACTIVATION_FACTORY: Guid = Guid::parse("00000035-0000-0000-c000-000000000046");
const ACTIVATE_INSTANCE: usize = FIRST_METHOD;

/// Construct a WinRT class that has a default constructor.
pub fn activate(class: classes::Class) -> Option<ComPtr> {
    let factory = activation_factory(class.0, IID_IACTIVATION_FACTORY)?;
    let mut instance: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
            factory.method(ACTIVATE_INSTANCE);
        f(factory.as_raw(), &mut instance)
    };
    if !succeeded(hr) {
        return None;
    }
    unsafe { ComPtr::adopt(instance) }
}

/// The statics of a class — `BluetoothLEDevice.FromBluetoothAddressAsync` and
/// the like, which are not on an instance.
pub fn statics(class: classes::Class, iid: Guid) -> Option<ComPtr> {
    activation_factory(class.0, iid)
}

// ── Reading properties ──────────────────────────────────────────────────────

/// A property returning another object.
pub fn get_object(object: &ComPtr, slot: usize) -> Option<ComPtr> {
    let mut out: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
            object.method(slot);
        f(object.as_raw(), &mut out)
    };
    if !succeeded(hr) {
        return None;
    }
    unsafe { ComPtr::adopt(out) }
}

/// A property returning a string.
pub fn get_string(object: &ComPtr, slot: usize) -> Option<String> {
    let mut out: HString = std::ptr::null_mut();
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut HString) -> Hresult =
            object.method(slot);
        f(object.as_raw(), &mut out)
    };
    if !succeeded(hr) {
        return None;
    }
    let text = WinRtString::read(out);
    // The handle is ours now, and reading does not consume it.
    unsafe { crate::winrt::delete_string(out) };
    Some(text)
}

/// A property returning a 32-bit integer or enum.
pub fn get_i32(object: &ComPtr, slot: usize) -> Option<i32> {
    let mut out: i32 = 0;
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult = object.method(slot);
        f(object.as_raw(), &mut out)
    };
    succeeded(hr).then_some(out)
}

/// A property returning a boolean — `IsEnabled` is one.
pub fn get_bool(object: &ComPtr, slot: usize) -> Option<bool> {
    let mut out: u8 = 0;
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut u8) -> Hresult = object.method(slot);
        f(object.as_raw(), &mut out)
    };
    succeeded(hr).then_some(out != 0)
}

/// A property returning a 16-bit integer — `RawSignalStrengthInDBm` is one.
/// A property returning an unsigned 16-bit integer — `MaxPduSize` is one.
pub fn get_u16(object: &ComPtr, slot: usize) -> Option<u16> {
    let mut out: u16 = 0;
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut u16) -> Hresult = object.method(slot);
        f(object.as_raw(), &mut out)
    };
    succeeded(hr).then_some(out)
}

pub fn get_i16(object: &ComPtr, slot: usize) -> Option<i16> {
    let mut out: i16 = 0;
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut i16) -> Hresult = object.method(slot);
        f(object.as_raw(), &mut out)
    };
    succeeded(hr).then_some(out)
}

/// A property returning a 64-bit integer — a Bluetooth address is one.
pub fn get_u64(object: &ComPtr, slot: usize) -> Option<u64> {
    let mut out: u64 = 0;
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut u64) -> Hresult = object.method(slot);
        f(object.as_raw(), &mut out)
    };
    succeeded(hr).then_some(out)
}

/// A property returning a GUID by value — every `Uuid` is one.
pub fn get_guid(object: &ComPtr, slot: usize) -> Option<Guid> {
    let mut out = Guid::default();
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut Guid) -> Hresult = object.method(slot);
        f(object.as_raw(), &mut out)
    };
    succeeded(hr).then_some(out)
}

/// Call a method that takes nothing and returns nothing.
pub fn call_void(object: &ComPtr, slot: usize) -> bool {
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void) -> Hresult = object.method(slot);
        f(object.as_raw())
    };
    succeeded(hr)
}

// ── Collections ─────────────────────────────────────────────────────────────

/// Walk an `IVectorView<T>`.
///
/// The slot order is the generic's own and is the same for every
/// instantiation, which is why this works without knowing `T`.
pub fn for_each(vector: &ComPtr, mut f: impl FnMut(ComPtr)) {
    let mut size: u32 = 0;
    let ok = unsafe {
        let g: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult =
            vector.method(slots::ivector_view::SIZE);
        succeeded(g(vector.as_raw(), &mut size))
    };
    if !ok {
        return;
    }
    for index in 0..size {
        let mut item: *mut c_void = std::ptr::null_mut();
        let hr = unsafe {
            let g: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> Hresult =
                vector.method(slots::ivector_view::GET_AT);
            g(vector.as_raw(), index, &mut item)
        };
        if succeeded(hr) {
            if let Some(item) = unsafe { ComPtr::adopt(item) } {
                f(item);
            }
        }
    }
}

// ── Async ───────────────────────────────────────────────────────────────────

/// The signature of `AsyncOperationCompletedHandler<T>`, whose IID a completion
/// handler must answer to.
pub fn completed_handler_iid(result: Signature) -> Guid {
    Signature::Parameterized {
        generic: generics::ASYNC_OPERATION_COMPLETED_HANDLER,
        arguments: vec![Signature::Parameterized {
            generic: generics::I_ASYNC_OPERATION,
            arguments: vec![result],
        }],
    }
    .iid()
    .expect("a parameterised signature always has an IID")
}

/// Run `then` when an `IAsyncOperation<T>` completes, with its result.
///
/// The delegate answers to a computed IID, so `result` must describe exactly
/// what the operation returns — a mismatch is refused by `put_Completed` rather
/// than producing a wrong value.
pub fn on_completed(
    operation: &ComPtr,
    result: Signature,
    then: impl FnOnce(Option<ComPtr>) + Send + 'static,
) -> bool {
    // The handler is a Fn but fires once; this is what lets it own a FnOnce.
    let once = Mutex::new(Some(then));

    let handler =
        crate::com::Delegate::new(completed_handler_iid(result), move |sender, _status| {
            let Ok(mut guard) = once.lock() else { return };
            let Some(then) = guard.take() else { return };

            // `sender` is the completed operation, borrowed — GetResults does not
            // consume it, so it must not be adopted.
            let mut value: *mut c_void = std::ptr::null_mut();
            let ok = if sender.is_null() {
                false
            } else {
                unsafe {
                    let vtable = *(sender as *const *const *const c_void);
                    let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
                        std::mem::transmute_copy(
                            &*vtable.add(slots::iasync_operation::GET_RESULTS),
                        );
                    succeeded(f(sender, &mut value))
                }
            };
            then(if ok {
                unsafe { ComPtr::adopt(value) }
            } else {
                None
            });
        });

    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult =
            operation.method(slots::iasync_operation::SET_COMPLETED);
        f(operation.as_raw(), handler.as_raw())
    };
    succeeded(hr)
}

// ── Buffers ─────────────────────────────────────────────────────────────────

/// Copy an `IBuffer` out of the runtime.
pub fn buffer_to_bytes(buffer: &ComPtr) -> Option<Vec<u8>> {
    let statics = statics(classes::DATA_READER, interfaces::I_DATA_READER_STATICS)?;

    let mut reader: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hresult =
            statics.method(slots::idata_reader_statics::FROM_BUFFER);
        f(statics.as_raw(), buffer.as_raw(), &mut reader)
    };
    if !succeeded(hr) {
        return None;
    }
    let reader = unsafe { ComPtr::adopt(reader) }?;

    let length = get_i32(&reader, slots::idata_reader::UNCONSUMED_BUFFER_LENGTH)? as usize;
    let mut bytes = vec![0u8; length];
    if length == 0 {
        return Some(bytes);
    }
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, u32, *mut u8) -> Hresult =
            reader.method(slots::idata_reader::READ_BYTES);
        f(reader.as_raw(), length as u32, bytes.as_mut_ptr())
    };
    succeeded(hr).then_some(bytes)
}

/// Hand bytes to the runtime as an `IBuffer`.
pub fn bytes_to_buffer(bytes: &[u8]) -> Option<ComPtr> {
    let writer = activate(classes::DATA_WRITER)?;
    if !bytes.is_empty() {
        let hr = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, u32, *const u8) -> Hresult =
                writer.method(slots::idata_writer::WRITE_BYTES);
            f(writer.as_raw(), bytes.len() as u32, bytes.as_ptr())
        };
        if !succeeded(hr) {
            return None;
        }
    }
    get_object(&writer, slots::idata_writer::DETACH_BUFFER)
}

// ── Bluetooth addresses ─────────────────────────────────────────────────────

/// Format a `BluetoothAddress` as `AA:BB:CC:DD:EE:FF`.
///
/// WinRT carries it as a 64-bit integer with the address in the low 48 bits,
/// most significant octet first — the opposite of the socket APIs on Linux.
pub fn format_address(address: u64) -> String {
    let b = address.to_be_bytes();
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        b[2], b[3], b[4], b[5], b[6], b[7]
    )
}

/// Parse `AA:BB:CC:DD:EE:FF` back into WinRT's form.
pub fn parse_address(address: &str) -> Option<u64> {
    let parts: Vec<&str> = address.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut value: u64 = 0;
    for part in parts {
        value = (value << 8) | u64::from(u8::from_str_radix(part, 16).ok()?);
    }
    Some(value)
}

/// A WinRT `GUID` as the canonical lowercase UUID the spec uses.
pub fn uuid_string(guid: Guid) -> String {
    guid.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_the_low_48_bits_most_significant_first() {
        // The opposite of the socket APIs, which are little-endian.
        assert_eq!(format_address(0x0000_AABB_CCDD_EEFF), "AA:BB:CC:DD:EE:FF");
        assert_eq!(format_address(0x0000_0000_0000_0001), "00:00:00:00:00:01");
    }

    #[test]
    fn address_parsing_is_the_inverse_of_formatting() {
        for text in [
            "AA:BB:CC:DD:EE:FF",
            "00:00:00:00:00:01",
            "12:34:56:78:9A:BC",
        ] {
            let parsed = parse_address(text).expect("should parse");
            assert_eq!(format_address(parsed), text);
        }
        assert_eq!(
            parse_address("AA:BB:CC:DD:EE:FF"),
            Some(0x0000_AABB_CCDD_EEFF)
        );
    }

    #[test]
    fn malformed_addresses_are_rejected() {
        for bad in [
            "",
            "AA:BB:CC:DD:EE",
            "AA:BB:CC:DD:EE:FF:00",
            "ZZ:BB:CC:DD:EE:FF",
        ] {
            assert_eq!(parse_address(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_completion_handler_iid_is_stable_and_type_dependent() {
        // Each instantiation is a distinct interface; handing an operation the
        // wrong handler is refused rather than mis-delivered.
        let of = |s: Signature| completed_handler_iid(s);
        assert_eq!(of(Signature::BOOL), of(Signature::BOOL));
        assert_ne!(of(Signature::BOOL), of(Signature::I32));

        let device = Signature::Class {
            name: classes::BLUETOOTH_LE_DEVICE.0,
            default_interface: classes::BLUETOOTH_LE_DEVICE.1,
        };
        assert_ne!(of(device), of(Signature::BOOL));
    }

    #[test]
    fn the_handler_signature_nests_operation_inside_handler() {
        // AsyncOperationCompletedHandler<IAsyncOperation<T>> — getting the
        // nesting backwards yields a plausible IID that matches nothing.
        let signature = Signature::Parameterized {
            generic: generics::ASYNC_OPERATION_COMPLETED_HANDLER,
            arguments: vec![Signature::Parameterized {
                generic: generics::I_ASYNC_OPERATION,
                arguments: vec![Signature::BOOL],
            }],
        };
        let text = signature.text();
        assert!(text.starts_with("pinterface({fcdcf02c-e5d8-4478-915a-4d90b74b83a5};"));
        assert!(text.contains("pinterface({9fc2b0bb-e446-44e2-aa61-9cab8f636af2};b1)"));
        assert_eq!(
            signature.iid(),
            Some(completed_handler_iid(Signature::BOOL))
        );
    }

    #[test]
    fn activation_slots_are_after_iinspectable() {
        assert_eq!(ACTIVATE_INSTANCE, 6);
        assert_eq!(slots::iasync_operation::GET_RESULTS, 8);
        assert_eq!(slots::ivector_view::GET_AT, 6);
        assert_eq!(slots::ivector_view::SIZE, 7);
    }
}
