//! `BluetoothUUID` — always the canonical 128-bit lowercase form.
//!
//! The Web Bluetooth spec canonicalises every UUID to
//! `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, lowercase, before it is compared or
//! reported. CoreBluetooth does not: `CBUUID.UUIDString` hands back whichever
//! width it was built from, uppercased, so `"180F"` and
//! `"0000180F-0000-1000-8000-00805F9B34FB"` are the same UUID with two
//! different strings. Everything crossing that boundary goes through this type,
//! which is why an allowlist check or a blocklist lookup cannot be fooled by
//! the spelling.

use crate::error::{Error, Result};
use std::fmt;

/// The Bluetooth Base UUID: 16- and 32-bit assigned numbers are the high bits
/// of `0000xxxx-0000-1000-8000-00805f9b34fb`.
const BASE_SUFFIX: &str = "-0000-1000-8000-00805f9b34fb";

/// The base with its first eight digits zeroed, which is what the assigned
/// number is written over.
const BASE: [u8; 36] = *b"00000000-0000-1000-8000-00805f9b34fb";

/// Where the `n`th hex digit of a canonical UUID sits, stepping over the four
/// dashes at 8, 13, 18 and 23.
const fn hex_position(n: usize) -> usize {
    n + match n {
        0..=7 => 0,
        8..=11 => 1,
        12..=15 => 2,
        16..=19 => 3,
        _ => 4,
    }
}

/// A Bluetooth UUID in canonical 128-bit lowercase form.
///
/// The canonical text, not a number: 36 ASCII bytes, which is what every
/// platform and every comparison in the specification works in. Storing it
/// rather than formatting it on demand makes the type [`Copy`] and — the part
/// that matters more — lets an assigned number be a `const`, so
/// [`services::HEART_RATE`] is a `BluetoothUuid` you can pass anywhere a UUID
/// goes, and match on.
///
/// It used to be a `String`, which meant a `format!` for every service,
/// characteristic and descriptor discovered on every connection, and meant the
/// assigned numbers had to be `u16` because a `String` cannot be a `const`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BluetoothUuid([u8; 36]);

impl BluetoothUuid {
    /// A 16-bit assigned number, e.g. `0x180F` → the Battery Service.
    pub const fn from_u16(v: u16) -> Self {
        Self::from_u32(v as u32)
    }

