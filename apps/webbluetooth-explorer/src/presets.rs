//! Known values for the control points a device is likely to publish.
//!
//! Most writes in a GATT browser are to a *control point*: a characteristic
//! whose value is an opcode, not data. Their encodings live in the profile
//! specifications rather than in anything the device advertises, so the normal
//! experience is to type `01 01` into a box having just looked it up. nRF
//! Connect answers this with a dialog per profile; this is the same idea
//! reduced to a list of named byte strings.
//!
//! Deliberately a short list. Every entry here is one whose encoding is stated
//! plainly in its profile specification, and nothing is included on the basis
//! that it is probably right — a wrong guess writes wrong bytes to somebody's
//! hardware. Anything not listed is what the hex field is for.
//!
//! Nordic's DFU control points are excluded on purpose: putting a device into
//! bootloader mode from a browser's write box is a destructive operation
//! dressed up as a convenience.

use webbluetooth::uuid::BluetoothUuid;

/// One named value for a characteristic or descriptor.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    /// What it does, short enough for a button.
    pub label: &'static str,
    /// The bytes to write.
    pub value: &'static [u8],
    /// What it means, and where that is written down.
    pub note: &'static str,
}

/// Presets for a characteristic, by UUID.
pub fn for_characteristic(uuid: &BluetoothUuid) -> &'static [Preset] {
    match uuid.as_u16() {
        // Alert Level — Immediate Alert, Link Loss.
        Some(0x2A06) => &[
            Preset { label: "No alert", value: &[0x00], note: "Alert Level 0. Stops whatever alert is currently running." },
            Preset { label: "Mild alert", value: &[0x01], note: "Alert Level 1. A gentler alert than High; what a device does with it is up to the device." },
            Preset { label: "High alert", value: &[0x02], note: "Alert Level 2. This is the one that makes a tracker beep." },
        ],
        // Ringer Control Point — Phone Alert Status.
        Some(0x2A40) => &[
            Preset { label: "Silence", value: &[0x01], note: "Set Silent Mode. The ringer stays off until Cancel Silent Mode is written." },
            Preset { label: "Mute once", value: &[0x02], note: "Mute Once. Silences the current alert only." },
            Preset { label: "Cancel silence", value: &[0x03], note: "Cancel Silent Mode, putting the ringer back the way it was." },
        ],
        // Heart Rate Control Point.
        Some(0x2A39) => &[
            Preset { label: "Reset energy", value: &[0x01], note: "Reset Energy Expended to zero. The only opcode this control point defines." },
        ],
        // Record Access Control Point — Glucose, Continuous Glucose, Weight.
        // Byte one is the opcode, byte two the operator: 0x01 is "all records".
        Some(0x2A52) => &[
            Preset { label: "Report all", value: &[0x01, 0x01], note: "Report Stored Records, operator All Records. The records arrive as notifications on the measurement characteristic." },
            Preset { label: "Count records", value: &[0x04, 0x01], note: "Report Number of Stored Records, operator All Records." },
            Preset { label: "Abort", value: &[0x03, 0x00], note: "Abort Operation, operator Null." },
            Preset { label: "Delete all", value: &[0x02, 0x01], note: "Delete Stored Records, operator All Records. This erases the device's stored measurements." },
        ],
        // SC Control Point — Cycling Speed and Cadence, Running Speed and Cadence.
        Some(0x2A55) => &[
            Preset { label: "Zero distance", value: &[0x01, 0x00, 0x00, 0x00, 0x00], note: "Set Cumulative Value to 0. The value is a uint32, little-endian." },
            Preset { label: "Calibrate", value: &[0x02], note: "Start Sensor Calibration. The result comes back as an indication on this same control point." },
            Preset { label: "List locations", value: &[0x04], note: "Request Supported Sensor Locations. The list comes back as an indication on this control point." },
        ],
        // BBC micro:bit LED matrix: five bytes, one per row, bits 4..0 being
        // the columns left to right.
        _ if is_microbit_matrix(uuid) => &[
            Preset { label: "All on", value: &[0x1F, 0x1F, 0x1F, 0x1F, 0x1F], note: "Five rows of five columns, every LED lit. One byte per row." },
            Preset { label: "All off", value: &[0x00, 0x00, 0x00, 0x00, 0x00], note: "Five rows of five columns, nothing lit. One byte per row." },
            Preset { label: "Heart", value: &[0x0A, 0x1F, 0x1F, 0x0E, 0x04], note: "One byte per row, bits 4..0 left to right." },
        ],
        _ => &[],
    }
}

