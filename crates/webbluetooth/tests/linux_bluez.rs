//! End-to-end tests of the Linux backend against a mocked BlueZ.
//!
//! No container has a Bluetooth controller, so `docker/mock-bluez.py` publishes
//! the exact D-Bus interfaces `bluetoothd` would. Everything below is real
//! traffic over a real bus — `GetManagedObjects`, discovery filters, `Connect`,
//! `ReadValue`, `WriteValue`, `StartNotify`, `PropertiesChanged` — against the
//! public API, with nothing stubbed on the Rust side.
//!
//! Skipped unless a bus is configured, so `cargo test` elsewhere still passes.
//! Run them with `./scripts/test-linux.sh`.

// The mock serves org.bluez, so these exercise the BlueZ backend specifically.
// With `linux-hci` the crate talks to a controller instead, which no container
// has.
#![cfg(all(target_os = "linux", not(feature = "linux-hci")))]

use futures_executor::block_on;
use futures_util::StreamExt;
use std::time::Duration;
use webbluetooth::uuid::{characteristics, services, BluetoothUuid};
use webbluetooth::{Bluetooth, DeviceFilter, Error, RequestDeviceOptions, Result};

const MOCK_ADDRESS: &str = "AA:BB:CC:DD:EE:FF";
const CONTROL_POINT: BluetoothUuid = characteristics::HEART_RATE_CONTROL_POINT;

/// There is exactly one mock device, so these tests cannot overlap: one of them
/// disconnects it, which every other connected client sees. Serialising here
/// rather than relying on `--test-threads=1` makes the constraint hold however
/// the suite is invoked.
///
/// The lock is deliberately poison-tolerant — a panicking test should fail on
/// its own terms, not take the rest down with it.
static MOCK_DEVICE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    MOCK_DEVICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn have_bus() -> bool {
    std::env::var("WEBBLUETOOTH_DBUS_ADDRESS").is_ok()
        || std::env::var("DBUS_SYSTEM_BUS_ADDRESS").is_ok()
}

/// Scan for the mock device and take the grant.
async fn granted() -> Result<(Bluetooth, webbluetooth::BluetoothDevice)> {
    let bluetooth = Bluetooth::new();
    bluetooth
        .availability()
        .await
        .map_err(Error::NotAvailable)?;
    let device = bluetooth
        .request_device(
            RequestDeviceOptions::new()
                .filter(DeviceFilter::new().service(services::HEART_RATE)?)
                .optional_service(services::BATTERY_SERVICE)?,
        )
        .await?;
    Ok((bluetooth, device))
}

#[test]
fn the_adapter_is_reachable() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    let bluetooth = Bluetooth::new();
    assert!(
        block_on(bluetooth.get_availability()),
        "mock adapter not usable"
    );
    assert_eq!(
        webbluetooth::authorization(),
        webbluetooth::Authorization::Allowed
    );
    // BlueZ does support the peripheral role, via a GATT application
    // registered with bluetoothd. `dual_role.rs` checks the constant against
    // the module; here it only has to be true.
    const { assert!(webbluetooth::PERIPHERAL_ROLE) };
}

#[test]
fn a_scan_finds_the_device_and_matches_on_its_service() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    let (_bluetooth, device) = block_on(granted()).expect("request_device failed");
    // On Linux the id is the Bluetooth address — BlueZ exposes it, Apple never
    // does, and this is the one visible API difference between the two.
    assert_eq!(device.id(), MOCK_ADDRESS);
    assert_eq!(device.name().as_deref(), Some("Mock Heart Rate"));
}

#[test]
fn a_filter_that_matches_nothing_finds_nothing() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    let bluetooth = Bluetooth::with_chooser(webbluetooth::chooser::FirstMatch::with_timeout(
        Duration::from_millis(600),
    ));
    let result = block_on(async {
        bluetooth
            .request_device(
                RequestDeviceOptions::new()
                    .filter(DeviceFilter::new().name_prefix("Nothing Called This")),
            )
            .await
    });
    assert!(matches!(result, Err(Error::NotFound(_))), "got {result:?}");
}

#[test]
fn connect_resolves_services_and_reads_a_characteristic() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    block_on(async {
        let (_bluetooth, device) = granted().await.expect("request_device failed");
        let gatt = device.gatt();
        gatt.connect().await.expect("connect failed");
        assert!(gatt.connected());

        let service = gatt
            .get_primary_service(services::HEART_RATE)
            .await
            .expect("no heart rate service");
        assert_eq!(
            service.uuid().as_str(),
            "0000180d-0000-1000-8000-00805f9b34fb"
        );
        assert!(service.is_primary());

        let measurement = service
            .get_characteristic(characteristics::HEART_RATE_MEASUREMENT)
            .await
            .expect("no measurement characteristic");

        // BlueZ reports properties as flag strings; they must arrive as bits.
        let props = measurement.properties();
        assert!(props.read(), "read flag lost in translation");
        assert!(props.notify(), "notify flag lost in translation");
        assert!(!props.write());

        let value = measurement.read_value().await.expect("ReadValue failed");
        assert_eq!(value, vec![0x00, 72], "wrong bytes off the wire");

        // A second characteristic, to prove discovery is not returning one thing.
        let location = service
            .get_characteristic(characteristics::BODY_SENSOR_LOCATION)
            .await
            .expect("no body sensor location");
        assert_eq!(
            location.read_value().await.expect("ReadValue failed"),
            vec![0x02]
        );
    });
}

