//! L2CAP connection-oriented channels.
//!
//! `CBL2CAPChannel` is the one place CoreBluetooth stops being a
//! request/response API: instead of attributes it hands over an
//! `NSInputStream` and an `NSOutputStream`, a real bidirectional byte pipe with
//! no 512-byte attribute ceiling and no per-write ATT round trip.
//!
//! # Getting bytes out of an `NSStream` without a Cocoa application
//!
//! `NSStream` is runloop-driven. Its asynchronous mode wants the stream
//! scheduled on an `NSRunLoop` with a delegate that receives
//! `stream:handleEvent:`; its synchronous mode blocks. A Rust program has
//! neither a runloop nor a thread to spare for blocking reads, so each channel
//! gets a **pump**: one thread that owns both streams, runs a `CFRunLoop`, and
//! is the only place any stream method is ever called.
//!
//! * Inbound bytes are read on `NSStreamEventHasBytesAvailable` and pushed down
//!   a channel to Rust.
//! * Outbound bytes are queued by whatever thread calls [`Channel::send`] and
//!   written by the pump, either on `NSStreamEventHasSpaceAvailable` or
//!   immediately — the writer calls `CFRunLoopWakeUp`, which is documented as
//!   thread-safe, so a queued write does not wait for the next timeout.
//!
//! That keeps every `NSStream` call on one thread while leaving the API
//! callable from anywhere.
//!
//! The delegate class is synthesised at runtime like the others, conforming to
//! `NSStreamDelegate`.

use crate::objc::{self, global_nsstring, require_class, ClassBuilder, Id, Retained, NIL};
use core::ffi::c_void;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// An L2CAP Protocol/Service Multiplexer — the port number of a channel.
///
/// Dynamic PSMs assigned by the system are in the range `0x0080..=0x00FF`.
pub use webbluetooth_core::l2cap::{ChannelSink, Closed, Psm};

/// How much to read from the input stream at a time.
const READ_CHUNK: usize = 4096;

// ── CoreFoundation runloop ──────────────────────────────────────────────────

type CFRunLoopRef = *mut c_void;
type CFStringRef = *const c_void;

unsafe extern "C" {
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source: bool) -> i32;
    fn CFRunLoopWakeUp(rl: CFRunLoopRef);
    static kCFRunLoopDefaultMode: CFStringRef;
}

// ── CBL2CAPChannel accessors ────────────────────────────────────────────────

/// `[channel PSM]`.
///
/// # Safety
/// `channel` must be a `CBL2CAPChannel`.
pub unsafe fn channel_psm(channel: Id) -> Psm {
    unsafe { crate::msg_send_t![u16; channel, PSM] }
}

/// `[[channel peer] identifier]` as a string.
///
/// # Safety
/// `channel` must be a `CBL2CAPChannel`.
pub unsafe fn channel_peer_id(channel: Id) -> Option<String> {
    unsafe {
        let peer: Id = crate::msg_send![channel, peer];
        if peer.is_null() {
            return None;
        }
        objc::to_string(crate::msg_send![
            crate::msg_send![peer, identifier],
            UUIDString
        ])
    }
}

/// `[channel inputStream]`, retained.
///
/// # Safety
/// `channel` must be a `CBL2CAPChannel`.
pub unsafe fn channel_input(channel: Id) -> Option<Retained> {
    unsafe { Retained::retain(crate::msg_send![channel, inputStream]) }
}

/// `[channel outputStream]`, retained.
///
/// # Safety
/// `channel` must be a `CBL2CAPChannel`.
pub unsafe fn channel_output(channel: Id) -> Option<Retained> {
    unsafe { Retained::retain(crate::msg_send![channel, outputStream]) }
}

// ── The pump ────────────────────────────────────────────────────────────────

/// `NSStreamEvent`.
mod event {
    pub const OPEN_COMPLETED: usize = 1 << 0;
    pub const HAS_BYTES_AVAILABLE: usize = 1 << 1;
    pub const HAS_SPACE_AVAILABLE: usize = 1 << 2;
    pub const ERROR_OCCURRED: usize = 1 << 3;
    pub const END_ENCOUNTERED: usize = 1 << 4;
}

