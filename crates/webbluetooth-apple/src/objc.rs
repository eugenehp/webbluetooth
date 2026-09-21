//! Objective-C runtime: message sends, collection readers, and class synthesis.
//!
//! CoreBluetooth exports no Swift-native symbols — its `.tbd` lists Objective-C
//! classes and nothing mangled — so the Swift metadata synthesis that
//! `swiftui-native`'s `state`/`conformance` modules perform has no target here.
//! The equivalent for an Objective-C framework is [`ClassBuilder`], modelled on
//! `swiftui-native::objc`: allocate a class pair, give it methods whose
//! implementations are `extern "C"` Rust functions, declare the protocol, and
//! register it. That is how [`crate::delegate`] produces a
//! `CBCentralManagerDelegate` without a line of Objective-C or Swift.
//!
//! Message sends go through `apple-objc-sys`, whose `msg_send!` family
//! transmutes `objc_msgSend` to a correctly-typed, *non-variadic* signature per
//! call site. That matters on arm64: variadic arguments are passed on the
//! stack, so a variadic declaration delivers them where the receiver never
//! looks.

use core::ffi::{c_char, c_void, CStr};

/// An opaque Objective-C object pointer.
pub type Id = *mut c_void;
/// An opaque Objective-C selector.
pub type Sel = *const c_void;
/// `nil`.
pub const NIL: Id = core::ptr::null_mut();

