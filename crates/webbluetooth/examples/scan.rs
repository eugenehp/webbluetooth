//! List every BLE device in range.
//!
//! ```sh
//! cargo run -p webbluetooth --example scan
//! ```
//!
//! Also the smallest complete example of a custom [`DeviceChooser`]: this one
//! prints what it is offered and deliberately picks nothing, which is how you
//! observe a scan without granting access to anything.

use futures_executor::block_on;
use std::collections::HashMap;
use std::time::Duration;
use webbluetooth::prelude::*;
use webbluetooth::{
    Bluetooth, Candidates, DeviceChooser, DeviceFilter, Error, RequestDeviceOptions,
};

const WINDOW: Duration = Duration::from_secs(8);

/// Prints each new device, then declines to choose.
struct Printer;

impl DeviceChooser for Printer {
    fn choose(&self, mut candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        Box::pin(async move {
            let mut seen: HashMap<String, i32> = HashMap::new();
            let listen = async {
                while let Some(c) = candidates.next().await {
                    let rssi = c.rssi().unwrap_or(i32::MIN);
                    // Report a device once, then only when it gets notably closer.
                    let is_new = match seen.get(&c.id) {
                        None => true,
                        Some(best) => rssi > best + 8,
                    };
                    if !is_new {
                        continue;
                    }
                    seen.insert(c.id.clone(), rssi);

                    let services = c.advertisement.service_uuids.len();
                    println!(
                        "  {:>4}  {:<30} {}{}",
                        c.rssi()
                            .map(|d| format!("{d}"))
                            .unwrap_or_else(|| "—".into()),
                        c.label(),
                        c.id,
                        if services > 0 {
                            format!("  ({services} services)")
                        } else {
                            String::new()
                        }
                    );
                    for uuid in &c.advertisement.service_uuids {
                        println!("           service {uuid}");
                    }
                    for (company, data) in &c.advertisement.manufacturer_data {
                        println!(
                            "           manufacturer 0x{company:04x}: {} bytes",
                            data.len()
                        );
                    }
                }
            };
            let _ = webbluetooth::timeout(WINDOW, listen).await;
            println!("\n{} device(s) seen.", seen.len());
            None // decline — this example only observes
        })
    }
}

fn main() {
    let bluetooth = Bluetooth::new();

    if let Err(why) = block_on(bluetooth.availability()) {
        eprintln!("Bluetooth unavailable: {why}");
        eprintln!("Run `cargo run -p webbluetooth --example doctor` for the details.");
        std::process::exit(1);
    }

    println!("  dBm   name                           id");
    println!("  ----  -----------------------------  --------------------------------");

    // `accept_all_devices` is the unfiltered scan; a real application should
    // filter, both to narrow the results and because a service-filtered scan
    // also catches peripherals advertising in the overflow area.
    let options = RequestDeviceOptions::new().accept_all_devices();

    match block_on(bluetooth.request_device_with(options, &Printer)) {
        // Expected: the chooser declined.
        Err(Error::NotFound(_)) => {}
        Err(e) => eprintln!("scan failed: {e}"),
        Ok(device) => println!("unexpectedly granted {}", device.id()),
    }

    // Filtering by name prefix, for contrast — nothing is granted here either.
    let _unused = DeviceFilter::new().name_prefix("Polar");
}
