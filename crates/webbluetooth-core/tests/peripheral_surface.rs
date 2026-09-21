//! The four peripheral-role adapters have to present the same surface too.
//!
//! The same problem as the central side, and until this test nothing watching
//! it: `webbluetooth::peripheral`'s types *are* the platform's types,
//! re-exported. So the public API of the peripheral role is whichever adapter
//! was linked, and a method only three of them have is a method that silently
//! is not there on the fourth.
//!
//! Writing this found exactly that. See `PublishedCharacteristic::try_notify_centrals`
//! below.
//!
//! The parser is the one `webbluetooth-core`'s backend test uses; there is no
//! reason for two.

mod common;

use common::{Exception, Surface};

const ADAPTERS: &[(&str, &str)] = &[
    (
        "apple",
        include_str!("../../webbluetooth-apple/src/peripheral_backend.rs"),
    ),
    (
        "linux",
        include_str!("../../webbluetooth-linux/src/peripheral_backend.rs"),
    ),
    (
        "android",
        include_str!("../../webbluetooth-android/src/peripheral_backend.rs"),
    ),
    (
        "windows",
        include_str!("../../webbluetooth-windows/src/peripheral_backend.rs"),
    ),
];

/// The types `peripheral::mod` re-exports, which therefore have to exist in
/// every adapter or the module does not compile on that platform.
const RE_EXPORTED: &[&str] = &[
    "Peripheral",
    "PublishedCharacteristic",
    "PublishedService",
    "ReadRequest",
    "RemoteCentral",
    "Request",
    "Requests",
    "WriteRequest",
];

const EXCEPTIONS: &[Exception] = &[Exception {
    member: "PublishedCharacteristic::try_notify_centrals",
    absent_from: &["linux"],
    why: "BlueZ's GATT server has no per-central addressing: a value reaches \
          subscribers by emitting PropertiesChanged on the characteristic's \
          own D-Bus object, which every subscriber is watching. Sending to \
          some of them is not a thing the API can express, and pretending \
          otherwise by notifying everybody would be wider than the caller asked",
}];

const POLYMORPHIC: &[(&str, &str)] = &[];

fn adapters() -> Vec<(&'static str, Surface)> {
    ADAPTERS
        .iter()
        .map(|(n, s)| (*n, common::read(s)))
        .collect()
}

#[test]
fn every_adapter_declares_the_same_members() {
    let problems = common::membership_problems(&adapters(), EXCEPTIONS);
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn every_adapter_declares_them_the_same_way() {
    let problems = common::signature_problems(&adapters(), POLYMORPHIC);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Every type the module re-exports has to be in every adapter.
#[test]
fn every_adapter_declares_the_re_exported_types() {
    let problems = common::type_problems(&adapters(), RE_EXPORTED);
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn the_exceptions_are_all_load_bearing() {
    let problems = common::stale_exceptions(&adapters(), EXCEPTIONS, POLYMORPHIC);
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn the_parser_still_reads_them() {
    for (name, surface) in adapters() {
        assert!(
            surface.methods.len() >= 25,
            "{name}: only {} members parsed",
            surface.methods.len()
        );
    }
}

#[test]
fn the_surface_is_reported() {
    let adapters = adapters();
    let all: std::collections::BTreeSet<_> = adapters
        .iter()
        .flat_map(|(_, s)| s.methods.keys())
        .collect();
    println!(
        "Peripheral surface: {} members across {} adapters",
        all.len(),
        adapters.len()
    );
    for (name, surface) in &adapters {
        println!("  {name:9} {} members", surface.methods.len());
    }
}
