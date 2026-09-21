//! Strings and activation — the two things `combase.dll` is needed for.
//!
//! WinRT strings are `HSTRING`: immutable, reference-counted, UTF-16, and
//! allocated by the runtime. WinRT objects are reached through an *activation
//! factory*, asked for by class name, which is why the class names in
//! [`crate::iids::classes`] matter as much as the IIDs beside them.
//!
//! The UTF-16 conversion is ordinary Rust and is tested here; everything that
//! crosses into `combase` is not, and cannot be without Windows.

// An HSTRING is a handle, not something dereferenced here — it is passed
// straight back to combase, which is what validates it.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::com::{succeeded, ComPtr, Hresult};
use crate::guid::Guid;
use std::ffi::c_void;

/// An `HSTRING`.
pub type HString = *mut c_void;

#[cfg(windows)]
#[link(name = "combase")]
unsafe extern "system" {
    fn WindowsCreateString(source: *const u16, length: u32, out: *mut HString) -> Hresult;
    fn WindowsDeleteString(string: HString) -> Hresult;
    fn WindowsGetStringRawBuffer(string: HString, length: *mut u32) -> *const u16;
    fn RoInitialize(kind: i32) -> Hresult;
    fn RoUninitialize();
    fn RoGetActivationFactory(
        class_id: HString,
        iid: *const Guid,
        factory: *mut *mut c_void,
    ) -> Hresult;
}

// Off Windows these are never called — the backend is `cfg`'d out — but the
// module still compiles so its conversions can be tested anywhere.
#[cfg(not(windows))]
#[allow(non_snake_case, unused_variables)]
mod stubs {
    use super::*;
    pub unsafe fn WindowsCreateString(_: *const u16, _: u32, _: *mut HString) -> Hresult {
        crate::com::E_NOINTERFACE
    }
    pub unsafe fn WindowsDeleteString(_: HString) -> Hresult {
        crate::com::S_OK
    }
    pub unsafe fn WindowsGetStringRawBuffer(_: HString, length: *mut u32) -> *const u16 {
        if !length.is_null() {
            unsafe { *length = 0 };
        }
        std::ptr::null()
    }
    pub unsafe fn RoInitialize(_: i32) -> Hresult {
        crate::com::E_NOINTERFACE
    }
    pub unsafe fn RoUninitialize() {}
    pub unsafe fn RoGetActivationFactory(
        _: HString,
        _: *const Guid,
        _: *mut *mut c_void,
    ) -> Hresult {
        crate::com::E_NOINTERFACE
    }
}
#[cfg(not(windows))]
use stubs::*;

/// `RO_INIT_MULTITHREADED`. Single-threaded apartments need a message pump, and
/// a library has no business installing one.
const RO_INIT_MULTITHREADED: i32 = 1;

/// An owned `HSTRING`, deleted on drop.
pub struct WinRtString(HString);

impl WinRtString {
    /// Copy a Rust string into the runtime.
    pub fn new(s: &str) -> Option<Self> {
        let utf16 = to_utf16(s);
        let mut out: HString = std::ptr::null_mut();
        // The length excludes the terminator, which the runtime adds.
        let hr = unsafe { WindowsCreateString(utf16.as_ptr(), (utf16.len() - 1) as u32, &mut out) };
        (succeeded(hr) && !out.is_null()).then_some(Self(out))
    }

    #[inline]
    pub fn as_raw(&self) -> HString {
        self.0
    }

    /// Read an `HSTRING` back, borrowed. An empty string is a null handle, not
    /// a pointer to nothing — which is why this is not simply a null check.
    pub fn read(handle: HString) -> String {
        if handle.is_null() {
            return String::new();
        }
        let mut length = 0u32;
        let buffer = unsafe { WindowsGetStringRawBuffer(handle, &mut length) };
        if buffer.is_null() || length == 0 {
            return String::new();
        }
        let slice = unsafe { std::slice::from_raw_parts(buffer, length as usize) };
        String::from_utf16_lossy(slice)
    }
}

impl Drop for WinRtString {
    fn drop(&mut self) {
        unsafe { WindowsDeleteString(self.0) };
    }
}

/// Release an `HSTRING` obtained from a property getter.
///
/// # Safety
/// `handle` must be an `HSTRING` this process owns and has not already deleted.
pub unsafe fn delete_string(handle: HString) {
    unsafe { WindowsDeleteString(handle) };
}

/// UTF-16, NUL-terminated.
///
/// Windows wants sixteen-bit units with a terminator; Rust strings are UTF-8
/// with neither. Astral characters become surrogate pairs, which `encode_utf16`
/// handles and a naive cast would not.
pub fn to_utf16(s: &str) -> Vec<u16> {
    let mut out: Vec<u16> = s.encode_utf16().collect();
    out.push(0);
    out
}

/// Join the multi-threaded apartment for this thread.
///
/// Every thread that touches WinRT must, and it is refcounted, so calling twice
/// is fine as long as each is balanced.
pub fn initialize() -> bool {
    // RPC_E_CHANGED_MODE means the thread is already in a single-threaded
    // apartment — someone else's, and workable.
    const RPC_E_CHANGED_MODE: Hresult = -2147417850;
    let hr = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    succeeded(hr) || hr == RPC_E_CHANGED_MODE
}

pub fn uninitialize() {
    unsafe { RoUninitialize() };
}

/// The activation factory for a runtime class.
///
/// Every WinRT object begins here: `BluetoothLEDevice` has no constructor, only
/// static methods on a factory reached by name.
pub fn activation_factory(class_name: &str, iid: Guid) -> Option<ComPtr> {
    let name = WinRtString::new(class_name)?;
    let mut factory: *mut c_void = std::ptr::null_mut();
    let hr = unsafe { RoGetActivationFactory(name.as_raw(), &iid, &mut factory) };
    if !succeeded(hr) {
        return None;
    }
    // SAFETY: the factory arrives at +1.
    unsafe { ComPtr::adopt(factory) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_is_terminated() {
        assert_eq!(to_utf16(""), vec![0]);
        assert_eq!(to_utf16("ab"), vec![0x61, 0x62, 0]);
        assert_eq!(*to_utf16("anything").last().unwrap(), 0);
    }

    #[test]
    fn astral_characters_become_surrogate_pairs() {
        // A naive byte-to-u16 cast would mangle these; encode_utf16 does not.
        let coffee = to_utf16("☕");
        assert_eq!(coffee, vec![0x2615, 0]);

        let emoji = to_utf16("🦀");
        assert_eq!(emoji.len(), 3, "one pair plus the terminator");
        assert_eq!(emoji, vec![0xD83E, 0xDD80, 0]);
    }

    #[test]
    fn the_length_passed_to_windows_excludes_the_terminator() {
        // WindowsCreateString takes the character count, not the buffer length;
        // passing the latter creates a string with a stray NUL inside it.
        for s in ["", "a", "hello", "☕", "🦀"] {
            let utf16 = to_utf16(s);
            assert_eq!(utf16.len() - 1, s.encode_utf16().count());
        }
    }

    #[test]
    fn reading_a_null_handle_gives_an_empty_string() {
        // WinRT represents the empty string as a null HSTRING, so this is a
        // normal value rather than a failure.
        assert_eq!(WinRtString::read(std::ptr::null_mut()), "");
    }

    #[test]
    fn class_names_round_trip_through_utf16() {
        let name = "Windows.Devices.Bluetooth.BluetoothLEDevice";
        let utf16 = to_utf16(name);
        assert_eq!(String::from_utf16_lossy(&utf16[..utf16.len() - 1]), name);
    }
}
