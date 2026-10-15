//! RFCOMM over IOBluetooth — macOS only, behind the `rfcomm` feature.
//!
//! CoreBluetooth is LE-only and always will be; Bluetooth Classic on a Mac is
//! a different framework, IOBluetooth, with a different idea of what a device
//! is. That difference is the whole shape of this module:
//!
//! * **A device is an address or a name, never a CoreBluetooth identifier** —
//!   see [`crate::classic`]. A dual-mode device, LE for control and Classic
//!   for data, is bridged by its name: [`Target::Name`](crate::classic::Target::Name).
//! * **Callbacks arrive on the main thread's run loop** — the open completing,
//!   every chunk of data, the peer closing — whichever thread opened the
//!   channel. So the process has to keep that run loop turning: an app does,
//!   with `NSApplication`; a command-line program has to call `CFRunLoopRun`
//!   on its main thread and do its own work on another. Where it does not,
//!   the open fails and says so, rather than opening a channel that never
//!   delivers. (This was established against a real Linux peer; the mock in
//!   the tests delivers on the opening thread, so it tests the pump, not this.)
//! * Each channel still gets a **pump**: one thread that looks the device up,
//!   opens the channel and does every write, so a blocking `writeSync` never
//!   stalls the caller or the main thread.
//!
//! The delegate is synthesised at run time like every other one in this crate.
//! `IOBluetoothRFCOMMChannelDelegate` is an informal protocol, so there is
//! nothing to declare conformance to: the channel asks the object whether it
//! responds to each selector and calls the ones it does.
//!
//! # What macOS asks of the process
//!
//! Opening a channel fails with `kIOReturnNotPermitted` unless the binary is
//! signed with the `com.apple.security.device.bluetooth` entitlement, and the
//! device has to be paired in System Settings first: IOBluetooth will not pair
//! on a channel's behalf.

use crate::classic::{device_address, find_device};
use crate::objc::{self, global_nsstring, require_class, ClassBuilder, Id, Retained, NIL};
use core::ffi::c_void;
use futures_channel::oneshot;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use crate::classic::{paired_devices, ClassicDevice, Target};
pub use webbluetooth_core::l2cap::{ChannelSink, Closed, Incoming};

type CFRunLoopRef = *mut c_void;
type CFStringRef = *const c_void;

unsafe extern "C" {
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source: bool) -> i32;
    fn CFRunLoopStop(rl: CFRunLoopRef);
    static kCFRunLoopDefaultMode: CFStringRef;
}

/// `kIOReturnSuccess`.
const IO_RETURN_SUCCESS: i32 = 0;
/// `kIOReturnNotPermitted`, which here means the process lacks the Bluetooth
/// entitlement.
const IO_RETURN_NOT_PERMITTED: u32 = 0xe000_02bc;

/// A name for an `IOReturn` worth recognising in an error message.
fn io_return_name(code: i32) -> &'static str {
    match code as u32 {
        0x0000_0000 => "kIOReturnSuccess",
        0xe000_02bc => "kIOReturnNotPermitted",
        0xe000_02be => "kIOReturnExclusiveAccess",
        0xe000_02c0 => "kIOReturnNotAttached",
        0xe000_02c7 => "kIOReturnAborted",
        0xe000_02d8 => "kIOReturnNotOpen",
        0xe000_02ed => "kIOReturnTimeout",
        _ => "unrecognised IOReturn",
    }
}

fn io_error(what: &str, code: i32) -> String {
    let hint = if code as u32 == IO_RETURN_NOT_PERMITTED {
        "; the process needs the com.apple.security.device.bluetooth entitlement, which recent macOS \
         only honours inside a signed .app bundle"
    } else {
        ""
    };
    format!(
        "{what} ({}, 0x{:08x}){hint}",
        io_return_name(code),
        code as u32
    )
}

