//! Every public type implements the traits a caller expects of one.
//!
//! Rust's API guidelines put it as "eagerly implement common traits", and the
//! reason is that the absence only shows up in somebody else's code: a type
//! without `Debug` cannot go in a struct that derives it, cannot be `dbg!`ed,
//! and cannot be printed in the error message that would have explained what
//! went wrong. Nothing in this crate notices, because nothing in this crate
//! tries.
//!
//! Three types were missing `Debug` when this was written —
//! `RemoteGattServer`, `Notifications` and `DisconnectEvents` — while the
//! handles either side of them had it. That is the shape of the problem: not a
//! decision, an omission, and invisible from the inside.
//!
//! These are compile-time assertions. The bodies never run.

#![allow(dead_code)]

use webbluetooth::{
    Advertisements, AvailabilityEvents, Bluetooth, BluetoothDevice, DisconnectEvents, LeScan,
    Notifications, RemoteGattCharacteristic, RemoteGattDescriptor, RemoteGattServer,
    RemoteGattService, ServiceEvents,
};

fn debug<T: std::fmt::Debug>() {}
fn clone<T: Clone>() {}
fn send_sync<T: Send + Sync>() {}
fn display<T: std::fmt::Display>() {}
fn error<T: std::error::Error>() {}

/// Anything a caller holds can be printed.
fn the_public_types_are_debug() {
    debug::<Bluetooth>();
    debug::<BluetoothDevice>();
    debug::<RemoteGattServer>();
    debug::<RemoteGattService>();
    debug::<RemoteGattCharacteristic>();
    debug::<RemoteGattDescriptor>();
    debug::<webbluetooth::Advertisement>();
    debug::<webbluetooth::AdapterInfo>();
    debug::<webbluetooth::CharacteristicProperties>();
    debug::<webbluetooth::DeviceFilter>();
    debug::<webbluetooth::RequestDeviceOptions>();
    debug::<webbluetooth::BluetoothUuid>();
    debug::<webbluetooth::Error>();
    debug::<webbluetooth::Availability>();
    debug::<webbluetooth::WriteType>();
}

/// Including the event sources, which end up in application state.
fn the_streams_are_debug() {
    debug::<Notifications>();
    debug::<webbluetooth::Tagged>();
    debug::<DisconnectEvents>();
    debug::<Advertisements>();
    debug::<AvailabilityEvents>();
    debug::<ServiceEvents>();
    debug::<LeScan>();
}

/// A handle is a handle: cloning one is another way to reach the same device,
/// not another device. A *stream* is deliberately not `Clone` — two clones
/// would divide the items between them rather than each seeing all of them.
fn the_handles_are_clone() {
    clone::<Bluetooth>();
    clone::<BluetoothDevice>();
    clone::<RemoteGattServer>();
    clone::<RemoteGattService>();
    clone::<RemoteGattCharacteristic>();
    clone::<RemoteGattDescriptor>();
}

/// The error type behaves like one.
fn the_error_type_is_an_error() {
    error::<webbluetooth::Error>();
    display::<webbluetooth::Error>();
    // `availability()` hands this back on its own, so it has to print on its
    // own — it used to have its message written inside `Error`'s `Display`
    // and nowhere else, which left `{availability}` unavailable to a caller.
    display::<webbluetooth::Availability>();
    // And be an error on its own, for the same reason: a caller reporting one
    // beside its other failures should not have to translate it first. It is
    // not an `Error` variant — an unusable radio is a state rather than a
    // failed call — but it is still something that went wrong.
    error::<webbluetooth::Availability>();
    display::<webbluetooth::BluetoothUuid>();
}

/// A handle that could not cross a thread would rule out every multi-threaded
/// executor. `tests/futures_are_send.rs` makes the same point about the
/// futures; this is about the values they are called on.
fn the_handles_cross_threads() {
    send_sync::<Bluetooth>();
    send_sync::<BluetoothDevice>();
    send_sync::<RemoteGattServer>();
    send_sync::<RemoteGattService>();
    send_sync::<RemoteGattCharacteristic>();
    send_sync::<RemoteGattDescriptor>();
}

/// The peripheral role's definitions are values, and values get compared and
/// copied around before anything is published.
#[cfg(peripheral_role)]
fn the_peripheral_definitions_are_values() {
    use webbluetooth::peripheral;

    debug::<peripheral::Service>();
    debug::<peripheral::Characteristic>();
    debug::<peripheral::Descriptor>();
    debug::<peripheral::Advertising>();
    debug::<peripheral::Permissions>();
    debug::<peripheral::AttError>();
    clone::<peripheral::Service>();
    clone::<peripheral::Characteristic>();
    clone::<peripheral::Descriptor>();
    clone::<peripheral::Advertising>();
}

#[test]
fn they_all_type_check() {
    // The assertions are the function signatures above; this exists so the
    // file is a test rather than dead code, and so `cargo test` reports it.
}
