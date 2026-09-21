//! An executor that runs many operations at once on one thread.
//!
//! The distinction that matters here is between *one thread* and *one
//! operation at a time*. A Bluetooth operation is almost entirely waiting — a
//! read is a request on the air and a reply a connection interval later, 7.5 ms
//! at best and commonly 30 ms or more. One thread is ample to supervise
//! hundreds of those. One *operation* is not: ten concurrent reads would take
//! ten times as long as one, and the thread would sit idle throughout.
//!
//! The wrong way to get there is a thread pool, which spends a thread per
//! operation on work that is not CPU-bound. The right way is to poll the
//! futures cooperatively: each one parks itself when it has nothing to do, and
//! a wake from whichever backend thread has news puts it back in the queue.
//!
//! That is what this does. It is small because futures do the hard part; the
//! executor only has to answer "what is ready".

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A handle for submitting work.
pub struct Executor {
    inner: Arc<Shared>,
}

struct Shared {
    /// Futures that have started and not finished.
    ///
    /// A task is *removed* while being polled, so a wake arriving from another
    /// thread mid-poll finds nothing to take and simply queues the id — which
    /// the poll loop notices when it puts the task back.
    tasks: Mutex<HashMap<u64, Task>>,
    /// Ids with something to do.
    ready: Mutex<Option<mpsc::Sender<u64>>>,
    next: AtomicU64,
}

/// Wakes one task by putting its id back in the queue.
struct TaskWaker {
    id: u64,
    shared: Arc<Shared>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // Called from whichever thread had news: CoreBluetooth's dispatch
        // queue, the D-Bus reader, the ATT reader. Sending an id is all that
        // happens here, so none of them is ever blocked by this executor.
        if let Some(ready) = self.shared.ready.lock().unwrap().as_ref() {
            let _ = ready.send(self.id);
        }
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    /// Start the executor thread.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel::<u64>();
        let inner = Arc::new(Shared {
            tasks: Mutex::new(HashMap::new()),
            ready: Mutex::new(Some(sender)),
            next: AtomicU64::new(1),
        });

        let shared = inner.clone();
        std::thread::Builder::new()
            .name("webbluetooth-node".into())
            .spawn(move || {
                // Ends when the last sender is dropped, which happens when the
                // executor does.
                while let Ok(id) = receiver.recv() {
                    // Out of the map for the duration of the poll: a wake from
                    // inside this very poll then has nothing to race with, and
                    // a second wake from another thread only queues the id.
                    let Some(mut task) = shared.tasks.lock().unwrap().remove(&id) else {
                        continue; // Already finished.
                    };
                    let waker: Waker = Arc::new(TaskWaker {
                        id,
                        shared: shared.clone(),
                    })
                    .into();
                    if task.as_mut().poll(&mut Context::from_waker(&waker)) == Poll::Pending {
                        shared.tasks.lock().unwrap().insert(id, task);
                    }
                }
            })
            .expect("could not start the webbluetooth executor thread");

