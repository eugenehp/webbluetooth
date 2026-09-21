//! L2CAP connection-oriented channels.
//!
//! The one place CoreBluetooth stops being request/response. Instead of
//! attributes you get a bidirectional byte pipe with no 512-byte ceiling and no
//! ATT round trip per write — the right tool for firmware images, audio, or
//! anything else that would otherwise be thousands of characteristic writes.
//!
//! Neither side is Web Bluetooth; the specification has no L2CAP.
//!
//! ```no_run
//! # use webbluetooth::stream::StreamExt;
//! # async fn example(device: webbluetooth::BluetoothDevice) -> webbluetooth::Result<()> {
//! let channel = device.open_l2cap_channel(0x0080).await?;
//! let mut incoming = channel.take_incoming().expect("first call");
//!
//! channel.send(b"hello")?;
//! while let Some(chunk) = incoming.next().await {
//!     println!("{} bytes", chunk.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! **A chunk is not a message.** L2CAP CoC is a byte stream: what arrives in
//! one chunk is whatever one read returned. Frame it yourself.

//! # Where the pieces are
//!
//! The channel type itself is [`webbluetooth_core::l2cap`], which is where the
//! backlog, the async stream and the close notification live — they are the
//! same on every platform, and were written three times over before they were
//! written once. What is here is the choice of *which* platform socket fills
//! it in, and the step that turns whatever a backend hands over into a
//! channel.

use crate::error::Result;

#[cfg(target_os = "android")]
use webbluetooth_android::l2cap as platform;
#[cfg(target_vendor = "apple")]
use webbluetooth_apple::l2cap as platform;
#[cfg(target_os = "linux")]
use webbluetooth_linux::l2cap as platform;

pub use platform::L2capChannel;
pub use webbluetooth_core::l2cap::{Closed, Incoming, Psm};

/// Build a channel from whatever the backend had to hand over.
///
/// One name, three platforms. A backend returns the raw thing its stack
/// produced — a `CBL2CAPChannel` from Apple, an address to connect to on
/// Linux, an already-connected socket on Android — because opening a channel
/// is the only part of this that is platform work. This is where that becomes
/// a channel.
#[cfg(target_vendor = "apple")]
pub(crate) fn from_target(target: webbluetooth_apple::Retained, _psm: Psm) -> Result<L2capChannel> {
    // SAFETY: `target` came from a `didOpenL2CAPChannel:` callback.
    unsafe { platform::adopt(target) }
}

/// See [`from_target`].
#[cfg(target_os = "linux")]
pub(crate) fn from_target(target: (String, String), psm: Psm) -> Result<L2capChannel> {
    platform::open(&target.0, &target.1, psm)
}

/// See [`from_target`].
#[cfg(target_os = "android")]
pub(crate) fn from_target(
    target: webbluetooth_android::jni::JObject,
    _psm: Psm,
) -> Result<L2capChannel> {
    platform::from_socket(target)
}
