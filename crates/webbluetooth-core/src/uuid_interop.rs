//! Conversions to and from the [`uuid`](https://docs.rs/uuid) crate.
//!
//! Enabled by the `uuid` feature, off by default — nobody pays for a
//! dependency they do not use.
//!
//! [`BluetoothUuid`] is deliberately not `uuid::Uuid`: the specification
//! defines `BluetoothUUID` as canonical text and resolves assigned names like
//! `"battery_service"` through it, neither of which a general-purpose UUID
//! type does. But the rest of the Rust Bluetooth world — `btleplug` above all
//! — speaks `uuid::Uuid`, so a program arriving from there has a module full
//! of `Uuid` constants and, without this, writes a conversion helper on its
//! way in. That helper is the same four lines every time, and while the
//! constructors were fallible it contained an `expect`.
//!
//! ```
//! # #[cfg(feature = "uuid")] {
//! use webbluetooth_core::uuid::BluetoothUuid;
//!
//! const CONTROL: uuid::Uuid = uuid::Uuid::from_u128(0x273e0001_4c4d_454d_96be_f03bac821358);
//!
//! // Straight into anything that takes a UUID, no helper in between.
//! let uuid: BluetoothUuid = CONTROL.into();
//! assert_eq!(uuid.as_str(), "273e0001-4c4d-454d-96be-f03bac821358");
//! assert_eq!(uuid.as_uuid(), CONTROL);
//! # }
//! ```

use crate::error::Result;
use crate::uuid::{BluetoothUuid, IntoUuid};

impl BluetoothUuid {
    /// This UUID as a [`uuid::Uuid`].
    ///
    /// Both types are 128 bits, so this is a re-spelling: no parsing, no
    /// allocation, and nothing that can fail.
    pub const fn as_uuid(&self) -> ::uuid::Uuid {
        ::uuid::Uuid::from_u128(self.as_u128())
    }
}

impl From<::uuid::Uuid> for BluetoothUuid {
    fn from(uuid: ::uuid::Uuid) -> Self {
        Self::from_u128(uuid.as_u128())
    }
}

impl From<&::uuid::Uuid> for BluetoothUuid {
    fn from(uuid: &::uuid::Uuid) -> Self {
        Self::from_u128(uuid.as_u128())
    }
}

impl From<BluetoothUuid> for ::uuid::Uuid {
    fn from(uuid: BluetoothUuid) -> Self {
        uuid.as_uuid()
    }
}

/// So a `Uuid` goes straight into `get_primary_service`, `get_characteristic`
/// and everything else that names an attribute.
impl IntoUuid for ::uuid::Uuid {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        Ok(self.into())
    }
}

impl IntoUuid for &::uuid::Uuid {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        Ok(self.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vendor UUID has to survive the trip in both directions — it is the
    /// case the assigned-number path never exercises.
    #[test]
    fn round_trips_a_vendor_uuid() {
        const MUSE_CONTROL: ::uuid::Uuid =
            ::uuid::Uuid::from_u128(0x273e0001_4c4d_454d_96be_f03bac821358);

        let uuid: BluetoothUuid = MUSE_CONTROL.into();
        assert_eq!(uuid.as_str(), "273e0001-4c4d-454d-96be-f03bac821358");
        assert_eq!(uuid.as_uuid(), MUSE_CONTROL);
        assert_eq!(::uuid::Uuid::from(uuid), MUSE_CONTROL);
    }

    /// And an assigned number keeps its Bluetooth Base UUID, which is the part
    /// a hand-written conversion gets wrong.
    #[test]
    fn keeps_the_base_uuid_of_an_assigned_number() {
        let battery = BluetoothUuid::from_u16(0x180f);
        assert_eq!(
            battery.as_uuid(),
            ::uuid::Uuid::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb)
        );
        assert_eq!(BluetoothUuid::from(battery.as_uuid()), battery);
    }

    /// The point of the trait impl: no conversion at the call site at all.
    #[test]
    fn a_uuid_names_an_attribute_directly() {
        const SERVICE: ::uuid::Uuid =
            ::uuid::Uuid::from_u128(0x0000fe8d_0000_1000_8000_00805f9b34fb);

        assert_eq!(
            SERVICE.into_uuid().unwrap(),
            BluetoothUuid::from_u16(0xfe8d)
        );
        assert_eq!(
            (&SERVICE).into_uuid().unwrap(),
            BluetoothUuid::from_u16(0xfe8d)
        );
    }
}
