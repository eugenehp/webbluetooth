//! HFP/HSP AT command framing and call-state modeling.

use std::fmt;

/// Hands-Free call state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallState {
    /// No active call.
    #[default]
    Idle,
    /// An incoming call is ringing.
    Incoming,
    /// An outgoing call is being established.
    Outgoing,
    /// A call is connected.
    Connected,
}

/// A parsed HFP/HSP line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtLine {
    /// A command or command response.
    Command(String),
    /// A successful command response.
    Ok,
    /// A failed command response.
    Error,
    /// A ring indication.
    Ring,
    /// A call-state indication (`+CIEV` or `+CLCC`).
    CallState(CallState),
    /// A current-call listing entry.
    CurrentCall(CallInfo),
}

/// One HFP `+CLCC` current-call entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallInfo {
    /// Call index assigned by the gateway.
    pub index: u8,
    /// Direction: `true` for outgoing, `false` for incoming.
    pub outgoing: bool,
    /// Status encoded by HFP (`0` active through `6` waiting).
    pub status: u8,
    /// Whether the call is a multiparty call.
    pub multiparty: bool,
    /// Remote number, if supplied.
    pub number: Option<String>,
}

/// Standard HFP call-control command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallCommand {
    /// Dial a phone number.
    Dial(String),
    /// Answer the current incoming call.
    Answer,
    /// Hang up the current call.
    HangUp,
    /// Query current call list/status.
    QueryCurrentCalls,
}

impl CallCommand {
    /// Encode this command as an AT command line.
    pub fn encode(&self) -> Result<Vec<u8>, AtError> {
        match self {
            Self::Dial(number) => encode_command(&format!("ATD{number};")),
            Self::Answer => encode_command("ATA"),
            Self::HangUp => encode_command("AT+CHUP"),
            Self::QueryCurrentCalls => encode_command("AT+CLCC"),
        }
    }
}

/// Stateful HFP/HSP AT parser.
#[derive(Debug, Default)]
pub struct AtParser {
    buffer: Vec<u8>,
    call_state: CallState,
}

impl AtParser {
    /// Feed bytes and return complete parsed lines.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<AtLine> {
        self.buffer.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(position) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=position).collect();
            let text = String::from_utf8_lossy(&line).trim().to_owned();
            if !text.is_empty() {
                lines.push(self.parse_line(&text));
            }
        }
        lines
    }

    /// Current call state.
    pub fn call_state(&self) -> CallState {
        self.call_state
    }

    fn parse_line(&mut self, line: &str) -> AtLine {
        match line {
            "OK" => AtLine::Ok,
            "ERROR" => AtLine::Error,
            "RING" => {
                self.call_state = CallState::Incoming;
                AtLine::Ring
            }
            value if value.starts_with("+CIEV:") => {
                let state = value
                    .split(',')
                    .nth(1)
                    .and_then(|part| part.trim().parse::<u8>().ok())
                    .map(|state| match state {
                        0 => CallState::Idle,
                        1 => CallState::Incoming,
                        2 => CallState::Outgoing,
                        _ => CallState::Connected,
                    })
                    .unwrap_or(self.call_state);
                self.call_state = state;
                AtLine::CallState(state)
            }
            value if value.starts_with("+CLCC:") => {
                let fields: Vec<_> = value[6..].trim().split(',').collect();
                if fields.len() < 5 {
                    return AtLine::Command(value.to_owned());
                }
                let parsed = fields[0]
                    .parse::<u8>()
                    .ok()
                    .zip(fields[1].parse::<u8>().ok())
                    .zip(fields[2].parse::<u8>().ok())
                    .zip(fields[3].parse::<u8>().ok())
                    .map(|(((index, direction), status), multiparty)| CallInfo {
                        index,
                        outgoing: direction == 0,
                        status,
                        multiparty: multiparty != 0,
                        number: fields
                            .get(5)
                            .map(|number| number.trim_matches('"').to_owned()),
                    });
                parsed
                    .map(AtLine::CurrentCall)
                    .unwrap_or_else(|| AtLine::Command(value.to_owned()))
            }
            value => AtLine::Command(value.to_owned()),
        }
    }
}

/// Why an AT frame could not be parsed or encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtError {
    /// A command contained a line terminator.
    InvalidCommand,
}

impl fmt::Display for AtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid HFP/HSP AT command")
    }
}

impl std::error::Error for AtError {}

/// Encode an AT command with the required CR/LF terminator.
pub fn encode_command(command: &str) -> Result<Vec<u8>, AtError> {
    if command.contains('\r') || command.contains('\n') {
        return Err(AtError::InvalidCommand);
    }
    let mut bytes = command.as_bytes().to_vec();
    bytes.extend_from_slice(b"\r\n");
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_ring_and_call_state() {
        let mut parser = AtParser::default();
        assert!(parser.feed(b"RI").is_empty());
        assert_eq!(
            parser.feed(b"NG\r\n+CIEV: 2,3\r\n"),
            vec![AtLine::Ring, AtLine::CallState(CallState::Connected)]
        );
        assert_eq!(parser.call_state(), CallState::Connected);
        let call = parser.feed(b"+CLCC: 1,0,0,0,0,\"555\"\r\n");
        assert_eq!(
            call[0],
            AtLine::CurrentCall(CallInfo {
                index: 1,
                outgoing: true,
                status: 0,
                multiparty: false,
                number: Some("555".into()),
            })
        );
    }

    #[test]
    fn encodes_and_validates_commands() {
        assert_eq!(encode_command("AT+CHUP").unwrap(), b"AT+CHUP\r\n");
        assert_eq!(encode_command("AT\n").unwrap_err(), AtError::InvalidCommand);
        assert_eq!(CallCommand::Answer.encode().unwrap(), b"ATA\r\n");
        assert_eq!(CallCommand::HangUp.encode().unwrap(), b"AT+CHUP\r\n");
    }
}
