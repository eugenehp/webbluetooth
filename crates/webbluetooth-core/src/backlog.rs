//! A bounded queue for things a peer sends us.
//!
//! Notifications, L2CAP data and writes to a published characteristic all
//! arrive on a platform's callback thread: CoreBluetooth's dispatch queue, the
//! D-Bus reader, the ATT reader, a JNI callback. Every one of those must return
//! promptly. Blocking one stalls the whole stack, and blocking CoreBluetooth's
//! dispatch queue can deadlock the framework.
//!
//! So there is no backpressure to apply. The peer has already sent the value;
//! nobody is left to push back on. The only real choices are to queue without
//! limit — which turns a stalled consumer into an out-of-memory crash — or to
//! bound the queue and lose something.
//!
//! This bounds it, keeps the newest, and counts what it dropped. The count is
//! the part that matters: a consumer that silently reassembles a byte stream
//! with a hole in it produces a bug that takes days to find, whereas one that
//! can see [`Receiver::lost`] move can decide for itself whether to start over.
//!
//! Keeping the *newest* is right for the common case — a sensor whose old
//! readings are worthless — and wrong for a transfer being reassembled, which
//! wants the intact prefix. That is a per-subscription choice; see
//! [`Overflow`].

use futures_core::Stream;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// What to discard when the queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// Drop the oldest queued value.
    ///
    /// The default, and right for anything that reports current state: a
    /// stale battery level or heart rate is worthless once a newer one has
    /// arrived.
    KeepNewest,
    /// Drop the value just arrived.
    ///
    /// Right for anything being reassembled in order — a firmware image, a
    /// log download — where a contiguous prefix and a known stopping point
    /// beat a fresher fragment with a hole before it.
    KeepOldest,
}

/// How many values a subscription queues before it has to drop one.
///
/// At the largest ATT payload this is about 250 KB per subscription, which is
/// enough to absorb a consumer stalling for seconds at the fastest rate a
/// connection interval allows, and small enough that a forgotten subscription
/// cannot exhaust memory.
pub const DEFAULT_CAPACITY: usize = 1024;

struct Shared<T> {
    queue: VecDeque<T>,
    capacity: usize,
    overflow: Overflow,
    /// Cumulative, never reset: a caller compares it across reads to find the
    /// size of a gap, and a counter that reset under them could not be
    /// compared at all.
    lost: u64,
    waker: Option<Waker>,
    /// Set when the receiving half goes away.
    closed: bool,
    /// Set when a sender declares the stream over.
    ///
    /// Distinct from every sender being dropped: a producer can know the peer
    /// has hung up while still holding its handle — an L2CAP channel closing
    /// is exactly that — and the reader should finish rather than wait.
    finished: bool,
    /// How many senders exist.
    ///
    /// When the last one goes the stream is finished, not merely idle, and the
    /// receiver says so — otherwise a caller waiting on a subscription that
    /// has been torn down waits for ever. A quiet peer is a different thing
    /// and does *not* end the stream: the sender is still there.
    senders: usize,
}

/// The producer half. Never blocks, never waits.
pub struct Sender<T>(Arc<Mutex<Shared<T>>>);

/// The consumer half, as a [`Stream`].
pub struct Receiver<T>(Arc<Mutex<Shared<T>>>);

/// A bounded queue with the given policy.
pub fn bounded<T>(capacity: usize, overflow: Overflow) -> (Sender<T>, Receiver<T>) {
    // A zero-capacity queue would drop everything and report nothing useful,
    // which is never what a caller means.
    let capacity = capacity.max(1);
    let shared = Arc::new(Mutex::new(Shared {
        queue: VecDeque::new(),
        capacity,
        overflow,
        lost: 0,
        waker: None,
        closed: false,
        finished: false,
        senders: 1,
    }));
    (Sender(shared.clone()), Receiver(shared))
}

/// A bounded queue with the default capacity and policy.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    bounded(DEFAULT_CAPACITY, Overflow::KeepNewest)
}