struct Pump {
    input: Retained,
    output: Retained,
    /// Chunks waiting to go out, front first.
    outgoing: Mutex<VecDeque<Vec<u8>>>,
    /// Set by `NSStreamEventHasSpaceAvailable`, cleared on a short write.
    space_available: AtomicBool,
    sink: Arc<dyn ChannelSink>,
    closed: Mutex<Option<Closed>>,
    stop: AtomicBool,
    /// The pump thread's runloop, so a writer can wake it.
    runloop: AtomicPtr<c_void>,
}

// The streams are only ever messaged from the pump thread; everything else is
// behind a lock or an atomic.
unsafe impl Send for Pump {}
unsafe impl Sync for Pump {}

impl Pump {
    fn on_event(&self, stream: Id, events: usize) {
        if events & event::HAS_BYTES_AVAILABLE != 0 && stream == self.input.as_ptr() {
            self.drain_input();
        }
        if events & event::HAS_SPACE_AVAILABLE != 0 && stream == self.output.as_ptr() {
            self.space_available.store(true, Ordering::Release);
            self.drain_output();
        }
        if events & event::END_ENCOUNTERED != 0 {
            self.finish(Closed::ByPeer);
        }
        if events & event::ERROR_OCCURRED != 0 {
            let message = unsafe {
                let err: Id = crate::msg_send![stream, streamError];
                objc::error_message(err).map(|(_, m)| m)
            }
            .unwrap_or_else(|| "stream error".into());
            self.finish(Closed::Error(message));
        }
        let _ = event::OPEN_COMPLETED;
    }

    /// Read everything the input stream has. Pump thread only.
    fn drain_input(&self) {
        loop {
            let available =
                unsafe { crate::msg_send_t![bool; self.input.as_ptr(), hasBytesAvailable] };
            if !available {
                return;
            }
            let mut buf = vec![0u8; READ_CHUNK];
            let n = unsafe {
                crate::msg_send_t![
                    isize;
                    self.input.as_ptr(),
                    read: buf.as_mut_ptr(),
                    maxLength: READ_CHUNK
                ]
            };
            match n {
                n if n > 0 => {
                    buf.truncate(n as usize);
                    self.sink.on_bytes(buf);
                }
                0 => {
                    self.finish(Closed::ByPeer);
                    return;
                }
                _ => {
                    self.finish(Closed::Error("read failed".into()));
                    return;
                }
            }
        }
    }

    /// Write as much of the queue as the stream will take. Pump thread only.
    fn drain_output(&self) {
        loop {
            let mut queue = self.outgoing.lock().unwrap();
            let Some(front) = queue.front_mut() else {
                return;
            };

            if !unsafe { crate::msg_send_t![bool; self.output.as_ptr(), hasSpaceAvailable] } {
                self.space_available.store(false, Ordering::Release);
                return;
            }
            let n = unsafe {
                crate::msg_send_t![
                    isize;
                    self.output.as_ptr(),
                    write: front.as_ptr(),
                    maxLength: front.len()
                ]
            };
            match n {
                n if n > 0 && (n as usize) < front.len() => {
                    // Partial write: keep the tail at the front of the queue.
                    front.drain(..n as usize);
                    self.space_available.store(false, Ordering::Release);
                    return;
                }
                n if n > 0 => {
                    queue.pop_front();
                }
                0 => {
                    self.space_available.store(false, Ordering::Release);
                    return;
                }
                _ => {
                    drop(queue);
                    self.finish(Closed::Error("write failed".into()));
                    return;
                }
            }
        }
    }

    fn finish(&self, reason: Closed) {
        {
            let mut closed = self.closed.lock().unwrap();
            if closed.is_some() {
                // Already finished; `on_closed` is called exactly once.
                return;
            }
            *closed = Some(reason.clone());
        }
        self.stop.store(true, Ordering::Release);
        self.sink.on_closed(reason);
    }

    fn wake(&self) {
        let rl = self.runloop.load(Ordering::Acquire);
        if !rl.is_null() {
            unsafe { CFRunLoopWakeUp(rl) };
        }
    }
}

// ── NSStreamDelegate, synthesised ───────────────────────────────────────────

static PUMPS: OnceLock<Mutex<HashMap<usize, Arc<Pump>>>> = OnceLock::new();

fn pumps() -> &'static Mutex<HashMap<usize, Arc<Pump>>> {
    PUMPS.get_or_init(|| Mutex::new(HashMap::new()))
}

static STREAM_DELEGATE_CLASS: OnceLock<usize> = OnceLock::new();