/// Presets for a descriptor, by UUID.
pub fn for_descriptor(uuid: &BluetoothUuid) -> &'static [Preset] {
    match uuid.as_u16() {
        // Client Characteristic Configuration. Writing this by hand is how you
        // find out whether a peripheral actually honours its own CCCD, which
        // `startNotifications` otherwise hides.
        Some(0x2902) => &[
            Preset {
                label: "Notify on",
                value: &[0x01, 0x00],
                note: "Bit 0 set. Equivalent to Subscribe, but done by hand.",
            },
            Preset {
                label: "Indicate on",
                value: &[0x02, 0x00],
                note: "Bit 1 set. Indications are acknowledged; notifications are not.",
            },
            Preset {
                label: "Off",
                value: &[0x00, 0x00],
                note: "Neither bit set, which stops both notifications and indications.",
            },
        ],
        // Characteristic User Description is free text, so there is nothing to
        // offer — but the fact it is writable at all is worth knowing.
        _ => &[],
    }
}

/// `E95D7B77-251D-470A-A062-FA1922DFA9A8`, the micro:bit's LED matrix state.
fn is_microbit_matrix(uuid: &BluetoothUuid) -> bool {
    uuid.as_u128() == 0xE95D_7B77_251D_470A_A062_FA19_22DF_A9A8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_point_offers_its_opcodes() {
        let alert = for_characteristic(&BluetoothUuid::from_u16(0x2A06));
        assert_eq!(alert.len(), 3);
        assert_eq!(alert[0].value, &[0x00]);
        assert_eq!(alert[2].value, &[0x02]);

        // Record Access Control Point takes an operator byte as well as an
        // opcode, and getting that wrong is the classic mistake.
        let racp = for_characteristic(&BluetoothUuid::from_u16(0x2A52));
        let report = racp.iter().find(|p| p.label == "Report all").unwrap();
        assert_eq!(report.value, &[0x01, 0x01], "opcode then operator");
        let abort = racp.iter().find(|p| p.label == "Abort").unwrap();
        assert_eq!(abort.value, &[0x03, 0x00], "Abort takes the null operator");
    }

    #[test]
    fn a_characteristic_with_no_defined_values_offers_none() {
        // Battery Level is read-only data, not a control point.
        assert!(for_characteristic(&BluetoothUuid::from_u16(0x2A19)).is_empty());
        let vendor = BluetoothUuid::parse("f000aa00-0451-4000-b000-000000000000").unwrap();
        assert!(for_characteristic(&vendor).is_empty());
    }

    #[test]
    fn the_cccd_offers_the_three_states_it_has() {
        let cccd = for_descriptor(&BluetoothUuid::from_u16(0x2902));
        assert_eq!(cccd.len(), 3);
        // Two bytes, little-endian, and the bits are not interchangeable.
        assert_eq!(cccd[0].value, &[0x01, 0x00]);
        assert_eq!(cccd[1].value, &[0x02, 0x00]);
        assert_eq!(cccd[2].value, &[0x00, 0x00]);
        assert!(for_descriptor(&BluetoothUuid::from_u16(0x2901)).is_empty());
    }

    /// A 128-bit vendor UUID must be matched in full, not by a truncated form.
    #[test]
    fn the_microbit_matrix_is_matched_by_its_whole_uuid() {
        let matrix = BluetoothUuid::parse("e95d7b77-251d-470a-a062-fa1922dfa9a8").unwrap();
        assert_eq!(for_characteristic(&matrix).len(), 3);

        let neighbour = BluetoothUuid::parse("e95d7b78-251d-470a-a062-fa1922dfa9a8").unwrap();
        assert!(for_characteristic(&neighbour).is_empty());
    }

    /// Every preset has to say what it does and where that is written down —
    /// an unlabelled byte string is no better than typing it.
    #[test]
    fn every_preset_explains_itself() {
        let every: Vec<&Preset> = [0x2A06_u16, 0x2A40, 0x2A39, 0x2A52, 0x2A55]
            .into_iter()
            .flat_map(|u| for_characteristic(&BluetoothUuid::from_u16(u)))
            .chain(for_descriptor(&BluetoothUuid::from_u16(0x2902)))
            .collect();
        assert!(every.len() >= 13);
        for preset in every {
            assert!(!preset.label.is_empty());
            assert!(!preset.value.is_empty(), "{}", preset.label);
            assert!(preset.note.len() > 20, "{} has no real note", preset.label);
        }
    }
}
