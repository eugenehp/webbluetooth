//! Getting answers back onto the runtime's thread.
//!
//! Node runs JavaScript on one thread and a `napi_value` is only valid there.
//! The Bluetooth backends do not know that: CoreBluetooth calls back on a
//! dispatch queue, BlueZ on the D-Bus reader thread, `linux-hci` on the ATT
//! reader. Touching the runtime from any of them is undefined behaviour, not
//! an error that comes back.
//!
//! So work happens off-thread and results cross back through a *threadsafe
//! function*, which is the one Node-API call that may be made from anywhere.
//! The shape is:
//!
//! 1. A call creates a promise and hands the deferred to a task.
//! 2. The task runs on this module's own thread and produces a value.
//! 3. It calls the threadsafe function, which schedules a callback on the
//!    runtime's thread.
//! 4. That callback converts the value and settles the promise.
//!
//! Step 4 is the only place a `napi_value` is built.

use crate::napi::*;
use crate::value::{self, Result, Thrown};

/// What a finished task wants done, on the runtime's thread.
///
/// Boxed rather than an enum: the closure already captures everything the
/// conversion needs, and the alternative is an enum with a variant per call.
type Settle = Box<dyn FnOnce(napi_env) -> Result<napi_value> + Send>;

/// Something to do on the runtime's thread.
///
/// Two kinds, because there are two reasons to cross back: an answer to a call
/// that was made, and an event nobody asked for. They share one threadsafe
/// function so that they stay in order — a notification that arrives after a
/// disconnection must not be delivered before it.
enum Work {
    /// Settle a promise.
    Settle(Completion),
    /// Hand a value to every listener registered for it.
    Deliver {
        listeners: std::sync::Arc<std::sync::Mutex<Vec<std::sync::Arc<crate::object::Held>>>>,
        value: Vec<u8>,
    },
}

/// One completed task: how to build its value, and which promise to settle.
struct Completion {
    settle: Settle,
    deferred: napi_deferred,
    /// Whether to resolve or reject. Carried separately because a rejection
    /// still has a value to build — the `Error` object.
    rejected: bool,
}

// SAFETY: `napi_deferred` is an opaque pointer the runtime owns. It is created
// on the runtime's thread, moved here, and used again only on the runtime's
// thread inside the threadsafe callback. It is never dereferenced in between.
unsafe impl Send for Completion {}
// SAFETY: as above — `Work` carries a `Completion` or a listener handle, and
// neither is touched off the runtime's thread.
unsafe impl Send for Work {}

/// Somewhere for futures to run, and a way back.
pub struct Runtime {
    /// Where futures run — all of them at once, on one thread.
    executor: crate::exec::Executor,
    /// Created lazily, because it needs an `env` and there is none until the
    /// first call. Leaked once created: the callback below needs it as raw
    /// context, and it lives as long as the module does.
    bridge: std::sync::Mutex<Option<&'static Bridge>>,
}

/// How many operations are relying on the event loop staying open.
///
/// Separated from the runtime calls it drives so that the counting — which is
/// where the mistakes live — can be reasoned about and tested on its own. Each
/// method answers one question: *does the caller now have to tell Node?*
#[derive(Default)]
pub struct LoopHold(std::sync::atomic::AtomicUsize);

