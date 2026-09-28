//! Advertising data, split into the records it is made of.
//!
//! An advertising packet is a sequence of `length, type, value` records — the
//! Core Specification's *Advertising and Scan Response data format*, with the
//! type codes listed in Assigned Numbers under *Common Data Types*. Everything
//! parsed elsewhere in this program is derived from these records, and the
//! derivation loses things: an AD type nobody has a field for, a record that is
//! malformed, a flags byte that was folded into a boolean.
//!
//! This is the view that does not lose them. It is only available where the
//! transport hands over the packet at all — see `Advertisement::raw`, which
//! only a raw HCI socket fills in.

/// One record from an advertising packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The AD type code.
    pub kind: u8,
    /// What the specification calls it.
    pub name: &'static str,
    /// The bytes after the type, without the length or the type.
    pub value: Vec<u8>,
    /// What those bytes say, where the type is one this knows how to read.
    pub reading: Option<String>,
}

/// Split a packet into its records.
///
/// Stops at a zero length, which terminates the data — padding after that is
/// not a record, and reading it as one is a classic way to produce garbage.
/// Stops at a length that runs past the end, which means a truncated or
/// hostile packet.
pub fn parse(data: &[u8]) -> Vec<Record> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let length = data[i] as usize;
        if length == 0 {
            break;
        }
        if i + 1 + length > data.len() {
            break;
        }
        let kind = data[i + 1];
        let value = data[i + 2..i + 1 + length].to_vec();
        out.push(Record {
            kind,
            name: name(kind),
            reading: read(kind, &value),
            value,
        });
        i += 1 + length;
    }
    out
}

/// The name Assigned Numbers gives an AD type.
pub fn name(kind: u8) -> &'static str {
    match kind {
        0x01 => "Flags",
        0x02 => "Incomplete 16-bit Service UUIDs",
        0x03 => "Complete 16-bit Service UUIDs",
        0x04 => "Incomplete 32-bit Service UUIDs",
        0x05 => "Complete 32-bit Service UUIDs",
        0x06 => "Incomplete 128-bit Service UUIDs",
        0x07 => "Complete 128-bit Service UUIDs",
        0x08 => "Shortened Local Name",
        0x09 => "Complete Local Name",
        0x0A => "TX Power Level",
        0x0D => "Class of Device",
        0x0E => "Simple Pairing Hash C-192",
        0x0F => "Simple Pairing Randomizer R-192",
        0x10 => "Device ID",
        0x11 => "Security Manager Out of Band Flags",
        0x12 => "Peripheral Connection Interval Range",
        0x14 => "16-bit Service Solicitation UUIDs",
        0x15 => "128-bit Service Solicitation UUIDs",
        0x16 => "Service Data — 16-bit UUID",
        0x17 => "Public Target Address",
        0x18 => "Random Target Address",
        0x19 => "Appearance",
        0x1A => "Advertising Interval",
        0x1B => "LE Bluetooth Device Address",
        0x1C => "LE Role",
        0x1F => "32-bit Service Solicitation UUIDs",
        0x20 => "Service Data — 32-bit UUID",
        0x21 => "Service Data — 128-bit UUID",
        0x24 => "URI",
        0x25 => "Indoor Positioning",
        0x26 => "Transport Discovery Data",
        0x27 => "LE Supported Features",
        0x28 => "Channel Map Update Indication",
        0x2D => "Mesh Provisioning PB-ADV",
        0x2A => "Mesh Message",
        0x2B => "Mesh Beacon",
        0x2C => "BIGInfo",
        0x30 => "Broadcast Name",
        0x3D => "3D Information Data",
        0xFF => "Manufacturer Specific Data",
        _ => "Unknown",
    }
}

