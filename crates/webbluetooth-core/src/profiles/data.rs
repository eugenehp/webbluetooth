//! PAN, DUN, and OBEX/FTP protocol state and framing foundations.

use std::fmt;

/// PAN roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanRole {
    /// Personal Area Networking User.
    Panu,
    /// Network Access Point.
    Nap,
    /// Group Ad-hoc Network.
    Gn,
}

/// DUN session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DunState {
    /// No modem session.
    #[default]
    Idle,
    /// AT link is negotiating.
    Negotiating,
    /// The data session is active.
    Connected,
    /// The session ended with an error.
    Failed,
}

/// An OBEX operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObexOperation {
    /// Establish an OBEX session.
    Connect,
    /// Request an object.
    Get,
    /// Send an object.
    Put,
    /// End an OBEX session.
    Disconnect,
}

/// An OBEX packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObexHeader {
    /// OBEX operation code without the final-bit marker.
    pub operation: ObexOperation,
    /// Packet length including the three-byte OBEX header.
    pub length: u16,
}

/// A complete OBEX packet assembled from an operation and payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObexPacket(Vec<u8>);

impl ObexPacket {
    /// Build an OBEX Connect packet.
    pub fn connect(version: u8, flags: u8, max_packet: u16) -> Self {
        let mut bytes = vec![0x80, version, flags];
        bytes.extend_from_slice(&max_packet.to_be_bytes());
        Self::with_header(bytes)
    }

    /// Build an OBEX Disconnect packet.
    pub fn disconnect() -> Self {
        Self::with_header(vec![0x81])
    }

    /// Build an OBEX Get packet, optionally carrying a body.
    pub fn get(body: &[u8]) -> Result<Self, ObexError> {
        Self::operation(0x03, body)
    }

    /// Build a final OBEX Get packet, optionally carrying a body.
    pub fn get_final(body: &[u8]) -> Result<Self, ObexError> {
        Self::operation(0x83, body)
    }

    /// Build an OBEX Put packet, optionally carrying a body.
    pub fn put(body: &[u8]) -> Result<Self, ObexError> {
        Self::operation(0x02, body)
    }

    /// Build a final OBEX Put packet, optionally carrying a body.
    pub fn put_final(body: &[u8]) -> Result<Self, ObexError> {
        Self::operation(0x82, body)
    }

    /// Borrow the encoded packet.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn operation(code: u8, body: &[u8]) -> Result<Self, ObexError> {
        let length = 3usize
            .checked_add(body.len())
            .ok_or(ObexError::InvalidLength)?;
        if length > u16::MAX as usize {
            return Err(ObexError::InvalidLength);
        }
        let mut bytes = vec![code];
        bytes.extend_from_slice(&(length as u16).to_be_bytes());
        bytes.extend_from_slice(body);
        Ok(Self(bytes))
    }

    fn with_header(mut bytes: Vec<u8>) -> Self {
        let length = bytes.len() as u16 + 2;
        bytes.splice(1..1, length.to_be_bytes());
        Self(bytes)
    }
}

/// Why an OBEX packet could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObexError {
    /// The packet is shorter than its fixed header.
    Truncated,
    /// The packet length is invalid.
    InvalidLength,
    /// The operation code is not supported by this model.
    UnsupportedOperation,
}

impl fmt::Display for ObexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "truncated OBEX packet",
            Self::InvalidLength => "invalid OBEX packet length",
            Self::UnsupportedOperation => "unsupported OBEX operation",
        })
    }
}

impl std::error::Error for ObexError {}

/// Parse an OBEX operation packet header.
pub fn parse_obex_header(bytes: &[u8]) -> Result<ObexHeader, ObexError> {
    if bytes.len() < 3 {
        return Err(ObexError::Truncated);
    }
    let operation = match bytes[0] {
        0x80 => ObexOperation::Connect,
        0x02 | 0x82 => ObexOperation::Put,
        0x03 | 0x83 => ObexOperation::Get,
        0x81 => ObexOperation::Disconnect,
        _ => return Err(ObexError::UnsupportedOperation),
    };
    let length = u16::from_be_bytes([bytes[1], bytes[2]]);
    if length < 3 || length as usize > bytes.len() {
        return Err(ObexError::InvalidLength);
    }
    Ok(ObexHeader { operation, length })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_obex_headers_and_rejects_bad_lengths() {
        assert_eq!(
            parse_obex_header(&[0x02, 0x00, 0x03]).unwrap().operation,
            ObexOperation::Put
        );
        assert_eq!(
            parse_obex_header(&[0x03, 0x00, 0x03]).unwrap().operation,
            ObexOperation::Get
        );
        assert_eq!(
            parse_obex_header(&[0x82, 0x00, 0x03]).unwrap().operation,
            ObexOperation::Put
        );
        assert_eq!(
            parse_obex_header(&[0x83, 0x00, 0x03]).unwrap().operation,
            ObexOperation::Get
        );
        assert_eq!(
            parse_obex_header(ObexPacket::connect(0x10, 0, 1024).as_bytes())
                .unwrap()
                .operation,
            ObexOperation::Connect
        );
        assert_eq!(
            parse_obex_header(&[0x02, 0, 2]).unwrap_err(),
            ObexError::InvalidLength
        );
        assert_eq!(ObexPacket::disconnect().as_bytes(), &[0x81, 0, 3]);
        assert_eq!(
            ObexPacket::get(&[1, 2]).unwrap().as_bytes(),
            &[3, 0, 5, 1, 2]
        );
        assert_eq!(
            ObexPacket::put_final(&[1]).unwrap().as_bytes(),
            &[0x82, 0, 4, 1]
        );
        assert_eq!(
            ObexPacket::get_final(&[2]).unwrap().as_bytes(),
            &[0x83, 0, 4, 2]
        );
    }
}
