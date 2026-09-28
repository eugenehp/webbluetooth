//! WebBluetoothExplorer — a Bluetooth Low Energy explorer.
//!
//! A portable reimplementation of Joseph Ross's macOS
//! [Bluetility](https://github.com/jnross/Bluetility) on top of
//! [`webbluetooth`]: scan, connect, walk the GATT tree, read and write
//! characteristics, subscribe to notifications, on macOS, Linux and Windows
//! from one source tree.
//!
//! The binary is `main.rs`; everything it is built from is here so that the
//! parts with rules worth checking — [`model`], [`mod@format`], [`names`] — can be
//! tested without a window open, and so that [`engine`] can be driven against a
//! real radio by an integration test.
//!
//! ```text
//! engine  every webbluetooth handle, on one thread. Commands in, events out.
//! model   what is on screen, and how an event changes it.
//! app     drawing. Reads the model, sends commands, holds nothing else.
//! names   assigned-number lookup: UUID to something readable.
//! format  bytes to hex / ASCII / decimal / readings, and hex back to bytes.
//! store   what survives the process exiting.
//! presets known values for the control points devices publish.
//! server_templates ready-made services to publish.
//! macros  recorded sequences of operations.
//! suites  sequences with assertions, run against chosen targets.
//! decode  characteristic values, read as what they mean.
//! adtypes advertising data, split into the records it is made of.
//! beacons beacon frames, recognised and read.
//! ```

#![warn(missing_docs)]

pub mod adtypes;
pub mod app;
pub mod beacons;
pub mod decode;
pub mod engine;
pub mod format;
pub mod macros;
pub mod model;
pub mod names;
pub mod presets;
pub mod server_templates;
pub mod store;
pub mod suites;
