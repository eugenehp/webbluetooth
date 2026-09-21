//! The GATT blocklist.
//!
//! Some attributes are too dangerous to expose to code that reached the device
//! through a permission prompt rather than a deliberate install: the HID
//! service would make a keylogger, an unsigned firmware-update service would
//! let a page brick or backdoor the device, and the serial number is a
//! tracking identifier. The Web Bluetooth Community Group maintains the
//! authoritative list; this crate vendors it and refuses the same attributes.
//!
//! The vendored copy is a **snapshot**. Re-run `scripts/update.sh blocklist` to
//! refresh it — the list grows as new device classes turn out to be
//! exploitable, and a stale copy silently under-blocks.
//!
//! Source: <https://github.com/WebBluetoothCG/registries>

use crate::filter::DataPrefix;
use crate::uuid::BluetoothUuid;
use std::collections::HashMap;
use std::sync::OnceLock;

/// What a blocklist entry forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// Not reachable at all — discovery must not return it.
    All,
    /// Readable, but never writable.
    Writes,
    /// Writable, but never readable.
    Reads,
}

/// The vendored registry file, parsed on first use.
const SOURCE: &str = include_str!("../spec/gatt-blocklist.txt");

fn table() -> &'static HashMap<BluetoothUuid, Blocked> {
    static TABLE: OnceLock<HashMap<BluetoothUuid, Blocked>> = OnceLock::new();
    TABLE.get_or_init(|| parse(SOURCE))
}

/// Parse the registry's format: one UUID per line, optionally followed by
/// `exclude-reads` or `exclude-writes`; `#` starts a comment.
fn parse(source: &str) -> HashMap<BluetoothUuid, Blocked> {
    let mut out = HashMap::new();
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(uuid) = parts.next() else { continue };
        let Ok(uuid) = BluetoothUuid::parse(uuid) else {
            continue;
        };
        let blocked = match parts.next() {
            Some("exclude-writes") => Blocked::Writes,
            Some("exclude-reads") => Blocked::Reads,
            _ => Blocked::All,
        };
        out.insert(uuid, blocked);
    }
    out
}

/// How `uuid` is restricted, if at all.
pub fn lookup(uuid: &BluetoothUuid) -> Option<Blocked> {
    table().get(uuid).copied()
}

/// Whether `uuid` may not be reached at all.
pub fn is_blocked(uuid: &BluetoothUuid) -> bool {
    matches!(lookup(uuid), Some(Blocked::All))
}

/// Whether reading `uuid` is forbidden.
pub fn reads_blocked(uuid: &BluetoothUuid) -> bool {
    matches!(lookup(uuid), Some(Blocked::All) | Some(Blocked::Reads))
}

/// Whether writing `uuid` is forbidden.
pub fn writes_blocked(uuid: &BluetoothUuid) -> bool {
    matches!(lookup(uuid), Some(Blocked::All) | Some(Blocked::Writes))
}

// ── The manufacturer data blocklist ─────────────────────────────────────────
//
// The second registry, and the one easiest to overlook: the GATT blocklist
// guards attributes you connect to, this one guards what a device shouts into
// the air. Today it holds a single entry — Apple's iBeacon — because an
// iBeacon's proximity UUID is a location fix, and reading one off a passing
// phone would locate its owner without connecting to anything.
//
// The registry's format is documented in the registries repository:
//
//     manufacturer <company> advdata-<data>/<mask>
//
// The specification also prints a parsing algorithm for this file, and its
// regular expression does not match the file: the expression wants the prefix
// to start with hex digits, but every entry writes it with a literal
// `advdata-` tag, which is what the registry documents and what the spec's own
// `parse an advertising data filter` expects already stripped. Followed
// literally the list parses as an error — and an error blocks *everything*, so
// no manufacturer data would reach any caller and every `manufacturerData`
// filter would throw. That is plainly not the intent, so this parser strips
// the tag and applies the spec's prefix/mask rule to the rest. See
// spec/README.md. The failure direction is kept: an entry we cannot parse
// blocks rather than opens.
const MANUFACTURER_SOURCE: &str = include_str!("../spec/manufacturer-data-blocklist.txt");