/// How long the open is given before it is reported as failed.
#[cfg(not(test))]
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// Shorter under test, where the mock that never answers would otherwise cost
/// ten seconds of every run.
#[cfg(test)]
const OPEN_TIMEOUT: Duration = Duration::from_secs(2);
/// How long one turn of the pump's run loop lasts when nothing happens. Also
/// the longest a queued write waits if it lands between two turns.
const TICK_SECONDS: f64 = 0.05;
/// What a write is split into when the channel reports no MTU.
const FALLBACK_MTU: u16 = 127;

// ── The pump ────────────────────────────────────────────────────────────────

struct Pump {
    channel_id: u8,
    /// Chunks waiting to go out, front first.
    outgoing: Mutex<VecDeque<Vec<u8>>>,
    sink: Arc<dyn ChannelSink>,
    closed: Mutex<Option<Closed>>,
    stop: AtomicBool,
    /// The status `rfcommChannelOpenComplete:status:` reported, once it has.
    open_status: Mutex<Option<i32>>,
    /// The largest single write the channel takes.
    mtu: AtomicU16,
    /// The pump thread's runloop, so a writer can interrupt its wait.
    runloop: AtomicPtr<c_void>,
}

impl Pump {
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

    /// End the run loop's current turn, so the pump looks at its queue now.
    fn wake(&self) {
        let rl = self.runloop.load(Ordering::Acquire);
        if !rl.is_null() {
            unsafe { CFRunLoopStop(rl) };
        }
    }

    /// Write everything queued. Pump thread only.
    ///
    /// # Safety
    /// `channel` must be an open `IOBluetoothRFCOMMChannel`.
    unsafe fn drain_output(&self, channel: Id) {
        let mtu = usize::from(self.mtu.load(Ordering::Acquire).max(1));
        loop {
            // Popped one at a time so the lock is not held across a write that
            // blocks until the controller has taken the bytes.
            let Some(chunk) = self.outgoing.lock().unwrap().pop_front() else {
                return;
            };
            for piece in chunk.chunks(mtu) {
                let status = unsafe {
                    crate::msg_send_t![
                        i32;
                        channel,
                        writeSync: piece.as_ptr() as *mut c_void,
                        length: piece.len() as u16
                    ]
                };
                if status != IO_RETURN_SUCCESS {
                    self.finish(Closed::Error(io_error("RFCOMM write failed", status)));
                    return;
                }
            }
        }
    }
}

// ── IOBluetoothRFCOMMChannelDelegate, synthesised ───────────────────────────

static PUMPS: OnceLock<Mutex<HashMap<usize, Arc<Pump>>>> = OnceLock::new();

fn pumps() -> &'static Mutex<HashMap<usize, Arc<Pump>>> {
    PUMPS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pump_for(delegate: Id) -> Option<Arc<Pump>> {
    pumps()
        .lock()
        .ok()
        .and_then(|m| m.get(&(delegate as usize)).cloned())
}

static DELEGATE_CLASS: OnceLock<usize> = OnceLock::new();

fn delegate_class() -> Id {
    *DELEGATE_CLASS.get_or_init(|| {
        for attempt in 0..64u32 {
            let name = if attempt == 0 {
                c"WBRustRFCOMMChannelDelegate".to_owned()
            } else {
                std::ffi::CString::new(format!("WBRustRFCOMMChannelDelegate{attempt}"))
                    .expect("class name")
            };
            let Some(builder) = (unsafe { ClassBuilder::new(c"NSObject", &name) }) else {
                continue;
            };
            // SAFETY: each implementation matches its encoding — an object, a
            // `void *` and a `size_t`; an object and an `IOReturn`; an object.
            return unsafe {
                builder
                    .method(
                        c"rfcommChannelData:data:length:",
                        on_data as *const c_void,
                        c"v@:@^vQ",
                    )
                    .method(
                        c"rfcommChannelOpenComplete:status:",
                        on_open_complete as *const c_void,
                        c"v@:@i",
                    )
                    .method(c"rfcommChannelClosed:", on_closed as *const c_void, c"v@:@")
                    .register()
            } as usize;
        }
        panic!("could not register an RFCOMM delegate class after 64 attempts");
    }) as Id
}

