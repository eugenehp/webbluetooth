//! A single-threaded executor, because a browser has no other kind.
//!
//! The rest of this crate is written in `async fn`, which needs something to
//! poll it. On every other platform that is the caller's runtime, and on
//! several of them a blocking `block_on` is a reasonable answer. In a browser
//! it is not: blocking the only thread stops the event loop, so the callback
//! that would have completed the future can never run. Blocking there does not
//! wait, it deadlocks.
//!
//! So a task is handed here, polled once, and then polled again whenever
//! something wakes it — which in practice means when the shim settles a
//! request or delivers an event. There is no thread and no queue drain on a
//! timer; waking *is* the schedule.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

type BoxFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

thread_local! {
    /// Tasks that have not finished.
    static TASKS: RefCell<HashMap<u64, BoxFuture>> = RefCell::new(HashMap::new());
    /// Tasks woken while a poll was already in progress.
    ///
    /// Polling a task can wake it again — synchronously, from inside its own
    /// poll. Re-entering the map at that moment would be a double borrow, so
    /// the wake is recorded and drained once the current poll returns.
    static READY: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// Whether a drain is already running, so a nested wake does not start a
    /// second one underneath the first.
    static DRAINING: RefCell<bool> = const { RefCell::new(false) };
}

static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

/// Identifies a spawned task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(u64);

/// Wakes one task by putting it back in the ready list.
struct TaskWaker(u64);

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let id = self.0;
        READY.with(|r| r.borrow_mut().push(id));
        drain();
    }
}

/// Run `future` to completion, polling it as it is woken.
///
/// Returns immediately; the future continues on the browser's event loop. This
/// is the entry point every asynchronous operation ultimately needs, and the
/// reason a page does not have to supply a runtime.
pub fn spawn(future: impl Future<Output = ()> + 'static) -> TaskId {
    let id = NEXT_TASK.fetch_add(1, Ordering::Relaxed);
    TASKS.with(|t| t.borrow_mut().insert(id, Box::pin(future)));
    READY.with(|r| r.borrow_mut().push(id));
    drain();
    TaskId(id)
}

/// Poll every task that has been woken, until none are left.
fn drain() {
    // A poll can wake something, which calls `drain` again. The outermost call
    // owns the loop; an inner one returns and lets it pick the work up.
    if DRAINING.with(|d| std::mem::replace(&mut *d.borrow_mut(), true)) {
        return;
    }

    loop {
        let Some(id) = READY.with(|r| r.borrow_mut().pop()) else {
            break;
        };
        // Taken out of the map for the duration of the poll, so that waking
        // this same task from inside its own poll does not need the map.
        let Some(mut future) = TASKS.with(|t| t.borrow_mut().remove(&id)) else {
            continue; // Already finished.
        };

        let waker: Waker = Arc::new(TaskWaker(id)).into();
        let mut context = Context::from_waker(&waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(()) => {}
            Poll::Pending => {
                TASKS.with(|t| t.borrow_mut().insert(id, future));
            }
        }
    }

    DRAINING.with(|d| *d.borrow_mut() = false);
}

/// How many tasks are alive. For tests, and for noticing a leak.
pub fn task_count() -> usize {
    TASKS.with(|t| t.borrow().len())
}

/// A future whose result is delivered once, from somewhere else.
///
/// The standard library has no single-threaded oneshot and this crate has no
/// dependencies, so here is one. `Rc<RefCell<..>>` rather than a channel:
/// there is one thread, so there is nothing to synchronise.
pub struct Oneshot<T> {
    slot: Rc<RefCell<OneshotState<T>>>,
}

struct OneshotState<T> {
    value: Option<T>,
    waker: Option<Waker>,
    /// Set when the sending half goes away without sending.
    dropped: bool,
}

/// Delivers a value to its [`Oneshot`].
pub struct Sender<T> {
    slot: Rc<RefCell<OneshotState<T>>>,
}

/// A pair to hand a value across an await point.
pub fn oneshot<T>() -> (Sender<T>, Oneshot<T>) {
    let slot = Rc::new(RefCell::new(OneshotState {
        value: None,
        waker: None,
        dropped: false,
    }));
    (Sender { slot: slot.clone() }, Oneshot { slot })
}