impl LoopHold {
    /// Take a hold. `true` if the loop must now be referenced.
    pub fn acquire(&self) -> bool {
        self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel) == 0
    }

    /// Give one back. `true` if the loop must now be released.
    pub fn release(&self) -> bool {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1
    }

    /// Undo an [`acquire`](Self::acquire) whose reference could not be taken.
    ///
    /// Without this the count stays raised with nothing left to lower it, and
    /// the process never exits.
    pub fn undo(&self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// How many holds are out. Used by the tests, and worth having when a
    /// process refuses to exit and the question is "how many, and whose".
    #[allow(dead_code)]
    pub fn outstanding(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// The threadsafe function, and how many operations are relying on it.
///
/// The count decides whether the runtime's event loop is held open. An addon
/// that holds it open unconditionally stops the process exiting for as long as
/// it is loaded; one that never holds it open lets a script exit before its
/// own `await` resolves, which is the more confusing failure — the call simply
/// produces nothing and the process reports success.
struct Bridge {
    function: napi_threadsafe_function,
    in_flight: LoopHold,
}

// SAFETY: a threadsafe function is explicitly documented as callable from any
// thread; that is the entire purpose of the type.
unsafe impl Send for Bridge {}
unsafe impl Sync for Bridge {}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    /// Start the executor thread.
    pub fn new() -> Self {
        Self {
            executor: crate::exec::Executor::new(),
            bridge: std::sync::Mutex::new(None),
        }
    }

    /// Make sure the threadsafe function exists, creating it on first use.
    fn bridge(&self, env: napi_env) -> Result<&'static Bridge> {
        let mut slot = self.bridge.lock().unwrap();
        if slot.is_none() {
            let name = value::string(env, "webbluetooth")?;
            let mut function: napi_threadsafe_function = std::ptr::null_mut();
            let bridge: &'static Bridge = Box::leak(Box::new(Bridge {
                function: std::ptr::null_mut(),
                in_flight: LoopHold::default(),
            }));
            let context = bridge as *const Bridge as *mut std::ffi::c_void;
            value::ok(
                unsafe {
                    napi_create_threadsafe_function(
                        env,
                        // No JavaScript function: the callback below does the
                        // work itself, which is what a null `func` means.
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        name,
                        // Unbounded, so a burst of notifications cannot block
                        // the thread that produced them.
                        0,
                        1,
                        std::ptr::null_mut(),
                        None,
                        // The context reaches `deliver`, which needs it to
                        // release the loop when the last operation finishes.
                        context,
                        Some(deliver),
                        &mut function,
                    )
                },
                "napi_create_threadsafe_function",
            )?;
            // Idle by default: a loaded addon with nothing in flight must not
            // be the reason a process refuses to exit. `promise` re-refs it
            // for as long as there is an answer still coming.
            value::ok(
                unsafe { napi_unref_threadsafe_function(env, function) },
                "napi_unref_threadsafe_function",
            )?;
            // SAFETY: nothing has observed the bridge yet — it was created two
            // statements ago and the lock is held.
            unsafe {
                let slot = bridge as *const Bridge as *mut Bridge;
                (*slot).function = function;
            }
            *slot = Some(bridge);
        }
        slot.ok_or_else(|| Thrown("the runtime bridge is missing".into()))
    }

    /// Run `future` for its effects, with no promise attached.
    ///
    /// For work that outlives the call that started it — the pump that feeds
    /// notification listeners. Deliberately does *not* hold the event loop
    /// open: a subscription should not be the reason a process refuses to
    /// exit, any more than an idle listener would be in a browser.
    pub fn spawn(&self, future: impl std::future::Future<Output = ()> + Send + 'static) -> bool {
        self.executor.spawn(future)
    }

    /// Run `future` and return a promise for its result.
    ///
    /// The future runs on this module's thread; the promise settles on the
    /// runtime's.
    pub fn promise<T, F>(&self, env: napi_env, future: F) -> Result<napi_value>
    where
        T: IntoJs + Send + 'static,
        F: std::future::Future<Output = std::result::Result<T, webbluetooth::Error>>
            + Send
            + 'static,
    {
        self.promise_with(env, future, |env, value: T| value.into_js(env))
    }

    /// As [`promise`](Self::promise), converting the result with `into_js`.
    ///
    /// For results that become objects rather than plain values. Building one
    /// needs the `env`, and an `env` is only valid on the runtime's thread —
    /// so the conversion is carried *to* that thread rather than done where
    /// the value was produced.
    pub fn promise_with<T, F, C>(&self, env: napi_env, future: F, into_js: C) -> Result<napi_value>
    where
        T: Send + 'static,
        F: std::future::Future<Output = std::result::Result<T, webbluetooth::Error>>
            + Send
            + 'static,
        C: FnOnce(napi_env, T) -> Result<napi_value> + Send + 'static,
    {
        let mut deferred: napi_deferred = std::ptr::null_mut();
        let mut promise: napi_value = std::ptr::null_mut();
        value::ok(
            unsafe { napi_create_promise(env, &mut deferred, &mut promise) },
            "napi_create_promise",
        )?;

        let bridge = self.bridge(env)?;
        // Hold the event loop open while this answer is outstanding. Without
        // it a script that does nothing but `await` one call exits before the
        // answer arrives, reporting success and printing nothing.
        hold(env, bridge)?;
        let deferred = SendDeferred(deferred);
        let queued = self.executor.spawn(async move {
            let deferred = deferred;
            let outcome = future.await;
            let completion = match outcome {
                Ok(value) => Completion {
                    settle: Box::new(move |env| into_js(env, value)),
                    deferred: deferred.0,
                    rejected: false,
                },
                Err(error) => {
                    let message = error.to_string();
                    Completion {
                        settle: Box::new(move |env| js_error(env, &message)),
                        deferred: deferred.0,
                        rejected: true,
                    }
                }
            };
            let boxed = Box::into_raw(Box::new(Work::Settle(completion))) as *mut std::ffi::c_void;
            let status = unsafe {
                napi_call_threadsafe_function(bridge.function, boxed, NAPI_TSFN_NONBLOCKING)
            };
            if status != NAPI_OK {
                // The runtime is shutting down and will not call back.
                // Reclaiming here is the only way the box is ever freed.
                drop(unsafe { Box::from_raw(boxed as *mut Work) });
            }
        });
        if !queued {
            // Nothing will ever settle this, so the hold taken above has to be
            // given back here. Leaving it would keep the loop open with no
            // answer coming, and the process would never exit.
            release(env, bridge);
            return Err(Thrown("the executor thread has stopped".into()));
        }

        Ok(promise)
    }
}

