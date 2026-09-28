//! Bluetooth PAN/BNEP frame modeling.

use std::fmt;

/// BNEP packet type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// General Ethernet frame.
    General,
    /// Control message.
    Control,
    /// Compressed Ethernet frame.
    Compressed,
    /// Compressed source/destination Ethernet frame.
    CompressedSourceDestination,
}

/// A BNEP control message carried after the control packet-type byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    /// A peer could not understand the requested control message.
    CommandNotUnderstood {
        /// The unrecognised control-message type.
        message_type: u8,
    },
    /// Request a connection between two profile UUIDs.
    SetupConnectionRequest {
        /// Destination profile service UUID.
        destination_service: u16,
        /// Source profile service UUID.
        source_service: u16,
    },
    /// Response to a setup request.
    SetupConnectionResponse {
        /// BNEP setup response code.
        response: u16,
    },
    /// Set the accepted Ethernet type ranges.
    FilterNetTypeSet {
        /// Inclusive `(start, end)` EtherType ranges.
        ranges: Vec<(u16, u16)>,
    },
    /// Response to a network-type filter request.
    FilterNetTypeResponse {
        /// BNEP filter response code.
        response: u8,
    },
    /// Set the accepted multicast address ranges.
    FilterMultiAddrSet {
        /// Inclusive `(start, end)` Ethernet multicast address ranges.
        ranges: Vec<([u8; 6], [u8; 6])>,
    },
    /// Response to a multicast-address filter request.
    FilterMultiAddrResponse {
        /// BNEP filter response code.
        response: u8,
    },
}

/// A BNEP Ethernet frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Packet type.
    pub packet_type: PacketType,
    /// Destination MAC address.
    pub destination: [u8; 6],
    /// Source MAC address.
    pub source: [u8; 6],
    /// EtherType.
    pub ether_type: u16,
    /// Ethernet payload.
    pub payload: Vec<u8>,
}

/// Why a BNEP frame could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The frame ended before its required headers or payload.
    Truncated,
    /// The packet type is unsupported by this frame model.
    UnsupportedPacketType,
    /// A control message has an invalid value or unsupported type.
    InvalidControlMessage,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "truncated BNEP frame",
            Self::UnsupportedPacketType => "unsupported BNEP packet type",
            Self::InvalidControlMessage => "invalid BNEP control message",
        })
    }
}

impl std::error::Error for Error {}

impl ControlMessage {
    /// Encode the control-message type and payload.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        match self {
            Self::CommandNotUnderstood { message_type } => {
                bytes.extend_from_slice(&[0x01, *message_type]);
            }
            Self::SetupConnectionRequest {
                destination_service,
                source_service,
            } => bytes.extend_from_slice(&[
                0x02,
                4,
                (destination_service >> 8) as u8,
                *destination_service as u8,
                (source_service >> 8) as u8,
                *source_service as u8,
            ]),
            Self::SetupConnectionResponse { response } => {
                bytes.extend_from_slice(&[0x03, (response >> 8) as u8, *response as u8]);
            }
            Self::FilterNetTypeSet { ranges } => {
                let count = ranges
                    .len()
                    .checked_mul(4)
                    .ok_or(Error::InvalidControlMessage)?;
                if count > u8::MAX as usize {
                    return Err(Error::InvalidControlMessage);
                }
                bytes.extend_from_slice(&[0x04, count as u8]);
                for (start, end) in ranges {
                    bytes.extend_from_slice(&start.to_be_bytes());
                    bytes.extend_from_slice(&end.to_be_bytes());
                }
            }
            Self::FilterNetTypeResponse { response } => bytes.extend_from_slice(&[0x05, *response]),
            Self::FilterMultiAddrSet { ranges } => {
                let count = ranges
                    .len()
                    .checked_mul(12)
                    .ok_or(Error::InvalidControlMessage)?;
                if count > u8::MAX as usize {
                    return Err(Error::InvalidControlMessage);
                }
                bytes.extend_from_slice(&[0x06, count as u8]);
                for (start, end) in ranges {
                    bytes.extend_from_slice(start);
                    bytes.extend_from_slice(end);
                }
            }
            Self::FilterMultiAddrResponse { response } => {
                bytes.extend_from_slice(&[0x07, *response])
            }
        }
        Ok(bytes)
    }

    /// Decode a control-message type and payload.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (&message_type, body) = bytes.split_first().ok_or(Error::Truncated)?;
        match message_type {
            0x01 if body.len() == 1 => Ok(Self::CommandNotUnderstood {
                message_type: body[0],
            }),
            0x02 if body.len() == 5 && body[0] == 4 => Ok(Self::SetupConnectionRequest {
                destination_service: u16::from_be_bytes([body[1], body[2]]),
                source_service: u16::from_be_bytes([body[3], body[4]]),
            }),
            0x03 if body.len() == 2 => Ok(Self::SetupConnectionResponse {
                response: u16::from_be_bytes([body[0], body[1]]),
            }),
            0x04 if !body.is_empty()
                && body[0] as usize == body.len() - 1
                && (body[0] & 3) == 0 =>
            {
                let ranges = body[1..]
                    .chunks_exact(4)
                    .map(|chunk| {
                        (
                            u16::from_be_bytes([chunk[0], chunk[1]]),
                            u16::from_be_bytes([chunk[2], chunk[3]]),
                        )
                    })
                    .collect();
                Ok(Self::FilterNetTypeSet { ranges })
            }
            0x05 if body.len() == 1 => Ok(Self::FilterNetTypeResponse { response: body[0] }),
            0x06 if !body.is_empty()
                && body[0] as usize == body.len() - 1
                && (body[0] % 12) == 0 =>
            {
                let ranges = body[1..]
                    .chunks_exact(12)
                    .map(|chunk| {
                        let mut start = [0; 6];
                        let mut end = [0; 6];
                        start.copy_from_slice(&chunk[..6]);
                        end.copy_from_slice(&chunk[6..]);
                        (start, end)
                    })
                    .collect();
                Ok(Self::FilterMultiAddrSet { ranges })
            }
            0x07 if body.len() == 1 => Ok(Self::FilterMultiAddrResponse { response: body[0] }),
            _ => Err(Error::InvalidControlMessage),
        }
    }
}