    /// A 32-bit assigned number.
    ///
    /// `const`, which is the whole point: the assigned-number tables below are
    /// built with it at compile time.
    pub const fn from_u32(v: u32) -> Self {
        let mut out = BASE;
        // Written by hand rather than with `format!`, which is not `const`.
        let mut i = 0;
        while i < 8 {
            let nibble = ((v >> (28 - i * 4)) & 0xf) as u8;
            out[i] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            };
            i += 1;
        }
        Self(out)
    }

    /// A full 128-bit UUID.
    ///
    /// `const` and infallible, which together are the point: every
    /// vendor-defined attribute is a 128-bit UUID outside the assigned-numbers
    /// registry, and without this the only way to name one was
    /// [`parse`](Self::parse) — so a caller holding a compile-time constant
    /// got back a `Result` it could only `expect`, and could not build one in
    /// a `const` at all.
    ///
    /// ```
    /// # use webbluetooth_core::uuid::BluetoothUuid;
    /// const MUSE_CONTROL: BluetoothUuid =
    ///     BluetoothUuid::from_u128(0x273e0001_4c4d_454d_96be_f03bac821358);
    /// assert_eq!(MUSE_CONTROL.as_str(), "273e0001-4c4d-454d-96be-f03bac821358");
    /// ```
    ///
    /// Also the bridge from any other crate's UUID type: `uuid::Uuid`,
    /// `windows::core::GUID` and `java.util.UUID` all reduce to a `u128`.
    pub const fn from_u128(v: u128) -> Self {
        let mut out = BASE;
        // Every one of the 32 hex positions is written, so the only thing kept
        // from BASE is where its four dashes are.
        let mut n = 0;
        while n < 32 {
            let nibble = ((v >> (124 - n * 4)) & 0xf) as u8;
            out[hex_position(n)] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            };
            n += 1;
        }
        Self(out)
    }

    /// This UUID as a 128-bit number — the inverse of
    /// [`from_u128`](Self::from_u128).
    ///
    /// Total, where [`as_u16`](Self::as_u16) is partial: every UUID has a
    /// 128-bit value and only some have an assigned number.
    pub const fn as_u128(&self) -> u128 {
        let mut value = 0u128;
        let mut n = 0;
        while n < 32 {
            let byte = self.0[hex_position(n)];
            // Lowercase by construction, so `a..f` is the only letter range.
            let nibble = if byte >= b'a' {
                byte - b'a' + 10
            } else {
                byte - b'0'
            };
            value = (value << 4) | nibble as u128;
            n += 1;
        }
        value
    }

    /// Take a 36-character dashed UUID, lowercasing as it goes.
    fn from_canonical(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() != 36 {
            return None;
        }
        let mut out = [0u8; 36];
        for (i, &c) in bytes.iter().enumerate() {
            out[i] = match i {
                8 | 13 | 18 | 23 if c == b'-' => c,
                8 | 13 | 18 | 23 => return None,
                _ if c.is_ascii_hexdigit() => c.to_ascii_lowercase(),
                _ => return None,
            };
        }
        Some(Self(out))
    }

    /// Parse a UUID in any of the forms the spec accepts: a 16-bit (`"180f"`,
    /// `"0x180F"`), 32-bit, or full 128-bit dashed string; or one of the
    /// assigned names in [`services`], [`characteristics`] or [`descriptors`]
    /// (`"battery_service"`).
    ///
    /// # Names that exist in more than one namespace
    ///
    /// The Bluetooth assigned-numbers registry reuses some names across
    /// namespaces — `current_time` is both the service `0x1805` and the
    /// characteristic `0x2A2B`. JavaScript keeps them apart with separate
    /// `BluetoothUUID.getService` / `getCharacteristic` / `getDescriptor`
    /// functions; this one lookup resolves **service, then characteristic, then
    /// descriptor**, so `parse("current_time")` is the service. When the
    /// namespace matters, call [`services::lookup`],
    /// [`characteristics::lookup`] or [`descriptors::lookup`] directly.
    pub fn parse(s: &str) -> Result<Self> {
        let t = s.trim();

        if let Some(u) = services::lookup(t)
            .or_else(|| characteristics::lookup(t))
            .or_else(|| descriptors::lookup(t))
        {
            return Ok(u);
        }

        let hex = t
            .strip_prefix("0x")
            .or_else(|| t.strip_prefix("0X"))
            .unwrap_or(t);

        // 16- or 32-bit assigned number.
        if !hex.contains('-')
            && (1..=8).contains(&hex.len())
            && hex.chars().all(|c| c.is_ascii_hexdigit())
        {
            let v = u32::from_str_radix(hex, 16)
                .map_err(|_| Error::Security(format!("not a valid UUID: {s:?}")))?;
            return Ok(if hex.len() <= 4 {
                Self::from_u16(v as u16)
            } else {
                Self::from_u32(v)
            });
        }

        // Full 128-bit dashed form.
        if let Some(uuid) = Self::from_canonical(hex) {
            return Ok(uuid);
        }

        Err(Error::Security(format!(
            "not a valid UUID or known name: {s:?} — expected a 16/32-bit hex value, \
             a 128-bit dashed UUID, or an assigned name like \"battery_service\""
        )))
    }

    /// The canonical 128-bit lowercase string.
    #[inline]
    pub fn as_str(&self) -> &str {
        // Every constructor writes ASCII and nothing else can reach the bytes.
        std::str::from_utf8(&self.0).expect("a UUID is ASCII by construction")
    }

    /// The 16-bit assigned number, if this UUID is in the Bluetooth Base range.
    pub fn as_u16(&self) -> Option<u16> {
        let s = self.as_str();
        if !s.ends_with(BASE_SUFFIX) || !s.starts_with("0000") {
            return None;
        }
        u16::from_str_radix(&s[4..8], 16).ok()
    }

    /// Build from a `CBUUID`'s string, whatever width it came back as.
    ///
    /// Apple-only: `CBUUID.UUIDString` is not canonical, so it has to come back
    /// through the parser. BlueZ always reports the full 128-bit form.
    #[cfg(target_vendor = "apple")]
    pub fn from_cbuuid_string(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl fmt::Display for BluetoothUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lowercase, as [`Display`](fmt::Display) — the canonical form, and the only
/// one this type stores.
impl fmt::LowerHex for BluetoothUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Uppercase, for the places that use it.
///
/// The specification canonicalises to lowercase and so does this type, which
/// is what makes a blocklist lookup or an allowlist check immune to the
/// spelling a platform hands back. But the platforms themselves are not
/// consistent — `CBUUID.UUIDString` is uppercase, and so is most vendor
/// documentation — so rendering one that way is a real need, and doing it with
/// `to_ascii_uppercase()` on the output of `as_str()` is an allocation and a
/// chance to forget.
///
/// `{:X}` is where the `uuid` crate puts the same thing, which is the
/// convention to follow rather than invent a method name.
///
/// ```
/// # use webbluetooth_core::uuid::services;
/// assert_eq!(
///     format!("{:X}", services::BATTERY_SERVICE),
///     "0000180F-0000-1000-8000-00805F9B34FB",
/// );
/// assert_eq!(
///     format!("{}", services::BATTERY_SERVICE),
///     "0000180f-0000-1000-8000-00805f9b34fb",
/// );
/// ```
impl fmt::UpperHex for BluetoothUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut upper = self.0;
        upper.make_ascii_uppercase();
        f.write_str(std::str::from_utf8(&upper).expect("a UUID is ASCII by construction"))
    }
}

