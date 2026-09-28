//! Ready-made services to publish.
//!
//! Composing a GATT server one UUID at a time is fine once you know what you
//! are doing and tedious every time after that. These are the services people
//! actually stand up to test a central against: a battery to read, a device
//! information block to enumerate, and a byte pipe to push traffic through.
//!
//! Each is a starting point, not a specification — everything is editable once
//! it is in the list.

use crate::engine::{CharacteristicSetup, ServiceSetup};
use webbluetooth::uuid::{characteristics, services, BluetoothUuid};
use webbluetooth::CharacteristicProperties as Props;

/// One offer in the template row.
pub struct Template {
    /// What the button says.
    pub label: &'static str,
    /// What it builds, and what it is for.
    pub note: &'static str,
    /// Builds it.
    pub build: fn() -> ServiceSetup,
}

/// Everything on offer.
pub const ALL: &[Template] = &[
    Template {
        label: "Battery",
        note: "Battery Service with a Battery Level that reads and notifies. \
               The smallest thing a central can usefully talk to.",
        build: battery,
    },
    Template {
        label: "Device Information",
        note: "Manufacturer, model and firmware strings. What a central reads \
               first to find out what it has connected to.",
        build: device_information,
    },
    Template {
        label: "Nordic UART",
        note: "The de-facto serial-over-BLE service: one characteristic you \
               write to, one that notifies back.",
        build: nordic_uart,
    },
    Template {
        label: "Heart Rate",
        note: "Heart Rate Measurement, notifying. Handy because almost every \
               BLE example programme in existence knows how to read it.",
        build: heart_rate,
    },
];

fn characteristic(uuid: BluetoothUuid, bits: u32, value: &[u8]) -> CharacteristicSetup {
    CharacteristicSetup {
        uuid,
        properties: Props(bits),
        value: value.to_vec(),
    }
}

fn battery() -> ServiceSetup {
    ServiceSetup {
        uuid: services::BATTERY_SERVICE,
        advertise: true,
        characteristics: vec![characteristic(
            characteristics::BATTERY_LEVEL,
            Props::READ | Props::NOTIFY,
            // A percentage, which is what the profile says this is.
            &[100],
        )],
    }
}

fn device_information() -> ServiceSetup {
    ServiceSetup {
        uuid: services::DEVICE_INFORMATION,
        // Rarely advertised: a central reads it after connecting, having found
        // the device some other way.
        advertise: false,
        characteristics: vec![
            characteristic(
                characteristics::MANUFACTURER_NAME_STRING,
                Props::READ,
                b"WebBluetoothExplorer",
            ),
            characteristic(
                characteristics::MODEL_NUMBER_STRING,
                Props::READ,
                b"explorer",
            ),
            characteristic(
                characteristics::FIRMWARE_REVISION_STRING,
                Props::READ,
                env!("CARGO_PKG_VERSION").as_bytes(),
            ),
        ],
    }
}

fn nordic_uart() -> ServiceSetup {
    // 6E400001-B5A3-F393-E0A9-E50E24DCCA9E and its two characteristics. Named
    // from the central's point of view, which is why "TX" is the one this side
    // notifies on.
    let base = |last: u16| {
        BluetoothUuid::from_u128(0x6E40_0000_B5A3_F393_E0A9_E50E_24DC_CA9E | ((last as u128) << 96))
    };
    ServiceSetup {
        uuid: base(0x0001),
        advertise: true,
        characteristics: vec![
            characteristic(
                base(0x0002),
                Props::WRITE | Props::WRITE_WITHOUT_RESPONSE,
                &[],
            ),
            characteristic(base(0x0003), Props::NOTIFY, &[]),
        ],
    }
}

fn heart_rate() -> ServiceSetup {
    ServiceSetup {
        uuid: services::HEART_RATE,
        advertise: true,
        characteristics: vec![characteristic(
            characteristics::HEART_RATE_MEASUREMENT,
            Props::NOTIFY,
            // Flags byte 0: 8-bit value, no contact bits, no energy, no RR.
            // Then the rate itself.
            &[0x00, 60],
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_builds_something_publishable() {
        for template in ALL {
            let service = (template.build)();
            assert!(
                !service.characteristics.is_empty(),
                "{} has no characteristics",
                template.label
            );
            assert!(
                template.note.len() > 40,
                "{} barely explains itself",
                template.label
            );
            for characteristic in &service.characteristics {
                assert_ne!(
                    characteristic.properties.0, 0,
                    "{} has a characteristic that supports nothing",
                    template.label
                );
            }
        }
    }

    /// A characteristic that notifies must not be published with a fixed value
    /// — the platform would serve the fixed value and never forward a read.
    /// Battery Level is the one template where both matter.
    #[test]
    fn the_battery_template_reads_and_notifies() {
        let battery = battery();
        assert_eq!(battery.uuid, services::BATTERY_SERVICE);
        let level = &battery.characteristics[0];
        assert!(level.properties.read() && level.properties.notify());
        assert_eq!(level.value, vec![100], "a percentage, per the profile");
    }

    /// The Nordic UART UUIDs are derived by substitution, which is exactly the
    /// sort of arithmetic that silently produces a neighbouring UUID.
    #[test]
    fn the_nordic_uart_uuids_are_the_real_ones() {
        let uart = nordic_uart();
        assert_eq!(uart.uuid.as_str(), "6e400001-b5a3-f393-e0a9-e50e24dcca9e");
        assert_eq!(
            uart.characteristics[0].uuid.as_str(),
            "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
        );
        assert_eq!(
            uart.characteristics[1].uuid.as_str(),
            "6e400003-b5a3-f393-e0a9-e50e24dcca9e"
        );
        // RX is written to, TX notifies back — from the central's point of view.
        assert!(uart.characteristics[0].properties.write());
        assert!(uart.characteristics[1].properties.notify());
    }

    #[test]
    fn device_information_is_not_advertised_but_the_rest_are() {
        assert!(!device_information().advertise);
        assert!(battery().advertise);
        assert!(heart_rate().advertise);
    }
}
