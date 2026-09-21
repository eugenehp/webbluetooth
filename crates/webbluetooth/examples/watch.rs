//! Watch advertisements without connecting — `requestLEScan()` and
//! `watchAdvertisements()`.
//!
//! ```sh
//! cargo run -p webbluetooth --example watch
//! ```
//!
//! Neither of these grants access to anything: they report what devices are
//! saying, which is all some peripherals ever do. The scan is shared, so this
//! also demonstrates the part that is easy to get wrong — a `requestDevice`
//! running at the same time must not cancel it.

use futures_executor::block_on;
use std::collections::HashMap;
use std::time::Duration;
use webbluetooth::chooser::StrongestSignal;
use webbluetooth::prelude::*;
use webbluetooth::{Bluetooth, LeScanOptions, RequestDeviceOptions, Result};

const WINDOW: Duration = Duration::from_secs(8);

fn main() -> Result<()> {
    block_on(run())
}

async fn run() -> Result<()> {
    let bluetooth = Bluetooth::new();
    if let Err(why) = bluetooth.availability().await {
        eprintln!("Bluetooth unavailable: {why} — try the `doctor` example.");
        std::process::exit(1);
    }

    // `keep_repeated_devices` is what makes RSSI and service data useful: with
    // it off, a device is reported once and never again.
    let mut scan = bluetooth
        .request_le_scan(LeScanOptions::accept_all_advertisements().keep_repeated_devices(true))
        .await?;
    println!("scanning for {WINDOW:?} — nothing is being granted\n");

    let mut packets: HashMap<String, (u32, i32)> = HashMap::new();
    let mut names: HashMap<String, String> = HashMap::new();
    let deadline = std::time::Instant::now() + WINDOW;

    while std::time::Instant::now() < deadline {
        let Some(event) = scan.next().await else {
            break;
        };
        let entry = packets.entry(event.id.clone()).or_insert((0, 127));
        entry.0 += 1;
        if let Some(rssi) = event.rssi() {
            entry.1 = rssi;
        }
        if let Some(name) = event.name.clone() {
            names.insert(event.id.clone(), name);
        }
    }
    assert!(scan.is_active(), "the scan should still be running");

    let mut rows: Vec<_> = packets.into_iter().collect();
    rows.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));

    println!("  packets  dBm   name / id");
    println!("  -------  ----  ---------------------------------");
    for (id, (count, rssi)) in rows.iter().take(15) {
        let label = names.get(id).cloned().unwrap_or_else(|| id.clone());
        let signal = if *rssi == 127 {
            "   —".to_string()
        } else {
            format!("{rssi:4}")
        };
        println!("  {count:>7}  {signal}  {label}");
    }
    println!(
        "\n{} device(s), {} repeat sightings",
        rows.len(),
        rows.iter()
            .map(|(_, (c, _))| c.saturating_sub(1))
            .sum::<u32>()
    );

    // ── The part that is easy to get wrong ──────────────────────────────────
    //
    // The radio scan is shared and reference-counted, so asking for a device
    // while a scan is running must not stop it on the way out. Before the hub,
    // `request_device` owned the scan and cancelled it when it finished.
    println!("\nrequesting a device while the scan runs…");
    let granted = Bluetooth::with_chooser(StrongestSignal::new(Duration::from_secs(3)))
        .request_device(RequestDeviceOptions::new().accept_all_devices())
        .await;

    match &granted {
        Ok(device) => println!(
            "  granted {}",
            device.name().unwrap_or_else(|| device.id().into())
        ),
        Err(e) => println!("  nothing granted: {e}"),
    }
    assert!(
        scan.is_active(),
        "a finished request_device must not stop somebody else's scan"
    );

    // Anything still arriving proves it, not just the flag.
    let mut after = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    while std::time::Instant::now() < deadline {
        if scan.next().await.is_some() {
            after += 1;
        }
    }
    println!("  {after} more sighting(s) after the request finished");
    assert!(after > 0, "the scan went quiet, so it was cancelled");
    drop(scan);

    // ── watchAdvertisements, on the device we were granted ──────────────────
    if let Ok(device) = granted {
        let mut watch = device.watch_advertisements().await?;
        println!("\nwatching {} alone…", device.id());
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        let mut seen = 0;
        while std::time::Instant::now() < deadline {
            let Some(event) = watch.next().await else {
                break;
            };
            // A watch reports one device and nothing else.
            assert_eq!(
                event.id,
                device.id(),
                "another device leaked into the watch"
            );
            seen += 1;
        }
        println!("  {seen} packet(s) from it, and nothing from anyone else");
    }
    Ok(())
}
