//! Linux HFP/HSP RFCOMM session wrapper.

use crate::rfcomm::RfcommChannel;
use futures_core::Stream;
use webbluetooth_core::{AtLine, AtParser, CallCommand, CallInfo, CallState, Result};

/// A Linux HFP/HSP AT session over RFCOMM.
pub struct HfpSession {
    channel: RfcommChannel,
    parser: AtParser,
}

impl HfpSession {
    /// Create a session around an already connected RFCOMM channel.
    pub fn new(channel: RfcommChannel) -> Self {
        Self {
            channel,
            parser: AtParser::default(),
        }
    }

    /// Send an AT command.
    pub fn send_command(&self, command: &str) -> Result<()> {
        let bytes = webbluetooth_core::encode_command(command)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        self.channel.send(&bytes)
    }

    /// Send a typed HFP call-control command.
    pub fn send_call_command(&self, command: CallCommand) -> Result<()> {
        self.channel.send(
            &command.encode().map_err(|error| {
                webbluetooth_core::Error::InvalidModification(error.to_string())
            })?,
        )
    }

    /// Parse incoming bytes and update the call state.
    pub fn parse_bytes(&mut self, bytes: &[u8]) -> Vec<AtLine> {
        self.parser.feed(bytes)
    }

    /// Parse current-call entries from an AT input fragment.
    pub fn parse_call_list(&mut self, bytes: &[u8]) -> Vec<CallInfo> {
        self.parse_bytes(bytes)
            .into_iter()
            .filter_map(|line| match line {
                AtLine::CurrentCall(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    /// Current parsed call state.
    pub fn call_state(&self) -> CallState {
        self.parser.call_state()
    }

    /// Access the underlying RFCOMM stream.
    pub fn channel(&self) -> &RfcommChannel {
        &self.channel
    }

    /// Take raw incoming RFCOMM bytes for application-driven parsing.
    pub fn take_incoming(&self) -> Option<impl Stream<Item = Vec<u8>>> {
        self.channel.take_incoming()
    }

    /// Close the HFP/HSP session.
    pub fn close(&self) {
        self.channel.close();
    }
}
