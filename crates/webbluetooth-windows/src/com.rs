//! COM, by hand.
//!
//! A COM object is a pointer to a pointer to a table of function pointers. The
//! first three entries are always `QueryInterface`, `AddRef` and `Release`; a
//! WinRT object adds three more for `IInspectable`, so an interface's own
//! methods begin at slot six. Calls use the `system` ABI — `stdcall` on 32-bit
//! Windows, the ordinary C convention elsewhere.
//!
//! Two directions matter here:
//!
//! * **Calling out** — [`ComPtr`] wraps a pointer, holds a reference for as
//!   long as it lives, and invokes methods by slot.
//! * **Being called** — Windows delivers events and async completions to an
//!   object *you* supply, so [`Delegate`] is one: a `#[repr(C)]` struct whose
//!   first field is a vtable, with a refcount and a Rust closure behind it.
//!
//! The second is the interesting half, and it is entirely ordinary Rust — which
//! is why its reference counting and `QueryInterface` behaviour are tested
//! directly, on any host, by calling through the vtable exactly as COM would.

use crate::guid::Guid;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

/// `HRESULT`. Negative means failure.
pub type Hresult = i32;

pub const S_OK: Hresult = 0;
pub const E_NOINTERFACE: Hresult = -2147467262; // 0x8000_4002
pub const E_POINTER: Hresult = -2147467261; // 0x8000_4003

/// Did the call succeed?
#[inline]
pub fn succeeded(hr: Hresult) -> bool {
    hr >= 0
}

/// `IUnknown`.
pub const IID_IUNKNOWN: Guid = Guid::parse("00000000-0000-0000-c000-000000000046");
/// `IAgileObject` — marks an object safe to call from any apartment. WinRT
/// asks for it, and a delegate that refuses is one the runtime may marshal.
pub const IID_IAGILE_OBJECT: Guid = Guid::parse("94ea2b94-e9cc-49e0-c0ff-ee64ca8f5b90");
/// `IInspectable`.
pub const IID_IINSPECTABLE: Guid = Guid::parse("af86e2e0-b12d-4c6a-9c5a-d7aa65101e90");

/// The three `IUnknown` slots every COM vtable starts with.
#[repr(C)]
pub struct IUnknownVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
}

/// `IInspectable`: `IUnknown` plus three.
#[repr(C)]
pub struct IInspectableVtbl {
    pub base: IUnknownVtbl,
    pub get_iids: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut Guid) -> Hresult,
    pub get_runtime_class_name: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    pub get_trust_level: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

/// The first slot an interface's own methods can occupy.
///
/// Three for `IUnknown`, three for `IInspectable`. A method called one slot out
/// is a jump into the wrong function with the wrong arguments.
pub const FIRST_METHOD: usize = 6;

/// An owned reference to a COM object.
///
/// Releases on drop and adds a reference on clone, so ownership follows Rust's
/// rules rather than the caller's memory.
pub struct ComPtr(*mut c_void);

impl ComPtr {
    /// Adopt a pointer the callee already added a reference for — which is the
    /// convention for every `[out]` parameter and return value in COM.
    ///
    /// # Safety
    /// `raw` must be null or a COM interface pointer owned at `+1`.
    pub unsafe fn adopt(raw: *mut c_void) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }

    #[inline]
    pub fn as_raw(&self) -> *mut c_void {
        self.0
    }

    /// A stable identity key.
    #[inline]
    pub fn key(&self) -> usize {
        self.0 as usize
    }

    #[inline]
    fn vtable(&self) -> *const *const c_void {
        // SAFETY: a COM pointer's first word is its vtable pointer.
        unsafe { *(self.0 as *const *const *const c_void) }
    }

    /// The function in slot `index`, typed.
    ///
    /// # Safety
    /// `T` must match the signature at that slot, receiver included.
    #[inline]
    pub unsafe fn method<T: Copy>(&self, index: usize) -> T {
        unsafe { std::mem::transmute_copy(&*self.vtable().add(index)) }
    }

    /// `QueryInterface`, which is how one interface on an object reaches
    /// another — and how WinRT's many small interfaces on one class are used.
    pub fn cast(&self, iid: Guid) -> Option<ComPtr> {
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: slot 0 is always QueryInterface.
        let hr = unsafe {
            let f: unsafe extern "system" fn(
                *mut c_void,
                *const Guid,
                *mut *mut c_void,
            ) -> Hresult = self.method(0);
            f(self.0, &iid, &mut out)
        };
        (succeeded(hr) && !out.is_null()).then_some(ComPtr(out))
    }
}

