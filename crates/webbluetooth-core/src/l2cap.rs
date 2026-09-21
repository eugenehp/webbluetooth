//! L2CAP connection-oriented channels — the portable half.
//!
//! The one place Bluetooth stops being request/response. Instead of attributes
//! you get a bidirectional byte pipe with no 512-byte ceiling and no ATT round
//! trip per write — the right tool for firmware images, audio, or anything
//! else that would otherwise be thousands of characteristic writes.
//!
//! Neither side is Web Bluetooth; the specification has no L2CAP.
//!
//! # What is here and what is not
//!
//! A channel is a platform socket: a `CBL2CAPChannel` with two `NSStream`s, an
//! `AF_BLUETOOTH` file descriptor, a Java `BluetoothSocket`. Opening one is
//! entirely platform work, so it stays in the platform crates.
//!
//! Everything *around* it is the same on all three — a reader callback on some
//! pump thread, a bounded backlog, an async stream out the other side, a
//! close reason delivered once — and that was written three times over before
//! it was written here. [`Closed`] and [`ChannelSink`] were declared in
//! `webbluetooth-apple`, `webbluetooth-linux` and `webbluetooth-android`
//! identically, because a shared declaration had nowhere to live.
//!
//! So: the platform crate opens the socket and implements [`PlatformChannel`]
//! on it; [`L2capChannel`] is generic over that, and is the type callers hold.
//! No dynamic dispatch — each build has exactly one channel type.
//!
//! **A chunk is not a message.** L2CAP CoC is a byte stream: what arrives in
//! one chunk is whatever one read returned. Frame it yourself.

use crate::error::{Error, Result};
use futures_core::Stream;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

/// A PSM — the L2CAP equivalent of a port number.
pub type Psm = u16;

/// How many packets a channel queues before it has to drop one.
///
/// Larger than the advertisement backlog and for the opposite reason: these
/// are not samples that go stale, they are a stream whose every packet
/// matters, so the queue absorbs as much of a stall as is reasonable before
/// anything is lost.
const L2CAP_BACKLOG: usize = 2048;

/// Why a channel closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closed {
    /// The peer closed cleanly.
    ByPeer,
    /// This side called [`L2capChannel::close`].
    Locally,
    /// The stream reported an error.
    Error(String),
}

/// Where a channel's inbound bytes go.
///
/// Implemented by [`Sink`] and called on whatever thread the platform reads
/// on — a CoreBluetooth run loop, a Linux reader thread, a Java pump.
pub trait ChannelSink: Send + Sync {
    /// Bytes arrived. Chunk boundaries are read boundaries, not message
    /// boundaries — L2CAP CoC is a byte stream.
    fn on_bytes(&self, bytes: Vec<u8>);
    /// The channel closed. Called exactly once.
    fn on_closed(&self, reason: Closed);
}

/// What an open platform socket can do, once somebody has opened one.
///
/// Every method here already exists, spelled exactly this way, on
/// `webbluetooth_apple::l2cap::Channel` and its two siblings; the trait is
/// what lets one wrapper serve all three.
pub trait PlatformChannel: Send + Sync + 'static {
    /// The channel's PSM.
    fn psm(&self) -> Psm;
    /// The peer's per-host identifier.
    fn peer_id(&self) -> &str;
    /// Queue bytes. Fails only if the channel is closed.
    fn send(&self, bytes: &[u8]) -> std::result::Result<(), Closed>;
    /// How many queued bytes have not been written yet.
    fn pending_bytes(&self) -> usize;
    /// Why the channel closed, or `None` while it is open.
    fn closed(&self) -> Option<Closed>;
    /// Close it.
    fn close(&self);
}

/// Bridges the platform's pump-thread callbacks into an async channel.
pub struct Sink {
    bytes: crate::backlog::Sender<Vec<u8>>,
    closed: Mutex<Vec<futures_channel::oneshot::Sender<Closed>>>,
}

impl ChannelSink for Sink {
    fn on_bytes(&self, bytes: Vec<u8>) {
        let _ = self.bytes.send(bytes);
    }

    fn on_closed(&self, reason: Closed) {
        // Ending the byte stream is what lets a `while let Some(..)` loop exit.
        self.bytes.close();
        for tx in self.closed.lock().unwrap().drain(..) {
            let _ = tx.send(reason.clone());
        }
    }
}

