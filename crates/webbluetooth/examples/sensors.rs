//! Subscribe to everything a device will notify, and print it as it arrives.
//!
//! ```sh
//! cargo run -p webbluetooth --example sensors -- heart_rate battery_service
//! cargo run -p webbluetooth --example sensors -- 0000fe8d-0000-1000-8000-00805f9b34fb
//! cargo run -p webbluetooth --example sensors --features terminal-chooser -- battery_service
//! ```
//!
//! The services are arguments because they have to be: Web Bluetooth grants
//! access to exactly the services a request names, so there is no "everything"
//! to ask for. That is the security model working, not a gap — and it is the
//! rule most newcomers meet as a `SecurityError` on their first
//! `get_primary_service`. Each argument is anything [`IntoUuid`] accepts: an
//! assigned name, a 16-bit number, or a full 128-bit UUID.
//!
//! Within those services this subscribes to every characteristic that will
//! notify or indicate, merges them into one stream, and prints what comes
//! back. Which is the shape of any real sensor: a Muse headset streams EEG on
//! four characteristics and optical on three, all at once, and what a program
//! needs from that is one loop that can still tell them apart.
//!
//! [`IntoUuid`]: webbluetooth::IntoUuid

use futures_executor::block_on;
use std::collections::BTreeMap;
use std::time::Duration;
use webbluetooth::future::{select, Either};
use webbluetooth::prelude::*;
use webbluetooth::stream::select_all;
use webbluetooth::{Bluetooth, BluetoothUuid, Result};

fn main() -> Result<()> {
    let services: Vec<String> = std::env::args().skip(1).collect();
    if services.is_empty() {
        eprintln!("usage: sensors <service>... \n");
        eprintln!("  a name       battery_service, heart_rate, device_information");
        eprintln!("  a number     180f, 0x180F");
        eprintln!("  a UUID       0000fe8d-0000-1000-8000-00805f9b34fb");
        eprintln!("\nWeb Bluetooth grants access to the services you name and");
        eprintln!("nothing else, so there is no way to ask for all of them.");
        std::process::exit(2);
    }
    block_on(run(services))
}