/// Company identifier to the advertising-data prefixes forbidden for it.
///
/// `None` in place of the map means the file did not parse. The specification
/// is explicit that an unreadable list blocks everything rather than nothing,
/// so the distinction is preserved rather than collapsed into an empty map.
type ManufacturerTable = Option<HashMap<u16, Vec<DataPrefix>>>;

fn manufacturer_table() -> &'static ManufacturerTable {
    static TABLE: OnceLock<ManufacturerTable> = OnceLock::new();
    TABLE.get_or_init(|| parse_manufacturer(MANUFACTURER_SOURCE))
}

fn parse_manufacturer(source: &str) -> ManufacturerTable {
    fn hex(text: &str) -> Option<Vec<u8>> {
        if text.is_empty() || !text.len().is_multiple_of(2) {
            return None;
        }
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
            .collect()
    }

    let mut out: HashMap<u16, Vec<DataPrefix>> = HashMap::new();
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        if parts.next() != Some("manufacturer") {
            return None;
        }
        // At most four hexadecimal digits, per the registry.
        let company = parts
            .next()
            .filter(|c| c.len() <= 4)
            .and_then(|c| u16::from_str_radix(c, 16).ok())?;
        let (data, mask) = parts.next()?.strip_prefix("advdata-")?.split_once('/')?;
        let (data, mask) = (hex(data)?, hex(mask)?);
        let prefix = DataPrefix::with_mask(data, mask).ok()?;
        out.entry(company).or_default().push(prefix);
    }
    Some(out)
}

/// Whether `data` advertised under `company` must be withheld from callers.
///
/// This is the specification's *blocklisted manufacturer data*.
pub fn manufacturer_data_blocked(company: u16, data: &[u8]) -> bool {
    let Some(table) = manufacturer_table() else {
        return true; // An unreadable list blocks everything.
    };
    table
        .get(&company)
        .is_some_and(|prefixes| prefixes.iter().any(|p| p.matches(data)))
}

/// Whether a caller may filter on `company` with `prefix`.
///
/// This is the specification's *blocklisted manufacturer data filter*, and it
/// is deliberately not the same test as the one above. A filter is refused
/// only when it is a *strict subset* of a blocked prefix — when everything it
/// could ever match is already blocked. A broader filter is allowed through,
/// because the data it returns gets stripped anyway; refusing it would only
/// tell the caller what the blocklist contains.
pub fn manufacturer_filter_blocked(company: u16, prefix: &DataPrefix) -> bool {
    let Some(table) = manufacturer_table() else {
        return true;
    };
    table
        .get(&company)
        .is_some_and(|blocked| blocked.iter().any(|b| prefix.strict_subset_of(b)))
}

