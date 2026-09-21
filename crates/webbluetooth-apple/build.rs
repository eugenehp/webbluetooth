//! No swiftc, no dylib, no generated shim.
//!
//! Everything this crate needs from CoreBluetooth is Objective-C, reached
//! through `objc_msgSend`; the delegate classes are built at runtime with
//! `objc_allocateClassPair`. So the build step is naming the frameworks to link
//! against, plus one capability flag.

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // The peripheral role is not available on every Apple platform. The classes
    // are present in every SDK's .tbd — `objc_getClass("CBPeripheralManager")`
    // succeeds on a Watch — but the *initialisers* are annotated
    // `API_UNAVAILABLE(watchos, tvos)` and `API_UNAVAILABLE(visionos)`:
    //
    //   CBPeripheralManager  -initWithDelegate:queue:[options:]
    //   CBMutableService     -initWithType:primary:
    //   CBMutableCharacteristic -initWithType:properties:value:permissions:
    //   CBMutableDescriptor  -initWithType:value:
    //
    // Calling an unavailable initialiser is unsupported, so the whole role is
    // compiled out rather than left to fail at runtime. The central role has no
    // such annotations and is built everywhere.
    println!("cargo::rustc-check-cfg=cfg(peripheral_role)");
    if matches!(os.as_str(), "macos" | "ios" | "android" | "windows") {
        println!("cargo::rustc-cfg=peripheral_role");
    }

    // Nothing to link anywhere else; the crate compiles to nothing off Apple
    // so that `cargo build --workspace` works on Linux.
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo:rustc-link-lib=framework=CoreBluetooth");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=dylib=objc");
    }
}
