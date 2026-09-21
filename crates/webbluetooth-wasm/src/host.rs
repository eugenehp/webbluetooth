//! The boundary: what this module imports from the page, and what it exports
//! back.
//!
//! WebAssembly in a browser cannot reach `navigator.bluetooth`. It has linear
//! memory and numbers, no DOM and no way to acquire one — so every call out is
//! an import the host fills in, and every call back is an export the host
//! invokes. That is true of any approach, `wasm-bindgen` included; the only
//! question is whether the glue is generated or written down. Here it is
//! written down, in `js/webbluetooth.js`, and it is small enough to read.
//!
//! One import carries every request:
//!
//! ```text
//! wbt_call(op, ptr, len) -> token
//! ```
//!
//! rather than one import per operation. The alternative is forty imports with
//! forty signatures to keep in step across two languages; this way the shape
//! of a call lives in [`crate::codec`], in one place, and adding an operation
//! does not change the ABI.
//!
//! Every call is asynchronous, because on the web every Bluetooth call is.
//! `wbt_call` returns a token immediately and the host later calls
//! [`wbt_settle`] with the answer.

use crate::codec::Writer;
use std::cell::RefCell;
use std::collections::HashMap;

/// What a request is asking for. Mirrored in `js/webbluetooth.js`.
///
/// Numbers rather than strings: a string op-code would mean allocating and
/// decoding one on every call, to express something that is already a closed
/// set known at compile time on both sides.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Availability = 1,
    RequestDevice = 2,
    GetDevices = 3,
    Forget = 4,
    Connect = 5,
    Disconnect = 6,
    DiscoverServices = 7,
    DiscoverIncludedServices = 8,
    DiscoverCharacteristics = 9,
    DiscoverDescriptors = 10,
    ReadCharacteristic = 11,
    WriteCharacteristic = 12,
    ReadDescriptor = 13,
    WriteDescriptor = 14,
    SetNotify = 15,
    WatchAdvertisements = 16,
    UnwatchAdvertisements = 17,
    SetTimeout = 18,
}

/// Why a request failed, as the host reports it.
///
/// These are the `DOMException` names Web Bluetooth throws, kept distinct
/// rather than flattened into one error because the caller acts on them
/// differently — a `NetworkError` is worth retrying and a `SecurityError`
/// never is.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    Unknown = 0,
    NotFound = 1,
    Security = 2,
    Network = 3,
    InvalidState = 4,
    NotSupported = 5,
    InvalidModification = 6,
    Abort = 7,
    NotAllowed = 8,
}

impl HostError {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::NotFound,
            2 => Self::Security,
            3 => Self::Network,
            4 => Self::InvalidState,
            5 => Self::NotSupported,
            6 => Self::InvalidModification,
            7 => Self::Abort,
            8 => Self::NotAllowed,
            _ => Self::Unknown,
        }
    }
}

/// An event the host pushes without being asked. Mirrored in the shim.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    CharacteristicValue = 1,
    GattServerDisconnected = 2,
    ServiceChanged = 3,
    AdvertisementReceived = 4,
    AvailabilityChanged = 5,
}

impl EventKind {
    pub fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            1 => Self::CharacteristicValue,
            2 => Self::GattServerDisconnected,
            3 => Self::ServiceChanged,
            4 => Self::AdvertisementReceived,
            5 => Self::AvailabilityChanged,
            _ => return None,
        })
    }
}

// ── Imports ─────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "webbluetooth")]
unsafe extern "C" {
    /// Start an operation. Returns a token the host will settle later.
    fn wbt_call(op: u32, ptr: *const u8, len: usize) -> u32;
}

/// Off wasm there is no host, so a call fails immediately rather than linking
/// against nothing. This exists so the crate builds, and its tests run, on the
/// machine it is developed on.
#[cfg(not(target_arch = "wasm32"))]
unsafe fn wbt_call(_op: u32, _ptr: *const u8, _len: usize) -> u32 {
    0
}

// ── Pending requests ────────────────────────────────────────────────────────

/// What a settled request produced.
pub type Answer = Result<Vec<u8>, HostError>;

thread_local! {
    /// Requests started and not yet settled.
    ///
    /// A browser is single-threaded and so is this: `thread_local` rather than
    /// a `Mutex` because there is no second thread to contend with, and a lock
    /// would be a lock nobody ever waits on.
    static PENDING: RefCell<HashMap<u32, PendingState>> = RefCell::new(HashMap::new());
}