unsafe extern "C" fn on_data(
    this: Id,
    _cmd: *const c_void,
    _channel: Id,
    data: *const u8,
    length: usize,
) {
    if data.is_null() || length == 0 {
        return;
    }
    if let Some(pump) = pump_for(this) {
        // The buffer is IOBluetooth's and is reused after this returns.
        let bytes = unsafe { core::slice::from_raw_parts(data, length) }.to_vec();
        pump.sink.on_bytes(bytes);
    }
}

unsafe extern "C" fn on_open_complete(this: Id, _cmd: *const c_void, _channel: Id, status: i32) {
    if let Some(pump) = pump_for(this) {
        *pump.open_status.lock().unwrap() = Some(status);
    }
}

unsafe extern "C" fn on_closed(this: Id, _cmd: *const c_void, _channel: Id) {
    if let Some(pump) = pump_for(this) {
        // A no-op if this side closed it: the reason is already recorded.
        pump.finish(Closed::ByPeer);
    }
}

// ── Channel ─────────────────────────────────────────────────────────────────

/// An open RFCOMM byte stream.
///
/// Dropping this closes the channel and stops its pump thread.
pub struct Channel {
    channel: u8,
    peer: String,
    pump: Arc<Pump>,
}

impl Channel {
    /// The RFCOMM server channel number.
    pub fn channel(&self) -> u8 {
        self.channel
    }

    /// The peer's Bluetooth address.
    pub fn peer_id(&self) -> &str {
        &self.peer
    }

    /// Queue bytes for sending.
    ///
    /// Returns once the bytes are queued, not once they are on the air.
    pub fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        if let Some(reason) = self.closed() {
            return Err(reason);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.pump.outgoing.lock().unwrap().push_back(bytes.to_vec());
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

    /// Why the stream closed, or `None` while it is open.
    pub fn closed(&self) -> Option<Closed> {
        self.pump.closed.lock().unwrap().clone()
    }

    /// Close the stream.
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
        f.debug_struct("RfcommChannel")
            .field("channel", &self.channel)
            .field("peer", &self.peer)
            .field("closed", &self.closed())
            .finish()
    }
}

