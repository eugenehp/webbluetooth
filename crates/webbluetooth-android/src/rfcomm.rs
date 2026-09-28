//! RFCOMM channels over Android `BluetoothSocket`.

use crate::bluetooth::Ref;
use crate::l2cap::Channel;
use crate::runtime::Runtime;
use futures_channel::mpsc;
use futures_core::Stream;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
pub use webbluetooth_core::l2cap::{ChannelSink, Closed, Incoming};

/// An Android RFCOMM byte stream.
pub type RfcommChannel = webbluetooth_core::l2cap::L2capChannel<Channel>;

/// Adopt and connect an RFCOMM `BluetoothSocket`.
pub fn from_socket(
    socket: crate::jni::JObject,
    peer: String,
) -> webbluetooth_core::Result<RfcommChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel =
        Channel::adopt(socket, 0, peer, channel_sink).map_err(webbluetooth_core::Error::Network)?;
    Ok(RfcommChannel::new(channel, sink, incoming))
}

/// An Android RFCOMM server listener.
pub struct RfcommListener {
    server: Ref,
    stopped: Arc<AtomicBool>,
    incoming: mpsc::UnboundedReceiver<webbluetooth_core::Result<RfcommChannel>>,
}

impl Stream for RfcommListener {
    type Item = webbluetooth_core::Result<RfcommChannel>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming).poll_next(cx)
    }
}

impl RfcommListener {
    /// Create a listener from an Android `BluetoothServerSocket`.
    pub fn new(server: Ref, channel: u8) -> Result<Self, String> {
        let stopped = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::unbounded();
        let accept_server = server.clone();
        let accept_stop = stopped.clone();
        std::thread::Builder::new()
            .name(format!("webbluetooth-android-rfcomm-{channel}"))
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
                    let result = from_accepted(socket, channel);
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

impl Drop for RfcommListener {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(runtime) = Runtime::get() {
            if let Ok(env) = runtime.env() {
                let _ = crate::ble::server_socket_close(env, self.server.as_ptr());
            }
        }
    }
}

pub fn from_accepted(
    socket: crate::jni::JObject,
    channel: u8,
) -> webbluetooth_core::Result<RfcommChannel> {
    let (channel_sink, sink, incoming) = webbluetooth_core::l2cap::sink();
    let channel = Channel::adopt_accepted(socket, channel as u16, channel_sink)
        .map_err(webbluetooth_core::Error::Network)?;
    Ok(RfcommChannel::new(channel, sink, incoming))
}
