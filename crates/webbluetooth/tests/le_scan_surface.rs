//! The scanning extension, member by member.
//!
//! `requestLEScan` is not part of the Web Bluetooth IDL that
//! `tests/web_bluetooth_surface.rs` checks: it lives in a separate draft,
//! [Web Bluetooth Scanning][spec], which `scripts/update.sh surface` does not
//! fetch. So the crate implemented an interface that nothing compared against
//! its specification — and it showed, because two of the five members of
//! `BluetoothLEScan` were missing and nobody had a way to notice.
//!
//! The IDL is short enough to state here in full, which is what makes this
//! checkable rather than remembered:
//!
//! ```text
//! dictionary BluetoothLEScanOptions {
//!   sequence<BluetoothLEScanFilterInit> filters;
//!   boolean keepRepeatedDevices = false;
//!   boolean acceptAllAdvertisements = false;
//! };
//!
//! [Exposed=Window, SecureContext]
//! interface BluetoothLEScan {
//!   readonly attribute FrozenArray<BluetoothLEScanFilterInit> filters;
//!   readonly attribute boolean keepRepeatedDevices;
//!   readonly attribute boolean acceptAllAdvertisements;
//!   readonly attribute boolean active;
//!   undefined stop();
//! };
//!
//! partial interface Bluetooth {
//!   Promise<BluetoothLEScan> requestLEScan(
//!       optional BluetoothLEScanOptions options = {});
//! };
//! ```
//!
//! [spec]: https://webbluetoothcg.github.io/web-bluetooth/scanning.html

#![allow(
    dead_code,
    unreachable_code,
    unused_variables,
    clippy::diverging_sub_expression
)]

use webbluetooth::{Bluetooth, DeviceFilter, LeScan, LeScanOptions};

/// Every member of `BluetoothLEScan`. The bodies never run; naming each one is
/// the assertion, and a member that disappears stops this compiling.
fn the_scan_interface_is_complete(scan: LeScan) {
    let _: &[DeviceFilter] = scan.filters();
    let _: bool = scan.keep_repeated_devices();
    let _: bool = scan.accept_all_advertisements();
    let _: bool = scan.is_active(); // `active`
    scan.stop();
}

/// Every member of `BluetoothLEScanOptions`, and the call that takes it.
fn the_options_dictionary_is_complete() {
    let _ = LeScanOptions::new()
        .filter(DeviceFilter::new().name("a")) // filters
        .filters([DeviceFilter::new().name("b")]) // …as a sequence
        .keep_repeated_devices(true); // keepRepeatedDevices
    let _ = LeScanOptions::accept_all_advertisements(); // acceptAllAdvertisements

    let bluetooth: Bluetooth = unreachable!();
    // Bound rather than `let _`, which drops a future on the spot and reads as
    // a mistake to `clippy::let_underscore_future` — fairly, since everywhere
    // else it would be one.
    let _requested = bluetooth.request_le_scan(LeScanOptions::accept_all_advertisements());
}

/// A scan's options are not a device request's.
///
/// The distinction is the reason this type exists separately: a scan reports
/// advertisements and grants nothing, so there is no `optionalServices` to
/// give it and no exclusion filter to apply. Passing a `RequestDeviceOptions`
/// here used to be how a filtered scan was written, and the grant-shaped half
/// of it did nothing at all.
fn the_options_are_not_a_device_request() {
    // Compiles only because the scan type has its own builder: if this took a
    // RequestDeviceOptions, `filter` would not be a method on it.
    let _ = LeScanOptions::new().filter(DeviceFilter::new().name_prefix("Muse"));
}

/// The algorithm's argument check: "if `filters` is present and
/// `acceptAllAdvertisements` is true, reject" and "if neither, reject".
#[test]
fn the_filter_combination_is_checked_as_the_algorithm_does() {
    let empty = LeScanOptions::new();
    assert!(
        empty.validate().is_err(),
        "a scan with neither filters nor acceptAllAdvertisements has to be refused"
    );

    let both = LeScanOptions::accept_all_advertisements().filter(DeviceFilter::new().name("x"));
    assert!(
        both.validate().is_err(),
        "filters and acceptAllAdvertisements are mutually exclusive"
    );

    assert!(LeScanOptions::accept_all_advertisements()
        .validate()
        .is_ok());
    assert!(LeScanOptions::new()
        .filter(DeviceFilter::new().name_prefix("Muse"))
        .validate()
        .is_ok());
}

/// A filter the specification would reject is rejected here too, rather than
/// at the radio.
#[test]
fn an_impossible_filter_is_refused() {
    // An empty filter matches everything, which is what acceptAllAdvertisements
    // is for; the specification requires each filter to have a criterion.
    let options = LeScanOptions::new().filter(DeviceFilter::new());
    assert!(
        options.validate().is_err(),
        "an empty filter is not a filter"
    );
}