/// A deferred, moved to the executor thread and back without being touched.
struct SendDeferred(napi_deferred);
// SAFETY: as for `Completion` — carried across threads, dereferenced only on
// the runtime's own thread.
unsafe impl Send for SendDeferred {}

/// Settle one promise. Runs on the runtime's thread.
///
/// # Safety
/// Called by the runtime with the pointer given to
/// `napi_call_threadsafe_function`.
unsafe extern "C" fn deliver(
    env: napi_env,
    _js_callback: napi_value,
    context: *mut std::ffi::c_void,
    data: *mut std::ffi::c_void,
) {
    if data.is_null() {
        return;
    }
    let work = unsafe { Box::from_raw(data as *mut Work) };

    // A null env means the runtime is tearing down: nothing can be settled or
    // delivered, and touching it would be a use-after-free. The box has
    // already been reclaimed above, which is the part that matters.
    if env.is_null() {
        return;
    }

    let completion = match *work {
        Work::Settle(completion) => completion,
        Work::Deliver { listeners, value } => {
            // Built once and handed to every listener, as a DOM event would
            // be — not once per listener.
            let Ok(buffer) = value::array_buffer(env, &value) else {
                return;
            };
            // Cloned out so a listener that registers another during its own
            // call does not deadlock on the lock it is being called under.
            let current: Vec<_> = listeners.lock().unwrap().clone();
            for listener in current {
                // One listener throwing must not stop the others.
                let _ = unsafe { listener.call(env, buffer) };
            }
            return;
        }
    };

    // The last answer releases the loop. Done before settling, because the
    // promise callback may start another operation, which takes a hold again.
    if !context.is_null() {
        // SAFETY: the context is the leaked `Bridge` passed at creation.
        release(env, unsafe { &*(context as *const Bridge) });
    }

    let Completion {
        settle,
        deferred,
        rejected,
    } = completion;
    let value = match settle(env) {
        Ok(value) => value,
        // Converting the result failed, which is this module's bug rather than
        // the caller's. Reported as a rejection so it is visible.
        Err(Thrown(message)) => match js_error(env, &message) {
            Ok(error) => {
                unsafe { napi_reject_deferred(env, deferred, error) };
                return;
            }
            Err(_) => return,
        },
    };
    unsafe {
        match rejected {
            true => napi_reject_deferred(env, deferred, value),
            false => napi_resolve_deferred(env, deferred, value),
        }
    };
}

/// Keep the runtime's event loop alive until the matching [`release`].
///
/// The count, not the flag, is what matters: several operations can be in
/// flight, and the loop must stay open until the last of them lands. Only the
/// transition into and out of zero touches the runtime.
///
/// Must be called on the runtime's thread — `napi_ref_threadsafe_function`
/// takes an `env`.
fn hold(env: napi_env, bridge: &Bridge) -> Result<()> {
    if !bridge.in_flight.acquire() {
        return Ok(());
    }
    let status = unsafe { napi_ref_threadsafe_function(env, bridge.function) };
    if let Err(e) = value::ok(status, "napi_ref_threadsafe_function") {
        bridge.in_flight.undo();
        return Err(e);
    }
    Ok(())
}

/// Give back a hold taken by [`hold`].
///
/// Must be called on the runtime's thread, and exactly once per `hold` — one
/// too few and the process never exits, one too many and the loop closes while
/// an answer is still coming.
fn release(env: napi_env, bridge: &Bridge) {
    if bridge.in_flight.release() {
        unsafe { napi_unref_threadsafe_function(env, bridge.function) };
    }
}

