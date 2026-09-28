//! WebBluetoothExplorer — a Bluetooth Low Energy explorer.
//!
//! ```sh
//! cargo run -p webbluetooth-explorer --release
//! ```
//!
//! # macOS
//!
//! Bluetooth is gated behind TCC, and a process with no
//! `NSBluetoothAlwaysUsageDescription` is reported unauthorised and **is never
//! prompted** — it silently sees no adapter. `build.rs` links this crate's
//! `Info.plist` into the binary so `cargo run` works, and
//! `scripts/bundle-macos.sh` builds a real `WebBluetoothExplorer.app`.
//!
//! The prompt is attributed to the *responsible* process, so the first launch
//! has to come from something that can be granted Bluetooth itself: run it from
//! Terminal.app, or open the bundle from Finder. Launched from an editor or an
//! agent it is denied with no dialog.

// A GUI binary should not also open a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("WebBluetoothExplorer")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "WebBluetoothExplorer",
        options,
        Box::new(|cc| Ok(Box::new(webbluetooth_explorer::app::App::new(&cc.egui_ctx)))),
    )
}