impl webbluetooth_core::l2cap::PlatformChannel for Channel {
    fn psm(&self) -> u16 {
        u16::from(self.channel)
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

/// The portable async wrapper around an IOBluetooth RFCOMM channel.
pub type RfcommChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

/// What the pump reports once the channel is open: the peer's address.
type Opened = oneshot::Sender<Result<String, String>>;

/// Where the pump gets its `IOBluetoothDevice`.
enum Source {
    /// Looked up by IOBluetooth, on the pump thread.
    Target(Target),
    /// Handed in ready-made: the seam the mock device in the tests uses.
    #[cfg(test)]
    Device(Retained),
}

/// One thread per channel: opens it, runs the runloop, does all I/O.
fn spawn_pump(pump: Arc<Pump>, source: Source, opened: Opened) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(format!("webbluetooth-rfcomm-{}", pump.channel_id))
        .spawn(move || {
            let _pool = objc::AutoreleasePool::new();
            unsafe { run_pump(&pump, source, opened) };
            pump.runloop.store(std::ptr::null_mut(), Ordering::Release);
        })
        .map(|_| ())
}

/// # Safety
/// Must run on a thread of its own, inside an autorelease pool.
unsafe fn run_pump(pump: &Arc<Pump>, source: Source, opened: Opened) {
    let fail = |opened: Opened, message: String| {
        pump.finish(Closed::Error(message.clone()));
        let _ = opened.send(Err(message));
    };

    let found = match source {
        Source::Target(target) => unsafe { find_device(&target) },
        #[cfg(test)]
        Source::Device(device) => Ok(device),
    };
    let device = match found {
        Ok(device) => device,
        Err(message) => return fail(opened, message),
    };

    // A delegate object per channel, so the callbacks can find this pump.
    let delegate = unsafe {
        let obj: Id = crate::msg_send![delegate_class(), alloc];
        let obj: Id = crate::msg_send![obj, init];
        match Retained::adopt(obj) {
            Some(delegate) => delegate,
            None => return fail(opened, "could not create the RFCOMM delegate".into()),
        }
    };
    pumps().lock().unwrap().insert(delegate.key(), pump.clone());

    // A run loop with no input source returns at once instead of waiting, so
    // it is given a port to hold it open. IOBluetooth adds its own sources
    // when the channel opens; this covers the time before it has.
    let (run_loop, mode, port) = unsafe {
        let run_loop: Id = crate::msg_send![require_class(c"NSRunLoop"), currentRunLoop];
        let mode = global_nsstring(c"NSDefaultRunLoopMode");
        let port: Id = crate::msg_send![require_class(c"NSMachPort"), port];
        if !run_loop.is_null() && !mode.is_null() && !port.is_null() {
            crate::msg_send_void![run_loop, addPort: port, forMode: mode];
        }
        (run_loop, mode, Retained::retain(port))
    };
    pump.runloop
        .store(unsafe { CFRunLoopGetCurrent() }, Ordering::Release);

    let channel = unsafe { open_channel(pump, &device, &delegate, opened) };
    if let Some(channel) = &channel {
        unsafe {
            while !pump.stop.load(Ordering::Acquire) {
                // Returns when a source fires, when `wake` stops it, or after
                // one tick — whichever is first.
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, TICK_SECONDS, true);
                if pump.stop.load(Ordering::Acquire) {
                    break;
                }
                pump.drain_output(channel.as_ptr());
                if !crate::msg_send_t![bool; channel.as_ptr(), isOpen] {
                    pump.finish(Closed::ByPeer);
                }
            }
            // Detached first, so closing does not call back into a pump that
            // is about to be forgotten.
            crate::msg_send_void![channel.as_ptr(), setDelegate: NIL];
            let _: i32 = crate::msg_send_t![i32; channel.as_ptr(), closeChannel];
        }
    }

    pumps().lock().unwrap().remove(&delegate.key());
    if let Some(port) = port {
        if !run_loop.is_null() && !mode.is_null() {
            unsafe { crate::msg_send_void![run_loop, removePort: port.as_ptr(), forMode: mode] };
        }
    }
}