/// Hand `value` to every listener, on the runtime's thread.
///
/// Called from the executor thread, where a `napi_value` cannot be built — so
/// the bytes travel and the `ArrayBuffer` is made on the other side.
pub fn deliver_to(
    bridge_of: &Runtime,
    listeners: std::sync::Arc<std::sync::Mutex<Vec<std::sync::Arc<crate::object::Held>>>>,
    value: Vec<u8>,
) {
    let Some(bridge) = *bridge_of.bridge.lock().unwrap() else {
        return; // Nothing has made a call yet, so there is nothing to deliver on.
    };
    let boxed =
        Box::into_raw(Box::new(Work::Deliver { listeners, value })) as *mut std::ffi::c_void;
    let status =
        unsafe { napi_call_threadsafe_function(bridge.function, boxed, NAPI_TSFN_NONBLOCKING) };
    if status != NAPI_OK {
        drop(unsafe { Box::from_raw(boxed as *mut Work) });
    }
}

/// An `Error` carrying `message`.
fn js_error(env: napi_env, message: &str) -> Result<napi_value> {
    let text = value::string(env, message)?;
    let mut error = std::ptr::null_mut();
    value::ok(
        unsafe { napi_create_error(env, std::ptr::null_mut(), text, &mut error) },
        "napi_create_error",
    )?;
    Ok(error)
}

/// Something that can become a JavaScript value, on the runtime's thread.
pub trait IntoJs {
    fn into_js(self, env: napi_env) -> Result<napi_value>;
}

impl IntoJs for () {
    fn into_js(self, env: napi_env) -> Result<napi_value> {
        value::undefined(env)
    }
}

impl IntoJs for bool {
    fn into_js(self, env: napi_env) -> Result<napi_value> {
        value::boolean(env, self)
    }
}

impl IntoJs for String {
    fn into_js(self, env: napi_env) -> Result<napi_value> {
        value::string(env, &self)
    }
}

impl IntoJs for i32 {
    fn into_js(self, env: napi_env) -> Result<napi_value> {
        value::number(env, self as f64)
    }
}

impl IntoJs for u16 {
    fn into_js(self, env: napi_env) -> Result<napi_value> {
        value::number(env, self as f64)
    }
}

impl IntoJs for Vec<u8> {
    fn into_js(self, env: napi_env) -> Result<napi_value> {
        value::array_buffer(env, &self)
    }
}

impl<T: IntoJs> IntoJs for Option<T> {
    fn into_js(self, env: napi_env) -> Result<napi_value> {
        match self {
            Some(value) => value.into_js(env),
            None => value::null(env),
        }
    }
}

impl<T: IntoJs> IntoJs for Vec<T> {
    fn into_js(self, env: napi_env) -> Result<napi_value> {
        let items: Result<Vec<_>> = self.into_iter().map(|i| i.into_js(env)).collect();
        value::array(env, &items?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// The loop is held while work is outstanding and released when the last
    /// of it lands — not per call, which would close the loop under a second
    /// operation still in flight.
    #[test]
    fn the_loop_is_released_only_by_the_last_holder() {
        let held = LoopHold::default();

        assert!(held.acquire(), "the first hold must reference the loop");
        assert!(!held.acquire(), "the second must not reference it again");
        assert!(!held.acquire());
        assert_eq!(held.outstanding(), 3);

        assert!(!held.release(), "two are still out");
        assert!(!held.release(), "one is still out");
        assert!(held.release(), "the last must release the loop");
        assert_eq!(held.outstanding(), 0);
    }

    /// A hold whose reference could not be taken has to be put back, or the
    /// count stays raised with nothing left to lower it and the process never
    /// exits.
    #[test]
    fn an_undone_hold_leaves_the_count_where_it_started() {
        let held = LoopHold::default();
        assert!(held.acquire());
        held.undo();
        assert_eq!(held.outstanding(), 0);
        // And the next hold is a first hold again, so it references the loop.
        assert!(held.acquire());
    }

    /// Holds and releases arriving from several threads must still net out —
    /// the whole point of the count being atomic.
    #[test]
    fn concurrent_holds_net_out() {
        let held = Arc::new(LoopHold::default());
        // One hold kept for the duration, so no release can be the last and
        // the test never depends on which thread finishes first.
        assert!(held.acquire());

        let threads: Vec<_> = (0..8)
            .map(|_| {
                let held = held.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        assert!(!held.acquire(), "the kept hold means never first");
                        assert!(!held.release(), "nor ever last");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("no thread should have panicked");
        }

        assert_eq!(held.outstanding(), 1, "only the kept hold remains");
        assert!(held.release());
    }

    /// The runtime starts an executor and it accepts work. What that work
    /// then does concurrently is `exec`'s business, and tested there.
    #[test]
    fn a_runtime_starts_and_accepts_work() {
        let runtime = Runtime::new();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();
        assert!(runtime.executor.spawn(async move {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline
            && !done.load(std::sync::atomic::Ordering::SeqCst)
        {
            std::thread::yield_now();
        }
        assert!(done.load(std::sync::atomic::Ordering::SeqCst));
    }
}
