//! Linux AVRCP pass-through transport over RFCOMM/AVCTP.

use crate::rfcomm::RfcommChannel;
use futures_core::Stream;
use webbluetooth_core::{AvrcpPassThrough, Result};

/// A Linux AVRCP pass-through session.
pub struct AvrcpSession {
    channel: RfcommChannel,
}

impl AvrcpSession {
    /// Create a session around a connected RFCOMM channel.
    pub fn new(channel: RfcommChannel) -> Self {
        Self { channel }
    }

    /// Send a pass-through command frame.
    pub fn send(&self, command: AvrcpPassThrough) -> Result<()> {
        let frame = command
            .encode()
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        self.channel.send(&frame)
    }

    /// Decode an incoming pass-through frame.
    pub fn parse(&self, bytes: &[u8]) -> Result<AvrcpPassThrough> {
        AvrcpPassThrough::decode(bytes)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))
    }

    /// Take raw incoming AVCTP frames for application-level event handling.
    pub fn take_incoming(&self) -> Option<impl Stream<Item = Vec<u8>>> {
        self.channel.take_incoming()
    }

    /// Access the underlying RFCOMM channel.
    pub fn channel(&self) -> &RfcommChannel {
        &self.channel
    }

    /// Close the session.
    pub fn close(&self) {
        self.channel.close();
    }
}
