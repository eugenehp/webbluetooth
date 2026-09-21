//! Web Bluetooth from Rust in the browser.
//!
//! Every other backend in this workspace reaches the radio: CoreBluetooth
//! through the Objective-C runtime, BlueZ over D-Bus, the kernel over an HCI
//! socket, WinRT through COM vtables. In a browser there is no radio to reach.
//! `navigator.bluetooth` *is* the interface, the page is sandboxed beneath it,
//! and WebAssembly cannot call it — a wasm module has linear memory and
//! numbers, no DOM and no way to obtain one.
//!
//! So this backend is the one that does not speak a protocol. It speaks to the
//! page, and the page speaks Web Bluetooth. That boundary needs glue in
//! JavaScript; `wasm-bindgen` would generate it, and here it is written down
//! instead, in `js/webbluetooth.js`, because a generated bridge is still a
//! bridge and this one is short enough to read in a sitting.
//!
//! What that buys is worth being clear about: the same Rust program, and the
//! same API, runs against a real controller on five platforms and against the
//! browser's own Web Bluetooth on the sixth. The permission model on this one
//! is the browser's — the chooser is Chrome's device picker, not this crate's,
//! and the blocklist enforced is the browser's own.
//!
//! # Shape of the boundary
//!
//! - [`host`] — requests out, answers back, one import for all of them.
//! - [`events`] — notifications and disconnections pushed in.
//! - [`codec`] — how a message is laid out, since wasm passes only numbers.
//! - [`exec`] — somewhere for futures to live, since a browser cannot block.
//!
//! # Using it
//!
//! ```html
//! <script type="module">
//!   import { start } from './webbluetooth.js';
//!   await start('./your_app.wasm');
//! </script>
//! ```
//!
//! The shim imports nothing and depends on nothing.

// The adapter: this platform expressed as the portable model.
// Public so `webbluetooth` can reach it, and not an API anyone else
// should call: the facade is what a caller holds. Hidden from the docs
// for the same reason, which also keeps the same 38 adapter methods from
// being documented six times over.
#[doc(hidden)]
pub mod backend;
pub mod codec;
pub mod events;
pub mod exec;
pub mod host;

pub use events::{set_sink, Event};
pub use exec::{oneshot, spawn, Oneshot, Sender, TaskId};
pub use host::{call, call_empty, call_str, Answer, HostError, Op};

/// The shim, so a build can emit it next to the `.wasm` without the file
/// having to be copied by hand or fetched from anywhere.
///
/// Writing it out is one line, and it cannot drift from the ABI above because
/// it is compiled in from the same source tree.
pub const SHIM_JS: &str = include_str!("../js/webbluetooth.js");

#[cfg(test)]
mod tests {
    use super::*;

    /// The shim is the other half of the ABI. If it is empty or truncated the
    /// module loads and then fails at the first call, which is a much worse
    /// place to find out.
    #[test]
    fn the_shim_is_present_and_implements_the_boundary() {
        assert!(
            SHIM_JS.len() > 1_000,
            "the shim looks truncated: {} bytes",
            SHIM_JS.len()
        );
        for expected in [
            "wbt_call",
            "wbt_settle",
            "wbt_event",
            "wbt_alloc",
            "wbt_free",
            "navigator.bluetooth",
        ] {
            assert!(
                SHIM_JS.contains(expected),
                "the shim does not mention {expected:?}"
            );
        }
    }

    /// Every operation, by the name the shim gives it and the number this
    /// side gives it.
    ///
    /// The pairing is the whole point: the two halves are separate languages
    /// with no shared header, so the only thing stopping `RequestDevice` from
    /// meaning `Connect` is that both files say `2`.
    const OPS: &[(Op, &str)] = &[
        (Op::Availability, "AVAILABILITY"),
        (Op::RequestDevice, "REQUEST_DEVICE"),
        (Op::GetDevices, "GET_DEVICES"),
        (Op::Forget, "FORGET"),
        (Op::Connect, "CONNECT"),
        (Op::Disconnect, "DISCONNECT"),
        (Op::DiscoverServices, "DISCOVER_SERVICES"),
        (Op::DiscoverIncludedServices, "DISCOVER_INCLUDED_SERVICES"),
        (Op::DiscoverCharacteristics, "DISCOVER_CHARACTERISTICS"),
        (Op::DiscoverDescriptors, "DISCOVER_DESCRIPTORS"),
        (Op::ReadCharacteristic, "READ_CHARACTERISTIC"),
        (Op::WriteCharacteristic, "WRITE_CHARACTERISTIC"),
        (Op::ReadDescriptor, "READ_DESCRIPTOR"),
        (Op::WriteDescriptor, "WRITE_DESCRIPTOR"),
        (Op::SetNotify, "SET_NOTIFY"),
        (Op::WatchAdvertisements, "WATCH_ADVERTISEMENTS"),
        (Op::UnwatchAdvertisements, "UNWATCH_ADVERTISEMENTS"),
        (Op::SetTimeout, "SET_TIMEOUT"),
    ];

    /// Both sides must agree on what number each operation is.
    ///
    /// This is the failure that would be worst to debug: the call succeeds,
    /// the wrong thing happens, and nothing reports an error — a read that
    /// silently connects, or a write that forgets the device.
    #[test]
    fn the_shim_and_this_side_agree_on_every_op_code() {
        for (op, name) in OPS {
            let expected = format!("{name}: {},", *op as u32);
            assert!(
                SHIM_JS.contains(&expected),
                "the shim should define `{expected}` for {op:?}, and does not"
            );
        }
    }

    /// And every one of them must actually be handled, or the call returns a
    /// token that is never settled and the caller waits forever.
    #[test]
    fn the_shim_handles_every_operation() {
        for (op, name) in OPS {
            assert!(
                SHIM_JS.contains(&format!("case OPS.{name}:")),
                "the shim has no case for {op:?}"
            );
        }
    }

    /// The event codes have the same problem in the other direction: a
    /// mismatch turns a disconnection into a notification.
    #[test]
    fn the_shim_and_this_side_agree_on_every_event_code() {
        for (kind, name) in [
            (
                crate::host::EventKind::CharacteristicValue,
                "CHARACTERISTIC_VALUE",
            ),
            (
                crate::host::EventKind::GattServerDisconnected,
                "DISCONNECTED",
            ),
            (crate::host::EventKind::ServiceChanged, "SERVICE_CHANGED"),
            (
                crate::host::EventKind::AdvertisementReceived,
                "ADVERTISEMENT",
            ),
            (
                crate::host::EventKind::AvailabilityChanged,
                "AVAILABILITY_CHANGED",
            ),
        ] {
            let expected = format!("{name}: {},", kind as u32);
            assert!(
                SHIM_JS.contains(&expected),
                "the shim should define `{expected}` for {kind:?}, and does not"
            );
        }
    }

    /// As do the error codes, which decide whether a caller retries.
    #[test]
    fn the_shim_and_this_side_agree_on_every_error_code() {
        for (error, name) in [
            (HostError::NotFound, "NotFoundError"),
            (HostError::Security, "SecurityError"),
            (HostError::Network, "NetworkError"),
            (HostError::InvalidState, "InvalidStateError"),
            (HostError::NotSupported, "NotSupportedError"),
            (HostError::InvalidModification, "InvalidModificationError"),
            (HostError::Abort, "AbortError"),
            (HostError::NotAllowed, "NotAllowedError"),
        ] {
            let expected = format!("{name}: {},", error as u32);
            assert!(
                SHIM_JS.contains(&expected),
                "the shim should map `{expected}`, and does not"
            );
        }
    }
}