impl Clone for ComPtr {
    fn clone(&self) -> Self {
        // SAFETY: slot 1 is always AddRef.
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void) -> u32 = self.method(1);
            f(self.0);
        }
        Self(self.0)
    }
}

impl Drop for ComPtr {
    fn drop(&mut self) {
        // SAFETY: slot 2 is always Release.
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void) -> u32 = self.method(2);
            f(self.0);
        }
    }
}

impl std::fmt::Debug for ComPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ComPtr({:p})", self.0)
    }
}

// A COM object may be called from any thread; that is what IAgileObject
// asserts, and every object this crate hands out is agile.
unsafe impl Send for ComPtr {}
unsafe impl Sync for ComPtr {}

// ── Being called ────────────────────────────────────────────────────────────

/// The vtable of a WinRT delegate: `IUnknown` plus a single `Invoke`.
///
/// Delegates are not `IInspectable`, so `Invoke` sits at slot three rather than
/// six — the one place in this crate where [`FIRST_METHOD`] does not apply.
#[repr(C)]
pub struct DelegateVtbl {
    pub base: IUnknownVtbl,
    pub invoke: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> Hresult,
}

/// A delegate Windows can call: an event handler or an async completion.
///
/// `#[repr(C)]` with the vtable first, so a pointer to this *is* a valid COM
/// interface pointer. The refcount is atomic because the runtime may hold and
/// release it from several threads.
#[repr(C)]
pub struct Delegate {
    vtable: *const DelegateVtbl,
    references: AtomicU32,
    /// The IID this delegate answers to — computed, for a parameterised type.
    iid: Guid,
    /// Called with the delegate's two arguments: sender and args, or the
    /// completed operation and its status.
    handler: Box<dyn Fn(*mut c_void, *mut c_void) + Send + Sync>,
}

impl Delegate {
    /// Build one, returning a `+1` COM pointer.
    ///
    /// Not `-> Self`: the object owns itself and lives until the last
    /// `Release`, so what the caller gets is a reference to it, not the value.
    #[allow(clippy::new_ret_no_self)]
    ///
    /// The object owns itself: it is freed when the last `Release` arrives,
    /// which is why this hands back a pointer rather than a Rust value.
    pub fn new(
        iid: Guid,
        handler: impl Fn(*mut c_void, *mut c_void) + Send + Sync + 'static,
    ) -> ComPtr {
        static VTABLE: DelegateVtbl = DelegateVtbl {
            base: IUnknownVtbl {
                query_interface: delegate_query_interface,
                add_ref: delegate_add_ref,
                release: delegate_release,
            },
            invoke: delegate_invoke,
        };

        let boxed = Box::new(Delegate {
            vtable: &VTABLE,
            references: AtomicU32::new(1),
            iid,
            handler: Box::new(handler),
        });
        ComPtr(Box::into_raw(boxed) as *mut c_void)
    }
}

unsafe extern "system" fn delegate_query_interface(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `this` is a Delegate, by construction.
    let delegate = unsafe { &*(this as *const Delegate) };
    let requested = if iid.is_null() {
        Guid::default()
    } else {
        unsafe { *iid }
    };

    // A delegate answers to IUnknown, to IAgileObject — refusing that makes the
    // runtime marshal it across apartments — and to its own computed IID.
    if requested == IID_IUNKNOWN || requested == IID_IAGILE_OBJECT || requested == delegate.iid {
        unsafe {
            delegate_add_ref(this);
            *out = this;
        }
        return S_OK;
    }
    unsafe { *out = std::ptr::null_mut() };
    E_NOINTERFACE
}