enum PendingState {
    /// Started; nobody is waiting on it yet.
    Waiting(Option<std::task::Waker>),
    /// Settled; the answer is here and the future has not taken it.
    Done(Answer),
}

/// A request in flight, which resolves to what the host answered.
pub struct Pending {
    token: u32,
    /// Set once the answer has been taken, so a second poll does not look for
    /// a token that is no longer registered.
    taken: bool,
}

impl std::future::Future for Pending {
    type Output = Answer;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        if self.taken {
            // Polled after completing. Nothing sensible left to return, so
            // stay pending forever rather than inventing an answer.
            return std::task::Poll::Pending;
        }
        let token = self.token;
        let answer = PENDING.with(|p| {
            let mut map = p.borrow_mut();
            match map.get_mut(&token) {
                Some(PendingState::Done(_)) => match map.remove(&token) {
                    Some(PendingState::Done(answer)) => Some(answer),
                    _ => unreachable!("just matched Done"),
                },
                Some(state @ PendingState::Waiting(_)) => {
                    *state = PendingState::Waiting(Some(context.waker().clone()));
                    None
                }
                // The token was never registered, or was settled and taken.
                // Reported as an error rather than hanging.
                None => Some(Err(HostError::Unknown)),
            }
        });

        match answer {
            Some(answer) => {
                self.taken = true;
                std::task::Poll::Ready(answer)
            }
            None => std::task::Poll::Pending,
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.taken {
            // Dropped before settling — the host may still call back, so the
            // entry goes now rather than being left to accumulate.
            PENDING.with(|p| p.borrow_mut().remove(&self.token));
        }
    }
}

/// Ask the host to do something, and wait for the answer.
pub fn call(op: Op, message: &[u8]) -> Pending {
    let token = unsafe { wbt_call(op as u32, message.as_ptr(), message.len()) };
    PENDING.with(|p| p.borrow_mut().insert(token, PendingState::Waiting(None)));
    Pending {
        token,
        taken: false,
    }
}

/// Ask the host to do something, with nothing to say.
pub fn call_empty(op: Op) -> Pending {
    call(op, &[])
}

/// Ask, passing a single string — the commonest shape by far.
pub fn call_str(op: Op, value: &str) -> Pending {
    let mut w = Writer::new();
    w.str(value);
    call(op, &w.finish())
}

// ── Exports ─────────────────────────────────────────────────────────────────

/// Hand the host a buffer to write into.
///
/// JavaScript cannot allocate inside this module's linear memory, so it asks
/// for space here and writes into it. The length comes back to [`wbt_free`]
/// because `Vec` needs it to deallocate.
///
/// # Safety
/// Called by the host. The returned pointer is valid for `len` bytes until
/// passed to [`wbt_free`].
#[no_mangle]
pub extern "C" fn wbt_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Give back a buffer from [`wbt_alloc`].
///
/// # Safety
/// `ptr` must have come from [`wbt_alloc`] with the same `len`, and must not
/// be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn wbt_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
    }
}