#[test]
fn reading_a_write_only_characteristic_is_refused_locally() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    block_on(async {
        let (_bluetooth, device) = granted().await.expect("request_device failed");
        device.gatt().connect().await.expect("connect failed");
        let service = device
            .gatt()
            .get_primary_service(services::HEART_RATE)
            .await
            .unwrap();
        let control = service
            .get_characteristic(CONTROL_POINT)
            .await
            .expect("no control point");

        // The peer is never asked: the properties say it cannot be read.
        let result = control.read_value().await;
        assert!(
            matches!(result, Err(Error::NotSupported(_))),
            "got {result:?}"
        );
    });
}

#[test]
fn a_write_reaches_the_peer() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    block_on(async {
        let (_bluetooth, device) = granted().await.expect("request_device failed");
        device.gatt().connect().await.expect("connect failed");
        let service = device
            .gatt()
            .get_primary_service(services::HEART_RATE)
            .await
            .unwrap();
        let control = service
            .get_characteristic(CONTROL_POINT)
            .await
            .expect("no control point");

        control
            .write_value_with_response(&[0x01])
            .await
            .expect("WriteValue failed");
        // The mock stores what it was given, so reading the property back
        // proves the bytes crossed the bus intact.
        assert_eq!(control.value(), Some(vec![0x01]));

        control
            .write_value_without_response(&[0x02])
            .await
            .expect("command write failed");
    });
}

#[test]
fn notifications_arrive_as_property_changes() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    block_on(async {
        let (_bluetooth, device) = granted().await.expect("request_device failed");
        device.gatt().connect().await.expect("connect failed");
        let service = device
            .gatt()
            .get_primary_service(services::HEART_RATE)
            .await
            .unwrap();
        let measurement = service
            .get_characteristic(characteristics::HEART_RATE_MEASUREMENT)
            .await
            .unwrap();

        let mut notifications = measurement
            .start_notifications()
            .await
            .expect("StartNotify failed");

        // The mock pushes one value when notifications are enabled.
        let received = webbluetooth::timeout(Duration::from_secs(5), notifications.next()).await;
        let value = received
            .expect("timed out waiting for a notification")
            .expect("stream ended");
        assert_eq!(value, vec![0x00, 77]);
        assert!(measurement.is_notifying());

        measurement
            .stop_notifications()
            .await
            .expect("StopNotify failed");
    });
}

#[test]
fn a_descriptor_reads() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    block_on(async {
        let (_bluetooth, device) = granted().await.expect("request_device failed");
        device.gatt().connect().await.expect("connect failed");
        let service = device
            .gatt()
            .get_primary_service(services::HEART_RATE)
            .await
            .unwrap();
        let measurement = service
            .get_characteristic(characteristics::HEART_RATE_MEASUREMENT)
            .await
            .unwrap();

        // The CCCD is readable but write-blocklisted; reading it must work.
        let descriptors = measurement.get_descriptors().await.expect("no descriptors");
        assert!(!descriptors.is_empty(), "the CCCD should be discoverable");

        let cccd = measurement
            .get_descriptor(webbluetooth::uuid::descriptors::CLIENT_CHARACTERISTIC_CONFIGURATION)
            .await
            .expect("no CCCD");
        assert_eq!(
            cccd.read_value().await.expect("descriptor read failed"),
            vec![0, 0]
        );

        // …and writing it is refused before any traffic, because that would
        // bypass start_notifications().
        let refused = cccd.write_value(&[0x01, 0x00]).await;
        assert!(
            matches!(refused, Err(Error::Security(_))),
            "got {refused:?}"
        );
    });
}

#[test]
fn the_service_allowlist_is_enforced() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    block_on(async {
        let (_bluetooth, device) = granted().await.expect("request_device failed");
        device.gatt().connect().await.expect("connect failed");

        // Generic Attribute was never requested, so it is a SecurityError even
        // though the device might well offer it.
        let refused = device
            .gatt()
            .get_primary_service(services::GENERIC_ATTRIBUTE)
            .await;
        assert!(
            matches!(refused, Err(Error::Security(_))),
            "got {refused:?}"
        );

        // The blocklist is enforced on the same path.
        let blocked = device
            .gatt()
            .get_primary_service(services::HUMAN_INTERFACE_DEVICE)
            .await;
        assert!(
            matches!(blocked, Err(Error::Security(_))),
            "got {blocked:?}"
        );
    });
}

#[test]
fn handles_go_stale_after_a_disconnect() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    block_on(async {
        let (_bluetooth, device) = granted().await.expect("request_device failed");
        let gatt = device.gatt();
        gatt.connect().await.expect("connect failed");
        let service = gatt
            .get_primary_service(services::HEART_RATE)
            .await
            .unwrap();
        let measurement = service
            .get_characteristic(characteristics::HEART_RATE_MEASUREMENT)
            .await
            .unwrap();
        assert!(measurement.read_value().await.is_ok());

        let mut disconnected = device.watch_disconnect();
        gatt.disconnect();
        let _ = webbluetooth::timeout(Duration::from_secs(5), disconnected.next()).await;

        // Every handle taken before the disconnect is now invalid. The
        // specification checks `gatt.connected` first, so the reason given is
        // the lost connection rather than the stale handle.
        let stale = measurement.read_value().await;
        assert!(matches!(stale, Err(Error::Network(_))), "got {stale:?}");
    });
}

#[test]
fn granted_devices_are_listed_and_can_be_forgotten() {
    if !have_bus() {
        return;
    }
    let _mock = exclusive();
    block_on(async {
        let (bluetooth, device) = granted().await.expect("request_device failed");
        let id = device.id().to_owned();
        assert!(bluetooth.get_devices().iter().any(|d| d.id() == id));

        device.forget();
        assert!(!bluetooth.get_devices().iter().any(|d| d.id() == id));
    });
}
