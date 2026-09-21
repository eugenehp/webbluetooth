//! Runtime-agnostic timers.
//!
//! The crate promises to work on any executor, so it cannot reach for
//! `tokio::time`. One background thread owns a deadline heap and resolves a
//! oneshot per expiry, which is enough for the handful of deadlines Web
//! Bluetooth actually needs — a scan window, a connect timeout — without
//! spawning a thread per sleep.

use futures_channel::oneshot;
use futures_util::future::{select, Either};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::future::Future;
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

struct Entry {
    deadline: Instant,
    waker: oneshot::Sender<()>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline.cmp(&other.deadline)
    }
}

fn timer() -> &'static mpsc::Sender<Entry> {
    static TIMER: OnceLock<mpsc::Sender<Entry>> = OnceLock::new();
    TIMER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Entry>();
        std::thread::Builder::new()
            .name("webbluetooth-timer".into())
            .spawn(move || {
                let mut heap: BinaryHeap<Reverse<Entry>> = BinaryHeap::new();
                loop {
                    // Fire everything already due.
                    let now = Instant::now();
                    while heap.peek().is_some_and(|Reverse(e)| e.deadline <= now) {
                        if let Some(Reverse(e)) = heap.pop() {
                            let _ = e.waker.send(());
                        }
                    }
                    // Sleep until the next deadline, or until something arrives.
                    let next = heap
                        .peek()
                        .map(|Reverse(e)| e.deadline.saturating_duration_since(now));
                    let received = match next {
                        Some(d) => match rx.recv_timeout(d) {
                            Ok(e) => Some(e),
                            Err(mpsc::RecvTimeoutError::Timeout) => None,
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        },
                        None => match rx.recv() {
                            Ok(e) => Some(e),
                            Err(_) => return,
                        },
                    };
                    if let Some(e) = received {
                        heap.push(Reverse(e));
                    }
                }
            })
            .expect("could not start the webbluetooth timer thread");
        tx
    })
}

/// Complete after `duration`.
pub fn sleep(duration: Duration) -> impl Future<Output = ()> {
    let (tx, rx) = oneshot::channel();
    let _ = timer().send(Entry {
        deadline: Instant::now() + duration,
        waker: tx,
    });
    async move {
        let _ = rx.await;
    }
}

/// Run `future`, giving up after `duration`.
///
/// `Err(())` means the deadline passed; the future is dropped at that point.
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, ()> {
    let future = std::pin::pin!(future);
    let delay = std::pin::pin!(sleep(duration));
    match select(future, delay).await {
        Either::Left((value, _)) => Ok(value),
        Either::Right(((), _)) => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;

    #[test]
    fn sleep_actually_waits() {
        let start = Instant::now();
        block_on(sleep(Duration::from_millis(120)));
        assert!(
            start.elapsed() >= Duration::from_millis(100),
            "returned too early"
        );
    }

    #[test]
    fn timeout_returns_the_value_when_fast_enough() {
        let got = block_on(timeout(Duration::from_secs(5), async { 42 }));
        assert_eq!(got, Ok(42));
    }

    #[test]
    fn timeout_gives_up_on_a_future_that_never_finishes() {
        let got = block_on(timeout(
            Duration::from_millis(80),
            std::future::pending::<()>(),
        ));
        assert_eq!(got, Err(()));
    }

    #[test]
    fn deadlines_fire_in_order_not_submission_order() {
        // The later-submitted, shorter sleep must win — proof the heap sorts.
        let out = block_on(async {
            let long = sleep(Duration::from_millis(400));
            let short = sleep(Duration::from_millis(60));
            match select(std::pin::pin!(long), std::pin::pin!(short)).await {
                Either::Left(_) => "long",
                Either::Right(_) => "short",
            }
        });
        assert_eq!(out, "short");
    }
}