fn stream_delegate_class() -> Id {
    *STREAM_DELEGATE_CLASS.get_or_init(|| {
        for attempt in 0..64u32 {
            let name = if attempt == 0 {
                c"WBRustNSStreamDelegate".to_owned()
            } else {
                std::ffi::CString::new(format!("WBRustNSStreamDelegate{attempt}"))
                    .expect("class name")
            };
            // SAFETY: `handle_event` matches `v@:@Q` — object, then NSUInteger.
            let Some(builder) = (unsafe { ClassBuilder::new(c"NSObject", &name) }) else {
                continue;
            };
            return unsafe {
                builder
                    .method(
                        c"stream:handleEvent:",
                        handle_event as *const c_void,
                        c"v@:@Q",
                    )
                    .conforms(c"NSStreamDelegate")
                    .register()
            } as usize;
        }
        panic!("could not register a stream delegate class after 64 attempts");
    }) as Id
}

unsafe extern "C" fn handle_event(this: Id, _cmd: *const c_void, stream: Id, events: usize) {
    let pump = pumps()
        .lock()
        .ok()
        .and_then(|m| m.get(&(this as usize)).cloned());
    if let Some(pump) = pump {
        pump.on_event(stream, events);
    }
}

// ── Channel ─────────────────────────────────────────────────────────────────

/// An open L2CAP channel.
///
/// Dropping this closes the channel and stops its pump thread.
pub struct Channel {
    psm: Psm,
    peer_id: String,
    pump: Arc<Pump>,
    /// Kept alive: the streams belong to it.
    _channel: Retained,
}

impl Channel {
    /// Take ownership of a `CBL2CAPChannel` and start pumping it.
    ///
    /// # Safety
    /// `channel` must be a `CBL2CAPChannel` from a delegate callback.
    pub unsafe fn adopt(channel: Retained, sink: Arc<dyn ChannelSink>) -> Option<Self> {
        let (psm, peer_id, input, output) = unsafe {
            (
                channel_psm(channel.as_ptr()),
                channel_peer_id(channel.as_ptr()).unwrap_or_default(),
                channel_input(channel.as_ptr())?,
                channel_output(channel.as_ptr())?,
            )
        };

        let pump = Arc::new(Pump {
            input,
            output,
            outgoing: Mutex::new(VecDeque::new()),
            space_available: AtomicBool::new(false),
            sink,
            closed: Mutex::new(None),
            stop: AtomicBool::new(false),
            runloop: AtomicPtr::new(std::ptr::null_mut()),
        });

        spawn_pump(pump.clone(), psm);

        Some(Self {
            psm,
            peer_id,
            pump,
            _channel: channel,
        })
    }

    /// The channel's PSM.
    pub fn psm(&self) -> Psm {
        self.psm
    }

    /// The peer's per-host identifier.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Queue bytes for sending.
    ///
    /// Returns once the bytes are queued, not once they are on the air. The
    /// pump writes them as the peer grants credit.
    pub fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        if let Some(reason) = self.closed() {
            return Err(reason);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.pump.outgoing.lock().unwrap().push_back(bytes.to_vec());
        // Wake the pump so the write does not wait for the next tick.
        self.pump.wake();
        Ok(())
    }

    /// How many queued bytes have not yet been written.
    pub fn pending_bytes(&self) -> usize {
        self.pump
            .outgoing
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .sum()
    }

    /// Why the channel closed, or `None` while it is open.
    pub fn closed(&self) -> Option<Closed> {
        self.pump.closed.lock().unwrap().clone()
    }

    /// Close the channel.
    pub fn close(&self) {
        self.pump.finish(Closed::Locally);
        self.pump.wake();
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.close();
    }
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("L2capChannel")
            .field("psm", &self.psm)
            .field("peer", &self.peer_id)
            .field("closed", &self.closed())
            .finish()
    }
}

