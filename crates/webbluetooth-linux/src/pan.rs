//! Linux PAN session over a Classic L2CAP BNEP channel.

use crate::l2cap_classic::ClassicL2capChannel;
use futures_core::Stream;
use webbluetooth_core::{PanControlMessage, PanFrame, Result};

/// A Linux PAN/BNEP session.
pub struct PanSession {
    channel: ClassicL2capChannel,
}

impl PanSession {
    /// Create a session around a connected BNEP channel.
    pub fn new(channel: ClassicL2capChannel) -> Self {
        Self { channel }
    }
    /// Send an Ethernet frame over BNEP.
    pub fn send(&self, frame: &PanFrame) -> Result<()> {
        let bytes = frame
            .encode()
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        self.channel.send(&bytes)
    }

    /// Send a BNEP control message.
    pub fn send_control_message(&self, message: &PanControlMessage) -> Result<()> {
        let mut bytes = vec![0x10];
        bytes.extend(
            message.encode().map_err(|error| {
                webbluetooth_core::Error::InvalidModification(error.to_string())
            })?,
        );
        self.channel.send(&bytes)
    }

    /// Decode a BNEP control message received from the peer.
    pub fn parse_control_message(&self, bytes: &[u8]) -> Result<PanControlMessage> {
        if bytes.first().copied() != Some(0x10) {
            return Err(webbluetooth_core::Error::InvalidModification(
                "not a BNEP control packet".into(),
            ));
        }
        PanControlMessage::decode(&bytes[1..])
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))
    }
    /// Take raw BNEP frame bytes.
    pub fn take_incoming(&self) -> Option<impl Stream<Item = Vec<u8>>> {
        self.channel.take_incoming()
    }
    /// Access the underlying channel.
    pub fn channel(&self) -> &ClassicL2capChannel {
        &self.channel
    }
    /// Close the session.
    pub fn close(&self) {
        self.channel.close();
    }
}
