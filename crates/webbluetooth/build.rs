//! Mirrors `webbluetooth-apple`'s capability flag, since a `cfg` emitted by one
//! crate's build script does not reach another's — and embeds the `Info.plist`
//! that macOS reads before it will hand a process Bluetooth.

/// Link this crate's `Info.plist` into every executable it builds: its
/// examples and its test binaries.
///
/// macOS gates Bluetooth behind TCC, which reads
/// `NSBluetoothAlwaysUsageDescription` from the running image's
/// `__TEXT,__info_plist` section. An app bundle supplies it from
/// `Contents/Info.plist`; a bare `cargo run` binary has no bundle, so the
/// section is linked in instead. Without it the process is reported
/// `Unauthorized` and is never prompted — it just sees no adapter.
///
/// # Why here rather than in `.cargo/config.toml`
///
/// That is where this lived, as a `target.'cfg(target_os = "macos")'.rustflags`
/// entry naming `Info.plist` relative to the directory cargo was invoked from.
/// Two things follow from that, both silent:
///
/// * A `.cargo/config.toml` is discovered by walking up from the *invocation*
///   directory, so `cargo build --manifest-path …/webbluetooth/Cargo.toml` run
///   from anywhere else — an IDE, a wrapper script, a parent workspace — never
///   finds it. The build succeeds and produces a binary with no section in it,
///   which fails later as an authorization error with nothing to connect it to
///   the build that caused it.
/// * `cargo package` verifies an archive by compiling it in
///   `target/…/package/<crate>-<version>/`, which is a different workspace
///   root with no `Info.plist` in it, so the link fails outright — on
///   `webbluetooth-apple`'s *build script*, before a line of the crate is
///   compiled. Packaging on a Mac could not pass at all.
///
/// A build script has neither problem: `CARGO_MANIFEST_DIR` is absolute and
/// points at this package wherever it has been unpacked, and
/// `cargo::rustc-link-arg` applies to this package's own executables and not
/// to anyone's build script.
fn embed_info_plist(os: &str) {
    // macOS only. Every other Apple platform ships as a bundle and takes the
    // key from `Contents/Info.plist`; `scripts/ios.sh` writes one.
    if os != "macos" {
        return;
    }
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets this");
    println!("cargo::rerun-if-changed=Info.plist");
    println!("cargo::rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{dir}/Info.plist");
}

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    embed_info_plist(&os);
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // A browser is not an operating system: on `wasm32-unknown-unknown` the
    // target OS is the literal string "unknown", so anything phrased as
    // "every OS except X" silently includes the web.
    let web = arch == "wasm32";

    println!("cargo::rustc-check-cfg=cfg(peripheral_role)");
    // Windows exposes no LE L2CAP connection-oriented channel API — not in
    // WinRT, not in Win32. Neither does the web, which has no socket of any
    // kind. Every other platform has one.
    println!("cargo::rustc-check-cfg=cfg(l2cap)");
    if os != "windows" && !web {
        println!("cargo::rustc-cfg=l2cap");
    }
    if matches!(
        os.as_str(),
        "macos" | "ios" | "android" | "windows" | "linux"
    ) {
        println!("cargo::rustc-cfg=peripheral_role");
    }
}