        Self { inner }
    }

    /// Run `future` alongside whatever else is in flight.
    ///
    /// Returns as soon as it is queued. The future is `Send` because it starts
    /// here and runs there; it does not need to be `Sync`, and it is never
    /// moved again once polling begins.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> bool {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner
            .tasks
            .lock()
            .unwrap()
            .insert(id, Box::pin(future));
        let queued = self
            .inner
            .ready
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| r.send(id).is_ok())
            .unwrap_or(false);
        if !queued {
            // The thread is gone and nothing will ever poll this, so it is
            // dropped now rather than held for the life of the process.
            self.inner.tasks.lock().unwrap().remove(&id);
        }
        queued
    }

    /// How many futures are in flight.
    ///
    /// For the tests, and worth having when the question is whether an
    /// operation is stuck or simply slow.
    #[allow(dead_code)]
    pub fn in_flight(&self) -> usize {
        self.inner.tasks.lock().unwrap().len()
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        // Dropping the sender is what ends the thread's loop.
        self.inner.ready.lock().unwrap().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    /// A future that completes when something else says so.
    struct Gate(Arc<GateState>);
    struct GateState {
        open: Mutex<bool>,
        waker: Mutex<Option<Waker>>,
    }
    impl GateState {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                open: Mutex::new(false),
                waker: Mutex::new(None),
            })
        }
        fn open(self: &Arc<Self>) {
            *self.open.lock().unwrap() = true;
            if let Some(w) = self.waker.lock().unwrap().take() {
                w.wake();
            }
        }
    }
    impl Future for Gate {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if *self.0.open.lock().unwrap() {
                return Poll::Ready(());
            }
            *self.0.waker.lock().unwrap() = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    fn eventually(mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            // Sleeping rather than `yield_now`: this waits for work on other
            // threads, and a spin that only yields still holds a core. Under
            // `cargo test --workspace`, where every core already has a test
            // binary on it, that starves the threads being waited for and the
            // five seconds can run out with no progress made. Seen once, as a
            // one-in-many-runs failure of
            // `many_operations_run_concurrently`, which passes alone.
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }

    #[test]
    fn a_future_that_never_waits_finishes() {
        let executor = Executor::new();
        let ran = Arc::new(AtomicUsize::new(0));
        let flag = ran.clone();
        executor.spawn(async move {
            flag.fetch_add(1, Ordering::SeqCst);
        });
        assert!(eventually(|| ran.load(Ordering::SeqCst) == 1));
        assert!(eventually(|| executor.in_flight() == 0));
    }

    /// The property the old `block_on` loop did not have: a second operation
    /// makes progress while the first is still waiting.
    ///
    /// With one-at-a-time execution the second future is never even polled
    /// until the first completes, so this deadlocks — the test would hang
    /// rather than fail, which is why the gates are opened in reverse order.
    #[test]
    fn operations_do_not_queue_behind_each_other() {
        let executor = Executor::new();
        let first = GateState::new();
        let second = GateState::new();
        let done = Arc::new(AtomicUsize::new(0));

        for gate in [first.clone(), second.clone()] {
            let done = done.clone();
            executor.spawn(async move {
                Gate(gate).await;
                done.fetch_add(1, Ordering::SeqCst);
            });
        }

        // Both are waiting, neither has finished.
        assert!(eventually(|| executor.in_flight() == 2));
        assert_eq!(done.load(Ordering::SeqCst), 0);

        // Finish the *second* one first. Under serial execution it could not
        // have been started, so this would never complete.
        second.open();
        assert!(eventually(|| done.load(Ordering::SeqCst) == 1));

        first.open();
        assert!(eventually(|| done.load(Ordering::SeqCst) == 2));
        assert!(eventually(|| executor.in_flight() == 0));
    }

    /// Many at once, woken from many threads — the shape of a busy adapter.
    #[test]
    fn many_operations_run_concurrently() {
        const N: usize = 64;
        let executor = Executor::new();
        let gates: Vec<_> = (0..N).map(|_| GateState::new()).collect();
        let done = Arc::new(AtomicUsize::new(0));

        for gate in &gates {
            let (gate, done) = (gate.clone(), done.clone());
            executor.spawn(async move {
                Gate(gate).await;
                done.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert!(eventually(|| executor.in_flight() == N));

        // Opened from several threads at once, as the backends would.
        let threads: Vec<_> = gates
            .chunks(8)
            .map(|chunk| {
                let chunk: Vec<_> = chunk.to_vec();
                std::thread::spawn(move || {
                    for gate in chunk {
                        gate.open();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("no thread should have panicked");
        }

        assert!(eventually(|| done.load(Ordering::SeqCst) == N));
        assert!(eventually(|| executor.in_flight() == 0));
    }

    /// A wake that lands while the task is being polled must not be lost: the
    /// task is out of the map at that moment, so the id has to queue.
    #[test]
    fn a_wake_during_a_poll_is_not_lost() {
        struct WakeFromInside(usize);
        impl Future for WakeFromInside {
            type Output = ();
            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                self.0 += 1;
                if self.0 >= 4 {
                    return Poll::Ready(());
                }
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }

        let executor = Executor::new();
        let done = Arc::new(AtomicUsize::new(0));
        let flag = done.clone();
        executor.spawn(async move {
            WakeFromInside(0).await;
            flag.fetch_add(1, Ordering::SeqCst);
        });
        assert!(eventually(|| done.load(Ordering::SeqCst) == 1));
        assert!(eventually(|| executor.in_flight() == 0));
    }

    /// Submitting after the thread has gone must not leave the future held
    /// forever — it can never be polled, so it is dropped.
    #[test]
    fn work_submitted_after_shutdown_is_refused_and_dropped() {
        let executor = Executor::new();
        let inner = executor.inner.clone();
        drop(executor);

        let outer = Executor { inner };
        assert!(
            !outer.spawn(async {}),
            "a closed executor should refuse work"
        );
        assert_eq!(outer.in_flight(), 0, "and not hold onto it");
        std::mem::forget(outer);
    }
}
