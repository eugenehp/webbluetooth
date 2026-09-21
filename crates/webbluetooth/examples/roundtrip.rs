//! Exercise a real GATT server over a real radio, and report what worked.
//!
//! ```sh
//! ./scripts/ios.sh ios-harness peripheral   # on one machine
//! cargo run -p webbluetooth --example roundtrip
//! ```
//!
//! The peer is the `ios-harness` peripheral: a Battery Service whose level is
//! readable and notifying, plus a writable control point. Everything here is
//! the central half of that conversation, so a run covers read, write, notify
//! and the blocklist against hardware rather than a mock.
//!
//! The peer is selected by the service it advertises, not by name, so the same
//! run works against the iOS harness and against `scripts/qemu/peer.py`.

use futures_executor::block_on;
use std::time::Duration;
use webbluetooth::prelude::*;
use webbluetooth::uuid::services;
use webbluetooth::{Bluetooth, DeviceFilter, RequestDeviceOptions, Result};

/// The harness's private service and its two characteristics.
const SERVICE: &str = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";
const LEVEL: &str = "6e400003-b5a3-f393-e0a9-e50e24dcca9e";
const CONTROL: &str = "6e400002-b5a3-f393-e0a9-e50e24dcca9e";
/// Where the peer publishes the PSM of its L2CAP channel. Read only where
/// there are channels to open, which is everywhere except Windows and the web.
#[cfg(l2cap)]
const PSM_CHAR: &str = "6e400004-b5a3-f393-e0a9-e50e24dcca9e";
/// Serial Number String, which the Web Bluetooth blocklist forbids.
const SERIAL: &str = "00002a25-0000-1000-8000-00805f9b34fb";

fn main() -> Result<()> {
    block_on(run())
}

/// Count of failures, so the exit code means something to a script.
static mut FAILED: u32 = 0;

fn check(label: &str, outcome: std::result::Result<String, String>) {
    match outcome {
        Ok(detail) => println!("  \u{2713} {label:<22} {detail}"),
        Err(why) => {
            println!("  \u{2717} {label:<22} {why}");
            unsafe { FAILED += 1 };
        }
    }
}

async fn run() -> Result<()> {
    let bluetooth = Bluetooth::with_chooser(webbluetooth::chooser::StrongestSignal::default());
    if let Err(why) = bluetooth.availability().await {
        eprintln!("Bluetooth unavailable: {why} — try the `doctor` example.");
        std::process::exit(1);
    }

    println!("looking for a peer advertising {SERVICE}…");
    // Filter on the service rather than the name: it is what makes this peer
    // the right one, and it grants access to it in the same breath.
    let device = bluetooth
        .request_device(
            RequestDeviceOptions::new()
                .filter(DeviceFilter::new().service(SERVICE)?)
                .optional_service(services::DEVICE_INFORMATION)?,
        )
        .await?;

    let gatt = device.gatt();
    gatt.connect().await?;
    println!(
        "connected to {}\n",
        device.name().unwrap_or_else(|| device.id().into())
    );

    let fixture = gatt.get_primary_service(SERVICE).await?;
    let level = fixture.get_characteristic(LEVEL).await?;

    // ── read ────────────────────────────────────────────────────────────────
    check(
        "read",
        match level.read_value().await {
            Ok(v) if v.len() == 1 => Ok(format!("level {}", v[0])),
            Ok(v) => Err(format!("expected one byte, got {v:?}")),
            Err(e) => Err(e.to_string()),
        },
    );

    // ── notify ──────────────────────────────────────────────────────────────
    // The harness pushes a new level every five seconds, so two ticks prove
    // the subscription is live rather than replaying the read above.
    check(
        "notify",
        match level.start_notifications().await {
            Err(e) => Err(e.to_string()),
            Ok(mut ticks) => {
                let mut seen = Vec::new();
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                while seen.len() < 2 && std::time::Instant::now() < deadline {
                    match ticks.next().await {
                        Some(v) => seen.push(v.first().copied().unwrap_or_default()),
                        None => break,
                    }
                }
                if seen.len() >= 2 {
                    Ok(format!("{seen:?}"))
                } else {
                    Err(format!("only {} tick(s) in 20s", seen.len()))
                }
            }
        },
    );

    // ── write ───────────────────────────────────────────────────────────────
    check(
        "write",
        match fixture.get_characteristic(CONTROL).await {
            Err(e) => Err(format!("no control point: {e}")),
            Ok(control) => match control.write_value_with_response(&[0x2a]).await {
                Ok(()) => Ok("control point accepted 0x2a".into()),
                Err(e) => Err(e.to_string()),
            },
        },
    );

    // ── blocklist ───────────────────────────────────────────────────────────
    // Serial Number String is on the Web Bluetooth blocklist, and the peer here
    // is an iPad, which really does publish one. Being refused is the pass.
    check(
        "blocklist",
        match gatt.get_primary_service(services::DEVICE_INFORMATION).await {
            Err(e) => Err(format!("no Device Information: {e}")),
            Ok(info) => match info.get_characteristic(SERIAL).await {
                Err(webbluetooth::Error::Security(_)) => Ok("serial number refused".into()),
                Err(e) => Err(format!("refused, but as {e} rather than a SecurityError")),
                Ok(_) => Err("serial number was handed over".into()),
            },
        },
    );

    // ── included services ───────────────────────────────────────────────────
    // An Include declaration lets a composite service reuse another rather
    // than restate it. Most peers declare none, so "none" is reported rather
    // than failed — what is being checked is that discovery answers cleanly
    // either way, and that the allowlist still applies to what it finds.
    check(
        "included",
        match fixture.get_included_services(None).await {
            Ok(included) => Ok(included
                .iter()
                .map(|s| s.uuid().as_str().to_owned())
                .collect::<Vec<_>>()
                .join(", ")),
            Err(webbluetooth::Error::NotFound(_)) => Ok("none declared by this peer".into()),
            Err(e) => Err(e.to_string()),
        },
    );

    // ── L2CAP ───────────────────────────────────────────────────────────────
    // The one part of this crate that was never exercised over a real radio.
    // Absent on Windows and the web, which have no channel API at all.
    #[cfg(l2cap)]
    check("l2cap", l2cap_echo(&device, &fixture).await);
    #[cfg(not(l2cap))]
    check("l2cap", Ok("no channel API on this platform".into()));

    gatt.disconnect();
    let failed = unsafe { FAILED };
    println!();
    if failed == 0 {
        println!("all checks passed");
        Ok(())
    } else {
        println!("{failed} check(s) failed");
        std::process::exit(1)
    }
}

