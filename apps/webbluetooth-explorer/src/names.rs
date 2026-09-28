//! Friendly names for assigned UUIDs.
//!
//! A raw 128-bit UUID tells an operator nothing, and the great majority of what
//! a device publishes is in the Bluetooth SIG's assigned-numbers list. The
//! library already vendors that list keyed by name, for `get_characteristic`
//! and friends; this inverts it, which is the direction an explorer reads in.
//!
//! Anything not in the list keeps its UUID. That is not a gap — a vendor's
//! private service has no assigned name to find, and inventing one would be
//! worse than printing the number.

use std::collections::HashMap;
use std::sync::OnceLock;
use webbluetooth::uuid::{characteristics, descriptors, services, BluetoothUuid};

/// What kind of attribute a UUID is being looked up as.
///
/// The three registries are separate and do overlap: `0x2A00` is the Device
/// Name characteristic, and a service with that number would be something
/// else entirely. Looking up in the right one keeps a wrong label from being
/// confidently printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A GATT service.
    Service,
    /// A GATT characteristic.
    Characteristic,
    /// A characteristic descriptor.
    Descriptor,
}

fn table(kind: Kind) -> &'static HashMap<u128, &'static str> {
    static SERVICES: OnceLock<HashMap<u128, &'static str>> = OnceLock::new();
    static CHARACTERISTICS: OnceLock<HashMap<u128, &'static str>> = OnceLock::new();
    static DESCRIPTORS: OnceLock<HashMap<u128, &'static str>> = OnceLock::new();

    fn build(entries: &'static [(&'static str, BluetoothUuid)]) -> HashMap<u128, &'static str> {
        entries
            .iter()
            .map(|(name, u)| (u.as_u128(), *name))
            .collect()
    }

    match kind {
        Kind::Service => SERVICES.get_or_init(|| build(services::ALL)),
        Kind::Characteristic => CHARACTERISTICS.get_or_init(|| build(characteristics::ALL)),
        Kind::Descriptor => DESCRIPTORS.get_or_init(|| build(descriptors::ALL)),
    }
}

/// The assigned name for `uuid`, title-cased — `Heart Rate Measurement`.
///
/// `None` when the SIG has not assigned one, which is every vendor UUID.
pub fn assigned(uuid: &BluetoothUuid, kind: Kind) -> Option<String> {
    table(kind).get(&uuid.as_u128()).map(|name| titlecase(name))
}

