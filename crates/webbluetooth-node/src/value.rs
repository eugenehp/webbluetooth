//! Converting between JavaScript values and Rust ones.
//!
//! Every function here takes a `napi_env` and is only valid on the runtime's
//! own thread. That is not a convention — a `napi_value` is a handle into a
//! scope the runtime owns, and using one from a backend's callback thread is
//! undefined behaviour rather than an error you get back. Anything arriving
//! from elsewhere goes through a threadsafe function first; see [`crate::emit`].

use crate::napi::*;
use std::ffi::{c_char, CString};

/// A JavaScript exception, carried back to the call that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thrown(pub String);

pub type Result<T> = std::result::Result<T, Thrown>;

/// Check a status, naming what failed.
pub fn ok(status: napi_status, what: &str) -> Result<()> {
    match status == NAPI_OK {
        true => Ok(()),
        false => Err(Thrown(format!("{what} failed (napi status {status})"))),
    }
}

/// `undefined`.
pub fn undefined(env: napi_env) -> Result<napi_value> {
    let mut out = std::ptr::null_mut();
    ok(unsafe { napi_get_undefined(env, &mut out) }, "undefined")?;
    Ok(out)
}

/// `null`.
pub fn null(env: napi_env) -> Result<napi_value> {
    let mut out = std::ptr::null_mut();
    ok(unsafe { napi_get_null(env, &mut out) }, "null")?;
    Ok(out)
}

pub fn boolean(env: napi_env, value: bool) -> Result<napi_value> {
    let mut out = std::ptr::null_mut();
    ok(unsafe { napi_get_boolean(env, value, &mut out) }, "boolean")?;
    Ok(out)
}

pub fn number(env: napi_env, value: f64) -> Result<napi_value> {
    let mut out = std::ptr::null_mut();
    ok(
        unsafe { napi_create_double(env, value, &mut out) },
        "number",
    )?;
    Ok(out)
}

pub fn string(env: napi_env, value: &str) -> Result<napi_value> {
    let mut out = std::ptr::null_mut();
    // The length is given explicitly, so an interior NUL is carried rather
    // than truncating the string at it.
    ok(
        unsafe {
            napi_create_string_utf8(env, value.as_ptr() as *const c_char, value.len(), &mut out)
        },
        "string",
    )?;
    Ok(out)
}

pub fn object(env: napi_env) -> Result<napi_value> {
    let mut out = std::ptr::null_mut();
    ok(unsafe { napi_create_object(env, &mut out) }, "object")?;
    Ok(out)
}

/// An `ArrayBuffer` holding a copy of `bytes`.
///
/// Copied rather than borrowed: the runtime's buffer outlives this call and
/// the Rust slice does not.
pub fn array_buffer(env: napi_env, bytes: &[u8]) -> Result<napi_value> {
    let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut out = std::ptr::null_mut();
    ok(
        unsafe { napi_create_arraybuffer(env, bytes.len(), &mut data, &mut out) },
        "ArrayBuffer",
    )?;
    if !bytes.is_empty() {
        // SAFETY: the runtime just gave us `bytes.len()` writable bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len()) };
    }
    Ok(out)
}

/// An array built from an iterator of already-converted values.
pub fn array(env: napi_env, items: &[napi_value]) -> Result<napi_value> {
    let mut out = std::ptr::null_mut();
    ok(
        unsafe { napi_create_array_with_length(env, items.len(), &mut out) },
        "array",
    )?;
    for (i, item) in items.iter().enumerate() {
        ok(
            unsafe { napi_set_element(env, out, i as u32, *item) },
            "array element",
        )?;
    }
    Ok(out)
}

/// Set a property, taking the name as a Rust string.
pub fn set(env: napi_env, object: napi_value, name: &str, value: napi_value) -> Result<()> {
    let name = CString::new(name).map_err(|_| Thrown("a property name held a NUL".into()))?;
    ok(
        unsafe { napi_set_named_property(env, object, name.as_ptr(), value) },
        "set property",
    )
}

/// Read a property, or `None` if the object does not have it.
pub fn get(env: napi_env, object: napi_value, name: &str) -> Result<Option<napi_value>> {
    let cname = CString::new(name).map_err(|_| Thrown("a property name held a NUL".into()))?;
    let mut has = false;
    ok(
        unsafe { napi_has_named_property(env, object, cname.as_ptr(), &mut has) },
        "has property",
    )?;
    if !has {
        return Ok(None);
    }
    let mut out = std::ptr::null_mut();
    ok(
        unsafe { napi_get_named_property(env, object, cname.as_ptr(), &mut out) },
        "get property",
    )?;
    // A property that is present but undefined is the same as absent for
    // every option this module reads, and saying so here keeps each caller
    // from having to check twice.
    match type_of(env, out)? {
        NAPI_UNDEFINED | NAPI_NULL => Ok(None),
        _ => Ok(Some(out)),
    }
}

pub fn type_of(env: napi_env, value: napi_value) -> Result<napi_valuetype> {
    let mut out = 0;
    ok(unsafe { napi_typeof(env, value, &mut out) }, "typeof")?;
    Ok(out)
}

