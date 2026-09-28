//! Length-checked raw Bluetooth packet containers.
//!
//! These types do not open sockets or grant privileges. They provide a common
//! framing boundary for platform-specific ACL, SCO/eSCO, and ISO backends.

/// Why a raw HCI packet failed structural validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPacketError {
    /// The packet does not contain its complete HCI data header.
    Truncated {
        /// Minimum number of bytes required by the packet header.
        minimum: usize,
        /// Number of bytes supplied.
        actual: usize,
    },
    /// The HCI header length does not match the supplied payload.
    LengthMismatch {
        /// Payload length declared by the HCI header.
        declared: usize,
        /// Payload length present in the buffer.
        actual: usize,
    },
    /// A reserved HCI header bit was set.
    ReservedBitsSet,
}

/// A raw ACL data packet without an HCI packet-type byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclPacket(Vec<u8>);

impl AclPacket {
    /// Wrap an ACL payload.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    /// Validate and wrap an ACL packet.
    pub fn try_new(bytes: Vec<u8>) -> Result<Self, RawPacketError> {
        let packet = Self(bytes);
        packet.validate()?;
        Ok(packet)
    }
    /// Validate the ACL data header and declared payload length.
    pub fn validate(&self) -> Result<(), RawPacketError> {
        validate_length(
            &self.0,
            4,
            u16::from_le_bytes([
                self.0.get(2).copied().unwrap_or(0),
                self.0.get(3).copied().unwrap_or(0),
            ]) as usize,
        )
    }
    /// Borrow the ACL payload.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Consume the packet into its payload.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// A raw SCO/eSCO data packet without an HCI packet-type byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoPacket(Vec<u8>);

impl ScoPacket {
    /// Wrap an SCO/eSCO payload.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    /// Validate and wrap an SCO/eSCO packet.
    pub fn try_new(bytes: Vec<u8>) -> Result<Self, RawPacketError> {
        let packet = Self(bytes);
        packet.validate()?;
        Ok(packet)
    }
    /// Validate the SCO data header and declared payload length.
    pub fn validate(&self) -> Result<(), RawPacketError> {
        if self.0.len() >= 2 && u16::from_le_bytes([self.0[0], self.0[1]]) & 0xc000 != 0 {
            return Err(RawPacketError::ReservedBitsSet);
        }
        validate_length(&self.0, 3, self.0.get(2).copied().unwrap_or(0) as usize)
    }
    /// Borrow the SCO/eSCO payload.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Consume the packet into its payload.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// A Bluetooth LE Isochronous data packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoPacket(Vec<u8>);

impl IsoPacket {
    /// Wrap an ISO payload.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    /// Validate and wrap an ISO packet.
    pub fn try_new(bytes: Vec<u8>) -> Result<Self, RawPacketError> {
        let packet = Self(bytes);
        packet.validate()?;
        Ok(packet)
    }
    /// Validate the ISO data header and declared payload length.
    pub fn validate(&self) -> Result<(), RawPacketError> {
        if self.0.len() < 4 {
            return Err(RawPacketError::Truncated {
                minimum: 4,
                actual: self.0.len(),
            });
        }
        if u16::from_le_bytes([self.0[0], self.0[1]]) & 0x8000 != 0 {
            return Err(RawPacketError::ReservedBitsSet);
        }
        let length = u16::from_le_bytes([self.0[2], self.0[3]]);
        if length & 0xc000 != 0 {
            return Err(RawPacketError::ReservedBitsSet);
        }
        validate_length(&self.0, 4, (length & 0x3fff) as usize)
    }
    /// Borrow the ISO payload.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Consume the packet into its payload.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

fn validate_length(bytes: &[u8], header_len: usize, declared: usize) -> Result<(), RawPacketError> {
    if bytes.len() < header_len {
        return Err(RawPacketError::Truncated {
            minimum: header_len,
            actual: bytes.len(),
        });
    }
    let actual = bytes.len() - header_len;
    if declared != actual {
        return Err(RawPacketError::LengthMismatch { declared, actual });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_acl_and_sco_lengths() {
        assert!(AclPacket::try_new(vec![0, 0, 2, 0, 1, 2]).is_ok());
        assert_eq!(
            AclPacket::try_new(vec![0, 0, 2, 0, 1]),
            Err(RawPacketError::LengthMismatch {
                declared: 2,
                actual: 1
            })
        );
        assert!(ScoPacket::try_new(vec![0, 0, 1, 7]).is_ok());
        assert_eq!(
            ScoPacket::try_new(vec![0, 0xc0, 0]),
            Err(RawPacketError::ReservedBitsSet)
        );
    }

    #[test]
    fn validates_iso_length_and_reserved_bits() {
        assert!(IsoPacket::try_new(vec![0, 0, 1, 0, 9]).is_ok());
        assert_eq!(
            IsoPacket::try_new(vec![0, 0x80, 0, 0]),
            Err(RawPacketError::ReservedBitsSet)
        );
        assert_eq!(
            IsoPacket::try_new(vec![0, 0, 0x00, 0xc0]),
            Err(RawPacketError::ReservedBitsSet)
        );
    }

    #[test]
    fn rejects_truncated_packets_and_keeps_unchecked_constructor() {
        assert!(matches!(
            AclPacket::try_new(vec![0, 0, 0]),
            Err(RawPacketError::Truncated { .. })
        ));
        assert!(matches!(
            IsoPacket::try_new(Vec::new()),
            Err(RawPacketError::Truncated { .. })
        ));
        assert_eq!(AclPacket::new(vec![1]).as_bytes(), &[1]);
    }
}