/// The text, not the 36 bytes a derive would print.
impl fmt::Debug for BluetoothUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BluetoothUuid({})", self.as_str())
    }
}

impl std::str::FromStr for BluetoothUuid {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl TryFrom<&str> for BluetoothUuid {
    type Error = Error;
    fn try_from(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl From<u128> for BluetoothUuid {
    fn from(v: u128) -> Self {
        Self::from_u128(v)
    }
}

impl From<u16> for BluetoothUuid {
    fn from(v: u16) -> Self {
        Self::from_u16(v)
    }
}

/// Anything that can name a GATT attribute: a [`BluetoothUuid`], a 16-bit
/// number, or a string in any accepted form.
///
/// This is what lets `get_primary_service(0x180f)`,
/// `get_primary_service("battery_service")` and
/// `get_primary_service(uuid)` all work, the way the JavaScript API's
/// `BluetoothServiceUUID` union does.
pub trait IntoUuid {
    /// Resolve to a UUID, or fail with the reason it could not.
    fn into_uuid(self) -> Result<BluetoothUuid>;
}

impl IntoUuid for BluetoothUuid {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        Ok(self)
    }
}
impl IntoUuid for &BluetoothUuid {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        Ok(*self)
    }
}
impl IntoUuid for u16 {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        Ok(BluetoothUuid::from_u16(self))
    }
}
impl IntoUuid for u32 {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        Ok(BluetoothUuid::from_u32(self))
    }
}
impl IntoUuid for u128 {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        Ok(BluetoothUuid::from_u128(self))
    }
}
impl IntoUuid for &str {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        BluetoothUuid::parse(self)
    }
}
impl IntoUuid for &String {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        BluetoothUuid::parse(self)
    }
}
impl IntoUuid for String {
    fn into_uuid(self) -> Result<BluetoothUuid> {
        BluetoothUuid::parse(&self)
    }
}

/// Anything that can name *an optional* GATT attribute: everything
/// [`IntoUuid`] accepts, plus `None` for "all of them".
///
/// This is what the plural getters take, so that
/// `get_primary_services(None)`, `get_primary_services(services::HEART_RATE)`
/// and `get_primary_services(Some(services::HEART_RATE))` all work.
///
/// # Why exactly one `Option` shape
///
/// `None` carries no type, so it can only be inferred when exactly one
/// `Option<T>` is acceptable. Accepting `Option<u16>` as well as
/// `Option<BluetoothUuid>` makes `None` ambiguous — `cannot infer type of the
/// type parameter T` — and `None` is what people write. So the `Option` impl
/// is for [`BluetoothUuid`] alone, which costs nothing now that the assigned
/// numbers *are* `BluetoothUuid`s: `Some(services::HEART_RATE)` is already an
/// `Option<BluetoothUuid>`. Loose forms go in bare, without the `Some`.
pub trait IntoOptionalUuid {
    /// Resolve to a UUID, or to `None` meaning "no filter".
    fn into_optional_uuid(self) -> Result<Option<BluetoothUuid>>;
}

impl<T: IntoUuid> IntoOptionalUuid for T {
    fn into_optional_uuid(self) -> Result<Option<BluetoothUuid>> {
        Ok(Some(self.into_uuid()?))
    }
}

