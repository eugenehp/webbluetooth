//! The engine, against a real radio.
//!
//! Everything else in this crate is checked without hardware. This is the part
//! that cannot be: whether an advertisement actually arrives, whether a grant of
//! every service actually returns the services, whether a characteristic
//! actually reads. A mock would only re-assert what the unit tests already pin.
//!
//! Off by default, because it needs a radio, something in range to talk to, and
//! several seconds:
//!
//! ```sh
//! WEBBLUETOOTH_EXPLORER_RADIO_TEST=1 cargo test -p webbluetooth-explorer --test radio -- --nocapture
//! ```
//!
//! On macOS run it from Terminal.app. The Bluetooth prompt is attributed to the
//! responsible process, so from an editor or a build agent it is denied silently
//! and this reports no adapter.

use std::time::{Duration, Instant};
use webbluetooth_explorer::engine::{self, Command, Event, Handle, Level};

fn enabled() -> bool {
    std::env::var("WEBBLUETOOTH_EXPLORER_RADIO_TEST").is_ok()
}

/// Pump the engine until `done` says so, or `budget` runs out.
///
/// The engine wakes an `egui::Context` rather than a condvar, so there is
/// nothing to block on here — which is exactly how the UI consumes it too.
fn pump(
    engine: &mut Handle,
    budget: Duration,
    mut done: impl FnMut(&[Event]) -> bool,
) -> Vec<Event> {
    let deadline = Instant::now() + budget;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        seen.extend(engine.drain());
        if done(&seen) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    seen
}

fn report(events: &[Event]) {
    for event in events {
        if let Event::Log(level, text) = event {
            let mark = if *level == Level::Error { "!" } else { " " };
            println!("  {mark} {text}");
        }
    }
}