/// Open the channel and wait for the delegate to say it is up.
///
/// Resolves `opened` either way. `None` means the pump has nothing to run.
///
/// # Safety
/// Pump thread only; `device` must be an `IOBluetoothDevice`.
unsafe fn open_channel(
    pump: &Arc<Pump>,
    device: &Retained,
    delegate: &Retained,
    opened: Opened,
) -> Option<Retained> {
    let fail = |opened: Opened, message: String| -> Option<Retained> {
        pump.finish(Closed::Error(message.clone()));
        let _ = opened.send(Err(message));
        None
    };

    unsafe {
        // The channel open brings the baseband link up itself, but doing it
        // first separates "the device is not there" from "the device refused
        // the channel". Its result is not fatal: the open below reports.
        if !crate::msg_send_t![bool; device.as_ptr(), isConnected] {
            let _: i32 = crate::msg_send_t![i32; device.as_ptr(), openConnection];
        }

        // The asynchronous open, deliberately. The synchronous one returns an
        // open channel whose delegate is never called with data.
        let mut raw: Id = NIL;
        let status = crate::msg_send_t![
            i32;
            device.as_ptr(),
            openRFCOMMChannelAsync: &mut raw as *mut Id,
            withChannelID: pump.channel_id,
            delegate: delegate.as_ptr()
        ];
        if status != IO_RETURN_SUCCESS {
            let what = format!("could not open RFCOMM channel {}", pump.channel_id);
            return fail(opened, io_error(&what, status));
        }
        // Returned unowned; held for as long as the pump runs.
        let Some(channel) = Retained::retain(raw) else {
            return fail(opened, "IOBluetooth returned no RFCOMM channel".into());
        };

        let deadline = Instant::now() + OPEN_TIMEOUT;
        loop {
            if CFRunLoopRunInMode(kCFRunLoopDefaultMode, TICK_SECONDS, true) == RUN_FINISHED {
                // No sources at all: do not spin while waiting.
                std::thread::sleep(Duration::from_millis(10));
            }
            // The open status first: a peer that opens, sends and hangs up at
            // once delivers all three in one turn, and its data is still owed
            // to the caller — on a channel that is already closed.
            let status = *pump.open_status.lock().unwrap();
            match status {
                Some(IO_RETURN_SUCCESS) => break,
                Some(status) => {
                    let what = format!("RFCOMM channel {} did not open", pump.channel_id);
                    return fail(opened, io_error(&what, status));
                }
                None => {}
            }
            // Nobody is waiting for the channel any more, or it closed before
            // it finished opening.
            if opened.is_canceled() || pump.stop.load(Ordering::Acquire) {
                pump.finish(Closed::Locally);
                crate::msg_send_void![channel.as_ptr(), setDelegate: NIL];
                let _: i32 = crate::msg_send_t![i32; channel.as_ptr(), closeChannel];
                let reason = pump.closed.lock().unwrap().clone();
                let _ = opened.send(Err(format!(
                    "RFCOMM channel {} closed before it opened: {reason:?}",
                    pump.channel_id
                )));
                return None;
            }
            // `isOpen` alone is not enough. IOBluetooth delivers every
            // delegate callback — this one, and all incoming data — on the
            // main thread's run loop, and a channel that opens in a process
            // whose main run loop is not turning would deliver nothing
            // after it. Waiting for the callback is what finds that out.
            if Instant::now() >= deadline {
                let opened_anyway = crate::msg_send_t![bool; channel.as_ptr(), isOpen];
                let _: i32 = crate::msg_send_t![i32; channel.as_ptr(), closeChannel];
                let message = if opened_anyway {
                    format!(
                        "RFCOMM channel {} opened, but IOBluetooth never called back: it \
                         delivers RFCOMM events on the main thread's run loop, which is not \
                         running in this process. An app's is; a command-line program has to \
                         turn it (CFRunLoopRun on the main thread) and do its work elsewhere",
                        pump.channel_id
                    )
                } else {
                    format!(
                        "RFCOMM channel {} did not open within {} s",
                        pump.channel_id,
                        OPEN_TIMEOUT.as_secs()
                    )
                };
                return fail(opened, message);
            }
        }

        let mtu = crate::msg_send_t![u16; channel.as_ptr(), getMTU];
        pump.mtu
            .store(if mtu == 0 { FALLBACK_MTU } else { mtu }, Ordering::Release);

        let peer = device_address(device.as_ptr()).unwrap_or_default();
        if opened.send(Ok(peer)).is_err() {
            // The caller gave up in the instant before the answer.
            pump.finish(Closed::Locally);
        }
        Some(channel)
    }
}

/// `kCFRunLoopRunFinished`: the run loop had no sources or timers to wait on.
const RUN_FINISHED: i32 = 1;

/// Open an RFCOMM stream to `target` on a server channel.
///
/// Resolves once the channel is open. The device must already be paired.
pub async fn open(target: Target, channel: u8) -> webbluetooth_core::Result<RfcommChannel> {
    open_source(Source::Target(target), channel).await
}