/// Open a channel to the PSM the peer published, send bytes, and get them back.
///
/// A PSM is not discoverable — nothing in GATT advertises one — so the peer
/// puts the number in a characteristic and this reads it first. A peer that
/// publishes none is reported rather than failed: the harness does, most
/// devices do not.
#[cfg(l2cap)]
async fn l2cap_echo(
    device: &webbluetooth::BluetoothDevice,
    fixture: &webbluetooth::RemoteGattService,
) -> std::result::Result<String, String> {
    let published = match fixture.get_characteristic(PSM_CHAR).await {
        Ok(c) => c,
        Err(webbluetooth::Error::NotFound(_)) => return Ok("this peer publishes no PSM".into()),
        Err(e) => return Err(e.to_string()),
    };

    let bytes = published.read_value().await.map_err(|e| e.to_string())?;
    let [lo, hi] = bytes[..] else {
        return Err(format!("the PSM characteristic held {bytes:02x?}"));
    };
    let psm = u16::from_le_bytes([lo, hi]);

    let channel = device
        .open_l2cap_channel(psm)
        .await
        .map_err(|e| format!("psm {psm:#06x}: {e}"))?;
    let mut incoming = channel
        .take_incoming()
        .ok_or("the channel arrived with no inbound stream")?;

    const SENT: &[u8] = b"webbluetooth";
    channel.send(SENT).map_err(|e| e.to_string())?;
    // `send` returns once queued; this waits for it to reach the air.
    channel.flushed().await.map_err(|e| e.to_string())?;

    // A chunk is a read boundary, not a message boundary, so a short message
    // could in principle arrive split. Collect until it matches or time runs
    // out rather than assuming one chunk.
    let mut got = Vec::new();
    let deadline = webbluetooth::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = incoming.next().await {
            got.extend_from_slice(&chunk);
            if got.len() >= SENT.len() {
                break;
            }
        }
    })
    .await;

    match deadline {
        Err(()) => Err(format!("psm {psm:#06x}: nothing echoed within 5s")),
        Ok(()) if got == SENT => Ok(format!("psm {psm:#06x}, {} bytes echoed", got.len())),
        Ok(()) if got.is_empty() => Err(format!("psm {psm:#06x}: the channel closed unanswered")),
        Ok(()) => Err(format!("psm {psm:#06x}: echoed {got:02x?}")),
    }
}
