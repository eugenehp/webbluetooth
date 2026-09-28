//! Link `Info.plist` into the Mach-O image.
//!
//! macOS gates Bluetooth behind TCC, and a process with no
//! `NSBluetoothAlwaysUsageDescription` is reported `Unauthorized` and is never
//! prompted — it just silently sees no adapter. A bundle carries the key in
//! `Contents/Info.plist`; a bare `cargo run` binary has no bundle, so the
//! plist goes into a `__TEXT,__info_plist` section instead.
//!
//! The path has to be absolute. A relative one — in here or in
//! `.cargo/config.toml` — is resolved against the directory cargo was invoked
//! from, so building from the workspace root would quietly produce a binary
//! with no section at all.
fn main() {
    // `webbluetooth::peripheral` only exists where the platform has a
    // peripheral role, and the cfg deciding that is set by *its* build script
    // — build script cfgs do not reach dependants. So the same rule is
    // restated here, because code that names `webbluetooth::peripheral` has to
    // be gated the same way or it fails to compile on the platforms that
    // module is absent from.
    //
    // Kept deliberately identical to `crates/webbluetooth/build.rs`; if that
    // list gains a platform, this one has to as well.
    println!("cargo::rustc-check-cfg=cfg(peripheral_role)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if matches!(
        os.as_str(),
        "macos" | "ios" | "android" | "windows" | "linux"
    ) {
        println!("cargo::rustc-cfg=peripheral_role");
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo::rerun-if-changed=Info.plist");
        println!("cargo::rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{dir}/Info.plist");
    }
}