/// Settle a request the host was given a token for.
///
/// `error` is zero for success, otherwise a [`HostError`]. On success the
/// payload is whatever the operation returns; on failure it is ignored.
///
/// # Safety
/// `ptr` must be valid for `len` bytes for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn wbt_settle(token: u32, error: u32, ptr: *const u8, len: usize) {
    let answer = if error == 0 {
        let payload = if ptr.is_null() || len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
        };
        Ok(payload)
    } else {
        Err(HostError::from_u32(error))
    };

    let waker = PENDING.with(|p| {
        let mut map = p.borrow_mut();
        match map.insert(token, PendingState::Done(answer)) {
            Some(PendingState::Waiting(waker)) => waker,
            // Settled twice, or for a token nobody is waiting on. The newer
            // answer replaced the older; there is nothing to wake.
            _ => None,
        }
    });
    // Woken outside the borrow: waking polls the task, which may start
    // another request and borrow `PENDING` again.
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// How many requests are in flight. For tests and for diagnosing a leak.
pub fn pending_count() -> usize {
    PENDING.with(|p| p.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct Counter(AtomicUsize);
    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn pending_for(token: u32) -> Pending {
        PENDING.with(|p| p.borrow_mut().insert(token, PendingState::Waiting(None)));
        Pending {
            token,
            taken: false,
        }
    }

    #[test]
    fn a_request_completes_when_the_host_settles_it() {
        let woken = Arc::new(Counter(AtomicUsize::new(0)));
        let waker: Waker = woken.clone().into();
        let mut context = Context::from_waker(&waker);

        let mut future = pending_for(700);
        assert!(matches!(
            Pin::new(&mut future).poll(&mut context),
            Poll::Pending
        ));
        assert_eq!(woken.0.load(Ordering::SeqCst), 0);

        let payload = [1u8, 2, 3];
        unsafe { wbt_settle(700, 0, payload.as_ptr(), payload.len()) };
        assert_eq!(woken.0.load(Ordering::SeqCst), 1, "settling must wake");

        assert_eq!(
            Pin::new(&mut future).poll(&mut context),
            Poll::Ready(Ok(vec![1, 2, 3]))
        );
    }

    #[test]
    fn an_error_comes_back_as_the_name_the_browser_threw() {
        let waker: Waker = Arc::new(Counter(AtomicUsize::new(0))).into();
        let mut context = Context::from_waker(&waker);

        let mut future = pending_for(701);
        let _ = Pin::new(&mut future).poll(&mut context);
        unsafe { wbt_settle(701, HostError::Security as u32, std::ptr::null(), 0) };

        assert_eq!(
            Pin::new(&mut future).poll(&mut context),
            Poll::Ready(Err(HostError::Security))
        );
    }

    /// Settling before anyone polls must not lose the answer — the host is
    /// free to resolve synchronously.
    #[test]
    fn settling_before_the_first_poll_still_delivers() {
        let waker: Waker = Arc::new(Counter(AtomicUsize::new(0))).into();
        let mut context = Context::from_waker(&waker);

        let mut future = pending_for(702);
        unsafe { wbt_settle(702, 0, [9u8].as_ptr(), 1) };
        assert_eq!(
            Pin::new(&mut future).poll(&mut context),
            Poll::Ready(Ok(vec![9]))
        );
    }

    /// A dropped request must not leave its entry behind; a page that scans
    /// and cancels repeatedly would otherwise grow without bound.
    #[test]
    fn dropping_a_request_removes_it() {
        let before = pending_count();
        {
            let _future = pending_for(703);
            assert_eq!(pending_count(), before + 1);
        }
        assert_eq!(pending_count(), before, "the entry should be gone");
    }

    /// A completed request must not be left registered either.
    #[test]
    fn a_completed_request_leaves_nothing_behind() {
        let waker: Waker = Arc::new(Counter(AtomicUsize::new(0))).into();
        let mut context = Context::from_waker(&waker);

        let before = pending_count();
        let mut future = pending_for(704);
        let _ = Pin::new(&mut future).poll(&mut context);
        unsafe { wbt_settle(704, 0, std::ptr::null(), 0) };
        let _ = Pin::new(&mut future).poll(&mut context);
        drop(future);
        assert_eq!(pending_count(), before);
    }

    /// The host is not trusted to settle each token once.
    #[test]
    fn settling_an_unknown_token_is_ignored() {
        let before = pending_count();
        unsafe { wbt_settle(999_999, 0, std::ptr::null(), 0) };
        // It registers an answer nobody will take, so clean it up the way a
        // dropped future would.
        PENDING.with(|p| p.borrow_mut().remove(&999_999));
        assert_eq!(pending_count(), before);
    }

    #[test]
    fn allocation_round_trips() {
        let ptr = wbt_alloc(64);
        assert!(!ptr.is_null());
        unsafe { wbt_free(ptr, 64) };
        // Freeing nothing is allowed, because the host may not have allocated.
        unsafe { wbt_free(std::ptr::null_mut(), 0) };
    }

    #[test]
    fn error_codes_map_both_ways() {
        for (code, expected) in [
            (0, HostError::Unknown),
            (1, HostError::NotFound),
            (2, HostError::Security),
            (3, HostError::Network),
            (4, HostError::InvalidState),
            (5, HostError::NotSupported),
            (6, HostError::InvalidModification),
            (7, HostError::Abort),
            (8, HostError::NotAllowed),
            (77, HostError::Unknown),
        ] {
            assert_eq!(HostError::from_u32(code), expected);
        }
    }

    #[test]
    fn event_kinds_map_and_reject_the_unknown() {
        assert_eq!(EventKind::from_u32(1), Some(EventKind::CharacteristicValue));
        assert_eq!(EventKind::from_u32(5), Some(EventKind::AvailabilityChanged));
        assert_eq!(EventKind::from_u32(0), None);
        assert_eq!(EventKind::from_u32(99), None);
    }
}
