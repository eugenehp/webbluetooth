//! Write the generated callback classes to disk, so Android's own tooling can
//! check them.
//!
//! ```sh
//! cargo run -p webbluetooth-android --example emit-dex -- /tmp/out
//! dexdump -d /tmp/out/gatt.dex
//! ```
//!
//! `scripts/check.sh android` does exactly this inside Docker, where `dexdump`
//! is available.

use webbluetooth_android::dex::DexBuilder;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp".into());
    std::fs::create_dir_all(&dir).expect("could not create the output directory");

    for (name, dex) in webbluetooth_android::dex_classes() {
        let path = format!("{dir}/{name}.dex");
        std::fs::write(&path, &dex).expect("could not write the dex");
        println!("{path}  {} bytes", dex.len());
    }

    // A degenerate case worth keeping honest: no overrides at all.
    let empty = DexBuilder::new("dev/webbluetooth/Empty", "java/lang/Object").build();
    let path = format!("{dir}/empty.dex");
    std::fs::write(&path, &empty).expect("could not write the dex");
    println!("{path}  {} bytes", empty.len());
}