/// The bridge from a platform's reader callbacks to an async stream.
///
/// A platform crate calls this, hands the first value to its own `Channel`
/// constructor, and passes the rest to [`L2capChannel::new`].
pub fn sink() -> (Arc<dyn ChannelSink>, Arc<Sink>, Incoming) {
    // A byte stream being reassembled in order, so the *oldest* are kept: a
    // contiguous prefix and a known stopping point beat a fresher fragment
    // with a hole before it. The peer is already sending at link rate and the
    // reader callback cannot be made to wait, so something has to give when a
    // consumer falls behind — `lost` says how much.
    let (tx, rx) = crate::backlog::bounded(L2CAP_BACKLOG, crate::backlog::Overflow::KeepOldest);
    let sink = Arc::new(Sink {
        bytes: tx,
        closed: Mutex::new(Vec::new()),
    });
    (sink.clone(), sink, Incoming { rx })
}

/// An open L2CAP channel. Closes on drop.
pub struct L2capChannel<C: PlatformChannel> {
    channel: C,
    incoming: Mutex<Option<Incoming>>,
    sink: Arc<Sink>,
}

impl<C: PlatformChannel> L2capChannel<C> {
    /// Wrap a platform socket that was opened with [`sink`]'s first value.
    pub fn new(channel: C, sink: Arc<Sink>, incoming: Incoming) -> Self {
        Self {
            channel,
            incoming: Mutex::new(Some(incoming)),
            sink,
        }
    }

    /// The channel's PSM.
    pub fn psm(&self) -> Psm {
        self.channel.psm()
    }

    /// The peer's per-host identifier.
    pub fn peer_id(&self) -> &str {
        self.channel.peer_id()
    }

    /// Take the inbound byte stream. `None` on a second call — there is one
    /// stream and consuming it twice would split the bytes between consumers.
    pub fn take_incoming(&self) -> Option<Incoming> {
        self.incoming.lock().unwrap().take()
    }

    /// Queue bytes for sending.
    ///
    /// Returns once queued, not once on the air; the pump writes them as the
    /// peer grants credit. Use [`L2capChannel::flushed`] to wait for the queue
    /// to drain.
    pub fn send(&self, bytes: &[u8]) -> Result<()> {
        self.channel.send(bytes).map_err(closed_error)
    }

    /// How many queued bytes have not been written yet.
    pub fn pending_bytes(&self) -> usize {
        self.channel.pending_bytes()
    }

    /// Wait until everything queued has been written, or the channel closes.
    ///
    /// Polls, because `NSOutputStream` reports space rather than completion.
    pub async fn flushed(&self) -> Result<()> {
        while self.pending_bytes() > 0 {
            if let Some(reason) = self.closed() {
                return Err(match reason {
                    Closed::ByPeer => Error::Network("the peer closed mid-write".into()),
                    Closed::Locally => Error::InvalidState("the channel was closed".into()),
                    Closed::Error(m) => Error::Network(format!("L2CAP channel failed: {m}")),
                });
            }
            crate::timer::sleep(std::time::Duration::from_millis(5)).await;
        }
        Ok(())
    }

    /// Why the channel closed, or `None` while it is open.
    pub fn closed(&self) -> Option<Closed> {
        self.channel.closed()
    }

    /// Resolve when the channel closes, with the reason.
    pub async fn on_close(&self) -> Closed {
        if let Some(reason) = self.closed() {
            return reason;
        }
        let rx = {
            let (tx, rx) = futures_channel::oneshot::channel();
            // Re-check under the lock: it may have closed in between.
            if let Some(reason) = self.closed() {
                return reason;
            }
            self.sink.closed.lock().unwrap().push(tx);
            rx
        };
        rx.await.unwrap_or(Closed::Locally)
    }

    /// Close the channel.
    pub fn close(&self) {
        self.channel.close()
    }
}

/// A closed channel, as the error a caller sees.
fn closed_error(reason: Closed) -> Error {
    match reason {
        Closed::ByPeer => Error::Network("the peer closed the L2CAP channel".into()),
        Closed::Locally => Error::InvalidState("the L2CAP channel is closed".into()),
        Closed::Error(m) => Error::Network(format!("L2CAP channel failed: {m}")),
    }
}

impl<C: PlatformChannel> std::fmt::Debug for L2capChannel<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("L2capChannel")
            .field("psm", &self.psm())
            .field("peer_id", &self.peer_id())
            .field("closed", &self.closed())
            .finish()
    }
}

/// The inbound byte stream of one channel.
pub struct Incoming {
    rx: crate::backlog::Receiver<Vec<u8>>,
}

impl Stream for Incoming {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}
