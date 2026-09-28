//! The desktop launcher.
//!
//! Everything lives in the library beside this, because iOS does not start a
//! process at a Rust `main`: the generated Xcode project links a staticlib
//! and calls [`webbluetooth_browser::run`] from its own entry point. Keeping
//! one implementation and two thin entry points is Tauri's mobile layout.

// Not `windows_subsystem = "windows"`: this is a macOS-first application and
// keeping stderr attached is worth more than a hidden console.

fn main() {
    webbluetooth_browser::run();
}