async fn run(services: Vec<String>) -> Result<()> {
    // `FirstMatch` would take whatever advertises first, which is rarely the
    // device you meant when the filter is this wide.
    #[cfg(feature = "terminal-chooser")]
    let bluetooth = Bluetooth::with_chooser(webbluetooth::chooser::TerminalChooser::default());
    #[cfg(not(feature = "terminal-chooser"))]
    let bluetooth = Bluetooth::with_chooser(webbluetooth::chooser::StrongestSignal::default());

    if let Err(why) = bluetooth.availability().await {
        eprintln!("Bluetooth unavailable: {why} — try the `doctor` example.");
        std::process::exit(1);
    }

    // Accepting all devices and granting the named services is the widest
    // request that still means something: a device that has these services
    // often does not advertise them, so filtering on them would miss it.
    let mut options = RequestDeviceOptions::new().accept_all_devices();
    for service in &services {
        options = options.optional_service(service.as_str())?;
    }

    println!("choosing a device…");
    let device = bluetooth.request_device(options).await?;
    println!(
        "connecting to {}",
        device.name().unwrap_or_else(|| device.id().into())
    );

    // `connect()` waits for the peripheral to come into range for as long as
    // that takes, which is right for a library and wrong for a program a
    // person is watching: most of what a wide scan turns up advertises
    // happily and accepts no connections at all, so without a deadline this
    // sits in silence forever. The first run of this example did.
    let gatt = device.gatt();
    match webbluetooth::timeout(Duration::from_secs(15), gatt.connect()).await {
        Ok(Ok(())) => {}
        Ok(Err(why)) => {
            eprintln!("could not connect: {why}");
            std::process::exit(1);
        }
        Err(()) => {
            eprintln!("connect timed out after 15 s.");
            eprintln!("Plenty of devices advertise without accepting connections —");
            eprintln!("try --features terminal-chooser to pick a different one.");
            std::process::exit(1);
        }
    }

    // ── Subscribe to everything that will talk ──────────────────────────────
    let mut subscriptions = Vec::new();
    let mut names: BTreeMap<BluetoothUuid, String> = BTreeMap::new();

    for service in &services {
        let uuid = BluetoothUuid::parse(service)?;
        let service = match gatt.get_primary_service(uuid).await {
            Ok(service) => service,
            // Not every device has every service asked for, and a missing one
            // is worth a line rather than the end of the run.
            Err(why) => {
                println!("  {uuid}  not on this device ({why})");
                continue;
            }
        };

        let characteristics = match service.get_characteristics(None).await {
            Ok(characteristics) => characteristics,
            Err(why) => {
                println!("  {uuid}  no readable characteristics ({why})");
                continue;
            }
        };

        for characteristic in characteristics {
            let properties = characteristic.properties();
            if !properties.notify() && !properties.indicate() {
                continue;
            }
            let uuid = *characteristic.uuid();
            match characteristic.start_notifications().await {
                Ok(notifications) => {
                    println!(
                        "  {uuid}  subscribed ({})",
                        if properties.notify() {
                            "notify"
                        } else {
                            "indicate"
                        }
                    );
                    names.insert(uuid, short(&uuid));
                    subscriptions.push(notifications.tagged());
                }
                Err(why) => println!("  {uuid}  would not subscribe ({why})"),
            }
        }
    }

    if subscriptions.is_empty() {
        println!("\nNothing here notifies. Nothing to listen to.");
        return Ok(());
    }

    // ── One loop over all of them ───────────────────────────────────────────
    //
    // `tagged()` is what keeps the values distinguishable once merged: without
    // it every notification arrives as a bare `Vec<u8>` with no way back to
    // the characteristic that sent it.
    println!("\n{} subscription(s) — ^C to stop\n", subscriptions.len());
    let mut values = select_all(subscriptions);
    let mut disconnected = device.watch_disconnect();
    let mut counts: BTreeMap<BluetoothUuid, u64> = BTreeMap::new();

    loop {
        let next_value = std::pin::pin!(values.next());
        let next_drop = std::pin::pin!(disconnected.next());
        match select(next_value, next_drop).await {
            Either::Left((Some((uuid, value)), _)) => {
                let count = counts.entry(uuid).or_default();
                *count += 1;
                // Every packet from a 256 Hz sensor would be unreadable; the
                // first few of each, then a heartbeat, says what it is doing.
                if *count <= 3 || count.is_multiple_of(100) {
                    println!(
                        "{:>8}  #{count:<6} {} bytes  {}",
                        names.get(&uuid).map(String::as_str).unwrap_or("?"),
                        value.len(),
                        preview(&value),
                    );
                }
            }
            Either::Left((None, _)) => break,
            Either::Right(_) => {
                println!("\ndevice disconnected");
                break;
            }
        }
    }

    // ── What each subscription actually delivered ───────────────────────────
    //
    // `lost()` is per subscription, and `Tagged` keeps it reachable through
    // the merge — so a gap can be attributed rather than merely noticed.
    println!("\n{:>8}  {:>8}  {:>6}", "", "received", "lost");
    for subscription in values.iter() {
        let uuid = subscription.uuid();
        println!(
            "{:>8}  {:>8}  {:>6}",
            short(uuid),
            counts.get(uuid).copied().unwrap_or(0),
            subscription.lost(),
        );
    }
    Ok(())
}

/// The distinguishing part of a UUID: its assigned number where it has one,
/// and the first group otherwise.
fn short(uuid: &BluetoothUuid) -> String {
    match uuid.as_u16() {
        Some(assigned) => format!("{assigned:04x}"),
        None => uuid.as_str()[..8].to_owned(),
    }
}

/// The first few bytes, which is usually enough to tell whether a decoder is
/// looking at what it thinks it is.
fn preview(value: &[u8]) -> String {
    let shown = value.len().min(8);
    let mut out = String::new();
    for byte in &value[..shown] {
        out.push_str(&format!("{byte:02x} "));
    }
    if value.len() > shown {
        out.push('…');
    }
    out
}
