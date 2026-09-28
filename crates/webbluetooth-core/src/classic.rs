//! Portable Bluetooth Classic vocabulary.
//!
//! These types describe Classic peers and transport endpoints without choosing
//! a platform socket or profile implementation. They are available behind the
//! `classic` feature; the Web Bluetooth-compatible facade does not re-export
//! them.

use std::fmt;
use std::str::FromStr;

/// A Bluetooth device address in the conventional written order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BluetoothAddress([u8; 6]);

impl BluetoothAddress {
    /// Construct an address from its six octets in written order.
    pub const fn from_octets(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    /// Return the address octets in written order.
    pub const fn octets(self) -> [u8; 6] {
        self.0
    }

    /// Return the address as `AA:BB:CC:DD:EE:FF`.
    pub fn as_str(&self) -> String {
        self.to_string()
    }
}

impl fmt::Debug for BluetoothAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BluetoothAddress")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for BluetoothAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, octet) in self.0.iter().enumerate() {
            if index != 0 {
                f.write_str(":")?;
            }
            write!(f, "{octet:02X}")?;
        }
        Ok(())
    }
}

impl FromStr for BluetoothAddress {
    type Err = ClassicAddressError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = value.split(':').collect();
        if parts.len() != 6 {
            return Err(ClassicAddressError::WrongOctetCount);
        }
        let mut octets = [0; 6];
        for (index, part) in parts.iter().enumerate() {
            if part.len() != 2 {
                return Err(ClassicAddressError::InvalidOctet);
            }
            octets[index] =
                u8::from_str_radix(part, 16).map_err(|_| ClassicAddressError::InvalidOctet)?;
        }
        Ok(Self(octets))
    }
}

/// Why parsing a Bluetooth address failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicAddressError {
    /// The address did not contain six colon-separated octets.
    WrongOctetCount,
    /// One octet was not exactly two hexadecimal digits.
    InvalidOctet,
}

impl fmt::Display for ClassicAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongOctetCount => f.write_str("a Bluetooth address needs six octets"),
            Self::InvalidOctet => f.write_str("a Bluetooth address octet must be two hex digits"),
        }
    }
}

impl std::error::Error for ClassicAddressError {}

/// Bluetooth Classic service, major-device, and minor-device class bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ClassOfDevice {
    /// Service class bits from the upper eleven bits.
    pub service_class: u16,
    /// Major device class, five bits.
    pub major_device: u8,
    /// Minor device class, six bits.
    pub minor_device: u8,
}

/// A Bluetooth Classic device returned by native discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassicDevice {
    /// The public Bluetooth address, when the platform exposes it.
    pub address: Option<BluetoothAddress>,
    /// The cached or inquiry name.
    pub name: Option<String>,
    /// Bluetooth Class of Device bits, when available.
    pub class_of_device: Option<ClassOfDevice>,
    /// Whether the device is paired/bonded.
    pub bonded: bool,
    /// Whether the current link is authenticated.
    pub authenticated: bool,
    /// Whether the device is currently connected.
    pub connected: bool,
}

impl ClassOfDevice {
    /// Decode the 24-bit wire representation used by inquiry results.
    pub const fn from_raw(raw: u32) -> Self {
        Self {
            service_class: ((raw >> 13) & 0x07ff) as u16,
            major_device: ((raw >> 8) & 0x1f) as u8,
            minor_device: ((raw >> 2) & 0x3f) as u8,
        }
    }

    /// Encode this value as a 24-bit Class of Device value.
    pub const fn raw(self) -> u32 {
        ((self.service_class as u32 & 0x07ff) << 13)
            | ((self.major_device as u32 & 0x1f) << 8)
            | ((self.minor_device as u32 & 0x3f) << 2)
    }
}

/// RFCOMM server channel number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RfcommChannel(u8);

impl RfcommChannel {
    /// Create a channel in the Bluetooth-defined 1..=30 range.
    pub const fn new(channel: u8) -> Option<Self> {
        if channel == 0 || channel > 30 {
            None
        } else {
            Some(Self(channel))
        }
    }

    /// Return the server channel number.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Bluetooth Classic L2CAP Protocol/Service Multiplexer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassicPsm(u16);

impl ClassicPsm {
    /// Create a PSM, rejecting zero and reserved values with the low bits set.
    pub const fn new(psm: u16) -> Option<Self> {
        if psm == 0 || psm & 0x0101 != 0x0001 {
            None
        } else {
            Some(Self(psm))
        }
    }

    /// Return the PSM value.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Security requested for a Classic socket or profile connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClassicSecurity {
    /// No authentication or encryption requirement.
    None,
    /// Require link authentication.
    Authentication,
    /// Require an encrypted link.
    Encryption,
    /// Require authenticated encryption with a stronger key.
    SecureEncryption,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip_in_written_order() {
        let address: BluetoothAddress = "AA:bb:01:02:03:FF".parse().unwrap();
        assert_eq!(address.octets(), [0xAA, 0xBB, 1, 2, 3, 0xFF]);
        assert_eq!(address.to_string(), "AA:BB:01:02:03:FF");
    }

    #[test]
    fn malformed_addresses_are_rejected() {
        assert!("AA:BB:CC:DD:EE".parse::<BluetoothAddress>().is_err());
        assert!("AA:BB:CC:DD:EE:GG".parse::<BluetoothAddress>().is_err());
        assert!("A:BB:CC:DD:EE:FF".parse::<BluetoothAddress>().is_err());
    }

    #[test]
    fn class_of_device_round_trips() {
        let class = ClassOfDevice {
            service_class: 0x123,
            major_device: 0x12,
            minor_device: 0x21,
        };
        assert_eq!(ClassOfDevice::from_raw(class.raw()), class);
    }

    #[test]
    fn endpoint_ranges_are_checked() {
        assert_eq!(RfcommChannel::new(1).unwrap().get(), 1);
        assert!(RfcommChannel::new(0).is_none());
        assert!(RfcommChannel::new(31).is_none());
        assert!(ClassicPsm::new(0x1002).is_none());
        assert_eq!(ClassicPsm::new(0x1003).unwrap().get(), 0x1003);
    }
}
