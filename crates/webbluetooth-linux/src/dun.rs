//! Linux DUN AT session over an SDP-resolved RFCOMM channel.

use crate::rfcomm::RfcommChannel;
use futures_core::Stream;
use webbluetooth_core::{AtLine, AtParser, DunState, Result};

/// A Linux Dial-up Networking AT session.
pub struct DunSession {
    channel: RfcommChannel,
    parser: AtParser,
    state: DunState,
}

impl DunSession {
    /// Create a session around a connected RFCOMM channel.
    pub fn new(channel: RfcommChannel) -> Self {
        Self {
            channel,
            parser: AtParser::default(),
            state: DunState::Idle,
        }
    }

    /// Begin modem negotiation with the standard attention command.
    pub fn start(&mut self) -> Result<()> {
        self.state = DunState::Negotiating;
        self.send_command("AT")
    }

    /// Send an AT command.
    pub fn send_command(&self, command: &str) -> Result<()> {
        let bytes = webbluetooth_core::encode_command(command)
            .map_err(|error| webbluetooth_core::Error::InvalidModification(error.to_string()))?;
        self.channel.send(&bytes)
    }

    /// Dial a phone number and transition into negotiation.
    pub fn dial(&mut self, number: &str) -> Result<()> {
        self.state = DunState::Negotiating;
        self.send_command(&format!("ATD{number};"))
    }

    /// Hang up the current modem call and return to idle.
    pub fn hang_up(&mut self) -> Result<()> {
        self.send_command("ATH")?;
        self.state = DunState::Idle;
        Ok(())
    }

    /// Send payload bytes after the modem enters the connected state.
    pub fn send_data(&self, data: &[u8]) -> Result<()> {
        if self.state != DunState::Connected {
            return Err(webbluetooth_core::Error::InvalidState(
                "DUN data cannot be sent before CONNECT".into(),
            ));
        }
        self.channel.send(data)
    }

    /// Parse incoming AT bytes and update the DUN state.
    pub fn parse_bytes(&mut self, bytes: &[u8]) -> Vec<AtLine> {
        let lines = self.parser.feed(bytes);
        for line in &lines {
            // A modem confirms the link either by answering the dial string
            // with CONNECT or, if it was still negotiating, with a bare OK.
            if matches!(line, AtLine::Command(command) if command == "CONNECT")
                || (matches!(line, AtLine::Ok) && self.state == DunState::Negotiating)
            {
                self.state = DunState::Connected;
            } else if matches!(line, AtLine::Command(command) if command == "NO CARRIER") {
                self.state = DunState::Idle;
            } else if matches!(line, AtLine::Error) {
                self.state = DunState::Failed;
            }
        }
        lines
    }

    /// Current DUN session state.
    pub fn state(&self) -> DunState {
        self.state
    }

    /// Take raw incoming AT/data bytes.
    pub fn take_incoming(&self) -> Option<impl Stream<Item = Vec<u8>>> {
        self.channel.take_incoming()
    }

    /// Access the underlying RFCOMM channel.
    pub fn channel(&self) -> &RfcommChannel {
        &self.channel
    }

    /// Close the DUN session.
    pub fn close(&self) {
        self.channel.close();
    }
}