async fn open_source(source: Source, channel: u8) -> webbluetooth_core::Result<RfcommChannel> {
    use webbluetooth_core::Error;

    if !(1..=30).contains(&channel) {
        return Err(Error::InvalidModification(
            "RFCOMM channel must be between 1 and 30".into(),
        ));
    }

    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let pump = Arc::new(Pump {
        channel_id: channel,
        outgoing: Mutex::new(VecDeque::new()),
        sink: channel_sink,
        closed: Mutex::new(None),
        stop: AtomicBool::new(false),
        open_status: Mutex::new(None),
        mtu: AtomicU16::new(FALLBACK_MTU),
        runloop: AtomicPtr::new(std::ptr::null_mut()),
    });

    let (opened_tx, opened_rx) = oneshot::channel();
    spawn_pump(pump.clone(), source, opened_tx)
        .map_err(|error| Error::Network(format!("could not start the RFCOMM pump: {error}")))?;

    match opened_rx.await {
        Ok(Ok(peer)) => Ok(RfcommChannel::new(
            Channel {
                channel,
                peer,
                pump,
            },
            sink,
            incoming,
        )),
        Ok(Err(message)) => Err(Error::Network(message)),
        Err(_) => Err(Error::Aborted(
            "the RFCOMM pump stopped before opening".into(),
        )),
    }
}
#[cfg(test)]
#[path = "rfcomm_mock.rs"]
mod mock;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delegate_class_registers_and_answers_its_selectors() {
        let class = delegate_class();
        assert!(!class.is_null());
        // Stable across calls: registering twice would fail.
        assert_eq!(class, delegate_class());

        // IOBluetooth asks before it calls. A selector that is not answered is
        // a callback that silently never arrives.
        for selector in [
            c"rfcommChannelData:data:length:",
            c"rfcommChannelOpenComplete:status:",
            c"rfcommChannelClosed:",
        ] {
            let responds = unsafe {
                crate::msg_send_t![bool; class, instancesRespondToSelector: objc::sel(selector)]
            };
            assert!(responds, "{selector:?}");
        }
    }

    #[test]
    fn the_channel_class_is_present() {
        assert!(!objc::class(c"IOBluetoothRFCOMMChannel").is_null());
    }

    #[test]
    fn the_default_runloop_mode_resolves() {
        assert!(!unsafe { kCFRunLoopDefaultMode }.is_null());
    }

    #[test]
    fn an_io_return_names_the_entitlement_when_that_is_the_cause() {
        let message = io_error("could not open", IO_RETURN_NOT_PERMITTED as i32);
        assert!(message.contains("kIOReturnNotPermitted"), "{message}");
        assert!(message.contains("entitlement"), "{message}");
        assert!(!io_error("could not open", 0xe000_02ed_u32 as i32).contains("entitlement"));
    }

    /// Out of range is refused before a thread is spawned or a device looked
    /// up, so it can be checked without a radio.
    #[test]
    fn a_channel_number_out_of_range_is_refused() {
        for channel in [0u8, 31] {
            let result = futures_executor::block_on(open(
                Target::Address("AA:BB:CC:DD:EE:FF".into()),
                channel,
            ));
            assert!(
                matches!(
                    result,
                    Err(webbluetooth_core::Error::InvalidModification(_))
                ),
                "channel {channel}"
            );
        }
    }

    /// A name nothing is paired under fails with a message that says so,
    /// rather than hanging until the open times out.
    #[test]
    fn an_unknown_name_fails_without_waiting_for_a_timeout() {
        let started = Instant::now();
        let result = futures_executor::block_on(open(
            Target::Name("webbluetooth-no-such-device-0f3a".into()),
            1,
        ));
        let Err(webbluetooth_core::Error::Network(message)) = result else {
            panic!("expected a NetworkError");
        };
        assert!(message.contains("no paired"), "{message}");
        assert!(started.elapsed() < OPEN_TIMEOUT / 2);
    }
}
