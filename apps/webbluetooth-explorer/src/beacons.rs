//! Beacon frames, recognised and read.
//!
//! Most of what a busy scan actually contains is beacons, and in a scanner that
//! does not know them they are an unlabelled run of manufacturer bytes. The
//! formats are few, fixed and well documented, so recognising them is a matter
//! of a length check and a prefix.
//!
//! Note what is deliberately *not* here: an iBeacon's proximity UUID, major and
//! minor are a location fix, and `webbluetooth`'s manufacturer blocklist strips
//! Apple's iBeacon frames before they ever reach this program. So the iBeacon
//! reader below runs on frames from other companies that use the same layout,
//! and on nothing else — which is the correct outcome, not a limitation.

use crate::format;

/// A beacon frame, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    /// Which kind it is.
    pub kind: &'static str,
    /// Its fields, in the order they are worth reading.
    pub fields: Vec<(&'static str, String)>,
}

fn field(name: &'static str, value: impl Into<String>) -> (&'static str, String) {
    (name, value.into())
}

/// Read a manufacturer-data payload as a beacon, if it is one.
///
/// `company` is the identifier the payload arrived under, and `data` is what
/// followed it.
pub fn from_manufacturer_data(company: u16, data: &[u8]) -> Option<Beacon> {
    // The iBeacon layout: type 0x02, length 0x15, then 16 bytes of proximity
    // UUID, two of major, two of minor, and a calibrated power.
    if data.len() >= 23 && data[0] == 0x02 && data[1] == 0x15 {
        let uuid = &data[2..18];
        let major = u16::from_be_bytes([data[18], data[19]]);
        let minor = u16::from_be_bytes([data[20], data[21]]);
        let power = data[22] as i8;
        return Some(Beacon {
            kind: if company == 0x004C {
                "iBeacon"
            } else {
                // The same layout under another company identifier. Common, and
                // not an iBeacon in the trademark sense.
                "iBeacon-format"
            },
            fields: vec![
                field("proximity UUID", hyphenate(uuid)),
                field("major", major.to_string()),
                field("minor", minor.to_string()),
                field("power at 1 m", format!("{power} dBm")),
            ],
        });
    }

    // AltBeacon: a 2-byte beacon code 0xBEAC, 20 bytes of id, a reference RSSI
    // and a manufacturer-reserved byte.
    if data.len() >= 24 && data[0] == 0xBE && data[1] == 0xAC {
        return Some(Beacon {
            kind: "AltBeacon",
            fields: vec![
                field("beacon id", format::hex(&data[2..22])),
                field("power at 1 m", format!("{} dBm", data[22] as i8)),
                field("reserved", format!("0x{:02X}", data[23])),
            ],
        });
    }

    None
}

/// Read a service-data payload as an Eddystone frame, if it is one.
///
/// Eddystone lives in service data under `0xFEAA`, and the first byte says
/// which of its frame types this is.
pub fn from_service_data(service: u16, data: &[u8]) -> Option<Beacon> {
    if service != 0xFEAA || data.is_empty() {
        return None;
    }
    match data[0] {
        // UID: a 10-byte namespace and a 6-byte instance.
        0x00 if data.len() >= 18 => Some(Beacon {
            kind: "Eddystone-UID",
            fields: vec![
                field("power at 0 m", format!("{} dBm", data[1] as i8)),
                field("namespace", format::hex(&data[2..12])),
                field("instance", format::hex(&data[12..18])),
            ],
        }),
        // URL: a scheme byte then the encoded host, with common suffixes
        // compressed into single bytes.
        0x10 if data.len() >= 3 => Some(Beacon {
            kind: "Eddystone-URL",
            fields: vec![
                field("power at 0 m", format!("{} dBm", data[1] as i8)),
                field("url", decode_url(data[2], &data[3..])),
            ],
        }),
        // TLM: telemetry. Battery, temperature, and two counters.
        0x20 if data.len() >= 14 => {
            let voltage = u16::from_be_bytes([data[2], data[3]]);
            // An 8.8 fixed-point signed temperature.
            let temperature = i16::from_be_bytes([data[4], data[5]]);
            let advertisements = u32::from_be_bytes([data[6], data[7], data[8], data[9]]);
            // Deciseconds since power-on.
            let uptime = u32::from_be_bytes([data[10], data[11], data[12], data[13]]);
            Some(Beacon {
                kind: "Eddystone-TLM",
                fields: vec![
                    field(
                        "battery",
                        if voltage == 0 {
                            "not supported".to_owned()
                        } else {
                            format!("{voltage} mV")
                        },
                    ),
                    field(
                        "temperature",
                        if temperature == -32768 {
                            "not supported".to_owned()
                        } else {
                            format!("{:.2} °C", f32::from(temperature) / 256.0)
                        },
                    ),
                    field("advertisements", advertisements.to_string()),
                    field("uptime", uptime_of(uptime)),
                ],
            })
        }
        // EID: an ephemeral identifier, resolvable only by whoever holds the key.
        0x30 if data.len() >= 10 => Some(Beacon {
            kind: "Eddystone-EID",
            fields: vec![
                field("power at 0 m", format!("{} dBm", data[1] as i8)),
                field("ephemeral id", format::hex(&data[2..10])),
            ],
        }),
        _ => None,
    }
}

