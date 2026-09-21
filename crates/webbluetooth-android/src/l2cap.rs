//! L2CAP channels over `BluetoothSocket`.
//!
//! `BluetoothDevice.createL2capChannel(psm)` hands back a socket whose streams
//! are ordinary `java.io.InputStream` / `OutputStream` — so unlike Apple, where
//! the streams are runloop-driven, this needs only a reader thread.
//!
//! API 29 and newer. Below that the call does not exist and the channel cannot
//! be opened at all.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ble;
use crate::bluetooth::Ref;
use crate::runtime::Runtime;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use webbluetooth_core::l2cap::{ChannelSink, Closed, Psm};

struct Shared {
    socket: Ref,
    output: Ref,
    closed: Mutex<Option<Closed>>,
    stopped: AtomicBool,
}

/// An open L2CAP channel.
pub struct Channel {
    shared: Arc<Shared>,
    psm: Psm,
    peer: String,
    /// Serialises writes, so a partial one cannot interleave with another.
    writing: Mutex<()>,
}

impl Channel {
    /// Take ownership of a connected `BluetoothSocket`.
    pub fn adopt(
        socket: crate::jni::JObject,
        psm: Psm,
        peer: String,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        let runtime = Runtime::get().ok_or("the Android runtime is not started")?;
        let env = runtime.env().map_err(|e| e.to_string())?;

        // `connect()` blocks while the channel comes up.
        ble::socket_connect(env, socket).map_err(|e| format!("could not connect: {e}"))?;

        let input = ble::socket_input_stream(env, socket).map_err(|e| e.to_string())?;
        let output = ble::socket_output_stream(env, socket).map_err(|e| e.to_string())?;
        let (Some(socket), Some(input), Some(output)) = (
            Ref::new(env, socket),
            Ref::new(env, input),
            Ref::new(env, output),
        ) else {
            return Err("the socket had no usable streams".into());
        };

        let shared = Arc::new(Shared {
            socket,
            output,
            closed: Mutex::new(None),
            stopped: AtomicBool::new(false),
        });
        spawn_reader(shared.clone(), input, sink, psm);

        Ok(Self {
            shared,
            psm,
            peer,
            writing: Mutex::new(()),
        })
    }

    /// Adopt a socket from `BluetoothServerSocket.accept()`.
    ///
    /// Already connected, so unlike [`Channel::adopt`] this must not call
    /// `connect()` — doing so on an accepted socket throws.
    pub fn adopt_accepted(
        socket: crate::jni::JObject,
        psm: Psm,
        sink: Arc<dyn ChannelSink>,
    ) -> Result<Self, String> {
        let runtime = Runtime::get().ok_or("the Android runtime is not started")?;
        let env = runtime.env().map_err(|e| e.to_string())?;

        let peer = ble::call_object(
            env,
            socket,
            "getRemoteDevice",
            "()Landroid/bluetooth/BluetoothDevice;",
            &[],
        )
        .ok()
        .and_then(|d| ble::device_address(env, d))
        .unwrap_or_default();

        let input = ble::socket_input_stream(env, socket).map_err(|e| e.to_string())?;
        let output = ble::socket_output_stream(env, socket).map_err(|e| e.to_string())?;
        let (Some(socket), Some(input), Some(output)) = (
            Ref::new(env, socket),
            Ref::new(env, input),
            Ref::new(env, output),
        ) else {
            return Err("the accepted socket had no usable streams".into());
        };

        let shared = Arc::new(Shared {
            socket,
            output,
            closed: Mutex::new(None),
            stopped: AtomicBool::new(false),
        });
        spawn_reader(shared.clone(), input, sink, psm);
        Ok(Self {
            shared,
            psm,
            peer,
            writing: Mutex::new(()),
        })
    }

    pub fn psm(&self) -> Psm {
        self.psm
    }
    pub fn peer_id(&self) -> &str {
        &self.peer
    }

    pub fn send(&self, bytes: &[u8]) -> Result<(), Closed> {
        if let Some(reason) = self.closed() {
            return Err(reason);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let _order = self.writing.lock().unwrap();
        let Some(runtime) = Runtime::get() else {
            return Err(Closed::Error("the Android runtime went away".into()));
        };
        let Ok(env) = runtime.env() else {
            return Err(Closed::Error("could not attach to the VM".into()));
        };
        ble::stream_write(env, self.shared.output.as_ptr(), bytes).map_err(|e| {
            let reason = Closed::Error(format!("write failed: {e}"));
            *self.shared.closed.lock().unwrap() = Some(reason.clone());
            reason
        })
    }

    /// Always zero: writes go straight to the stream, not an internal queue.
    pub fn pending_bytes(&self) -> usize {
        0
    }

    pub fn closed(&self) -> Option<Closed> {
        self.shared.closed.lock().unwrap().clone()
    }

    pub fn close(&self) {
        {
            let mut closed = self.shared.closed.lock().unwrap();
            if closed.is_none() {
                *closed = Some(Closed::Locally);
            }
        }
        self.shared.stopped.store(true, Ordering::Release);
        // Closing the socket is what wakes the reader out of its blocking read.
        if let Some(env) = Runtime::get().and_then(|r| r.env().ok()) {
            let _ = ble::socket_close(env, self.shared.socket.as_ptr());
        }
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
            .field("peer", &self.peer)
            .field("closed", &self.closed())
            .finish()
    }
}

fn spawn_reader(shared: Arc<Shared>, input: Ref, sink: Arc<dyn ChannelSink>, psm: Psm) {
    std::thread::Builder::new()
        .name(format!("webbluetooth-l2cap-{psm}"))
        .spawn(move || {
            let Some(runtime) = Runtime::get() else {
                return;
            };
            // The reader is our own thread, so it has to attach itself.
            let Ok(env) = runtime.env() else { return };

            let reason = loop {
                if shared.stopped.load(Ordering::Acquire) {
                    break Closed::Locally;
                }
                match ble::stream_read(env, input.as_ptr(), 4096) {
                    Ok(Some(bytes)) if !bytes.is_empty() => sink.on_bytes(bytes),
                    Ok(Some(_)) => continue,
                    Ok(None) => break Closed::ByPeer,
                    Err(e) => {
                        if shared.stopped.load(Ordering::Acquire) {
                            break Closed::Locally;
                        }
                        break Closed::Error(e.to_string());
                    }
                }
            };

            {
                let mut closed = shared.closed.lock().unwrap();
                if closed.is_none() {
                    *closed = Some(reason.clone());
                }
            }
            sink.on_closed(reason);
            runtime.vm().detach();
        })
        .expect("could not start the L2CAP reader thread");
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

/// Adopt a connected `BluetoothSocket` — L2CAP arrives here as a socket with
/// ordinary Java streams.
pub fn from_socket(socket: crate::jni::JObject) -> webbluetooth_core::Result<L2capChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::adopt(socket, 0, String::new(), channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(L2capChannel::new(channel, sink, incoming))
}

/// Adopt a socket returned by `BluetoothServerSocket.accept()`, which is
/// already connected.
pub fn from_accepted(
    socket: crate::jni::JObject,
    psm: Psm,
) -> webbluetooth_core::Result<L2capChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::adopt_accepted(socket, psm, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(L2capChannel::new(channel, sink, incoming))
}
