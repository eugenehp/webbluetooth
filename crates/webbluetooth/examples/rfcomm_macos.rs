//! Open an RFCOMM channel to a paired Bluetooth Classic device, on macOS.
//!
//! ```sh
//! # List what is paired.
//! cargo run -p webbluetooth --example rfcomm_macos --features rfcomm
//! # Open channel 1 on one of them, by address or by name.
//! cargo run -p webbluetooth --example rfcomm_macos --features rfcomm -- AA:BB:CC:DD:EE:FF 1
//! cargo run -p webbluetooth --example rfcomm_macos --features rfcomm -- "Some Headset" 1
//! ```
//!
//! Not part of Web Bluetooth, and not CoreBluetooth either: Bluetooth Classic
//! on a Mac is IOBluetooth, which the `rfcomm` feature links. The device has to
//! be paired in System Settings beforehand.
//!
//! **Events arrive on the main thread's run loop**, so `main` keeps it turning
//! and the work runs on another thread — see `main` below.
//!
//! **Opening needs an entitlement.** macOS refuses an RFCOMM channel with
//! `kIOReturnNotPermitted` unless the binary is signed with
//! `com.apple.security.device.bluetooth`, which a bare `cargo run` binary is
//! not. Listing the paired devices works without it.

#[cfg(not(all(feature = "rfcomm", target_os = "macos")))]
fn main() {
    eprintln!("macOS only, and it needs --features rfcomm.");
    std::process::exit(1);
}

/// IOBluetooth delivers every RFCOMM event — the open completing, each chunk of
/// data, the peer closing — on the main thread's run loop. So the work runs on
/// another thread and the main thread keeps that run loop turning until it is
/// done. An app gets this from `NSApplication`; a command-line program has to
/// do it itself, and without it the open fails and says so.
#[cfg(all(feature = "rfcomm", target_os = "macos"))]
fn main() -> webbluetooth::Result<()> {
    use std::sync::mpsc;
    use std::time::Duration;

    unsafe extern "C" {
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, ret: bool) -> i32;
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
    }

    let (done_tx, done) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(futures_executor::block_on(run()));
    });
    loop {
        match done.recv_timeout(Duration::ZERO) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, false) };
    }
}

#[cfg(all(feature = "rfcomm", target_os = "macos"))]
async fn run() -> webbluetooth::Result<()> {
    use webbluetooth::stream::StreamExt;
    use webbluetooth::Bluetooth;

    let bluetooth = Bluetooth::shared();
    let paired = bluetooth.paired_classic_devices().await?;

    let mut args = std::env::args().skip(1);
    let Some(wanted) = args.next() else {
        println!("{} paired Bluetooth Classic device(s):", paired.len());
        for device in &paired {
            println!(
                "  {}  {}{}",
                device
                    .address
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "(no address)".into()),
                device.name.as_deref().unwrap_or("(unnamed)"),
                if device.connected {
                    "  [connected]"
                } else {
                    ""
                },
            );
        }
        return Ok(());
    };
    let channel: u8 = match args.next() {
        Some(arg) => arg
            .parse()
            .unwrap_or_else(|_| panic!("not a channel: {arg}")),
        None => 1,
    };

    // A name is resolved to its address here, so the open itself is by
    // address and cannot land on a different device of the same name.
    let address = paired
        .iter()
        .find(|device| device.name.as_deref() == Some(wanted.as_str()))
        .and_then(|device| device.address)
        .map(|a| a.to_string())
        .unwrap_or(wanted);

    println!("opening RFCOMM channel {channel} on {address}…");
    let rfcomm = bluetooth.open_rfcomm_channel(&address, channel).await?;
    println!("open. Ctrl-C to stop.");

    let mut incoming = rfcomm.take_incoming().expect("first take");
    while let Some(bytes) = incoming.next().await {
        let hex: Vec<String> = bytes.iter().take(32).map(|b| format!("{b:02x}")).collect();
        println!("{:>4} B  {}", bytes.len(), hex.join(" "));
    }
    println!("closed: {:?}", rfcomm.closed());
    Ok(())
}