impl IntoOptionalUuid for Option<BluetoothUuid> {
    fn into_optional_uuid(self) -> Result<Option<BluetoothUuid>> {
        Ok(self)
    }
}

/// Generate an assigned-numbers table plus its name lookup.
/// The assigned-number registries, vendored.
///
/// The constants below are the ergonomic half of this: `services::HEART_RATE`
/// is a [`BluetoothUuid`] you can pass anywhere a UUID goes, and — because the
/// type is a plain byte array with derived `PartialEq` — match on. Its 16-bit
/// number is still a question you can ask, with [`BluetoothUuid::as_u16`].
/// They are not the *complete* half — the Bluetooth
/// SIG assigns hundreds of names, and typing them all out is how a
/// transcription error gets in. So the names come from the registry files and
/// the constants are checked against them, rather than the constants being the
/// only thing that resolves.
///
/// `BluetoothUUID.getService("heart_rate")` is what this serves: the
/// specification says the name is resolved against these files, so every name
/// they define resolves here.
///
/// Source: <https://github.com/WebBluetoothCG/registries>
mod registry {
    use super::BluetoothUuid;
    use std::collections::HashMap;
    use std::sync::OnceLock;

    pub(super) const SERVICES: &str = include_str!("../spec/gatt-assigned-services.txt");
    pub(super) const CHARACTERISTICS: &str =
        include_str!("../spec/gatt-assigned-characteristics.txt");
    pub(super) const DESCRIPTORS: &str = include_str!("../spec/gatt-assigned-descriptors.txt");

    /// `name 0000xxxx-0000-1000-8000-00805f9b34fb`, one per line, `#` comments.
    ///
    /// A line that does not parse is skipped rather than failing the build: a
    /// name that does not resolve is an error at the call site, which is what
    /// an unknown name gives anyway. This is the opposite of the blocklist,
    /// where an unreadable file has to block everything — here, failing closed
    /// would mean the whole table vanishing over one bad line.
    fn parse(source: &str) -> HashMap<&str, BluetoothUuid> {
        let mut out = HashMap::new();
        for line in source.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, uuid)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            if let Some(uuid) = literal(uuid.trim()) {
                out.insert(name, uuid);
            }
        }
        out
    }

    /// A 128-bit dashed UUID, and nothing else.
    ///
    /// Deliberately *not* [`BluetoothUuid::parse`], which would be the obvious
    /// thing to call and deadlocks: `parse` resolves assigned names first, so
    /// it calls back into the very table being built, re-entering
    /// `OnceLock::get_or_init` from inside its own initialiser. That does not
    /// fail — it hangs, and a hanging test looks exactly like a slow one.
    ///
    /// Every line in these files is a full dashed UUID, so nothing is lost by
    /// accepting only that.
    fn literal(text: &str) -> Option<BluetoothUuid> {
        let ok = text.len() == 36
            && text.chars().enumerate().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit(),
            });
        ok.then(|| BluetoothUuid::from_canonical(text)).flatten()
    }

    macro_rules! table {
        ($name:ident, $source:ident) => {
            pub(super) fn $name(key: &str) -> Option<BluetoothUuid> {
                static TABLE: OnceLock<HashMap<&str, BluetoothUuid>> = OnceLock::new();
                TABLE.get_or_init(|| parse($source)).get(key).cloned()
            }
        };
    }
    table!(services, SERVICES);
    table!(characteristics, CHARACTERISTICS);
    table!(descriptors, DESCRIPTORS);

    /// Every `(name, uuid)` in one registry, for the tests that check the
    /// constants against it.
    #[cfg(test)]
    pub(super) fn entries(source: &str) -> Vec<(&str, BluetoothUuid)> {
        let mut all: Vec<_> = parse(source).into_iter().collect();
        all.sort();
        all
    }
}

