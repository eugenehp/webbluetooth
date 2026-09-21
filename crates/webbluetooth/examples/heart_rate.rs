//! Subscribe to a heart-rate monitor — the canonical Web Bluetooth demo.
//!
//! ```sh
//! cargo run -p webbluetooth --example heart_rate
//! ```
//!
//! With the `terminal-chooser` feature you pick the device from a list instead
//! of taking the first match:
//!
//! ```sh
//! cargo run -p webbluetooth --example heart_rate --features terminal-chooser
//! ```

use futures_executor::block_on;
use webbluetooth::future::{select, Either};
use webbluetooth::prelude::*;
use webbluetooth::uuid::{characteristics, services};
use webbluetooth::{Bluetooth, DeviceFilter, RequestDeviceOptions, Result};

fn main() -> Result<()> {
    block_on(run())
}

async fn run() -> Result<()> {
    #[cfg(feature = "terminal-chooser")]
    let bluetooth = Bluetooth::with_chooser(webbluetooth::chooser::TerminalChooser::default());
    #[cfg(not(feature = "terminal-chooser"))]
    let bluetooth = Bluetooth::new();

    if let Err(why) = bluetooth.availability().await {
        eprintln!("Bluetooth unavailable: {why} — try the `doctor` example.");
        std::process::exit(1);
    }

    // The grant covers exactly these two services and nothing else.
    let device = bluetooth
        .request_device(
            RequestDeviceOptions::new()
                .filter(DeviceFilter::new().service(services::HEART_RATE)?)
                .optional_service(services::BATTERY_SERVICE)?,
        )
        .await?;

    println!(
        "found {}",
        device.name().unwrap_or_else(|| device.id().into())
    );

    let gatt = device.gatt();
    gatt.connect().await?;
    println!("connected");

    // Battery level is reachable because it was listed as an optional service.
    if let Ok(battery) = gatt.get_primary_service(services::BATTERY_SERVICE).await {
        if let Ok(level) = battery
            .get_characteristic(characteristics::BATTERY_LEVEL)
            .await
        {
            if let Ok(value) = level.read_value().await {
                println!("battery {}%", value.first().copied().unwrap_or(0));
            }
        }
    }

    let service = gatt.get_primary_service(services::HEART_RATE).await?;
    let measurement = service
        .get_characteristic(characteristics::HEART_RATE_MEASUREMENT)
        .await?;

    // End cleanly when the link drops rather than hanging on a dead stream.
    let mut disconnected = device.watch_disconnect();
    let mut beats = measurement.start_notifications().await?;
    println!("subscribed — ^C to stop\n");

    loop {
        let next_beat = std::pin::pin!(beats.next());
        let next_drop = std::pin::pin!(disconnected.next());
        match select(next_beat, next_drop).await {
            Either::Left((Some(bytes), _)) => match parse_heart_rate(&bytes) {
                Some(bpm) => println!("{bpm} bpm"),
                None => println!("malformed measurement: {bytes:02x?}"),
            },
            // The notification stream ended.
            Either::Left((None, _)) => break,
            Either::Right(_) => {
                println!("device disconnected");
                break;
            }
        }
    }
    Ok(())
}

/// Decode `org.bluetooth.characteristic.heart_rate_measurement`.
///
/// Bit 0 of the flags byte selects the width of the rate field: clear means one
/// byte, set means two, little-endian.
fn parse_heart_rate(value: &[u8]) -> Option<u16> {
    let flags = *value.first()?;
    if flags & 0x01 == 0 {
        value.get(1).map(|b| *b as u16)
    } else {
        Some(u16::from_le_bytes([*value.get(1)?, *value.get(2)?]))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_heart_rate;

    #[test]
    fn decodes_both_widths() {
        assert_eq!(parse_heart_rate(&[0x00, 72]), Some(72));
        assert_eq!(parse_heart_rate(&[0x01, 0x2C, 0x01]), Some(300));
        assert_eq!(parse_heart_rate(&[0x01, 0x2C]), None); // truncated
        assert_eq!(parse_heart_rate(&[]), None);
    }
}
