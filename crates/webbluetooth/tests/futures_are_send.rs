//! Every public future has to be `Send`.
//!
//! Not a style preference: a `!Send` future can only be polled on the thread
//! that made it, which rules out every multi-threaded executor and, in
//! particular, the Node addon — which runs the crate's futures on a worker
//! thread so Node's own loop is never blocked.
//!
//! It is also easy to break by accident and invisible when you do. Holding a
//! raw pointer across an `await` is enough, and on Apple that is exactly what
//! happened: a `Vec<*mut c_void>` of CBUUIDs, built for one synchronous call
//! and left in scope over the suspension point after it. Nothing in the crate
//! noticed, because nothing in the crate needed `Send`.
//!
//! These are compile-time assertions. The bodies never run — constructing a
//! `Bluetooth` would touch the radio — and that is deliberate: the point is
//! that the code type-checks, not that it executes.

#![allow(
    dead_code,
    unreachable_code,
    unused_variables,
    clippy::diverging_sub_expression
)]

use webbluetooth::{Bluetooth, BluetoothDevice, RequestDeviceOptions};

fn assert_send<T: Send>(_: T) {}

fn the_entry_points_are_send() {
    let bluetooth: Bluetooth = unreachable!();
    assert_send(bluetooth.get_availability());
    assert_send(bluetooth.availability());
    assert_send(bluetooth.request_device(RequestDeviceOptions::new().accept_all_devices()));
    assert_send(bluetooth.adapters());
    assert_send(bluetooth.adapter());
    assert_send(bluetooth.select_adapter("hci0"));
}

fn the_device_operations_are_send() {
    let device: BluetoothDevice = unreachable!();
    assert_send(device.watch_advertisements());
    assert_send(device.rssi());
    assert_send(device.mtu());
    assert_send(device.connection_parameters());
}

/// The discovery path is the one that broke, so it is spelled out in full
/// rather than sampled.
fn the_gatt_tree_is_send() {
    let device: BluetoothDevice = unreachable!();
    let gatt = device.gatt();
    assert_send(gatt.connect());
    assert_send(gatt.get_primary_services(None));
    assert_send(gatt.get_primary_service("battery_service"));

    let service: webbluetooth::RemoteGattService = unreachable!();
    assert_send(service.get_characteristics(None));
    assert_send(service.get_characteristic("battery_level"));
    assert_send(service.get_included_services(None));

    let characteristic: webbluetooth::RemoteGattCharacteristic = unreachable!();
    assert_send(characteristic.read_value());
    assert_send(characteristic.write_value_with_response(&[]));
    assert_send(characteristic.write_value_without_response(&[]));
    assert_send(characteristic.start_notifications());
    assert_send(characteristic.stop_notifications());
    assert_send(characteristic.get_descriptors());

    let descriptor: webbluetooth::RemoteGattDescriptor = unreachable!();
    assert_send(descriptor.read_value());
    assert_send(descriptor.write_value(&[]));
}

/// The handles themselves have to cross threads too, or the futures above
/// could not be built on one thread and polled on another.
#[test]
fn the_public_types_are_send_and_sync() {
    fn both<T: Send + Sync>() {}
    both::<Bluetooth>();
    both::<BluetoothDevice>();
    both::<webbluetooth::RemoteGattServer>();
    both::<webbluetooth::RemoteGattService>();
    both::<webbluetooth::RemoteGattCharacteristic>();
    both::<webbluetooth::RemoteGattDescriptor>();
    both::<webbluetooth::Error>();
    both::<webbluetooth::AdapterInfo>();
}