unsafe extern "C" {
    /// Declared variadic to match the symbol; never called through this
    /// declaration. See [`crate::macros`].
    pub fn objc_msgSend(receiver: Id, sel: Sel, ...) -> Id;
    pub fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_getClass(name: *const c_char) -> Id;
    fn objc_getProtocol(name: *const c_char) -> *mut c_void;
    fn objc_allocateClassPair(sup: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn class_addMethod(cls: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> bool;
    fn class_addProtocol(cls: Id, protocol: *mut c_void) -> bool;
    fn objc_retain(obj: Id) -> Id;
    fn objc_release(obj: Id);
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(ctx: *mut c_void);
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

/// Register (or look up) a selector.
#[inline]
pub fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}

/// Look up a registered class by name. Null if the framework is not loaded.
#[inline]
pub fn class(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

/// Look up a class, panicking with the name if it is missing.
#[inline]
pub fn require_class(name: &CStr) -> Id {
    let c = class(name);
    assert!(!c.is_null(), "Objective-C class not found: {name:?}");
    c
}

/// Look up a protocol. Null unless some loaded image references it.
#[inline]
pub fn protocol(name: &CStr) -> *mut c_void {
    unsafe { objc_getProtocol(name.as_ptr()) }
}

/// Read a global `NSString * const` (e.g. `CBAdvertisementDataLocalNameKey`).
///
/// The symbol is a pointer *to* the string pointer, so this dereferences once.
pub fn global_nsstring(symbol: &CStr) -> Id {
    unsafe {
        let p = dlsym(RTLD_DEFAULT, symbol.as_ptr());
        if p.is_null() {
            NIL
        } else {
            *(p as *const Id)
        }
    }
}

// ── Ownership ───────────────────────────────────────────────────────────────

/// A `+1` Objective-C reference, released on drop.
///
/// Delegate callbacks hand over objects they own; anything kept past the end of
/// the callback — a peripheral we are about to connect to, the characteristic a
/// pending read belongs to — has to be retained, and the retain has to be
/// balanced from Rust's side rather than by an autorelease pool that drained
/// when the callback returned.
#[derive(Debug)]
pub struct Retained(Id);

impl Retained {
    /// Retain a borrowed pointer. `None` if null.
    ///
    /// # Safety
    /// `ptr` must be null or a live Objective-C object.
    pub unsafe fn retain(ptr: Id) -> Option<Self> {
        if ptr.is_null() {
            return None;
        }
        Some(Self(unsafe { objc_retain(ptr) }))
    }

    /// Adopt a pointer already owned at `+1` (from `alloc`/`new`/`copy`).
    ///
    /// # Safety
    /// `ptr` must be null or an object the caller owns a reference to.
    pub unsafe fn adopt(ptr: Id) -> Option<Self> {
        if ptr.is_null() {
            return None;
        }
        Some(Self(ptr))
    }

    #[inline]
    pub fn as_ptr(&self) -> Id {
        self.0
    }

    /// The pointer value, used as a stable identity key.
    ///
    /// CoreBluetooth hands back the *same* `CBService` / `CBCharacteristic`
    /// instances for the life of a connection, so the address correlates a
    /// delegate callback with the request that provoked it.
    #[inline]
    pub fn key(&self) -> usize {
        self.0 as usize
    }
}

impl Clone for Retained {
    fn clone(&self) -> Self {
        Self(unsafe { objc_retain(self.0) })
    }
}

impl Drop for Retained {
    fn drop(&mut self) {
        unsafe { objc_release(self.0) }
    }
}

impl PartialEq for Retained {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Retained {}

// CoreBluetooth objects are delivered on our own serial queue and are safe to
// message from any thread; the retain count itself is atomic.
unsafe impl Send for Retained {}
unsafe impl Sync for Retained {}

/// An autorelease pool, drained on drop.
///
/// A Rust program has no Cocoa event loop to provide one, so without this every
/// autoreleased temporary — every `UUIDString`, every `NSData` — accumulates
/// for the life of the process.
pub struct AutoreleasePool(*mut c_void);

impl AutoreleasePool {
    pub fn new() -> Self {
        Self(unsafe { objc_autoreleasePoolPush() })
    }
}
impl Default for AutoreleasePool {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

// ── Foundation readers ──────────────────────────────────────────────────────

/// Create an `NSString` from a Rust `&str`. Returned at `+1`.
///
/// # Safety
/// Caller owns the result.
pub unsafe fn nsstring(s: &str) -> Id {
    unsafe {
        let alloc = msg_send![require_class(c"NSString"), alloc];
        type InitBytes = unsafe extern "C" fn(Id, Sel, *const u8, usize, usize) -> Id;
        let f: InitBytes = core::mem::transmute(objc_msgSend as *const ());
        // 4 = NSUTF8StringEncoding
        f(
            alloc,
            sel(c"initWithBytes:length:encoding:"),
            s.as_ptr(),
            s.len(),
            4,
        )
    }
}

/// Read an `NSString` into a Rust `String`. `None` if null.
///
/// # Safety
/// `s` must be null or an `NSString`.
pub unsafe fn to_string(s: Id) -> Option<String> {
    if s.is_null() {
        return None;
    }
    unsafe {
        let c: *const c_char = msg_send_t![*const c_char; s, UTF8String];
        if c.is_null() {
            return None;
        }
        Some(CStr::from_ptr(c).to_string_lossy().into_owned())
    }
}

/// Number of elements in an `NSArray`. Zero if null.
///
/// # Safety
/// `array` must be null or an `NSArray`.
pub unsafe fn array_count(array: Id) -> usize {
    if array.is_null() {
        return 0;
    }
    unsafe { msg_send_t![usize; array, count] }
}

/// Element `i` of an `NSArray`, borrowed.
///
/// # Safety
/// `array` must be an `NSArray` and `i` within bounds.
pub unsafe fn array_get(array: Id, i: usize) -> Id {
    unsafe { msg_send![array, objectAtIndex: i] }
}

/// Collect an `NSArray` into a `Vec` by applying `f` to each borrowed element.
///
/// # Safety
/// `array` must be null or an `NSArray`.
pub unsafe fn array_map<T>(array: Id, mut f: impl FnMut(Id) -> T) -> Vec<T> {
    let n = unsafe { array_count(array) };
    (0..n).map(|i| f(unsafe { array_get(array, i) })).collect()
}

/// Build an `NSArray` from borrowed elements. Returned at `+1`.
///
/// # Safety
/// Every element must be a live object. Caller owns the result.
pub unsafe fn nsarray(items: &[Id]) -> Id {
    unsafe {
        type WithObjects = unsafe extern "C" fn(Id, Sel, *const Id, usize) -> Id;
        let f: WithObjects = core::mem::transmute(objc_msgSend as *const ());
        let arr = f(
            require_class(c"NSArray"),
            sel(c"arrayWithObjects:count:"),
            items.as_ptr(),
            items.len(),
        );
        objc_retain(arr)
    }
}

/// Build an `NSDictionary` from parallel borrowed key/value slices. `+1`.
///
/// # Safety
/// The slices must be the same length and hold live objects. Caller owns the
/// result.
pub unsafe fn nsdictionary(keys: &[Id], values: &[Id]) -> Option<Retained> {
    debug_assert_eq!(keys.len(), values.len());
    unsafe {
        let k = Retained::adopt(nsarray(keys))?;
        let v = Retained::adopt(nsarray(values))?;
        let d: Id = msg_send![
            require_class(c"NSDictionary"),
            dictionaryWithObjects: v.as_ptr(),
            forKeys: k.as_ptr()
        ];
        Retained::retain(d)
    }
}

/// `[dict objectForKey: key]`, borrowed. Null if either is null or absent.
///
/// # Safety
/// `dict` must be null or an `NSDictionary`.
pub unsafe fn dict_get(dict: Id, key: Id) -> Id {
    if dict.is_null() || key.is_null() {
        return NIL;
    }
    unsafe { msg_send![dict, objectForKey: key] }
}

/// All keys of an `NSDictionary`, borrowed.
///
/// # Safety
/// `dict` must be null or an `NSDictionary`.
pub unsafe fn dict_keys(dict: Id) -> Id {
    if dict.is_null() {
        return NIL;
    }
    unsafe { msg_send![dict, allKeys] }
}

/// Copy an `NSData` into a `Vec<u8>`. `None` if null.
///
/// # Safety
/// `data` must be null or an `NSData`.
pub unsafe fn data_bytes(data: Id) -> Option<Vec<u8>> {
    if data.is_null() {
        return None;
    }
    unsafe {
        let len = msg_send_t![usize; data, length];
        let ptr = msg_send_t![*const u8; data, bytes];
        if ptr.is_null() || len == 0 {
            return Some(Vec::new());
        }
        Some(core::slice::from_raw_parts(ptr, len).to_vec())
    }
}

/// Build an `NSData` from bytes. Returned at `+1`.
///
/// # Safety
/// Caller owns the result.
pub unsafe fn nsdata(bytes: &[u8]) -> Id {
    unsafe {
        // On the write path, so the class and both selectors are resolved
        // once rather than per packet. See `cached_sel`.
        let alloc = msg_send![crate::cached_class!(c"NSData"), alloc];
        type InitBytes = unsafe extern "C" fn(Id, Sel, *const u8, usize) -> Id;
        let f: InitBytes = core::mem::transmute(objc_msgSend as *const ());
        f(
            alloc,
            crate::cached_sel!("initWithBytes:length:\0"),
            bytes.as_ptr(),
            bytes.len(),
        )
    }
}

/// `[number integerValue]`. `None` if null.
///
/// # Safety
/// `n` must be null or an `NSNumber`.
pub unsafe fn number_i64(n: Id) -> Option<i64> {
    if n.is_null() {
        return None;
    }
    Some(unsafe { msg_send_t![isize; n, integerValue] } as i64)
}

/// `[number boolValue]`. `None` if null.
///
/// # Safety
/// `n` must be null or an `NSNumber`.
pub unsafe fn number_bool(n: Id) -> Option<bool> {
    if n.is_null() {
        return None;
    }
    Some(unsafe { msg_send_t![bool; n, boolValue] })
}

/// `[error localizedDescription]` plus its code. `None` if the error is null.
///
/// # Safety
/// `err` must be null or an `NSError`.
pub unsafe fn error_message(err: Id) -> Option<(i64, String)> {
    if err.is_null() {
        return None;
    }
    unsafe {
        let code = msg_send_t![isize; err, code] as i64;
        let desc = to_string(msg_send![err, localizedDescription])
            .unwrap_or_else(|| "unknown CoreBluetooth error".into());
        Some((code, desc))
    }
}

// ── Class synthesis ─────────────────────────────────────────────────────────

/// A class under construction. Add methods, declare protocols, then
/// [`register`](ClassBuilder::register).
///
/// This is the Objective-C counterpart of building a Swift type at runtime:
/// CoreBluetooth will not talk to anything but a delegate *object*, and writing
/// one in Objective-C would mean shipping a source file and a compiler step, so
/// it is synthesised here instead.
pub struct ClassBuilder {
    class: Id,
}

impl ClassBuilder {
    /// Begin a new class. `None` if `name` is already registered — which is how
    /// a second copy of this crate in the same process is detected.
    ///
    /// # Safety
    /// `superclass` must name a registered class.
    pub unsafe fn new(superclass: &CStr, name: &CStr) -> Option<Self> {
        unsafe {
            let sup = objc_getClass(superclass.as_ptr());
            assert!(!sup.is_null(), "superclass not found: {superclass:?}");
            let class = objc_allocateClassPair(sup, name.as_ptr(), 0);
            if class.is_null() {
                return None;
            }
            Some(Self { class })
        }
    }

    /// Add a method.
    ///
    /// `types` is an Objective-C type encoding: return type, then `@:` for the
    /// implicit receiver and selector, then one code per argument. Every
    /// CoreBluetooth delegate method returns void and takes only objects, so
    /// these are all `v@:` followed by one `@` per argument — no `BOOL`, whose
    /// encoding differs between arm64 (`B`) and x86_64 (`c`).
    ///
    /// # Safety
    /// `imp`'s signature must match `types`, receiver and selector included.
    pub unsafe fn method(self, selector: &CStr, imp: *const c_void, types: &CStr) -> Self {
        unsafe {
            let ok = class_addMethod(self.class, sel(selector), imp, types.as_ptr());
            assert!(ok, "could not add method {selector:?}");
        }
        self
    }

    /// Declare conformance to a protocol. Silently skipped if the protocol is
    /// not registered, which only happens if the framework is not loaded.
    ///
    /// # Safety
    /// The class must implement the protocol's required methods.
    pub unsafe fn conforms(self, name: &CStr) -> Self {
        unsafe {
            let p = objc_getProtocol(name.as_ptr());
            if !p.is_null() {
                class_addProtocol(self.class, p);
            }
        }
        self
    }

    /// Register the class with the runtime and return it.
    pub fn register(self) -> Id {
        unsafe { objc_registerClassPair(self.class) };
        self.class
    }
}
