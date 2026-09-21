//! The six central-role backends have to present the same surface, and only
//! one of them is ever compiled.
//!
//! That is the whole problem. `cargo check` on a Mac says nothing about the
//! Windows adapter; a method renamed in five backends and missed in the sixth
//! is a clean build until somebody targets the sixth. The old arrangement hid
//! this — the backends were one module reached through `#[cfg_attr(path)]`, so
//! "they all agree" was a thing six files happened to do rather than a thing
//! anybody checked.
//!
//! # Why this is a test and not a trait
//!
//! A trait was the obvious answer and it does not fit. Three of these methods
//! take `self: &Arc<Self>` — `connect`, `request_device` and
//! `set_radio_scanning` all hand a weak reference to a platform callback that
//! outlives the call — and arbitrary self types are not stable in traits. A
//! trait could only have them by changing fourteen method definitions to suit
//! the checking mechanism.
//!
//! The deeper reason is that a trait checks the backend being compiled, which
//! is the one that was going to be checked anyway. This reads all six on
//! whatever host it runs on, which is the case that actually goes wrong.

mod common;

use common::{Exception, Surface};

const BACKENDS: &[(&str, &str)] = &[
    (
        "apple",
        include_str!("../../webbluetooth-apple/src/backend.rs"),
    ),
    (
        "linux",
        include_str!("../../webbluetooth-linux/src/backend.rs"),
    ),
    (
        "linux-hci",
        include_str!("../../webbluetooth-linux/src/backend_hci.rs"),
    ),
    (
        "android",
        include_str!("../../webbluetooth-android/src/backend.rs"),
    ),
    (
        "windows",
        include_str!("../../webbluetooth-windows/src/backend.rs"),
    ),
    (
        "wasm",
        include_str!("../../webbluetooth-wasm/src/backend.rs"),
    ),
];

const EXCEPTIONS: &[Exception] = &[
    Exception {
        member: "Inner::l2cap_target",
        absent_from: &["windows", "wasm"],
        why: "neither WinRT nor the web exposes an L2CAP channel API",
    },
    Exception {
        member: "Inner::watch_advertisements",
        absent_from: &["apple", "linux", "linux-hci", "android", "windows"],
        why: "the browser watches per device; the native backends watch the \
              radio and filter, so BluetoothDevice drives it there instead",
    },
];

/// A member whose *type* is allowed to differ, and why.
const POLYMORPHIC: &[(&str, &str)] = &[(
    "Inner::l2cap_target",
    "returns whatever the platform hands over — a CBL2CAPChannel, an address \
     to connect to, an already-connected socket — which `L2capChannel::from_target` \
     turns into a channel",
)];

fn backends() -> Vec<(&'static str, Surface)> {
    BACKENDS
        .iter()
        .map(|(n, s)| (*n, common::read(s)))
        .collect()
}

#[test]
fn every_backend_declares_the_same_members() {
    let problems = common::membership_problems(&backends(), EXCEPTIONS);
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn every_backend_declares_them_the_same_way() {
    let problems = common::signature_problems(&backends(), POLYMORPHIC);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_exceptions_are_all_load_bearing() {
    let problems = common::stale_exceptions(&backends(), EXCEPTIONS, POLYMORPHIC);
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// The parser finding nothing would make every other assertion here vacuous.
#[test]
fn the_parser_still_reads_them() {
    for (name, surface) in backends() {
        assert!(
            surface.methods.len() >= 38,
            "{name}: only {} members parsed",
            surface.methods.len()
        );
    }
}

/// The count is printed rather than pinned: a new method is not a failure, it
/// is a thing to add to all six.
#[test]
fn the_surface_is_reported() {
    let backends = backends();
    let all: std::collections::BTreeSet<_> = backends
        .iter()
        .flat_map(|(_, s)| s.methods.keys())
        .collect();
    println!(
        "Backend surface: {} members across {} backends",
        all.len(),
        backends.len()
    );
    for (name, surface) in &backends {
        println!("  {name:10} {} members", surface.methods.len());
    }
}