/// A scan reports something, and reports it repeatedly.
///
/// `keep_repeated_devices` is what makes the RSSI column live rather than
/// frozen at first sighting, and it is set in the engine rather than by the
/// caller — so this is the only place it gets checked.
#[test]
fn a_scan_reports_advertisements() {
    if !enabled() {
        return;
    }
    let mut engine = engine::start(egui::Context::default());
    engine.send(Command::SetScanning(true));

    // Stop on a *repeat*, not on a count. Stopping at "five sightings" proves
    // nothing about duplicates: in any populated room five different devices
    // report inside a second, so the run ends before a second packet from any
    // one of them could have arrived, and the assertion below passes or fails
    // on how busy the room is.
    let events = pump(&mut engine, Duration::from_secs(20), |seen| {
        let mut ids: Vec<&str> = seen
            .iter()
            .filter_map(|e| match e {
                Event::Sighting { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        total > ids.len()
    });
    report(&events);

    for event in &events {
        if let Event::Availability(Err(why)) = event {
            panic!("no usable adapter: {why}");
        }
    }
    assert!(
        events.iter().any(|e| matches!(e, Event::Scanning(true))),
        "the scan never started"
    );

    let mut ids: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Sighting { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let total = ids.len();
    assert!(
        total > 0,
        "nothing advertised in 15 seconds — is anything on?"
    );
    ids.sort_unstable();
    ids.dedup();
    println!("  {total} sightings from {} devices", ids.len());
    assert!(
        total > ids.len(),
        "every device reported exactly once — keep_repeated_devices is not \
         taking effect, and the signal column will not update"
    );

    engine.send(Command::SetScanning(false));
    let stopped = pump(&mut engine, Duration::from_secs(3), |seen| {
        seen.iter().any(|e| matches!(e, Event::Scanning(false)))
    });
    assert!(
        stopped.iter().any(|e| matches!(e, Event::Scanning(false))),
        "the scan never stopped"
    );
}

/// The whole point of the `unrestricted` grant: connect to something nobody
/// declared an interest in, and get its services back anyway.
///
/// Connects to the strongest connectable advertiser, which in a normal room is
/// something that will accept one. A device that refuses is not a failure of
/// this program, so a refusal is reported and skipped rather than failed —
/// but discovery returning *nothing* on a device that did connect would mean
/// the grant is not working, and that is a failure.
#[test]
fn connecting_discovers_services_nobody_named() {
    if !enabled() {
        return;
    }
    let mut engine = engine::start(egui::Context::default());
    engine.send(Command::SetScanning(true));

    // Collect for a window, then take the strongest: the nearest device is the
    // one most likely to complete a connection.
    let events = pump(&mut engine, Duration::from_secs(10), |_| false);
    report(&events);

    let mut best: Option<(i32, String)> = None;
    for event in &events {
        if let Event::Sighting {
            id,
            rssi: Some(dbm),
            adv,
            ..
        } = event
        {
            if adv.connectable == Some(false) {
                continue;
            }
            if best.as_ref().is_none_or(|(seen, _)| dbm > seen) {
                best = Some((*dbm, id.clone()));
            }
        }
    }
    let Some((dbm, id)) = best else {
        eprintln!("nothing connectable in range; skipping");
        return;
    };
    println!("  connecting to {id} at {dbm} dBm");

    engine.send(Command::SetScanning(false));
    engine.send(Command::Connect(id.clone()));

    let events = pump(&mut engine, Duration::from_secs(25), |seen| {
        seen.iter()
            .any(|e| matches!(e, Event::Tree { .. } | Event::Disconnected { .. }))
    });
    report(&events);

    let tree = events.iter().find_map(|e| match e {
        Event::Tree { services, .. } => Some(services),
        _ => None,
    });
    let Some(services) = tree else {
        eprintln!("{id} would not stay connected; skipping");
        return;
    };

    assert!(
        !services.is_empty(),
        "connected but discovered nothing — a grant of every service returned \
         an empty set, which is what `unrestricted` exists to prevent"
    );
    for service in services {
        println!(
            "  {} — {} characteristics",
            service.uuid,
            service.characteristics.len()
        );
    }

    // Read the first readable characteristic there is. Whatever it holds, the
    // bytes coming back prove the tree's handles are live.
    let readable = services.iter().enumerate().find_map(|(si, service)| {
        service
            .characteristics
            .iter()
            .position(|c| c.properties.read())
            .map(|ci| engine::CharRef {
                device: id.clone(),
                service: si,
                characteristic: ci,
            })
    });
    if let Some(at) = readable {
        engine.send(Command::Read(at));
        let events = pump(&mut engine, Duration::from_secs(10), |seen| {
            seen.iter().any(|e| matches!(e, Event::Value { .. }))
        });
        report(&events);
        let value = events.iter().find_map(|e| match e {
            Event::Value { value, .. } => Some(value),
            _ => None,
        });
        match value {
            Some(bytes) => println!("  read {} bytes", bytes.len()),
            // A readable characteristic can still refuse without pairing, which
            // is the peripheral's decision and not this program's bug.
            None => eprintln!("  the read did not answer; not failing on that"),
        }
    }

    // The link operations taken from nRF Connect. Each is a separate platform
    // call and any of them may simply not be available — CoreBluetooth has no
    // PHY API at all, for one — so an absent answer is reported, not failed.
    // What is asserted is that asking does not break the link.
    engine.send(Command::ReadLinkDetail(id.clone()));
    let events = pump(&mut engine, Duration::from_secs(8), |seen| {
        seen.iter().any(|e| matches!(e, Event::LinkDetail { .. }))
    });
    report(&events);
    match events.iter().find_map(|e| match e {
        Event::LinkDetail { detail, .. } => Some(detail),
        _ => None,
    }) {
        Some(detail) => {
            println!("  link: {detail:?}");
            assert!(
                detail.mtu.is_some(),
                "every platform reports an ATT MTU on a live link"
            );
        }
        None => eprintln!("  no link detail came back; not failing on that"),
    }

    engine.send(Command::RequestMtu(id.clone(), 247));
    let events = pump(&mut engine, Duration::from_secs(8), |seen| {
        seen.iter().any(|e| matches!(e, Event::LinkDetail { .. }))
    });
    report(&events);

    engine.send(Command::Disconnect(id.clone()));
    let events = pump(&mut engine, Duration::from_secs(3), |seen| {
        seen.iter().any(|e| matches!(e, Event::Disconnected { .. }))
    });
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Disconnected { .. })),
        "the link never reported going down, so the operations above left it \
         in a state this program cannot get out of"
    );
}
