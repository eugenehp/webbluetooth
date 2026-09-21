//! Typed views of `objc_msgSend`.
//!
//! Every send needs its own signature. On arm64 variadic arguments are passed
//! on the stack, so calling the variadic declaration with real arguments
//! delivers them where the receiver never looks and the callee reads
//! uninitialised registers instead. These macros transmute the symbol's address
//! to a correctly-typed, non-variadic function pointer per call site.

/// Resolve a selector once per call site, then remember it.
///
/// `sel_registerName` is a hash lookup in the runtime's selector table. It is
/// idempotent and returns the same pointer forever, so doing it on every
/// message is work with no result — and it is *per message*, so a write path
/// that builds an `NSData` and sends it pays for three of them plus a class
/// lookup on every packet.
///
/// Hand-written Objective-C does not do this: the compiler emits an entry in
/// `__objc_selrefs` that the loader fixes up once. This is the same idea
/// reached from Rust.
///
/// `Relaxed` is sufficient and not a shortcut. Two threads racing here both
/// call `sel_registerName`, both get the identical pointer, and both store it;
/// there is no value to publish and no pointee of ours to synchronise, because
/// a selector is an opaque token owned by the runtime and valid for the life of
/// the process.
#[macro_export]
macro_rules! cached_sel {
    ($name:expr) => {{
        static CACHE: core::sync::atomic::AtomicPtr<core::ffi::c_void> =
            core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
        let cached = CACHE.load(core::sync::atomic::Ordering::Relaxed);
        if cached.is_null() {
            let resolved = $crate::objc::sel_registerName($name.as_ptr() as *const _);
            CACHE.store(
                resolved as *mut core::ffi::c_void,
                core::sync::atomic::Ordering::Relaxed,
            );
            resolved
        } else {
            cached as $crate::objc::Sel
        }
    }};
}

/// Look up a class once per call site, then remember it.
///
/// As [`cached_sel`], for `objc_getClass`. A class pointer is stable once its
/// image is loaded, and every framework this crate touches is linked at
/// startup — so a null here means the class genuinely does not exist, which is
/// why the lookup is retried rather than the null being cached.
#[macro_export]
macro_rules! cached_class {
    ($name:expr) => {{
        static CACHE: core::sync::atomic::AtomicPtr<core::ffi::c_void> =
            core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
        let cached = CACHE.load(core::sync::atomic::Ordering::Relaxed);
        if cached.is_null() {
            let resolved = $crate::objc::require_class($name);
            CACHE.store(
                resolved as *mut core::ffi::c_void,
                core::sync::atomic::Ordering::Relaxed,
            );
            resolved
        } else {
            cached as $crate::objc::Id
        }
    }};
}

/// Send a message returning an object pointer.
///
/// ```ignore
/// let obj: Id = msg_send![class, alloc];
/// let s: Id = msg_send![dict, objectForKey: key];
/// ```
#[macro_export]
macro_rules! msg_send {
    [$obj:expr, $sel:ident] => {{
        let f: unsafe extern "C" fn($crate::objc::Id, $crate::objc::Sel) -> $crate::objc::Id =
            core::mem::transmute($crate::objc::objc_msgSend as *const ());
        f($obj as $crate::objc::Id,
          $crate::cached_sel!(concat!(stringify!($sel), "\0")))
    }};
    [$obj:expr, $($sel:ident : $arg:expr),+ $(,)?] => {{
        let sel = $crate::cached_sel!(concat!($(stringify!($sel), ":",)+ "\0"));
        $crate::msg_send!(@call $obj, sel, $($arg),+)
    }};
    (@call $obj:expr, $sel:expr, $($arg:expr),+) => {{
        let f: unsafe extern "C" fn(
            $crate::objc::Id, $crate::objc::Sel, $($crate::replace_expr!($arg, _)),+
        ) -> $crate::objc::Id =
            core::mem::transmute($crate::objc::objc_msgSend as *const ());
        f($obj as $crate::objc::Id, $sel, $($arg),+)
    }};
}

/// Send a message returning a non-object value (`bool`, `isize`, a pointer…).
///
/// ```ignore
/// let n: usize = msg_send_t![array, count];
/// let ok: bool = msg_send_t![obj, isKindOfClass: cls];
/// ```
#[macro_export]
macro_rules! msg_send_t {
    [$t:ty; $obj:expr, $sel:ident] => {{
        let f: unsafe extern "C" fn($crate::objc::Id, $crate::objc::Sel) -> $t =
            core::mem::transmute($crate::objc::objc_msgSend as *const ());
        f($obj as $crate::objc::Id,
          $crate::cached_sel!(concat!(stringify!($sel), "\0")))
    }};
    [$t:ty; $obj:expr, $($sel:ident : $arg:expr),+ $(,)?] => {{
        let sel = $crate::cached_sel!(concat!($(stringify!($sel), ":",)+ "\0"));
        let f: unsafe extern "C" fn(
            $crate::objc::Id, $crate::objc::Sel, $($crate::replace_expr!($arg, _)),+
        ) -> $t = core::mem::transmute($crate::objc::objc_msgSend as *const ());
        f($obj as $crate::objc::Id, sel, $($arg),+)
    }};
}

/// Send a message returning nothing.
#[macro_export]
macro_rules! msg_send_void {
    [$obj:expr, $sel:ident] => {{
        let f: unsafe extern "C" fn($crate::objc::Id, $crate::objc::Sel) =
            core::mem::transmute($crate::objc::objc_msgSend as *const ());
        f($obj as $crate::objc::Id,
          $crate::cached_sel!(concat!(stringify!($sel), "\0")))
    }};
    [$obj:expr, $($sel:ident : $arg:expr),+ $(,)?] => {{
        let sel = $crate::cached_sel!(concat!($(stringify!($sel), ":",)+ "\0"));
        let f: unsafe extern "C" fn(
            $crate::objc::Id, $crate::objc::Sel, $($crate::replace_expr!($arg, _)),+
        ) = core::mem::transmute($crate::objc::objc_msgSend as *const ());
        f($obj as $crate::objc::Id, sel, $($arg),+)
    }};
}

/// Expand to `$sub` once per expression — how the macros above turn a list of
/// arguments into a list of inferred parameter types.
#[doc(hidden)]
#[macro_export]
macro_rules! replace_expr {
    ($_e:expr, $sub:tt) => {
        $sub
    };
}