impl<T> Sender<T> {
    /// Queue a value.
    ///
    /// `Err` only when the receiver is gone — which is how a producer knows to
    /// stop, and why the backends can drop a subscription by testing this. A
    /// full queue is not an error: something is dropped, counted, and the send
    /// still succeeds, because the alternative is telling a platform callback
    /// about a problem it cannot do anything about.
    pub fn send(&self, value: T) -> Result<(), Closed> {
        let waker = {
            let mut shared = self.0.lock().unwrap();
            if shared.closed || shared.finished {
                return Err(Closed);
            }
            if shared.queue.len() >= shared.capacity {
                shared.lost += 1;
                match shared.overflow {
                    Overflow::KeepNewest => {
                        shared.queue.pop_front();
                    }
                    Overflow::KeepOldest => return Ok(()),
                }
            }
            shared.queue.push_back(value);
            shared.waker.take()
        };
        // Woken outside the lock: waking polls the stream, which locks.
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    /// Whether anything is still listening.
    pub fn is_open(&self) -> bool {
        let shared = self.0.lock().unwrap();
        !shared.closed && !shared.finished
    }

    /// Declare the stream over while still holding this handle.
    ///
    /// For a producer that learns the source has ended — a peer closing an
    /// L2CAP channel — and cannot simply drop its sender because something
    /// else owns it. Whatever is queued is still delivered first.
    pub fn close(&self) {
        let waker = {
            let mut shared = self.0.lock().unwrap();
            shared.finished = true;
            shared.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

// Derived would demand `T: Clone`, which is not what is being cloned — the
// handle is. Several producers feeding one queue is the ordinary case: a
// characteristic can be registered under more than one key, and each
// registration keeps its own sender.
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.0.lock().unwrap().senders += 1;
        Self(self.0.clone())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut shared = self.0.lock().unwrap();
            shared.senders -= 1;
            if shared.senders > 0 {
                return;
            }
            // The last one. Anyone waiting is woken to discover the stream is
            // over rather than waiting for a value that cannot come.
            shared.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The receiver is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

impl<T> Receiver<T> {
    /// How many values have been dropped, in total, since this was created.
    ///
    /// Cumulative rather than reset-on-read, so two reads can be compared: if
    /// this has moved between one value and the next, the difference is
    /// exactly how many went missing in between.
    pub fn lost(&self) -> u64 {
        self.0.lock().unwrap().lost
    }

    /// Take the next value if one is waiting, without registering interest.
    ///
    /// For a caller draining what has already arrived rather than awaiting
    /// what has not — a chooser sweeping up the candidates seen so far. It
    /// deliberately does not store a waker: nobody is going to be woken.
    pub fn try_recv(&mut self) -> Option<T> {
        self.0.lock().unwrap().queue.pop_front()
    }

    /// How many values are waiting.
    ///
    /// A number that stays near [`DEFAULT_CAPACITY`] is a consumer that is not
    /// keeping up, which is worth knowing before [`lost`](Self::lost) starts
    /// moving.
    pub fn depth(&self) -> usize {
        self.0.lock().unwrap().queue.len()
    }
}

impl<T> Stream for Receiver<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<T>> {
        let mut shared = self.0.lock().unwrap();
        if let Some(value) = shared.queue.pop_front() {
            return Poll::Ready(Some(value));
        }
        // Drained *and* nobody left to send: the stream is over. Checked after
        // the queue, so values already sent are delivered before the end.
        if shared.senders == 0 || shared.finished {
            return Poll::Ready(None);
        }
        // A quiet peer is not an ending — the sender is still there, and a
        // subscription lasts until the caller drops it.
        shared.waker = Some(context.waker().clone());
        Poll::Pending
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut shared = self.0.lock().unwrap();
        shared.closed = true;
        // Anything still queued is never going to be read.
        shared.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{RawWaker, RawWakerVTable};

    fn noop_waker() -> Waker {
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        fn noop(_: *const ()) {}
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    fn drain<T>(rx: &mut Receiver<T>) -> Vec<T> {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        let mut out = Vec::new();
        while let Poll::Ready(Some(v)) = Pin::new(&mut *rx).poll_next(&mut context) {
            out.push(v);
        }
        out
    }

    #[test]
    fn values_come_out_in_the_order_they_went_in() {
        let (tx, mut rx) = channel();
        for i in 0..5 {
            tx.send(i).unwrap();
        }
        assert_eq!(drain(&mut rx), vec![0, 1, 2, 3, 4]);
        assert_eq!(rx.lost(), 0);
    }

    /// The default: a full queue keeps the newest, because a stale sensor
    /// reading is worthless once a newer one exists.
    #[test]
    fn keeping_the_newest_drops_from_the_front() {
        let (tx, mut rx) = bounded(3, Overflow::KeepNewest);
        for i in 0..5 {
            tx.send(i).expect("a full queue is not an error");
        }
        assert_eq!(drain(&mut rx), vec![2, 3, 4], "the newest three survive");
        assert_eq!(rx.lost(), 2, "and the two it dropped are counted");
    }

    /// The opposite policy, for something being reassembled in order: the
    /// prefix stays contiguous and the loss is at the end, where a consumer
    /// can see it and stop.
    #[test]
    fn keeping_the_oldest_drops_what_just_arrived() {
        let (tx, mut rx) = bounded(3, Overflow::KeepOldest);
        for i in 0..5 {
            tx.send(i).unwrap();
        }
        assert_eq!(drain(&mut rx), vec![0, 1, 2], "the first three survive");
        assert_eq!(rx.lost(), 2);
    }

    /// The property the whole design rests on: a gap is always visible. A
    /// consumer comparing `lost()` across two reads learns exactly how many
    /// values went missing between them.
    #[test]
    fn a_gap_is_measurable_between_two_reads() {
        let (tx, mut rx) = bounded(2, Overflow::KeepNewest);
        tx.send(1).unwrap();

        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        let before = rx.lost();
        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut context),
            Poll::Ready(Some(1))
        );

        // The consumer stalls; seven more arrive into a queue of two.
        for i in 2..9 {
            tx.send(i).unwrap();
        }
        let next = match Pin::new(&mut rx).poll_next(&mut context) {
            Poll::Ready(Some(v)) => v,
            other => panic!("expected a value, got {other:?}"),
        };
        let gap = rx.lost() - before;

        assert_eq!(gap, 5, "five values went missing");
        assert_eq!(next, 7, "and the stream resumes after them");
    }

    /// A full queue must not be an error: a platform callback cannot do
    /// anything about it, and failing the send would only lose the value
    /// *and* the information that it was lost.
    #[test]
    fn overflow_is_not_a_send_failure() {
        let (tx, _rx) = bounded(1, Overflow::KeepNewest);
        assert!(tx.send(1).is_ok());
        assert!(tx.send(2).is_ok());
        assert!(tx.send(3).is_ok());
    }

    /// Dropping the consumer is how a producer learns to stop — the backends
    /// prune their subscriber lists by testing exactly this.
    #[test]
    fn a_dropped_receiver_closes_the_sender() {
        let (tx, rx) = channel();
        assert!(tx.is_open());
        assert!(tx.send(1).is_ok());
        drop(rx);
        assert!(!tx.is_open());
        assert_eq!(tx.send(2), Err(Closed));
    }

    /// Queued values go with the receiver rather than being held by a sender
    /// nobody is reading.
    #[test]
    fn a_dropped_receiver_releases_what_was_queued() {
        let held = Arc::new(());
        let (tx, rx) = channel();
        for _ in 0..10 {
            tx.send(held.clone()).unwrap();
        }
        assert_eq!(Arc::strong_count(&held), 11);
        drop(rx);
        assert_eq!(Arc::strong_count(&held), 1, "the queue was released");
    }

    /// A waiting consumer is woken when something arrives.
    #[test]
    fn a_waiting_consumer_is_woken() {
        let (tx, mut rx) = channel::<u32>();
        let woken = Arc::new(Mutex::new(false));

        struct Flag(Arc<Mutex<bool>>);
        impl std::task::Wake for Flag {
            fn wake(self: Arc<Self>) {
                *self.0.lock().unwrap() = true;
            }
        }
        let waker: Waker = Arc::new(Flag(woken.clone())).into();
        let mut context = Context::from_waker(&waker);

        assert_eq!(Pin::new(&mut rx).poll_next(&mut context), Poll::Pending);
        assert!(!*woken.lock().unwrap());

        tx.send(7).unwrap();
        assert!(*woken.lock().unwrap(), "sending should wake the consumer");
        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut context),
            Poll::Ready(Some(7))
        );
    }

    /// A quiet peer and a finished subscription are different things, and
    /// conflating them is how a caller ends up waiting for ever.
    ///
    /// Nothing arriving means exactly that: the sender is still there and a
    /// value may come. The sender *going away* is the end, and the stream has
    /// to say so — a scan whose watcher was removed must finish, not hang.
    #[test]
    fn silence_is_pending_but_a_dropped_sender_is_the_end() {
        let (tx, mut rx) = channel::<u32>();
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        // Quiet, but alive.
        assert_eq!(Pin::new(&mut rx).poll_next(&mut context), Poll::Pending);

        tx.send(7).unwrap();
        drop(tx);

        // Anything already sent still arrives before the end.
        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut context),
            Poll::Ready(Some(7)),
            "a value sent before the sender went must not be lost"
        );
        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut context),
            Poll::Ready(None),
            "and then the stream is over"
        );
    }

    /// One clone going away is not the end; the last one is.
    #[test]
    fn the_stream_ends_only_when_every_sender_is_gone() {
        let (tx, mut rx) = channel::<u32>();
        let second = tx.clone();
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        drop(second);
        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut context),
            Poll::Pending,
            "one sender remains, so the stream is merely quiet"
        );

        drop(tx);
        assert_eq!(Pin::new(&mut rx).poll_next(&mut context), Poll::Ready(None));
    }

    /// A producer can end the stream without giving up its handle, and what
    /// is already queued still arrives first.
    #[test]
    fn closing_ends_the_stream_after_draining() {
        let (tx, mut rx) = channel::<u32>();
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.close();

        assert!(!tx.is_open(), "a closed sender reports itself closed");
        assert_eq!(tx.send(3), Err(Closed), "and takes nothing further");

        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut context),
            Poll::Ready(Some(1)),
            "what was queued before the close is still delivered"
        );
        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut context),
            Poll::Ready(Some(2))
        );
        assert_eq!(Pin::new(&mut rx).poll_next(&mut context), Poll::Ready(None));
    }

    /// A capacity of zero would drop everything; it is treated as one.
    #[test]
    fn a_zero_capacity_still_carries_a_value() {
        let (tx, mut rx) = bounded(0, Overflow::KeepNewest);
        tx.send(9).unwrap();
        assert_eq!(drain(&mut rx), vec![9]);
    }

    /// The stream stays open while the peer is quiet — a subscription ends
    /// when the caller drops it, not when nothing has arrived for a while.
    #[test]
    fn an_empty_queue_is_pending_rather_than_finished() {
        let (tx, mut rx) = channel::<u32>();
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        assert_eq!(Pin::new(&mut rx).poll_next(&mut context), Poll::Pending);
        tx.send(1).unwrap();
        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut context),
            Poll::Ready(Some(1))
        );
        assert_eq!(Pin::new(&mut rx).poll_next(&mut context), Poll::Pending);
    }

    /// Sends arriving from several threads at once must all be accounted for:
    /// every value either comes out or is counted as lost, never neither.
    #[test]
    fn nothing_vanishes_unaccounted_under_concurrent_senders() {
        const THREADS: usize = 8;
        const EACH: usize = 500;

        let (tx, mut rx) = bounded::<usize>(16, Overflow::KeepNewest);
        let tx = Arc::new(tx);
        let threads: Vec<_> = (0..THREADS)
            .map(|_| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for i in 0..EACH {
                        tx.send(i).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("no sender should have panicked");
        }

        let received = drain(&mut rx).len() as u64;
        assert_eq!(
            received + rx.lost(),
            (THREADS * EACH) as u64,
            "every value is either delivered or counted"
        );
    }

    /// Several producers, one queue — and cloning the handle must not require
    /// the payload to be cloneable.
    #[test]
    fn senders_clone_and_share_one_queue() {
        struct NotClone(u32);

        let (tx, mut rx) = bounded::<NotClone>(8, Overflow::KeepNewest);
        let second = tx.clone();
        tx.send(NotClone(1)).unwrap();
        second.send(NotClone(2)).unwrap();

        let got: Vec<u32> = drain(&mut rx).into_iter().map(|v| v.0).collect();
        assert_eq!(got, vec![1, 2], "both feed the same queue, in order");

        // And one clone going away does not close the queue under the others.
        drop(second);
        assert!(tx.is_open());
        assert!(tx.send(NotClone(3)).is_ok());
    }

    /// `try_recv` takes what is there and does not wait for what is not.
    #[test]
    fn try_recv_drains_without_waiting() {
        let (tx, mut rx) = channel();
        assert_eq!(rx.try_recv(), None, "nothing queued yet");

        tx.send(1).unwrap();
        tx.send(2).unwrap();
        assert_eq!(rx.try_recv(), Some(1));
        assert_eq!(rx.try_recv(), Some(2));
        assert_eq!(rx.try_recv(), None, "and it stops rather than blocking");
    }

    #[test]
    fn depth_reports_what_is_waiting() {
        let (tx, mut rx) = bounded(4, Overflow::KeepNewest);
        assert_eq!(rx.depth(), 0);
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        assert_eq!(rx.depth(), 2);
        drain(&mut rx);
        assert_eq!(rx.depth(), 0);
    }

    /// The published default has to be the one the module documents, since
    /// the memory figure in the docs is derived from it.
    #[test]
    fn the_default_is_the_documented_one() {
        assert_eq!(DEFAULT_CAPACITY, 1024);
        let (tx, rx) = channel::<u8>();
        for i in 0..(DEFAULT_CAPACITY + 10) {
            tx.send(i as u8).unwrap();
        }
        assert_eq!(rx.depth(), DEFAULT_CAPACITY);
        assert_eq!(rx.lost(), 10);
    }
}