/// `0102030405060708090A0B0C0D0E0F10` as a UUID.
fn hyphenate(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if hex.len() != 32 {
        return hex;
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Deciseconds since power-on, as something readable.
fn uptime_of(deciseconds: u32) -> String {
    let seconds = u64::from(deciseconds) / 10;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Expand an Eddystone-URL back into a URL.
///
/// The format compresses the scheme into one byte and the commonest suffixes
/// into one byte each, which is the only way a URL fits in an advertising
/// packet at all.
fn decode_url(scheme: u8, encoded: &[u8]) -> String {
    let mut out = String::from(match scheme {
        0x00 => "http://www.",
        0x01 => "https://www.",
        0x02 => "http://",
        0x03 => "https://",
        _ => "",
    });
    for byte in encoded {
        match byte {
            0x00 => out.push_str(".com/"),
            0x01 => out.push_str(".org/"),
            0x02 => out.push_str(".edu/"),
            0x03 => out.push_str(".net/"),
            0x04 => out.push_str(".info/"),
            0x05 => out.push_str(".biz/"),
            0x06 => out.push_str(".gov/"),
            0x07 => out.push_str(".com"),
            0x08 => out.push_str(".org"),
            0x09 => out.push_str(".edu"),
            0x0A => out.push_str(".net"),
            0x0B => out.push_str(".info"),
            0x0C => out.push_str(".biz"),
            0x0D => out.push_str(".gov"),
            // Anything else is a literal character.
            other => out.push(*other as char),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ibeacon_frame_reads_its_identifiers() {
        let mut data = vec![0x02, 0x15];
        data.extend_from_slice(&(0..16).collect::<Vec<u8>>());
        data.extend_from_slice(&[0x00, 0x0A]); // major 10
        data.extend_from_slice(&[0x00, 0x14]); // minor 20
        data.push(0xC5); // -59 dBm

        let beacon = from_manufacturer_data(0x004C, &data).expect("an iBeacon");
        assert_eq!(beacon.kind, "iBeacon");
        assert_eq!(beacon.fields[0].1, "00010203-0405-0607-0809-0a0b0c0d0e0f");
        // Major and minor are big-endian, unlike almost everything else in BLE.
        assert_eq!(beacon.fields[1].1, "10");
        assert_eq!(beacon.fields[2].1, "20");
        assert_eq!(beacon.fields[3].1, "-59 dBm");

        // The same layout under another company is not an Apple iBeacon.
        let other = from_manufacturer_data(0x0059, &data).unwrap();
        assert_eq!(other.kind, "iBeacon-format");
    }

    #[test]
    fn a_frame_that_is_not_a_beacon_is_not_claimed() {
        assert!(from_manufacturer_data(0x004C, &[0x09, 0x06, 0x03]).is_none());
        assert!(from_manufacturer_data(0x004C, &[]).is_none());
        // The right prefix but too short to be one.
        assert!(from_manufacturer_data(0x004C, &[0x02, 0x15, 0x00]).is_none());
        assert!(from_service_data(0x180F, &[0x00]).is_none());
    }

    #[test]
    fn eddystone_uid_and_eid_read_their_identifiers() {
        let mut uid = vec![0x00, 0xEE];
        uid.extend_from_slice(&[0xAA; 10]);
        uid.extend_from_slice(&[0xBB; 6]);
        let beacon = from_service_data(0xFEAA, &uid).expect("a UID frame");
        assert_eq!(beacon.kind, "Eddystone-UID");
        assert_eq!(beacon.fields[0].1, "-18 dBm");
        assert!(beacon.fields[1].1.starts_with("AA AA"));

        let eid = [0x30, 0xEE, 1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(
            from_service_data(0xFEAA, &eid).unwrap().kind,
            "Eddystone-EID"
        );
    }

    /// The URL format compresses the scheme and the commonest suffixes into
    /// single bytes, which is the only way a URL fits in an advertisement.
    #[test]
    fn an_eddystone_url_expands_back_into_a_url() {
        // https:// + "example" + ".com/"
        let mut frame = vec![0x10, 0xEE, 0x03];
        frame.extend_from_slice(b"example");
        frame.push(0x00);
        let beacon = from_service_data(0xFEAA, &frame).unwrap();
        assert_eq!(beacon.fields[1].1, "https://example.com/");

        // The www-prefixed schemes and a suffix with no trailing slash.
        let mut short = vec![0x10, 0xEE, 0x00];
        short.extend_from_slice(b"nordic");
        short.push(0x07);
        assert_eq!(
            from_service_data(0xFEAA, &short).unwrap().fields[1].1,
            "http://www.nordic.com"
        );
    }

    /// Telemetry has two "not supported" sentinels that must not read as
    /// measurements: a zero battery and the most negative temperature.
    #[test]
    fn eddystone_telemetry_reads_its_sentinels_as_sentinels() {
        // Version, battery 3000 mV, temperature 25.0 °C, 100 adverts, 1000 ds.
        let tlm = [
            0x20, 0x00, 0x0B, 0xB8, 0x19, 0x00, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x03, 0xE8,
        ];
        let beacon = from_service_data(0xFEAA, &tlm).unwrap();
        assert_eq!(beacon.kind, "Eddystone-TLM");
        assert_eq!(beacon.fields[0].1, "3000 mV");
        assert_eq!(beacon.fields[1].1, "25.00 °C");
        assert_eq!(beacon.fields[2].1, "100");
        assert_eq!(beacon.fields[3].1, "1m");

        let unsupported = [
            0x20, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let beacon = from_service_data(0xFEAA, &unsupported).unwrap();
        assert_eq!(beacon.fields[0].1, "not supported");
        assert_eq!(beacon.fields[1].1, "not supported");
    }

    #[test]
    fn an_altbeacon_is_recognised_by_its_code() {
        let mut data = vec![0xBE, 0xAC];
        data.extend_from_slice(&[0x11; 20]);
        data.push(0xC5);
        data.push(0x00);
        let beacon = from_manufacturer_data(0x0118, &data).expect("an AltBeacon");
        assert_eq!(beacon.kind, "AltBeacon");
        assert_eq!(beacon.fields[1].1, "-59 dBm");
    }
}