impl<T> Sender<T> {
    /// Deliver the value and wake whoever is waiting.
    pub fn send(self, value: T) {
        let waker = {
            let mut state = self.slot.borrow_mut();
            state.value = Some(value);
            state.waker.take()
        };
        // Outside the borrow: waking may poll the receiver, which borrows.
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.slot.borrow_mut();
            if state.value.is_some() {
                return; // Already sent.
            }
            state.dropped = true;
            state.waker.take()
        };
        // A receiver waiting on a sender that is gone would wait forever, so
        // it is woken to find out.
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Future for Oneshot<T> {
    /// `None` if the sender was dropped without sending.
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.slot.borrow_mut();
        if let Some(value) = state.value.take() {
            return Poll::Ready(Some(value));
        }
        if state.dropped {
            return Poll::Ready(None);
        }
        state.waker = Some(context.waker().clone());
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_task_that_never_waits_finishes_immediately() {
        let ran = Rc::new(Cell::new(false));
        let flag = ran.clone();
        spawn(async move { flag.set(true) });
        assert!(ran.get());
        assert_eq!(task_count(), 0, "a finished task is not kept");
    }

    #[test]
    fn a_task_resumes_when_its_value_arrives() {
        let got = Rc::new(Cell::new(0u32));
        let out = got.clone();
        let (tx, rx) = oneshot::<u32>();

        spawn(async move {
            if let Some(v) = rx.await {
                out.set(v);
            }
        });
        assert_eq!(got.get(), 0, "still waiting");
        assert_eq!(task_count(), 1);

        tx.send(42);
        assert_eq!(got.get(), 42, "sending should have resumed the task");
        assert_eq!(task_count(), 0);
    }

    /// A sender that goes away has to wake the receiver, or the task waits for
    /// a value that can never come — which in a browser is a leak with no
    /// thread to show for it.
    #[test]
    fn dropping_the_sender_wakes_the_waiter() {
        let finished = Rc::new(Cell::new(false));
        let flag = finished.clone();
        let (tx, rx) = oneshot::<u32>();

        spawn(async move {
            let value = rx.await;
            assert!(value.is_none(), "no value was ever sent");
            flag.set(true);
        });
        assert!(!finished.get());

        drop(tx);
        assert!(finished.get());
        assert_eq!(task_count(), 0);
    }

    /// Waking from inside a poll must not re-enter the task map. This is the
    /// case that deadlocks a naive implementation.
    #[test]
    fn waking_during_a_poll_does_not_re_enter() {
        struct WakeThenReady(bool);
        impl Future for WakeThenReady {
            type Output = ();
            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                if self.0 {
                    return Poll::Ready(());
                }
                self.0 = true;
                // Wake ourselves from inside our own poll.
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }

        let done = Rc::new(Cell::new(false));
        let flag = done.clone();
        spawn(async move {
            WakeThenReady(false).await;
            flag.set(true);
        });
        assert!(done.get(), "the task should have been polled again");
        assert_eq!(task_count(), 0);
    }

    #[test]
    fn many_tasks_run_independently() {
        let count = Rc::new(Cell::new(0u32));
        let mut senders = Vec::new();
        for _ in 0..5 {
            let (tx, rx) = oneshot::<u32>();
            senders.push(tx);
            let c = count.clone();
            spawn(async move {
                if rx.await.is_some() {
                    c.set(c.get() + 1);
                }
            });
        }
        assert_eq!(task_count(), 5);
        for (i, tx) in senders.into_iter().enumerate() {
            tx.send(i as u32);
        }
        assert_eq!(count.get(), 5);
        assert_eq!(task_count(), 0);
    }

    #[test]
    fn a_value_sent_before_the_first_poll_is_not_lost() {
        let (tx, rx) = oneshot::<u32>();
        tx.send(7);
        let got = Rc::new(Cell::new(0u32));
        let out = got.clone();
        spawn(async move {
            out.set(rx.await.unwrap_or(0));
        });
        assert_eq!(got.get(), 7);
    }
}
