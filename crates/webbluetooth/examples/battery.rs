//! Read the battery level of the nearest device that exposes one, then read its
//! Device Information service.
//!
//! ```sh
//! cargo run -p webbluetooth --example battery
//! ```

use futures_executor::block_on;
use webbluetooth::chooser::StrongestSignal;
use webbluetooth::uuid::{characteristics, services};
use webbluetooth::{Bluetooth, DeviceFilter, RequestDeviceOptions, Result};

fn main() -> Result<()> {
    block_on(run())
}

async fn run() -> Result<()> {
    // Several devices may advertise a battery service; take the closest.
    let bluetooth = Bluetooth::with_chooser(StrongestSignal::default());

    if let Err(why) = bluetooth.availability().await {
        eprintln!("Bluetooth unavailable: {why} — try the `doctor` example.");
        std::process::exit(1);
    }

    let device = bluetooth
        .request_device(
            RequestDeviceOptions::new()
                .filter(DeviceFilter::new().service(services::BATTERY_SERVICE)?)
                .optional_service(services::DEVICE_INFORMATION)?,
        )
        .await?;

    let gatt = device.gatt();
    gatt.connect().await?;
    println!("{}", device.name().unwrap_or_else(|| device.id().into()));
    if let Ok(rssi) = device.rssi().await {
        println!("  signal    {rssi} dBm");
    }

    let battery = gatt.get_primary_service(services::BATTERY_SERVICE).await?;
    let level = battery
        .get_characteristic(characteristics::BATTERY_LEVEL)
        .await?;
    println!(
        "  battery   {}%",
        level.read_value().await?.first().copied().unwrap_or(0)
    );

    if let Ok(info) = gatt.get_primary_service(services::DEVICE_INFORMATION).await {
        for (label, uuid) in [
            ("maker", characteristics::MANUFACTURER_NAME_STRING),
            ("model", characteristics::MODEL_NUMBER_STRING),
            ("firmware", characteristics::FIRMWARE_REVISION_STRING),
        ] {
            if let Ok(c) = info.get_characteristic(uuid).await {
                if let Ok(bytes) = c.read_value().await {
                    println!("  {label:<9} {}", String::from_utf8_lossy(&bytes).trim());
                }
            }
        }
        // The serial number is on the GATT blocklist — it identifies the user —
        // so this fails with SecurityError no matter what the device offers.
        match info
            .get_characteristic(characteristics::SERIAL_NUMBER_STRING)
            .await
        {
            Err(e) => println!("  serial    refused ({})", e.name()),
            Ok(_) => println!("  serial    unexpectedly allowed"),
        }
    }

    gatt.disconnect();
    Ok(())
}