/// Every entry in the vendored snapshot.
pub fn entries() -> impl Iterator<Item = (&'static BluetoothUuid, Blocked)> {
    table().iter().map(|(u, b)| (u, *b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uuid::{characteristics, descriptors, services};

    const APPLE: u16 = 0x004C;

    /// The vendored entry: Apple company data whose first byte is `0x02`,
    /// which is what an iBeacon frame starts with.
    #[test]
    fn ibeacon_data_is_blocked_because_it_is_a_location_fix() {
        assert!(
            manufacturer_data_blocked(APPLE, &[0x02, 0x15, 0xAB, 0xCD]),
            "an iBeacon frame must not reach a caller"
        );
        assert!(
            !manufacturer_data_blocked(APPLE, &[0x09, 0x06, 0x03]),
            "Apple's other advertisements are not blocked wholesale"
        );
        assert!(
            !manufacturer_data_blocked(0x00E0, &[0x02, 0x15]),
            "another company's 0x02 frame is not an iBeacon"
        );
    }

    /// Short data cannot match a longer prefix, and must not panic trying.
    #[test]
    fn data_shorter_than_the_prefix_is_not_blocked() {
        assert!(!manufacturer_data_blocked(APPLE, &[]));
    }

    /// A filter is refused only when everything it could match is already
    /// blocked; a broader one is allowed, and simply comes back stripped.
    #[test]
    fn only_filters_wholly_inside_the_blocklist_are_refused() {
        let ibeacon = DataPrefix::new(vec![0x02]);
        assert!(manufacturer_filter_blocked(APPLE, &ibeacon));

        let narrower = DataPrefix::new(vec![0x02, 0x15]);
        assert!(
            manufacturer_filter_blocked(APPLE, &narrower),
            "narrowing a blocked filter must not get around it"
        );

        let other = DataPrefix::new(vec![0x09]);
        assert!(
            !manufacturer_filter_blocked(APPLE, &other),
            "a filter for something else is fine"
        );

        // Masked so loosely that it also matches non-iBeacon frames. It is not
        // a subset of the blocked prefix, so it is allowed — the data still
        // comes back stripped of anything that is one.
        let loose = DataPrefix::with_mask(vec![0x02], vec![0x01]).unwrap();
        assert!(!manufacturer_filter_blocked(APPLE, &loose));
    }

    /// An unparseable list must block everything rather than nothing. This is
    /// the direction that matters: the failure mode of a privacy control is
    /// silently permitting what it was installed to forbid.
    #[test]
    fn an_unreadable_list_blocks_everything() {
        for bad in [
            "manufacturer",                              // truncated
            "manufacturer 4c",                           // no data prefix
            "manufacturer 4c 02/ff",                     // missing the advdata- tag
            "manufacturer 4c advdata-02",                // no mask
            "manufacturer 4c advdata-2/ff",              // odd digit count
            "manufacturer 4c advdata-02/fg",             // not hex
            "manufacturer 12345 advdata-02/ff",          // company too wide
            "uuid 00001812-0000-1000-8000-00805f9b34fb", // wrong file entirely
        ] {
            assert!(
                parse_manufacturer(bad).is_none(),
                "{bad:?} should have failed to parse"
            );
        }
    }

    #[test]
    fn the_vendored_list_parses_and_is_not_empty() {
        // Guards against a fetch that installed an error page, which would
        // parse as an error and — because that fails closed — block every
        // company's data on every platform.
        let table = manufacturer_table()
            .as_ref()
            .expect("the vendored manufacturer blocklist must parse");
        assert!(!table.is_empty());
    }

    /// Mismatched prefix and mask lengths are a parse failure, not a panic.
    #[test]
    fn prefix_and_mask_must_agree_in_length() {
        assert!(parse_manufacturer("manufacturer 4c advdata-0215/ff").is_none());
    }

    #[test]
    fn hid_service_is_fully_blocked() {
        // Direct access to a keyboard would make any caller a keylogger.
        let hid = services::HUMAN_INTERFACE_DEVICE;
        assert!(is_blocked(&hid));
        assert!(reads_blocked(&hid) && writes_blocked(&hid));
    }

    #[test]
    fn firmware_update_services_are_blocked() {
        for uuid in [
            "00001530-1212-efde-1523-785feabcd123", // Nordic legacy DFU
            "f000ffc0-0451-4000-b000-000000000000", // TI over-the-air download
        ] {
            assert!(
                is_blocked(&BluetoothUuid::parse(uuid).unwrap()),
                "{uuid} not blocked"
            );
        }
    }

    #[test]
    fn serial_number_is_blocked_because_it_identifies_the_user() {
        let sn = characteristics::SERIAL_NUMBER_STRING;
        assert!(is_blocked(&sn));
    }

    #[test]
    fn cccd_is_write_blocked_not_read_blocked() {
        // Writing the Client Characteristic Configuration descriptor by hand
        // would bypass `start_notifications`, so it is writes-only-blocked.
        let cccd = descriptors::CLIENT_CHARACTERISTIC_CONFIGURATION;
        assert_eq!(lookup(&cccd), Some(Blocked::Writes));
        assert!(writes_blocked(&cccd));
        assert!(!reads_blocked(&cccd));
        assert!(!is_blocked(&cccd));
    }

    #[test]
    fn ordinary_services_are_not_blocked() {
        for u in [
            services::BATTERY_SERVICE,
            services::HEART_RATE,
            services::DEVICE_INFORMATION,
        ] {
            assert!(lookup(&u).is_none());
        }
    }

    #[test]
    fn snapshot_parsed_completely() {
        // Every non-comment line in the vendored file became an entry; a silent
        // parse failure would under-block.
        let expected = SOURCE
            .lines()
            .filter(|l| {
                let l = l.split('#').next().unwrap_or("").trim();
                !l.is_empty()
            })
            .count();
        assert_eq!(
            table().len(),
            expected,
            "some blocklist lines failed to parse"
        );
        assert!(
            expected >= 12,
            "vendored blocklist looks truncated: {expected} entries"
        );
    }
}