impl Frame {
    /// Encode a BNEP Ethernet frame.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        match self.packet_type {
            PacketType::General => {
                bytes.push(0x00);
                bytes.extend_from_slice(&self.destination);
                bytes.extend_from_slice(&self.source);
            }
            PacketType::Compressed => {
                bytes.push(0x20);
                bytes.extend_from_slice(&self.destination);
            }
            PacketType::CompressedSourceDestination => bytes.push(0x30),
            PacketType::Control => return Err(Error::UnsupportedPacketType),
        }
        bytes.extend_from_slice(&self.ether_type.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }

    /// Decode a BNEP Ethernet frame.
    ///
    /// Compressed frames omit one or both MAC addresses. Omitted addresses are
    /// returned as zeroes and should be filled from the connection context.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let packet_type = match bytes.first().map(|byte| byte >> 4) {
            Some(0) => PacketType::General,
            Some(2) => PacketType::Compressed,
            Some(3) => PacketType::CompressedSourceDestination,
            Some(_) => return Err(Error::UnsupportedPacketType),
            None => return Err(Error::Truncated),
        };
        let header_len = match packet_type {
            PacketType::General => 15,
            PacketType::Compressed => 9,
            PacketType::CompressedSourceDestination => 3,
            PacketType::Control => unreachable!(),
        };
        if bytes.len() < header_len {
            return Err(Error::Truncated);
        }
        let mut destination = [0; 6];
        let mut source = [0; 6];
        let ether_type_offset = match packet_type {
            PacketType::General => {
                destination.copy_from_slice(&bytes[1..7]);
                source.copy_from_slice(&bytes[7..13]);
                13
            }
            PacketType::Compressed => {
                destination.copy_from_slice(&bytes[1..7]);
                7
            }
            PacketType::CompressedSourceDestination => 1,
            PacketType::Control => unreachable!(),
        };
        Ok(Self {
            packet_type,
            destination,
            source,
            ether_type: u16::from_be_bytes([
                bytes[ether_type_offset],
                bytes[ether_type_offset + 1],
            ]),
            payload: bytes[ether_type_offset + 2..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_general_ethernet_frames() {
        let frame = Frame {
            packet_type: PacketType::General,
            destination: [1, 2, 3, 4, 5, 6],
            source: [6, 5, 4, 3, 2, 1],
            ether_type: 0x0800,
            payload: vec![9, 8, 7],
        };
        assert_eq!(Frame::decode(&frame.encode().unwrap()).unwrap(), frame);
    }

    #[test]
    fn round_trips_compressed_ethernet_frames() {
        let compressed = Frame {
            packet_type: PacketType::Compressed,
            destination: [1, 2, 3, 4, 5, 6],
            source: [0; 6],
            ether_type: 0x0800,
            payload: vec![9, 8, 7],
        };
        assert_eq!(
            Frame::decode(&compressed.encode().unwrap()).unwrap(),
            compressed
        );

        let compressed_both = Frame {
            packet_type: PacketType::CompressedSourceDestination,
            destination: [0; 6],
            source: [0; 6],
            ether_type: 0x86dd,
            payload: vec![1, 2],
        };
        assert_eq!(
            Frame::decode(&compressed_both.encode().unwrap()).unwrap(),
            compressed_both
        );
    }

    #[test]
    fn round_trips_bnep_control_messages() {
        let messages = [
            ControlMessage::SetupConnectionRequest {
                destination_service: 0x1115,
                source_service: 0x1116,
            },
            ControlMessage::FilterNetTypeSet {
                ranges: vec![(0x0800, 0x0806)],
            },
            ControlMessage::FilterMultiAddrSet {
                ranges: vec![([0, 1, 2, 3, 4, 5], [6, 7, 8, 9, 10, 11])],
            },
        ];
        for message in messages {
            assert_eq!(
                ControlMessage::decode(&message.encode().unwrap()).unwrap(),
                message
            );
        }
    }

    #[test]
    fn rejects_malformed_bnep_control_messages() {
        assert!(ControlMessage::decode(&[]).is_err());
        assert!(ControlMessage::decode(&[0x02, 2, 0, 1]).is_err());
        assert!(ControlMessage::decode(&[0x04, 3, 0, 1, 0, 2]).is_err());
        assert!(ControlMessage::decode(&[0xff, 0]).is_err());
    }
}