unsafe extern "system" fn delegate_add_ref(this: *mut c_void) -> u32 {
    let delegate = unsafe { &*(this as *const Delegate) };
    delegate.references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn delegate_release(this: *mut c_void) -> u32 {
    let delegate = unsafe { &*(this as *const Delegate) };
    let remaining = delegate.references.fetch_sub(1, Ordering::Release) - 1;
    if remaining == 0 {
        // Acquire pairs with every Release above, so the box is not dropped
        // while another thread is still inside a method.
        std::sync::atomic::fence(Ordering::Acquire);
        drop(unsafe { Box::from_raw(this as *mut Delegate) });
    }
    remaining
}

unsafe extern "system" fn delegate_invoke(
    this: *mut c_void,
    sender: *mut c_void,
    args: *mut c_void,
) -> Hresult {
    let delegate = unsafe { &*(this as *const Delegate) };
    // A panic crossing back into Windows is undefined, so it stops here.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (delegate.handler)(sender, args);
    }));
    if result.is_err() {
        return E_POINTER;
    }
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    const TEST_IID: Guid = Guid::parse("1234abcd-0000-0000-0000-000000000001");
    const OTHER_IID: Guid = Guid::parse("1234abcd-0000-0000-0000-000000000002");

    /// Call through the vtable exactly as COM would.
    fn query(p: &ComPtr, iid: Guid) -> (Hresult, *mut c_void) {
        let mut out = std::ptr::null_mut();
        let hr = unsafe {
            let f: unsafe extern "system" fn(
                *mut c_void,
                *const Guid,
                *mut *mut c_void,
            ) -> Hresult = p.method(0);
            f(p.as_raw(), &iid, &mut out)
        };
        (hr, out)
    }

    fn release(raw: *mut c_void) -> u32 {
        unsafe { delegate_release(raw) }
    }

    #[test]
    fn the_vtable_layout_is_what_com_expects() {
        // Three pointers for IUnknown, six for IInspectable, four for a
        // delegate. A wrong size means every slot after it is misaddressed.
        assert_eq!(
            std::mem::size_of::<IUnknownVtbl>(),
            3 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::size_of::<IInspectableVtbl>(),
            6 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::size_of::<DelegateVtbl>(),
            4 * std::mem::size_of::<usize>()
        );
        assert_eq!(FIRST_METHOD, 6);
    }

    #[test]
    fn a_delegate_is_a_valid_com_pointer() {
        // Its first word must be the vtable, or Windows calls into nothing.
        let delegate = Delegate::new(TEST_IID, |_, _| {});
        let first_word = unsafe { *(delegate.as_raw() as *const usize) };
        assert_ne!(first_word, 0);
        assert_eq!(first_word, &raw const *delegate_vtable() as usize);
    }

    fn delegate_vtable() -> &'static DelegateVtbl {
        // The same static `Delegate::new` installs.
        let d = Delegate::new(TEST_IID, |_, _| {});
        unsafe { &*(*(d.as_raw() as *const *const DelegateVtbl)) }
    }

    #[test]
    fn invoke_reaches_the_closure() {
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        let delegate = Delegate::new(TEST_IID, move |sender, _args| {
            counter.fetch_add(sender as usize, Ordering::Relaxed);
        });

        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> Hresult =
                delegate.method(3);
            assert_eq!(
                f(delegate.as_raw(), 7 as *mut c_void, std::ptr::null_mut()),
                S_OK
            );
            assert_eq!(
                f(delegate.as_raw(), 5 as *mut c_void, std::ptr::null_mut()),
                S_OK
            );
        }
        assert_eq!(seen.load(Ordering::Relaxed), 12);
    }

    #[test]
    fn a_panic_does_not_unwind_into_windows() {
        // Unwinding across the ABI is undefined; it must become an HRESULT.
        let delegate = Delegate::new(TEST_IID, |_, _| panic!("handler exploded"));
        let hr = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> Hresult =
                delegate.method(3);
            f(
                delegate.as_raw(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert!(!succeeded(hr), "a panicking handler must report failure");
    }

    #[test]
    fn query_interface_answers_the_three_it_must() {
        let delegate = Delegate::new(TEST_IID, |_, _| {});

        for iid in [IID_IUNKNOWN, IID_IAGILE_OBJECT, TEST_IID] {
            let (hr, out) = query(&delegate, iid);
            assert_eq!(hr, S_OK, "should answer to {iid}");
            assert_eq!(out, delegate.as_raw());
            // QueryInterface adds a reference; give it back.
            release(out);
        }

        let (hr, out) = query(&delegate, OTHER_IID);
        assert_eq!(hr, E_NOINTERFACE);
        assert!(
            out.is_null(),
            "a failed QueryInterface must null the out pointer"
        );
    }

    #[test]
    fn a_null_out_pointer_is_rejected_rather_than_written_through() {
        let delegate = Delegate::new(TEST_IID, |_, _| {});
        let hr = unsafe {
            let f: unsafe extern "system" fn(
                *mut c_void,
                *const Guid,
                *mut *mut c_void,
            ) -> Hresult = delegate.method(0);
            f(delegate.as_raw(), &TEST_IID, std::ptr::null_mut())
        };
        assert_eq!(hr, E_POINTER);
    }

    #[test]
    fn reference_counting_frees_exactly_once() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let witness = dropped.clone();
        // The closure owns the Arc, so the count rises when the box is freed.
        let delegate = Delegate::new(TEST_IID, move |_, _| {
            let _ = &witness;
        });
        let raw = delegate.as_raw();

        let add_ref = |raw| unsafe { delegate_add_ref(raw) };
        assert_eq!(add_ref(raw), 2);
        assert_eq!(add_ref(raw), 3);
        assert_eq!(release(raw), 2);
        assert_eq!(release(raw), 1);

        assert_eq!(Arc::strong_count(&dropped), 2, "still held by the delegate");
        drop(delegate); // the last Release
        assert_eq!(
            Arc::strong_count(&dropped),
            1,
            "the closure was freed exactly once"
        );
    }

    #[test]
    fn cloning_a_com_pointer_holds_a_reference() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let witness = dropped.clone();
        let delegate = Delegate::new(TEST_IID, move |_, _| {
            let _ = &witness;
        });

        let second = delegate.clone();
        drop(delegate);
        assert_eq!(Arc::strong_count(&dropped), 2, "the clone still holds it");
        drop(second);
        assert_eq!(Arc::strong_count(&dropped), 1);
    }

    #[test]
    fn delegates_with_different_iids_do_not_answer_for_each_other() {
        // Parameterised delegates differ only by computed IID, so this is what
        // keeps an IAsyncOperation<bool> handler from being handed an
        // IAsyncOperation<i32>.
        let a = Delegate::new(TEST_IID, |_, _| {});
        let b = Delegate::new(OTHER_IID, |_, _| {});

        // Every *successful* QueryInterface hands back a +1 reference, so each
        // one has to be released or the object never reaches zero. A failed
        // one adds nothing and has nothing to release.
        let (hr, matched) = query(&a, TEST_IID);
        assert_eq!(hr, S_OK);
        release(matched);
        assert_eq!(query(&a, OTHER_IID).0, E_NOINTERFACE);

        let (hr, matched) = query(&b, OTHER_IID);
        assert_eq!(hr, S_OK);
        release(matched);
        assert_eq!(query(&b, TEST_IID).0, E_NOINTERFACE);
    }
}