macro_rules! assigned {
    ($(#[$m:meta])* $module:ident { $($name:ident = $value:expr, $key:literal;)* }) => {
        $(#[$m])*
        pub mod $module {
            use super::BluetoothUuid;
            $(
                #[doc = concat!("`", $key, "` — `0x", stringify!($value), "`")]
                pub const $name: BluetoothUuid = BluetoothUuid::from_u16($value);
            )*

            /// Resolve an assigned name, as `BluetoothUUID.getService` does.
            ///
            /// The constants below are tried first — a `match` on a string
            /// literal against values built at compile time, with nothing
            /// allocated — and the vendored registry answers for
            /// every other name the Bluetooth SIG has assigned. The two cannot
            /// disagree: a test asserts every constant matches the registry.
            pub fn lookup(name: &str) -> Option<BluetoothUuid> {
                match name {
                    $($key => Some($name),)*
                    _ => super::registry::$module(name),
                }
            }

            /// Every `(name, uuid)` in this table.
            pub const ALL: &[(&str, BluetoothUuid)] = &[$(($key, $name),)*];
        }
    };
}

assigned! {
    /// GATT services, by assigned name.
    services {
        GENERIC_ACCESS = 0x1800, "generic_access";
        GENERIC_ATTRIBUTE = 0x1801, "generic_attribute";
        IMMEDIATE_ALERT = 0x1802, "immediate_alert";
        LINK_LOSS = 0x1803, "link_loss";
        TX_POWER = 0x1804, "tx_power";
        CURRENT_TIME = 0x1805, "current_time";
        REFERENCE_TIME_UPDATE = 0x1806, "reference_time_update";
        NEXT_DST_CHANGE = 0x1807, "next_dst_change";
        GLUCOSE = 0x1808, "glucose";
        HEALTH_THERMOMETER = 0x1809, "health_thermometer";
        DEVICE_INFORMATION = 0x180A, "device_information";
        HEART_RATE = 0x180D, "heart_rate";
        PHONE_ALERT_STATUS = 0x180E, "phone_alert_status";
        BATTERY_SERVICE = 0x180F, "battery_service";
        BLOOD_PRESSURE = 0x1810, "blood_pressure";
        ALERT_NOTIFICATION = 0x1811, "alert_notification";
        HUMAN_INTERFACE_DEVICE = 0x1812, "human_interface_device";
        SCAN_PARAMETERS = 0x1813, "scan_parameters";
        RUNNING_SPEED_AND_CADENCE = 0x1814, "running_speed_and_cadence";
        AUTOMATION_IO = 0x1815, "automation_io";
        CYCLING_SPEED_AND_CADENCE = 0x1816, "cycling_speed_and_cadence";
        CYCLING_POWER = 0x1818, "cycling_power";
        LOCATION_AND_NAVIGATION = 0x1819, "location_and_navigation";
        ENVIRONMENTAL_SENSING = 0x181A, "environmental_sensing";
        BODY_COMPOSITION = 0x181B, "body_composition";
        USER_DATA = 0x181C, "user_data";
        WEIGHT_SCALE = 0x181D, "weight_scale";
        BOND_MANAGEMENT = 0x181E, "bond_management";
        CONTINUOUS_GLUCOSE_MONITORING = 0x181F, "continuous_glucose_monitoring";
        INTERNET_PROTOCOL_SUPPORT = 0x1820, "internet_protocol_support";
        INDOOR_POSITIONING = 0x1821, "indoor_positioning";
        PULSE_OXIMETER = 0x1822, "pulse_oximeter";
        HTTP_PROXY = 0x1823, "http_proxy";
        TRANSPORT_DISCOVERY = 0x1824, "transport_discovery";
        OBJECT_TRANSFER = 0x1825, "object_transfer";
        FITNESS_MACHINE = 0x1826, "fitness_machine";
        MESH_PROVISIONING = 0x1827, "mesh_provisioning";
        MESH_PROXY = 0x1828, "mesh_proxy";
        RECONNECTION_CONFIGURATION = 0x1829, "reconnection_configuration";
    }
}

assigned! {
    /// GATT characteristics, by assigned name.
    characteristics {
        DEVICE_NAME = 0x2A00, "gap.device_name";
        APPEARANCE = 0x2A01, "gap.appearance";
        PERIPHERAL_PRIVACY_FLAG = 0x2A02, "gap.peripheral_privacy_flag";
        RECONNECTION_ADDRESS = 0x2A03, "gap.reconnection_address";
        PREFERRED_CONNECTION_PARAMETERS = 0x2A04, "gap.peripheral_preferred_connection_parameters";
        SERVICE_CHANGED = 0x2A05, "gatt.service_changed";
        ALERT_LEVEL = 0x2A06, "alert_level";
        TX_POWER_LEVEL = 0x2A07, "tx_power_level";
        DATE_TIME = 0x2A08, "date_time";
        BATTERY_LEVEL = 0x2A19, "battery_level";
        TEMPERATURE_MEASUREMENT = 0x2A1C, "temperature_measurement";
        INTERMEDIATE_TEMPERATURE = 0x2A1E, "intermediate_temperature";
        SYSTEM_ID = 0x2A23, "system_id";
        MODEL_NUMBER_STRING = 0x2A24, "model_number_string";
        SERIAL_NUMBER_STRING = 0x2A25, "serial_number_string";
        FIRMWARE_REVISION_STRING = 0x2A26, "firmware_revision_string";
        HARDWARE_REVISION_STRING = 0x2A27, "hardware_revision_string";
        SOFTWARE_REVISION_STRING = 0x2A28, "software_revision_string";
        MANUFACTURER_NAME_STRING = 0x2A29, "manufacturer_name_string";
        CURRENT_TIME = 0x2A2B, "current_time";
        BLOOD_PRESSURE_MEASUREMENT = 0x2A35, "blood_pressure_measurement";
        HEART_RATE_MEASUREMENT = 0x2A37, "heart_rate_measurement";
        BODY_SENSOR_LOCATION = 0x2A38, "body_sensor_location";
        HEART_RATE_CONTROL_POINT = 0x2A39, "heart_rate_control_point";
        BLOOD_PRESSURE_FEATURE = 0x2A49, "blood_pressure_feature";
        PNP_ID = 0x2A50, "pnp_id";
        CSC_MEASUREMENT = 0x2A5B, "csc_measurement";
        CSC_FEATURE = 0x2A5C, "csc_feature";
        SENSOR_LOCATION = 0x2A5D, "sensor_location";
        RSC_MEASUREMENT = 0x2A53, "rsc_measurement";
        RSC_FEATURE = 0x2A54, "rsc_feature";
    }
}

assigned! {
    /// GATT descriptors, by assigned name.
    descriptors {
        CHARACTERISTIC_EXTENDED_PROPERTIES = 0x2900, "gatt.characteristic_extended_properties";
        CHARACTERISTIC_USER_DESCRIPTION = 0x2901, "gatt.characteristic_user_description";
        CLIENT_CHARACTERISTIC_CONFIGURATION = 0x2902, "gatt.client_characteristic_configuration";
        SERVER_CHARACTERISTIC_CONFIGURATION = 0x2903, "gatt.server_characteristic_configuration";
        CHARACTERISTIC_PRESENTATION_FORMAT = 0x2904, "gatt.characteristic_presentation_format";
        CHARACTERISTIC_AGGREGATE_FORMAT = 0x2905, "gatt.characteristic_aggregate_format";
        VALID_RANGE = 0x2906, "valid_range";
        EXTERNAL_REPORT_REFERENCE = 0x2907, "external_report_reference";
        REPORT_REFERENCE = 0x2908, "report_reference";
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 128-bit form has to round-trip: it is how every vendor-defined
    /// attribute is named, and nothing else canonicalises one.
    #[test]
    fn round_trips_a_vendor_uuid_through_u128() {
        const MUSE_CONTROL: u128 = 0x273e0001_4c4d_454d_96be_f03bac821358;
        // In a `const`, which is half the reason the constructor exists.
        const UUID: BluetoothUuid = BluetoothUuid::from_u128(MUSE_CONTROL);

        assert_eq!(UUID.as_str(), "273e0001-4c4d-454d-96be-f03bac821358");
        assert_eq!(UUID.as_u128(), MUSE_CONTROL);
        // Spelled out, it reaches the same value — which is what makes the two
        // constructors interchangeable.
        assert_eq!(BluetoothUuid::parse(UUID.as_str()).unwrap(), UUID);
    }

    /// Leading zeroes and the dash positions are where a hand-rolled formatter
    /// goes wrong, and both are silent when they do.
    #[test]
    fn places_every_digit_of_a_short_value() {
        let uuid = BluetoothUuid::from_u128(0x180f);
        assert_eq!(uuid.as_str(), "00000000-0000-0000-0000-00000000180f");
        assert_eq!(uuid.as_u128(), 0x180f);

        // The assigned-number constructors and `as_u128` have to agree, which
        // is the other direction the two representations meet in.
        let battery = BluetoothUuid::from_u16(0x180F);
        assert_eq!(battery.as_u128(), 0x0000180f_0000_1000_8000_00805f9b34fb);
        assert_eq!(BluetoothUuid::from_u128(battery.as_u128()), battery);
    }

    /// Case is what an allowlist or a blocklist lookup would be fooled by.
    ///
    /// `CBUUID.UUIDString` is uppercase and BlueZ's is lowercase, so the same
    /// attribute arrives spelled two ways depending on the platform. Equality
    /// is the shallow half of this; the *hash* is the half that matters,
    /// because the grant store and the registry are maps.
    #[test]
    fn case_cannot_change_a_uuid_s_identity() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let hash = |u: &BluetoothUuid| {
            let mut h = DefaultHasher::new();
            u.hash(&mut h);
            h.finish()
        };

        let upper = BluetoothUuid::parse("0000180F-0000-1000-8000-00805F9B34FB").unwrap();
        let lower = BluetoothUuid::parse("0000180f-0000-1000-8000-00805f9b34fb").unwrap();
        let mixed = BluetoothUuid::parse("0000180F-0000-1000-8000-00805f9B34fb").unwrap();

        assert_eq!(upper, lower);
        assert_eq!(mixed, lower);
        assert_eq!(hash(&upper), hash(&lower), "equal UUIDs must hash equal");
        assert_eq!(hash(&mixed), hash(&lower));
        // And the same for the short forms and the constant.
        assert_eq!(BluetoothUuid::parse("180F").unwrap(), lower);
        assert_eq!(BluetoothUuid::parse("0X180F").unwrap(), lower);
        assert_eq!(services::BATTERY_SERVICE, lower);
    }

    /// Nothing may construct an uppercase one, whichever door it comes in.
    ///
    /// `as_u128` reads `a..f` and nothing else — it is a `const fn` and says
    /// lowercase is an invariant rather than checking it — so an uppercase
    /// byte reaching the array would not be a cosmetic problem.
    #[test]
    fn every_constructor_stores_lowercase() {
        let vendor = 0x6e40_0001_b5a3_f393_e0a9_e50e_24dc_ca9eu128;
        let built = [
            BluetoothUuid::from_u16(0x180F),
            BluetoothUuid::from_u32(0xFFFF_180F),
            BluetoothUuid::from_u128(vendor),
            BluetoothUuid::parse("0000180F-0000-1000-8000-00805F9B34FB").unwrap(),
            BluetoothUuid::parse("FFFF180F").unwrap(),
            services::BATTERY_SERVICE,
            characteristics::BATTERY_LEVEL,
        ];
        for uuid in built {
            let s = uuid.as_str();
            assert_eq!(s, s.to_ascii_lowercase(), "{s} is not stored lowercase");
        }
        // The one that would silently misread an uppercase byte.
        assert_eq!(BluetoothUuid::from_u128(vendor).as_u128(), vendor);
    }

    /// Uppercase is available on demand, and is still the same UUID.
    #[test]
    fn upper_hex_renders_uppercase_and_round_trips() {
        let uuid = services::BATTERY_SERVICE;
        let upper = format!("{uuid:X}");
        assert_eq!(upper, "0000180F-0000-1000-8000-00805F9B34FB");
        assert_eq!(format!("{uuid:x}"), uuid.as_str());
        assert_eq!(format!("{uuid}"), uuid.as_str());
        assert_eq!(BluetoothUuid::parse(&upper).unwrap(), uuid);
    }

    #[test]
    fn canonicalises_every_accepted_spelling_to_one_string() {
        let expected = "0000180f-0000-1000-8000-00805f9b34fb";
        for spelling in [
            "180F",
            "180f",
            "0x180F",
            "0000180F-0000-1000-8000-00805F9B34FB",
            "0000180f-0000-1000-8000-00805f9b34fb",
            "battery_service",
        ] {
            assert_eq!(
                BluetoothUuid::parse(spelling).unwrap().as_str(),
                expected,
                "{spelling:?} did not canonicalise"
            );
        }
        assert_eq!(BluetoothUuid::from_u16(0x180F).as_str(), expected);
    }

    #[test]
    fn recovers_the_assigned_number() {
        assert_eq!(BluetoothUuid::from_u16(0x180F).as_u16(), Some(0x180F));
        // A vendor UUID is not in the Bluetooth Base range.
        let vendor = BluetoothUuid::parse("6e400001-b5a3-f393-e0a9-e50e24dcca9e").unwrap();
        assert_eq!(vendor.as_u16(), None);
    }

    #[test]
    fn rejects_nonsense() {
        for bad in [
            "",
            "xyz",
            "180G",
            "0000180f-0000-1000-8000",
            "not_a_service",
        ] {
            assert!(
                BluetoothUuid::parse(bad).is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn assigned_names_are_unique_within_each_namespace() {
        for table in [services::ALL, characteristics::ALL, descriptors::ALL] {
            let mut seen = std::collections::HashSet::new();
            for (name, _) in table {
                assert!(
                    seen.insert(*name),
                    "duplicate assigned name {name:?} in one namespace"
                );
            }
        }
    }

    #[test]
    fn every_assigned_name_resolves_in_its_own_namespace() {
        for (name, uuid) in services::ALL {
            assert_eq!(services::lookup(name), Some(*uuid));
        }
        for (name, uuid) in characteristics::ALL {
            assert_eq!(characteristics::lookup(name), Some(*uuid));
        }
        for (name, uuid) in descriptors::ALL {
            assert_eq!(descriptors::lookup(name), Some(*uuid));
        }
    }

    /// The point of vendoring: every constant written out by hand is checked
    /// against the registry it was copied from.
    ///
    /// This is the test that would have caught the three wrong blocklist
    /// entries. A constant that disagrees with the registry resolves silently
    /// to the wrong attribute — a read of the wrong characteristic, not an
    /// error — which is the worst shape a bug can take here.
    #[test]
    fn every_hand_written_constant_agrees_with_the_registry() {
        for (table, source, what) in [
            (services::ALL, registry::SERVICES, "service"),
            (
                characteristics::ALL,
                registry::CHARACTERISTICS,
                "characteristic",
            ),
            (descriptors::ALL, registry::DESCRIPTORS, "descriptor"),
        ] {
            let official: std::collections::HashMap<_, _> =
                registry::entries(source).into_iter().collect();
            for (name, uuid) in table {
                let expected = official.get(name).unwrap_or_else(|| {
                    panic!("{what} {name:?} is not in the vendored registry at all")
                });
                assert_eq!(
                    uuid, expected,
                    "{what} {name:?} is {uuid} here but {expected} in the registry"
                );
            }
        }
    }

    /// The registry is the source of truth, so a name only it knows still
    /// resolves. Before it was vendored, two thirds of the standard's names
    /// did not.
    #[test]
    fn names_outside_the_hand_written_table_still_resolve() {
        // In the registry, deliberately not in the constants above.
        let heart_rate = characteristics::lookup("heart_rate_measurement")
            .expect("a registry characteristic must resolve");
        assert_eq!(heart_rate, BluetoothUuid::from_u16(0x2A37));

        assert!(
            services::lookup("automation_io").is_some(),
            "a registry service must resolve"
        );
        assert!(
            descriptors::lookup("gatt.characteristic_user_description").is_some(),
            "a registry descriptor must resolve"
        );
        // And an invented name still does not.
        assert!(characteristics::lookup("not_a_real_characteristic").is_none());
    }

    /// The vendored files have to actually be there and parse, or every
    /// lookup silently falls back to the 79 constants.
    #[test]
    fn the_vendored_registries_are_present_and_populated() {
        assert!(registry::entries(registry::SERVICES).len() >= 30);
        assert!(registry::entries(registry::CHARACTERISTICS).len() >= 200);
        assert!(registry::entries(registry::DESCRIPTORS).len() >= 10);
    }

    /// The registry reuses names across namespaces; `parse` resolves
    /// service-first, and the per-namespace lookups stay exact.
    #[test]
    fn cross_namespace_names_resolve_service_first() {
        let collisions: Vec<&str> = services::ALL
            .iter()
            .filter(|(n, _)| {
                characteristics::lookup(n).is_some() || descriptors::lookup(n).is_some()
            })
            .map(|(n, _)| *n)
            .collect();
        assert!(
            collisions.contains(&"current_time"),
            "expected the known collision"
        );

        for name in collisions {
            assert_eq!(
                BluetoothUuid::parse(name).unwrap(),
                services::lookup(name).unwrap(),
                "{name:?} should resolve to the service"
            );
        }
        // …but the characteristic is still reachable, and is a different UUID.
        assert_eq!(
            characteristics::lookup("current_time"),
            Some(characteristics::CURRENT_TIME)
        );
        assert_ne!(
            services::lookup("current_time"),
            characteristics::lookup("current_time")
        );
    }
}
