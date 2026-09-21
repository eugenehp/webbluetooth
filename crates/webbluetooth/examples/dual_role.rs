//! Both roles at once: advertise a GATT server *while* scanning for others.
//!
//! ```sh
//! cargo run -p webbluetooth --example dual_role
//! ```
//!
//! `CBCentralManager` and `CBPeripheralManager` are independent objects, so one
//! process can hold both. This crate gives each its own runtime-synthesised
//! delegate class, its own delegate instance and its own private dispatch
//! queue, so they never contend.
//!
//! # You will not see yourself
//!
//! A Bluetooth radio cannot receive its own transmissions, so this program will
//! never discover the very service it is advertising — and neither will a
//! second process on the same Mac, because they share one controller. Loopback
//! testing needs a second machine or a phone. What you *will* see is both roles
//! running at once: a scan listing everything else in range while a central on
//! another device connects to the server published here.
#![allow(unused_imports)]

use futures_executor::block_on;
use std::collections::HashSet;
use std::time::Duration;
use webbluetooth::future::{join, select, BoxFuture};
use webbluetooth::prelude::*;
use webbluetooth::uuid::{characteristics, services};
use webbluetooth::{Bluetooth, Candidates, DeviceChooser, Error, RequestDeviceOptions, Result};

#[cfg(peripheral_role)]
use webbluetooth::peripheral::{
    Advertising, AttError, Characteristic, Peripheral, Request, Service,
};

#[cfg(peripheral_role)]
const RUN_FOR: Duration = Duration::from_secs(20);

/// Prints what the scan finds and never picks anything, so no grant is made.
#[cfg(peripheral_role)]
struct Printer;

#[cfg(peripheral_role)]
impl DeviceChooser for Printer {
    fn choose(&self, mut candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        Box::pin(async move {
            let mut seen = HashSet::new();
            let listen = async {
                while let Some(c) = candidates.next().await {
                    if seen.insert(c.id.clone()) {
                        println!(
                            "  [central]    saw {:<28} {:>4} dBm",
                            c.label(),
                            c.rssi()
                                .map(|d| d.to_string())
                                .unwrap_or_else(|| "—".into())
                        );
                    }
                }
            };
            let _ = webbluetooth::timeout(RUN_FOR, listen).await;
            println!(
                "\n  [central]    {} device(s) seen, none selected",
                seen.len()
            );
            None
        })
    }
}

#[cfg(not(peripheral_role))]
fn main() {
    eprintln!(
        "This example needs both roles, and the peripheral role is unavailable on \
         this platform (CBPeripheralManager is API_UNAVAILABLE on watchOS, tvOS and visionOS)."
    );
}

#[cfg(peripheral_role)]
fn main() -> Result<()> {
    block_on(run())
}

#[cfg(peripheral_role)]
async fn run() -> Result<()> {
    let bluetooth = Bluetooth::new();
    let (peripheral, mut requests) = Peripheral::new();

    // One radio, so the two roles agree about it.
    if let Err(why) = bluetooth.availability().await {
        eprintln!("Bluetooth unavailable: {why} — try the `doctor` example.");
        std::process::exit(1);
    }

    // ── Peripheral: publish and advertise ───────────────────────────────────
    let published = peripheral
        .publish(
            Service::new(services::BATTERY_SERVICE)?.characteristic(
                Characteristic::new(characteristics::BATTERY_LEVEL)?
                    .read()
                    .notify(),
            ),
        )
        .await?;
    peripheral
        .start_advertising(
            Advertising::new()
                .local_name("Rust Dual")
                .service(services::BATTERY_SERVICE)?,
        )
        .await?;
    println!("  [peripheral] advertising as \"Rust Dual\"");
    println!("  [central]    scanning for {}s\n", RUN_FOR.as_secs());
    println!("  (the radio cannot hear itself — \"Rust Dual\" will not appear below)\n");

    let level = published
        .characteristic(characteristics::BATTERY_LEVEL)
        .expect("just published");

    // ── Both roles driven concurrently on one thread ────────────────────────
    let scanning = async {
        let options = RequestDeviceOptions::new().accept_all_devices();
        match bluetooth.request_device_with(options, &Printer).await {
            Err(Error::NotFound(_)) | Ok(_) => {}
            Err(e) => eprintln!("  [central]    scan failed: {e}"),
        }
    };

    let serving = async {
        let deadline = webbluetooth::sleep(RUN_FOR);
        let handle = async {
            while let Some(request) = requests.next().await {
                match request {
                    Request::Read(read) => {
                        println!("  [peripheral] read from {}", read.central().id());
                        let _ = read.respond(&[91]);
                    }
                    Request::Write(write) => write.reject_all(AttError::WriteNotPermitted),
                    Request::Subscribed { central, .. } => {
                        println!("  [peripheral] {} subscribed", central.id());
                        let _ = level.notify(&[91]).await;
                    }
                    Request::Unsubscribed { central, .. } => {
                        println!("  [peripheral] {} unsubscribed", central.id());
                    }
                    #[cfg(l2cap)]
                    Request::ChannelOpened(channel) => {
                        println!("  [peripheral] L2CAP open from {}", channel.peer_id());
                    }
                    Request::ReadyToNotify => {}
                }
            }
        };
        select(std::pin::pin!(deadline), std::pin::pin!(handle)).await;
    };

    join(scanning, serving).await;

    peripheral.stop_advertising();
    println!("\n  both roles stopped cleanly");
    Ok(())
}