pub fn as_string(env: napi_env, value: napi_value) -> Result<String> {
    let mut len = 0;
    ok(
        unsafe { napi_get_value_string_utf8(env, value, std::ptr::null_mut(), 0, &mut len) },
        "string length",
    )?;
    // The runtime writes a trailing NUL it does not count.
    let mut buffer = vec![0u8; len + 1];
    let mut written = 0;
    ok(
        unsafe {
            napi_get_value_string_utf8(
                env,
                value,
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len(),
                &mut written,
            )
        },
        "string",
    )?;
    buffer.truncate(written);
    String::from_utf8(buffer).map_err(|_| Thrown("a string was not valid UTF-8".into()))
}

pub fn as_bool(env: napi_env, value: napi_value) -> Result<bool> {
    let mut out = false;
    ok(unsafe { napi_get_value_bool(env, value, &mut out) }, "bool")?;
    Ok(out)
}

pub fn as_f64(env: napi_env, value: napi_value) -> Result<f64> {
    let mut out = 0.0;
    ok(
        unsafe { napi_get_value_double(env, value, &mut out) },
        "number",
    )?;
    Ok(out)
}

/// Read bytes from an `ArrayBuffer`, a `TypedArray` or an array of numbers.
///
/// All three because all three are what a caller will reasonably pass: a
/// `Uint8Array` from `crypto.getRandomValues`, a `Buffer` from `fs`, or a
/// plain `[0x01, 0x02]` written inline.
pub fn as_bytes(env: napi_env, value: napi_value) -> Result<Vec<u8>> {
    let mut is_typed = false;
    ok(
        unsafe { napi_is_typedarray(env, value, &mut is_typed) },
        "is typed array",
    )?;
    if is_typed {
        let (mut kind, mut len, mut data, mut buffer, mut offset) =
            (0, 0, std::ptr::null_mut(), std::ptr::null_mut(), 0);
        ok(
            unsafe {
                napi_get_typedarray_info(
                    env,
                    value,
                    &mut kind,
                    &mut len,
                    &mut data,
                    &mut buffer,
                    &mut offset,
                )
            },
            "typed array",
        )?;
        // `length` is in elements. Only a byte-wide array can be read as
        // bytes without deciding an endianness the caller did not choose.
        const UINT8: i32 = 1;
        const UINT8_CLAMPED: i32 = 2;
        const INT8: i32 = 0;
        if !matches!(kind, UINT8 | UINT8_CLAMPED | INT8) {
            return Err(Thrown(
                "a value must be a Uint8Array, an ArrayBuffer or an array of bytes; \
                 a wider typed array would need a byte order this cannot guess"
                    .into(),
            ));
        }
        if len == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: the runtime reported `len` readable bytes at `data`.
        return Ok(unsafe { std::slice::from_raw_parts(data as *const u8, len) }.to_vec());
    }

    let mut is_buffer = false;
    ok(
        unsafe { napi_is_arraybuffer(env, value, &mut is_buffer) },
        "is ArrayBuffer",
    )?;
    if is_buffer {
        let (mut data, mut len) = (std::ptr::null_mut(), 0);
        ok(
            unsafe { napi_get_arraybuffer_info(env, value, &mut data, &mut len) },
            "ArrayBuffer",
        )?;
        if len == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: as above.
        return Ok(unsafe { std::slice::from_raw_parts(data as *const u8, len) }.to_vec());
    }

    // A plain array of numbers.
    let mut len = 0;
    if unsafe { napi_get_array_length(env, value, &mut len) } == NAPI_OK {
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            let mut item = std::ptr::null_mut();
            ok(
                unsafe { napi_get_element(env, value, i, &mut item) },
                "array element",
            )?;
            let n = as_f64(env, item)?;
            if !(0.0..=255.0).contains(&n) || n.fract() != 0.0 {
                return Err(Thrown(format!(
                    "{n} is not a byte; every element must be a whole number in 0..=255"
                )));
            }
            out.push(n as u8);
        }
        return Ok(out);
    }

    Err(Thrown(
        "expected bytes: a Uint8Array, an ArrayBuffer, or an array of numbers".into(),
    ))
}

/// Throw `message` as an `Error` and return `undefined`.
///
/// Returning a value is what a `napi_callback` must do; once an exception is
/// pending the runtime ignores it, but it still has to be there.
pub fn throw(env: napi_env, message: &str) -> napi_value {
    // Building the error can itself fail, in which case there is nothing left
    // to do but return — the runtime will report the original failure.
    if let (Ok(text), Ok(undef)) = (string(env, message), undefined(env)) {
        let mut error = std::ptr::null_mut();
        if unsafe { napi_create_error(env, std::ptr::null_mut(), text, &mut error) } == NAPI_OK {
            unsafe { napi_throw(env, error) };
        }
        return undef;
    }
    std::ptr::null_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every conversion above reports a failure rather than returning a value
    /// the caller would use as if it had worked.
    #[test]
    fn a_failing_status_becomes_an_error_naming_the_call() {
        let e = ok(7, "napi_create_string_utf8").unwrap_err();
        assert!(e.0.contains("napi_create_string_utf8"), "{}", e.0);
        assert!(e.0.contains('7'), "the status should be in the message");
        assert!(ok(NAPI_OK, "anything").is_ok());
    }

    /// A property name with a NUL in it cannot be passed to a C API, and
    /// truncating at the NUL would set a different property than asked for.
    #[test]
    fn a_property_name_with_a_nul_is_refused() {
        // Exercised through CString directly: the napi call needs a runtime.
        assert!(CString::new("has\0nul").is_err());
        assert!(CString::new("fine").is_ok());
    }
}
