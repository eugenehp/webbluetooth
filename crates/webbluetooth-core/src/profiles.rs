//! Protocol-neutral Bluetooth profile identifiers and roles.
//!
//! Transport and OS integration live in platform crates. These values let
//! callers describe a profile without coupling their configuration to a
//! particular backend.

use crate::uuid::BluetoothUuid;

/// A2DP codec capability and negotiation modeling.
pub mod a2dp;
/// AVRCP command and metadata modeling.
pub mod avrcp;
/// PAN, DUN, and OBEX/FTP framing foundations.
pub mod data;
/// HFP/HSP AT framing and call-state modeling.
pub mod hfp;
/// HID report descriptor parsing and report-field metadata.
pub mod hid;
/// PAN BNEP frame modeling.
pub mod pan;

/// Bluetooth profile families supported by the profile feature surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Advanced Audio Distribution Profile.
    A2dp,
    /// Audio/Video Remote Control Profile.
    Avrcp,
    /// Hands-Free Profile.
    Hfp,
    /// Headset Profile.
    Hsp,
    /// Classic Human Interface Device profile.
    Hid,
    /// Personal Area Networking.
    Pan,
    /// Dial-up Networking.
    Dun,
    /// Object Exchange.
    Obex,
    /// File Transfer Profile.
    Ftp,
}

/// The local role for a profile connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileRole {
    /// Initiates or controls a profile connection.
    Client,
    /// Accepts or provides a profile connection.
    Server,
    /// The profile can operate in either role.
    Either,
}

impl Profile {
    /// The Bluetooth SIG service UUID commonly used to identify this profile.
    pub const fn service_uuid(self) -> Option<BluetoothUuid> {
        match self {
            Self::A2dp => Some(BluetoothUuid::from_u16(0x110d)),
            Self::Avrcp => Some(BluetoothUuid::from_u16(0x110e)),
            Self::Hfp => Some(BluetoothUuid::from_u16(0x111e)),
            Self::Hsp => Some(BluetoothUuid::from_u16(0x1108)),
            Self::Hid => Some(BluetoothUuid::from_u16(0x1124)),
            Self::Pan => Some(BluetoothUuid::from_u16(0x1115)),
            Self::Dun => Some(BluetoothUuid::from_u16(0x1103)),
            Self::Obex => Some(BluetoothUuid::from_u16(0x1105)),
            Self::Ftp => Some(BluetoothUuid::from_u16(0x1106)),
        }
    }

    /// The usual local role for this profile family.
    pub const fn default_role(self) -> ProfileRole {
        match self {
            Self::A2dp | Self::Hfp | Self::Hsp | Self::Hid | Self::Pan | Self::Dun | Self::Ftp => {
                ProfileRole::Client
            }
            Self::Avrcp | Self::Obex => ProfileRole::Either,
        }
    }
}

/// A profile request that can be passed to a platform backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProfileRequest {
    /// Profile family.
    pub profile: Profile,
    /// Desired local role.
    pub role: ProfileRole,
}

impl ProfileRequest {
    /// Use the profile's conventional role.
    pub const fn conventional(profile: Profile) -> Self {
        Self {
            profile,
            role: profile.default_role(),
        }
    }
}
