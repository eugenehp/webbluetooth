//! Linux OBEX session over an SDP-resolved RFCOMM channel.

use crate::rfcomm::RfcommChannel;
use futures_core::Stream;
use webbluetooth_core::{parse_obex_header, ObexHeader, ObexPacket, Result};

/// A Linux OBEX session.
pub struct ObexSession {
    channel: RfcommChannel,
}

impl ObexSession {
    /// Create a session around a connected RFCOMM channel.
    pub fn new(channel: RfcommChannel) -> Self {
        Self { channel }
    }

    /// Send an encoded OBEX packet.
    pub fn send(&self, packet: &ObexPacket) -> Result<()> {
        self.channel.send(packet.as_bytes())
    }

    /// Send an OBEX Connect request.
    pub fn connect(&self, version: u8, flags: u8, max_packet: u16) -> Result<()> {
        self.send(&ObexPacket::connect(version, flags, max_packet))
    }

    /// Send an OBEX Get request.
    pub fn get(&self, body: &[u8]) -> Result<()> {
        let packet = ObexPacket::get(body)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        self.send(&packet)
    }

    /// Send an OBEX Put request.
    pub fn put(&self, body: &[u8]) -> Result<()> {
        let packet = ObexPacket::put(body)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        self.send(&packet)
    }

    /// Send an OBEX Disconnect request.
    pub fn disconnect(&self) -> Result<()> {
        self.send(&ObexPacket::disconnect())
    }

    /// Parse an incoming OBEX response header.
    pub fn parse_response_header(&self, bytes: &[u8]) -> Result<ObexHeader> {
        parse_obex_header(bytes)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))
    }

    /// Take raw OBEX response packets for application-level header parsing.
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
