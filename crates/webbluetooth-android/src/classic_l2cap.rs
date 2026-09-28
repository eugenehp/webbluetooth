//! Bluetooth Classic L2CAP channels over Android `BluetoothSocket`.

use crate::bluetooth::Ref;
use crate::l2cap::Channel;
use crate::runtime::Runtime;
use futures_channel::mpsc;
use futures_core::Stream;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use webbluetooth_core::l2cap::{ChannelSink, Closed, Incoming};

/// An Android Bluetooth Classic L2CAP byte stream.
pub type ClassicL2capChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

/// Adopt and connect a Classic L2CAP socket.
pub fn from_socket(
    socket: crate::jni::JObject,
    psm: u16,
    peer: String,
) -> webbluetooth_core::Result<ClassicL2capChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::adopt(socket, psm, peer, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(ClassicL2capChannel::new(channel, sink, incoming))
}

/// An Android Classic L2CAP server listener.
pub struct ClassicL2capListener {
    server: Ref,
    stopped: Arc<AtomicBool>,
    incoming: mpsc::UnboundedReceiver<webbluetooth_core::Result<ClassicL2capChannel>>,
}

impl Stream for ClassicL2capListener {
    type Item = webbluetooth_core::Result<ClassicL2capChannel>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming).poll_next(cx)
    }
}

impl ClassicL2capListener {
    /// Create a listener from an Android `BluetoothServerSocket`.
    pub fn new(server: Ref, psm: u16) -> Result<Self, String> {
        let stopped = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::unbounded();
        let accept_server = server.clone();
        let accept_stop = stopped.clone();
        std::thread::Builder::new()
            .name(format!("webbluetooth-android-l2cap-{psm}"))
            .spawn(move || {
                let Some(runtime) = Runtime::get() else {
                    return;
                };
                let Ok(env) = runtime.env() else {
                    return;
                };
                while !accept_stop.load(Ordering::Acquire) {
                    let Ok(socket) = crate::ble::server_socket_accept(env, accept_server.as_ptr())
                    else {
                        break;
                    };
                    if socket.is_null() {
                        break;
                    }
                    let result = from_accepted(socket, psm);
                    if tx.unbounded_send(result).is_err() {
                        break;
                    }
                }
                runtime.vm().detach();
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            server,
            stopped,
            incoming: rx,
        })
    }
}

impl Drop for ClassicL2capListener {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(runtime) = Runtime::get() {
            if let Ok(env) = runtime.env() {
                let _ = crate::ble::server_socket_close(env, self.server.as_ptr());
            }
        }
    }
}

/// Adopt a socket accepted by a Classic L2CAP server.
pub fn from_accepted(
    socket: crate::jni::JObject,
    psm: u16,
) -> webbluetooth_core::Result<ClassicL2capChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::adopt_accepted(socket, psm, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(ClassicL2capChannel::new(channel, sink, incoming))
}
