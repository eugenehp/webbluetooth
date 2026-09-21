//! Windows BLE from pure Rust, with no dependencies.
//!
//! WinRT is COM, and COM is a calling convention: an object is a pointer to a
//! table of function pointers, and the runtime is a handful of exports from
//! `combase.dll`. So this needs no bindings crate — only the right interface
//! identifiers, which are vendored in [`iids`] from Windows metadata rather
//! than transcribed, and the right vtable layouts.
//!
//! Callbacks work the way they do everywhere else here: Windows wants an object
//! it can call into, so one is built — in this case a COM object, which is a
//! struct whose first field is a vtable pointer.
//!
//! ## License
//! MIT — Copyright © 2025 [Eugene Hauptmann](https://github.com/eugenehp)

// The adapter: this platform expressed as the portable model.
// Public so `webbluetooth` can reach it, and not an API anyone else
// should call: the facade is what a caller holds. Hidden from the docs
// for the same reason, which also keeps the same 38 adapter methods from
// being documented six times over.
#[doc(hidden)]
pub mod backend;
// The peripheral-role adapter: a GATT server expressed as the portable model.
#[cfg(target_os = "windows")]
pub mod peripheral_backend;

pub mod ble;
pub mod com;
pub mod guid;
pub mod iids;
pub mod winrt;
