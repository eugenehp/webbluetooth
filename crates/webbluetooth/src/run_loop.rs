//! Keeping the main thread's run loop turning, which macOS's IOBluetooth needs.

/// Run `work` on a thread of its own while the calling thread — which must be the main thread — turns its
/// CoreFoundation run loop, and return what `work` returns.
///
/// IOBluetooth delivers every Bluetooth Classic event (an RFCOMM channel opening, each chunk of data, the peer
/// closing) on the **main** thread's run loop. An app gets that from `NSApplication`; a command-line program has to
/// do it itself, or [`crate::Bluetooth::open_rfcomm_channel`] never completes. Call this from `main` with the
/// program's Bluetooth work (a closure that builds its runtime and blocks on it).
///
/// On every other platform there is no such requirement and `work` simply runs on the calling thread.
///
/// # Panics
///
/// If `work` panics, the panic is resumed on the calling thread.
pub fn run_with_main_run_loop<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    #[cfg(target_os = "macos")]
    {
        use std::sync::mpsc;
        use std::time::Duration;
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, return_after_source_handled: bool) -> i32;
            static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        }
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        loop {
            match rx.recv_timeout(Duration::ZERO) {
                Ok(value) => {
                    let _ = worker.join();
                    return value;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => match worker.join() {
                    Err(panic) => std::panic::resume_unwind(panic),
                    Ok(()) => unreachable!("the worker sends its result before it exits"),
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // SAFETY: CoreFoundation's run-loop entry point with its default mode constant, on the calling thread.
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, false) };
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        work()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_what_the_work_returns() {
        assert_eq!(run_with_main_run_loop(|| 6 * 7), 42);
    }

    #[test]
    fn a_panic_in_the_work_reaches_the_caller() {
        let r = std::panic::catch_unwind(|| run_with_main_run_loop(|| -> u8 { panic!("boom") }));
        assert!(r.is_err());
    }
}
