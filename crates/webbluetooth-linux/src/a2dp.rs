//! Linux A2DP media transport over an established Classic L2CAP channel.

use crate::l2cap_classic::ClassicL2capChannel;
use futures_core::Stream;
use webbluetooth_core::{AvdtpSignalingPacket, Result, SbcConfiguration, SbcMediaPacket};

/// An A2DP SBC media session over an already-established channel.
///
/// AVDTP signaling, stream discovery, RTP reordering, and fragmented SBC
/// reassembly remain outside this transport wrapper.
pub struct A2dpSession {
    channel: ClassicL2capChannel,
    configuration: SbcConfiguration,
}

impl A2dpSession {
    /// Create a session with the negotiated SBC configuration.
    pub fn new(channel: ClassicL2capChannel, configuration: SbcConfiguration) -> Self {
        Self {
            channel,
            configuration,
        }
    }

    /// Send a complete SBC media packet.
    pub fn send(&self, packet: &SbcMediaPacket) -> Result<()> {
        let bytes = packet
            .encode()
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        self.channel.send(&bytes)
    }

    /// Decode a complete SBC media packet received from the peer.
    pub fn parse(&self, bytes: &[u8]) -> Result<SbcMediaPacket> {
        SbcMediaPacket::decode(bytes, &self.configuration)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))
    }

    /// Send one complete AVDTP signaling packet.
    pub fn send_signaling(&self, packet: &AvdtpSignalingPacket) -> Result<()> {
        let bytes = packet
            .encode()
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        self.channel.send(&bytes)
    }

    /// Decode one complete AVDTP signaling packet from the peer.
    pub fn parse_signaling(&self, bytes: &[u8]) -> Result<AvdtpSignalingPacket> {
        AvdtpSignalingPacket::decode(bytes)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))
    }

    /// Take raw incoming media bytes from the underlying channel.
    pub fn take_incoming(&self) -> Option<impl Stream<Item = Vec<u8>>> {
        self.channel.take_incoming()
    }

    /// Access the underlying Classic L2CAP channel.
    pub fn channel(&self) -> &ClassicL2capChannel {
        &self.channel
    }

    /// Close the session.
    pub fn close(&self) {
        self.channel.close();
    }
}
