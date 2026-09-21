//! Linux BLE from pure Rust, with no dependencies.
//!
//! Two ways to reach a controller, because Linux offers two:
//!
//! * **`bluez`** — the supported path, over `dbus` to `org.bluez`. Pairing,
//!   bonding and agents are the daemon's problem.
//! * **`hci` + `gatt`** — no daemon at all. ATT over an L2CAP socket for
//!   GATT, a raw HCI socket for scanning.
//!
//! Neither binds a C library. D-Bus is a wire protocol over a Unix socket and
//! is spoken as one; ATT, HCI and L2CAP are kernel sockets reached through
//! `sys`.
//!
//! [`dbus::codec`], [`dbus::value`] and `att` are pure byte manipulation and
//! build everywhere, so the parts where the bugs live — alignment, truncation,
//! signature-directed decoding — are unit-tested on any host rather than only
//! inside a container.
//!
//! ## License
//! MIT — Copyright © 2025 [Eugene Hauptmann](https://github.com/eugenehp)

pub mod att;
pub mod dbus;

/// LE scanning. The advertising-data parser is portable; only the raw socket
/// inside it is Linux-only.
pub mod hci;

// The adapter: BlueZ expressed as the portable model.
#[cfg(target_os = "linux")]
// Public so `webbluetooth` can reach it, and not an API anyone else
// should call: the facade is what a caller holds. Hidden from the docs
// for the same reason, which also keeps the same 38 adapter methods from
// being documented six times over.
#[doc(hidden)]
pub mod backend;
// The peripheral-role adapter: a GATT server expressed as the portable model.
#[cfg(target_os = "linux")]
pub mod peripheral_backend;

// The same adapter without BlueZ: ATT over an L2CAP socket for GATT, a raw
// HCI socket for scanning.
//
// Compiled whenever the other one is, rather than behind the `linux-hci`
// feature that selects it. Two adapters that are only ever built one at a
// time are two adapters that drift, and this one is the one almost nobody
// builds.
#[cfg(target_os = "linux")]
// Public so `webbluetooth` can reach it, and not an API anyone else
// should call: the facade is what a caller holds. Hidden from the docs
// for the same reason, which also keeps the same 38 adapter methods from
// being documented six times over.
#[doc(hidden)]
pub mod backend_hci;
#[cfg(target_os = "linux")]
pub mod bluez;
#[cfg(target_os = "linux")]
pub mod gatt;
#[cfg(target_os = "linux")]
pub mod l2cap;
#[cfg(target_os = "linux")]
pub mod sys;