/// What a record says, for the types worth spelling out.
fn read(kind: u8, value: &[u8]) -> Option<String> {
    match kind {
        0x01 => {
            let bits = *value.first()?;
            // The flags that actually decide how a device behaves. A scanner
            // showing "0x06" is showing you a number to go and look up.
            let mut set = Vec::new();
            if bits & 0x01 != 0 {
                set.push("LE Limited Discoverable");
            }
            if bits & 0x02 != 0 {
                set.push("LE General Discoverable");
            }
            if bits & 0x04 != 0 {
                set.push("BR/EDR Not Supported");
            }
            if bits & 0x08 != 0 {
                set.push("LE + BR/EDR Controller");
            }
            if bits & 0x10 != 0 {
                set.push("LE + BR/EDR Host");
            }
            Some(if set.is_empty() {
                format!("0x{bits:02X}")
            } else {
                set.join(", ")
            })
        }
        0x08 | 0x09 | 0x30 => std::str::from_utf8(value).ok().map(str::to_owned),
        0x0A => Some(format!("{} dBm", *value.first()? as i8)),
        0x19 => {
            let raw = u16::from_le_bytes([*value.first()?, *value.get(1)?]);
            Some(format!("0x{raw:04X}"))
        }
        0x1C => Some(
            match value.first()? {
                0 => "Peripheral only",
                1 => "Central only",
                2 => "Peripheral preferred",
                3 => "Central preferred",
                _ => "reserved",
            }
            .to_owned(),
        ),
        0x1A => {
            let raw = u16::from_le_bytes([*value.first()?, *value.get(1)?]);
            Some(format!("{:.2} ms ({raw})", f32::from(raw) * 0.625))
        }
        0x02 | 0x03 | 0x14 => Some(
            value
                .chunks_exact(2)
                .map(|pair| format!("0x{:04X}", u16::from_le_bytes([pair[0], pair[1]])))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        0xFF => {
            let company = u16::from_le_bytes([*value.first()?, *value.get(1)?]);
            let who = crate::names::company(company)
                .map_or_else(|| format!("0x{company:04X}"), str::to_owned);
            Some(format!("{who} — {}", crate::format::hex(&value[2..])))
        }
        0x16 => {
            let uuid = u16::from_le_bytes([*value.first()?, *value.get(1)?]);
            Some(format!(
                "0x{uuid:04X} — {}",
                crate::format::hex(&value[2..])
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real packet: flags, a 16-bit service list, and a complete local name.
    #[test]
    fn a_packet_splits_into_its_records() {
        let packet = [
            0x02, 0x01, 0x06, // Flags: general discoverable, no BR/EDR
            0x03, 0x03, 0x0F, 0x18, // Complete 16-bit UUIDs: 0x180F
            0x05, 0x09, b'T', b'e', b's', b't', // Complete Local Name: "Test"
        ];
        let records = parse(&packet);
        assert_eq!(records.len(), 3);

        assert_eq!(records[0].name, "Flags");
        assert_eq!(
            records[0].reading.as_deref(),
            Some("LE General Discoverable, BR/EDR Not Supported")
        );
        assert_eq!(records[1].reading.as_deref(), Some("0x180F"));
        assert_eq!(records[2].reading.as_deref(), Some("Test"));
        assert_eq!(records[2].value, b"Test");
    }

    /// A zero length terminates the data. Padding after it is not a record,
    /// and reading it as one is how a scanner invents fields.
    #[test]
    fn padding_after_the_data_is_not_a_record() {
        let packet = [0x02, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00];
        let records = parse(&packet);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, 0x01);
    }

    /// A length running past the end means truncated or hostile. Take what
    /// parsed and stop.
    #[test]
    fn a_length_past_the_end_stops_the_parse() {
        let packet = [0x02, 0x01, 0x06, 0x20, 0x09, b'x'];
        let records = parse(&packet);
        assert_eq!(records.len(), 1, "the second record claims 32 bytes");
        assert!(parse(&[0xFF]).is_empty());
        assert!(parse(&[]).is_empty());
    }

    #[test]
    fn manufacturer_data_names_its_company() {
        // 0x004C is Apple, then the payload.
        let records = parse(&[0x05, 0xFF, 0x4C, 0x00, 0x02, 0x15]);
        let reading = records[0].reading.as_deref().unwrap();
        assert!(reading.starts_with("Apple"), "{reading}");
        assert!(reading.contains("02 15"), "{reading}");
        // The company id is stripped from what is shown as payload.
        assert_eq!(records[0].value, vec![0x4C, 0x00, 0x02, 0x15]);
    }

    #[test]
    fn signed_and_scaled_fields_read_correctly() {
        // TX power is signed: 0xF4 is -12 dBm, not 244.
        assert_eq!(
            parse(&[0x02, 0x0A, 0xF4])[0].reading.as_deref(),
            Some("-12 dBm")
        );
        // Advertising interval is in 0.625 ms units.
        let interval = parse(&[0x03, 0x1A, 0x40, 0x00]);
        assert!(interval[0].reading.as_deref().unwrap().contains("40.00 ms"));
    }

    /// An unknown type is still a record — its bytes are exactly what this view
    /// exists to show.
    #[test]
    fn an_unknown_type_keeps_its_bytes() {
        let records = parse(&[0x03, 0x77, 0xAA, 0xBB]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, 0x77);
        assert_eq!(records[0].name, "Unknown");
        assert_eq!(records[0].value, vec![0xAA, 0xBB]);
        assert_eq!(records[0].reading, None);
    }
}
