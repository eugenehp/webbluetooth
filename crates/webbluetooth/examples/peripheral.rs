//! Act as a BLE peripheral: publish a GATT server and advertise it.
//!
//! ```sh
//! cargo run -p webbluetooth --example peripheral
//! ```
//!
//! Publishes a Battery Service with a dynamic level (answered on demand and
//! pushed to subscribers) plus a writable control point, and advertises under a
//! local name. Connect with any BLE explorer — nRF Connect, LightBlue, or
//! another Mac running the `scan` example.
//!
//! The peripheral role is not part of Web Bluetooth. Each platform reaches it
//! its own way — `CBPeripheralManager`, `BluetoothGattServer`,
//! `GattServiceProvider`, or a GATT application registered with `bluetoothd` —
//! behind the one API here.
//!
//! On Linux, `WEBBLUETOOTH_ADAPTER=hci1` puts it on a chosen controller, which
//! is how one machine can run both ends of a conversation.

#![allow(unused_imports)]

use futures_executor::block_on;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
#[cfg(peripheral_role)]
use webbluetooth::peripheral::{
    Advertising, AttError, Characteristic, Descriptor, Peripheral, Request, Service,
};
use webbluetooth::prelude::*;
use webbluetooth::uuid::{characteristics, services};
use webbluetooth::Result;

/// The value served for reads and pushed on notify.
#[cfg(peripheral_role)]
static BATTERY: AtomicU8 = AtomicU8::new(87);

#[cfg(not(peripheral_role))]
fn main() {
    eprintln!(
        "The peripheral role is unavailable on this platform: CoreBluetooth's \
         CBPeripheralManager initialisers are API_UNAVAILABLE on watchOS, tvOS \
         and visionOS."
    );
}

#[cfg(peripheral_role)]
fn main() -> Result<()> {
    block_on(run())
}

#[cfg(peripheral_role)]
async fn run() -> Result<()> {
    let (peripheral, mut requests) = Peripheral::new();

    if let Err(why) = peripheral.availability().await {
        eprintln!("Bluetooth unavailable: {why} — try the `doctor` example.");
        std::process::exit(1);
    }

    // A dynamic value: no fixed bytes, so reads arrive as Request::Read and the
    // characteristic is free to also notify. Giving it a fixed value here would
    // force it read-only and be rejected before anything is published.
    let level = Characteristic::new(characteristics::BATTERY_LEVEL)?
        .read()
        .notify()
        .descriptor(Descriptor::user_description("Battery level, percent"));

    // A writable control point, to show inbound writes.
    let control = Characteristic::new("6e400002-b5a3-f393-e0a9-e50e24dcca9e")?
        .write()
        .write_without_response();

    let published = peripheral
        .publish(
            Service::new(services::BATTERY_SERVICE)?
                .characteristic(level)
                .characteristic(control),
        )
        .await?;
    println!("published {}", published.uuid());

    peripheral
        .start_advertising(
            Advertising::new()
                .local_name("Rust Battery")
                .service(services::BATTERY_SERVICE)?,
        )
        .await?;
    println!("advertising as \"Rust Battery\" — ^C to stop\n");

    let battery_level = published
        .characteristic(characteristics::BATTERY_LEVEL)
        .expect("just published")
        .clone();

    // Drain one percent every few seconds and push it to whoever is listening.
    let ticker = Arc::new(battery_level.clone());
    std::thread::spawn({
        let ticker = ticker.clone();
        move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let next = BATTERY.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
            if next == 0 {
                BATTERY.store(100, Ordering::Relaxed);
            }
            // Fire-and-forget: if the queue is full this drops the tick rather
            // than blocking, and the next one carries the newer value anyway.
            if !ticker.subscribers().is_empty() && ticker.try_notify(&[next]) {
                println!("  notified {next}%");
            }
        }
    });

    while let Some(request) = requests.next().await {
        match request {
            Request::Read(read) => {
                let value = BATTERY.load(Ordering::Relaxed);
                println!(
                    "read  {} from {}",
                    read.characteristic(),
                    read.central().id()
                );
                read.respond(&[value])?;
            }

            Request::Write(write) => {
                for w in write.writes() {
                    println!("write {} <- {:02x?}", w.characteristic, w.value);
                }
                // A control point that only accepts a single byte.
                if write.writes().iter().all(|w| w.value.len() == 1) {
                    write.accept();
                } else {
                    write.reject_all(AttError::InvalidAttributeValueLength);
                }
            }

            Request::Subscribed {
                central,
                characteristic,
            } => {
                println!(
                    "subscribe {characteristic} by {} (mtu {})",
                    central.id(),
                    central.max_notification_length()
                );
                // Send the current value straight away so the subscriber has
                // something before the next tick.
                battery_level
                    .notify(&[BATTERY.load(Ordering::Relaxed)])
                    .await?;
            }

            Request::Unsubscribed {
                central,
                characteristic,
            } => {
                println!("unsubscribe {characteristic} by {}", central.id());
            }

            // A central connected to the published PSM. Echo whatever it sends.
            #[cfg(l2cap)]
            Request::ChannelOpened(channel) => {
                println!(
                    "L2CAP open on PSM 0x{:04x} from {}",
                    channel.psm(),
                    channel.peer_id()
                );
                let Some(mut incoming) = channel.take_incoming() else {
                    continue;
                };
                std::thread::spawn(move || {
                    block_on(async move {
                        while let Some(chunk) = incoming.next().await {
                            println!("  L2CAP {} bytes in", chunk.len());
                            if channel.send(&chunk).is_err() {
                                break;
                            }
                        }
                        println!("  L2CAP closed: {:?}", channel.closed());
                    });
                });
            }

            Request::ReadyToNotify => {}
        }
    }
    Ok(())
}
