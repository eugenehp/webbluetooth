//! Bluetooth Classic SDP data elements and service records.
//!
//! This module is a protocol-neutral parser. Platform backends can feed it
//! service attribute lists obtained from BlueZ, Android, or a native SDP stack.
//! It does not assume that every target can perform Classic discovery.

use crate::uuid::BluetoothUuid;
use std::collections::BTreeMap;
use std::fmt;

/// A Bluetooth SDP data element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdpDataElement {
    /// The SDP nil value.
    Nil,
    /// An unsigned integer, normalized to 64 bits.
    Uint(u64),
    /// A signed integer, normalized to 64 bits.
    Int(i64),
    /// A Bluetooth UUID.
    Uuid(BluetoothUuid),
    /// Raw text bytes.
    Text(Vec<u8>),
    /// A boolean value.
    Bool(bool),
    /// A nested sequence.
    Sequence(Vec<Self>),
    /// A nested alternative sequence.
    Alternative(Vec<Self>),
    /// A URL value.
    Url(Vec<u8>),
}

/// A parsed SDP service attribute list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SdpServiceRecord {
    attributes: BTreeMap<u16, SdpDataElement>,
}

impl SdpServiceRecord {
    /// Parse the attribute-list payload returned by SDP.
    pub fn parse_attribute_list(mut bytes: &[u8]) -> Result<Self, SdpError> {
        let mut attributes = BTreeMap::new();
        while !bytes.is_empty() {
            let (id, used) = parse_uint(bytes)?;
            if id > u16::MAX as u64 {
                return Err(SdpError::InvalidAttributeId);
            }
            bytes = &bytes[used..];
            let (value, used) = parse_element(bytes)?;
            bytes = &bytes[used..];
            attributes.insert(id as u16, value);
        }
        Ok(Self { attributes })
    }

    /// Get an SDP attribute by its 16-bit attribute ID.
    pub fn get(&self, id: u16) -> Option<&SdpDataElement> {
        self.attributes.get(&id)
    }

    /// Iterate over attributes in ascending ID order.
    pub fn attributes(&self) -> impl Iterator<Item = (u16, &SdpDataElement)> {
        self.attributes.iter().map(|(&id, value)| (id, value))
    }

    /// Return all UUIDs in the Service Class ID List (`0x0001`).
    pub fn service_class_uuids(&self) -> Vec<BluetoothUuid> {
        self.get(0x0001)
            .and_then(SdpDataElement::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(|element| match element {
                SdpDataElement::Uuid(uuid) => Some(*uuid),
                _ => None,
            })
            .collect()
    }

    /// Return the RFCOMM server channel from the Protocol Descriptor List.
    pub fn rfcomm_channel(&self) -> Option<u8> {
        self.protocol_uint(0x0003)
            .and_then(|value| u8::try_from(value).ok())
    }

    /// Return the L2CAP PSM from the Protocol Descriptor List.
    pub fn l2cap_psm(&self) -> Option<u16> {
        self.protocol_uint(0x0100)
            .and_then(|value| u16::try_from(value).ok())
    }

    fn protocol_uint(&self, protocol_uuid: u16) -> Option<u64> {
        let protocols = self.get(0x0004)?.as_sequence()?;
        for protocol in protocols {
            let values = protocol.as_sequence()?;
            let matches = values.first().is_some_and(|value| {
                matches!(value, SdpDataElement::Uuid(uuid) if *uuid == BluetoothUuid::from_u16(protocol_uuid))
            });
            if matches {
                return values.iter().find_map(|value| match value {
                    SdpDataElement::Uint(number) => Some(*number),
                    _ => None,
                });
            }
        }
        None
    }
}

impl SdpDataElement {
    fn as_sequence(&self) -> Option<&[Self]> {
        match self {
            Self::Sequence(values) | Self::Alternative(values) => Some(values),
            _ => None,
        }
    }
}

/// Why an SDP payload could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdpError {
    /// The payload ended before the current element was complete.
    Truncated,
    /// The element type or size descriptor is not valid.
    InvalidHeader,
    /// The UUID width was not one of SDP's supported widths.
    InvalidUuid,
    /// The attribute ID was not representable as `u16`.
    InvalidAttributeId,
    /// The UUID bytes were not a valid Bluetooth UUID.
    InvalidUuidValue,
}

impl fmt::Display for SdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "truncated SDP data element",
            Self::InvalidHeader => "invalid SDP data element header",
            Self::InvalidUuid => "invalid SDP UUID width",
            Self::InvalidAttributeId => "SDP attribute ID is not a 16-bit value",
            Self::InvalidUuidValue => "invalid SDP UUID value",
        })
    }
}

impl std::error::Error for SdpError {}

fn parse_uint(bytes: &[u8]) -> Result<(u64, usize), SdpError> {
    let (element, used) = parse_element(bytes)?;
    match element {
        SdpDataElement::Uint(value) => Ok((value, used)),
        _ => Err(SdpError::InvalidAttributeId),
    }
}