/// One thread per channel: owns both streams, runs the runloop, does all I/O.
fn spawn_pump(pump: Arc<Pump>, psm: Psm) {
    std::thread::Builder::new()
        .name(format!("webbluetooth-l2cap-{psm}"))
        .spawn(move || {
            let _pool = objc::AutoreleasePool::new();

            // A delegate object per channel, so `stream:handleEvent:` can find
            // this pump.
            let delegate = unsafe {
                let obj: Id = crate::msg_send![stream_delegate_class(), alloc];
                let obj: Id = crate::msg_send![obj, init];
                match Retained::adopt(obj) {
                    Some(d) => d,
                    None => return,
                }
            };
            pumps().lock().unwrap().insert(delegate.key(), pump.clone());

            unsafe {
                let run_loop: Id = crate::msg_send![require_class(c"NSRunLoop"), currentRunLoop];
                let mode = global_nsstring(c"NSDefaultRunLoopMode");
                for stream in [pump.input.as_ptr(), pump.output.as_ptr()] {
                    crate::msg_send_void![stream, setDelegate: delegate.as_ptr()];
                    if !run_loop.is_null() && !mode.is_null() {
                        crate::msg_send_void![stream, scheduleInRunLoop: run_loop, forMode: mode];
                    }
                    crate::msg_send_void![stream, open];
                }

                pump.runloop.store(CFRunLoopGetCurrent(), Ordering::Release);

                while !pump.stop.load(Ordering::Acquire) {
                    // Returns early when `CFRunLoopWakeUp` is called, so a
                    // queued write goes out immediately rather than waiting.
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.25, false);
                    if pump.space_available.load(Ordering::Acquire) {
                        pump.drain_output();
                    }
                }

                for stream in [pump.input.as_ptr(), pump.output.as_ptr()] {
                    crate::msg_send_void![stream, close];
                    if !run_loop.is_null() && !mode.is_null() {
                        crate::msg_send_void![stream, removeFromRunLoop: run_loop, forMode: mode];
                    }
                    crate::msg_send_void![stream, setDelegate: NIL];
                }
            }

            pump.runloop.store(std::ptr::null_mut(), Ordering::Release);
            pumps().lock().unwrap().remove(&delegate.key());
        })
        .expect("could not start the L2CAP pump thread");
}

// ── The portable wrapper ────────────────────────────────────────────────────

/// This platform's channel, wrapped in the portable one.
///
/// The wrapper — backlog, async stream, close notification — is
/// `webbluetooth-core`'s and identical on every platform. Only what is above
/// this line is Bluetooth-stack-specific.
pub type L2capChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

impl webbluetooth_core::l2cap::PlatformChannel for Channel {
    fn psm(&self) -> Psm {
        Channel::psm(self)
    }

    fn peer_id(&self) -> &str {
        Channel::peer_id(self)
    }

    fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        Channel::send(self, bytes)
    }

    fn pending_bytes(&self) -> usize {
        Channel::pending_bytes(self)
    }

    fn closed(&self) -> Option<Closed> {
        Channel::closed(self)
    }

    fn close(&self) {
        Channel::close(self)
    }
}

/// Adopt a `CBL2CAPChannel` handed over by a delegate.
///
/// # Safety
///
/// `handle` must be a live `CBL2CAPChannel`, which in practice means one that
/// arrived in a `didOpenL2CAPChannel:` callback.
pub unsafe fn adopt(handle: Retained) -> webbluetooth_core::Result<L2capChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    // SAFETY: the caller guarantees `handle` is a live channel.
    let channel = unsafe { Channel::adopt(handle, channel_sink) }.ok_or_else(|| {
        webbluetooth_core::Error::Network("the L2CAP channel had no usable streams".into())
    })?;
    Ok(L2capChannel::new(channel, sink, incoming))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn the_stream_delegate_class_registers() {
        // Proof the NSStreamDelegate conformance can be synthesised, which is
        // what the pump depends on. No radio needed.
        let class = stream_delegate_class();
        assert!(!class.is_null());
        // A second call returns the same registered class.
        assert_eq!(class, stream_delegate_class());
    }

    #[test]
    // Reaches into the Objective-C runtime, which Miri cannot call.
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn the_default_runloop_mode_resolves() {
        // `CFRunLoopRunInMode` with a null mode would spin hot.
        assert!(!unsafe { kCFRunLoopDefaultMode }.is_null());
    }

    #[test]
    // Reaches into the Objective-C runtime, which Miri cannot call.
    #[cfg_attr(miri, ignore = "calls into the Objective-C runtime")]
    fn nsstream_classes_are_present() {
        assert!(!objc::class(c"NSInputStream").is_null());
        assert!(!objc::class(c"NSOutputStream").is_null());
        assert!(!objc::class(c"NSRunLoop").is_null());
        assert!(!global_nsstring(c"NSDefaultRunLoopMode").is_null());
    }
}