/// `heart_rate_measurement` → `Heart Rate Measurement`.
///
/// The registry's keys are the identifiers the specification uses, which are
/// not what anybody wants to read down the side of a list.
fn titlecase(key: &str) -> String {
    // The descriptor registry namespaces its keys —
    // `gatt.client_characteristic_configuration` — and the namespace is not
    // part of the name. Left in, it came out as "Gatt.client Characteristic
    // Configuration", which is not what anything calls it.
    let key = key.rsplit_once('.').map_or(key, |(_, name)| name);
    key.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            // Initialisms the SIG writes in lower case, and which look like
            // typos title-cased into `Uuid` or `Rssi`.
            match word {
                "uuid" | "id" | "rssi" | "tx" | "ppcp" | "hid" | "dst" | "ieee" | "uri" | "url"
                | "utc" | "cgm" | "ess" | "ots" | "ip" | "ots6" => word.to_uppercase(),
                _ => {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How a UUID is written in a list: short form when it has one.
///
/// A 16-bit assigned number written out in full is 36 characters of which 32
/// are the Bluetooth base UUID, repeated down every row.
pub fn short(uuid: &BluetoothUuid) -> String {
    match uuid.as_u16() {
        Some(v) => format!("0x{v:04X}"),
        None => uuid.as_str().to_owned(),
    }
}

/// A label for a list row: the assigned name, else the UUID.
pub fn label(uuid: &BluetoothUuid, kind: Kind) -> String {
    assigned(uuid, kind).unwrap_or_else(|| short(uuid))
}

/// Names given to UUIDs by the person using the program.
///
/// The Bluetooth SIG names about four hundred attributes. A device's *own*
/// services are the interesting ones and the SIG has named none of them, so
/// the tree fills up with bare 128-bit numbers — which is the one thing the
/// `unrestricted` grant exists to let you see, and the one thing you then
/// cannot read. Naming them yourself is the missing half.
///
/// Checked before the assigned-numbers tables, deliberately: if you have given
/// `0x180F` a better name for your own device, that is the name you meant.
pub type Definitions = std::collections::BTreeMap<(Kind, u128), String>;

/// The name to show for `uuid`: yours, then the SIG's, then the number.
pub fn label_with(definitions: &Definitions, uuid: &BluetoothUuid, kind: Kind) -> String {
    definitions
        .get(&(kind, uuid.as_u128()))
        .cloned()
        .unwrap_or_else(|| label(uuid, kind))
}

/// As [`label_with`], but `None` when nobody has named it — yours or the SIG's.
///
/// For the places that want to show the number *and* a name beside it, and
/// must not print the number twice.
pub fn named_with(definitions: &Definitions, uuid: &BluetoothUuid, kind: Kind) -> Option<String> {
    definitions
        .get(&(kind, uuid.as_u128()))
        .cloned()
        .or_else(|| assigned(uuid, kind))
}

/// The company that owns a manufacturer-data block, where it is one of the
/// handful common enough to recognise on sight.
///
/// Not a vendored copy of the SIG's company-identifier list — that is 3000
/// entries and this program has no other use for it. These are the ones that
/// actually turn up in a room full of advertising devices.
pub fn company(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0000 => "Ericsson",
        0x0001 => "Nokia Mobile Phones",
        0x0002 => "Intel",
        0x0003 => "IBM",
        0x0004 => "Toshiba",
        0x0005 => "3Com",
        0x0006 => "Microsoft",
        0x0007 => "Lucent",
        0x0008 => "Motorola",
        0x000A => "Qualcomm",
        0x000D => "Texas Instruments",
        0x000F => "Broadcom",
        0x0010 => "Mitsubishi",
        0x0013 => "Atmel",
        0x001D => "Qualcomm",
        0x0025 => "NXP",
        0x002D => "Synopsys",
        0x0030 => "ST Microelectronics",
        0x0036 => "MediaTek",
        0x003D => "Dialog Semiconductor",
        0x0046 => "MediaTek",
        0x004C => "Apple",
        0x0059 => "Nordic Semiconductor",
        0x005D => "Realtek",
        0x0065 => "Hewlett-Packard",
        0x0075 => "Samsung",
        0x0078 => "Nike",
        0x0087 => "Garmin",
        0x008A => "BlueRadios",
        0x0094 => "Alpwise",
        0x009E => "Bose",
        0x00C4 => "LG Electronics",
        0x00D2 => "Dialog Semiconductor",
        0x00E0 => "Google",
        0x0104 => "Sony",
        0x0107 => "Polar Electro",
        0x0118 => "Bose",
        0x0131 => "Cypress Semiconductor",
        0x0142 => "Logitech",
        0x0154 => "Suunto",
        0x0157 => "Anhui Huami",
        0x0171 => "Amazon",
        0x0180 => "Fitbit",
        0x01A9 => "Tile",
        0x01D7 => "Xiaomi",
        0x01DA => "Logitech",
        0x0201 => "Espressif",
        0x0224 => "Tile",
        0x0276 => "Bose",
        0x02E5 => "Espressif Systems",
        0x0310 => "SGV Group",
        0x0499 => "Ruuvi Innovations",
        0x054C => "Sony",
        0x0590 => "Withings",
        0x05A7 => "Sonos",
        0x0644 => "Shenzhen Goodix",
        0x06D5 => "Xiaomi",
        0x0757 => "Tuya",
        0x08D6 => "Govee",
        0x0AAB => "Texas Instruments",
        0xFEED => "Tile (reserved)",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigned_names_come_back_readable() {
        assert_eq!(
            assigned(&services::HEART_RATE, Kind::Service).as_deref(),
            Some("Heart Rate")
        );
        assert_eq!(
            assigned(
                &characteristics::HEART_RATE_MEASUREMENT,
                Kind::Characteristic
            )
            .as_deref(),
            Some("Heart Rate Measurement")
        );
    }

    #[test]
    fn a_vendor_uuid_keeps_its_number() {
        let vendor = BluetoothUuid::parse("f000aa00-0451-4000-b000-000000000000").unwrap();
        assert_eq!(assigned(&vendor, Kind::Service), None);
        assert_eq!(label(&vendor, Kind::Service), vendor.as_str());
        assert_eq!(short(&vendor), vendor.as_str());
    }

    /// The three registries overlap numerically, and a label from the wrong one
    /// would be confidently wrong.
    #[test]
    fn the_registries_do_not_answer_for_each_other() {
        let device_name = &characteristics::DEVICE_NAME; // 0x2A00
        assert!(assigned(device_name, Kind::Characteristic).is_some());
        assert_eq!(assigned(device_name, Kind::Service), None);

        assert!(assigned(&services::BATTERY_SERVICE, Kind::Service).is_some());
        assert_eq!(
            assigned(&services::BATTERY_SERVICE, Kind::Characteristic),
            None
        );
    }

    #[test]
    fn short_form_is_used_where_there_is_one() {
        assert_eq!(short(&services::BATTERY_SERVICE), "0x180F");
        assert_eq!(short(&BluetoothUuid::from_u16(0x2A37)), "0x2A37");
    }

    /// The descriptor registry namespaces its keys, and the namespace is not
    /// part of the name anybody uses.
    #[test]
    fn a_namespaced_key_loses_its_namespace() {
        assert_eq!(
            titlecase("gatt.client_characteristic_configuration"),
            "Client Characteristic Configuration"
        );
        assert_eq!(
            assigned(&BluetoothUuid::from_u16(0x2902), Kind::Descriptor).as_deref(),
            Some("Client Characteristic Configuration")
        );
        // A key with no namespace is untouched.
        assert_eq!(titlecase("heart_rate"), "Heart Rate");
    }

    /// The point of the whole feature: a vendor UUID the SIG has never heard of
    /// becomes readable.
    #[test]
    fn a_name_you_gave_beats_the_number() {
        let vendor = BluetoothUuid::parse("f000aa00-0451-4000-b000-000000000000").unwrap();
        let mut definitions = Definitions::new();
        definitions.insert((Kind::Service, vendor.as_u128()), "SensorTag IR".to_owned());

        assert_eq!(
            label_with(&definitions, &vendor, Kind::Service),
            "SensorTag IR"
        );
        assert_eq!(
            named_with(&definitions, &vendor, Kind::Service).as_deref(),
            Some("SensorTag IR")
        );
        // Unnamed still falls back to the number, and `named_with` still says
        // nobody named it.
        let other = BluetoothUuid::parse("f000aa01-0451-4000-b000-000000000000").unwrap();
        assert_eq!(
            label_with(&definitions, &other, Kind::Service),
            other.as_str()
        );
        assert_eq!(named_with(&definitions, &other, Kind::Service), None);
    }

    /// A name you gave wins over the SIG's, because you meant it.
    #[test]
    fn a_name_you_gave_beats_the_assigned_one() {
        let battery = services::BATTERY_SERVICE;
        let mut definitions = Definitions::new();
        definitions.insert((Kind::Service, battery.as_u128()), "Cell gauge".to_owned());

        assert_eq!(
            label_with(&definitions, &battery, Kind::Service),
            "Cell gauge"
        );
        // And it is scoped to the kind it was given for.
        assert_eq!(
            label_with(&definitions, &battery, Kind::Characteristic),
            "0x180F"
        );
    }

    #[test]
    fn initialisms_are_not_title_cased_into_nonsense() {
        assert_eq!(titlecase("tx_power"), "TX Power");
        assert_eq!(titlecase("hid_control_point"), "HID Control Point");
        assert_eq!(titlecase("heart_rate"), "Heart Rate");
    }
}