fn parse_element(bytes: &[u8]) -> Result<(SdpDataElement, usize), SdpError> {
    let header = *bytes.first().ok_or(SdpError::Truncated)?;
    let kind = header >> 3;
    let size = header & 7;
    let (length, header_len) = element_length(size, &bytes[1..])?;
    let start = header_len;
    let end = start.checked_add(length).ok_or(SdpError::Truncated)?;
    if bytes.len() < end {
        return Err(SdpError::Truncated);
    }
    let body = &bytes[start..end];
    let value = match kind {
        0 => SdpDataElement::Nil,
        1 => SdpDataElement::Uint(unsigned(body)?),
        2 => SdpDataElement::Int(signed(body)?),
        3 => SdpDataElement::Uuid(parse_uuid(body)?),
        4 => SdpDataElement::Text(body.to_vec()),
        5 => {
            if body.len() != 1 {
                return Err(SdpError::InvalidHeader);
            }
            SdpDataElement::Bool(body[0] != 0)
        }
        6 => SdpDataElement::Sequence(parse_sequence(body)?),
        7 => SdpDataElement::Alternative(parse_sequence(body)?),
        8 => SdpDataElement::Url(body.to_vec()),
        _ => return Err(SdpError::InvalidHeader),
    };
    Ok((value, end))
}

fn element_length(size: u8, rest: &[u8]) -> Result<(usize, usize), SdpError> {
    match size {
        0..=4 => Ok((1usize << size, 1)),
        5 => rest
            .first()
            .copied()
            .map(|v| (v as usize, 2))
            .ok_or(SdpError::Truncated),
        6 => {
            if rest.len() < 2 {
                return Err(SdpError::Truncated);
            }
            Ok((u16::from_be_bytes([rest[0], rest[1]]) as usize, 3))
        }
        7 => {
            if rest.len() < 4 {
                return Err(SdpError::Truncated);
            }
            Ok((
                u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize,
                5,
            ))
        }
        _ => Err(SdpError::InvalidHeader),
    }
}

fn unsigned(bytes: &[u8]) -> Result<u64, SdpError> {
    if bytes.is_empty() || bytes.len() > 8 {
        return Err(SdpError::InvalidHeader);
    }
    Ok(bytes
        .iter()
        .fold(0u64, |value, &byte| (value << 8) | byte as u64))
}

fn signed(bytes: &[u8]) -> Result<i64, SdpError> {
    let value = unsigned(bytes)?;
    let bits = bytes.len() * 8;
    if bits == 64 {
        Ok(value as i64)
    } else if value & (1 << (bits - 1)) != 0 {
        Ok((value | (!0u64 << bits)) as i64)
    } else {
        Ok(value as i64)
    }
}

fn parse_uuid(bytes: &[u8]) -> Result<BluetoothUuid, SdpError> {
    match bytes.len() {
        2 => Ok(BluetoothUuid::from_u16(u16::from_be_bytes([
            bytes[0], bytes[1],
        ]))),
        4 => Ok(BluetoothUuid::from_u32(u32::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
        ]))),
        16 => {
            let mut text = String::with_capacity(36);
            for (index, byte) in bytes.iter().enumerate() {
                if matches!(index, 4 | 6 | 8 | 10) {
                    text.push('-');
                }
                text.push_str(&format!("{byte:02x}"));
            }
            BluetoothUuid::parse(&text).map_err(|_| SdpError::InvalidUuidValue)
        }
        _ => Err(SdpError::InvalidUuid),
    }
}

fn parse_sequence(mut bytes: &[u8]) -> Result<Vec<SdpDataElement>, SdpError> {
    let mut values = Vec::new();
    while !bytes.is_empty() {
        let (value, used) = parse_element(bytes)?;
        values.push(value);
        bytes = &bytes[used..];
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attribute_list_and_nested_uuid() {
        let record = SdpServiceRecord::parse_attribute_list(&[
            0x09, 0x00, 0x01, 0x35, 0x03, 0x19, 0x11, 0x01,
        ])
        .unwrap();
        assert_eq!(
            record.get(1),
            Some(&SdpDataElement::Sequence(vec![SdpDataElement::Uuid(
                BluetoothUuid::from_u16(0x1101)
            ),]))
        );
        assert_eq!(
            record.service_class_uuids(),
            vec![BluetoothUuid::from_u16(0x1101)]
        );
    }

    #[test]
    fn extracts_rfcomm_and_l2cap_protocol_endpoints() {
        let record = SdpServiceRecord::parse_attribute_list(&[
            0x09, 0x00, 0x04, 0x35, 0x0f, 0x35, 0x06, 0x19, 0x01, 0x00, 0x09, 0x10, 0x01, 0x35,
            0x05, 0x19, 0x00, 0x03, 0x08, 0x05,
        ])
        .unwrap();
        assert_eq!(record.l2cap_psm(), Some(0x1001));
        assert_eq!(record.rfcomm_channel(), Some(5));
    }

    #[test]
    fn rejects_truncated_elements() {
        assert_eq!(
            SdpServiceRecord::parse_attribute_list(&[0x09, 0x00]).unwrap_err(),
            SdpError::Truncated
        );
    }
}
