//! Tests that need a real message bus.
//!
//! Skipped unless `WEBBLUETOOTH_DBUS_ADDRESS` or `DBUS_SESSION_BUS_ADDRESS`
//! names one, so `cargo test` on a machine with no D-Bus still passes. The
//! Docker harness in `docker/` provides both a bus and a mocked `org.bluez`.

#![cfg(unix)]

use std::time::Duration;
use webbluetooth_linux::dbus::connection::Error;
use webbluetooth_linux::dbus::{Connection, Message, Value};

fn bus() -> Option<Connection> {
    let address = std::env::var("WEBBLUETOOTH_DBUS_ADDRESS")
        .or_else(|_| std::env::var("DBUS_SESSION_BUS_ADDRESS"))
        .ok()?;
    Some(Connection::connect(&address).expect("could not connect to the bus"))
}

#[test]
fn authenticates_and_gets_a_unique_name() {
    let Some(connection) = bus() else { return };
    // `Hello` runs during connect; a unique name proves SASL, framing and
    // reply dispatch all worked.
    let name = connection.unique_name();
    assert!(
        name.starts_with(':'),
        "expected a unique name like :1.7, got {name:?}"
    );
    assert!(connection.is_connected());
}

#[test]
fn calls_a_method_and_decodes_the_reply() {
    let Some(connection) = bus() else { return };
    let reply = connection
        .call_blocking(
            Message::call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "ListNames",
            ),
            Duration::from_secs(5),
        )
        .expect("ListNames failed");

    let names = reply.arg(0).expect("no reply body").as_strings();
    assert!(
        names.iter().any(|n| n == "org.freedesktop.DBus"),
        "the bus did not list itself: {names:?}"
    );
    assert!(names.iter().any(|n| *n == connection.unique_name()));
}

#[test]
fn a_failing_call_returns_the_bus_error() {
    let Some(connection) = bus() else { return };
    let err = connection
        .call_blocking(
            Message::call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NoSuchMethod",
            ),
            Duration::from_secs(5),
        )
        .expect_err("a bogus method should fail");
    // The point is that an error reply becomes an Err, not a hang.
    assert!(
        matches!(err, Error::Call { .. }),
        "expected a Call error, got {err:?}"
    );
}

#[test]
fn a_reply_body_decodes() {
    let Some(connection) = bus() else { return };
    let reply = connection
        .call_blocking(
            Message::call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "GetId",
            ),
            Duration::from_secs(5),
        )
        .expect("GetId failed");
    let id = reply.arg(0).and_then(Value::as_str).unwrap_or_default();
    assert_eq!(
        id.len(),
        32,
        "the bus id should be a 32-char hex guid, got {id:?}"
    );
}

#[test]
fn signals_are_delivered_to_handlers() {
    use std::sync::mpsc;
    let Some(connection) = bus() else { return };

    let (tx, rx) = mpsc::channel();
    connection
        .add_match(
            "type='signal',interface='org.freedesktop.DBus',member='NameOwnerChanged'",
            std::sync::Arc::new(move |m: &Message| {
                if m.member.as_deref() == Some("NameOwnerChanged") {
                    let _ = tx.send(
                        m.arg(0)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    );
                }
            }),
        )
        .expect("AddMatch failed");

    // Claiming a name makes the bus emit NameOwnerChanged.
    connection
        .request_name("dev.webbluetooth.SignalProbe")
        .expect("RequestName failed");

    // The bus emits NameOwnerChanged for *every* name, including this
    // connection acquiring its own unique `:1.x` when it connected — so read
    // until the one we asked for turns up rather than asserting on the first.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "dev.webbluetooth.SignalProbe never arrived"
        );
        match rx.recv_timeout(remaining) {
            Ok(name) if name == "dev.webbluetooth.SignalProbe" => break,
            Ok(_) => continue,
            Err(_) => panic!("no signal arrived"),
        }
    }
}

#[test]
fn an_exported_object_answers_inbound_calls() {
    let Some(connection) = bus() else { return };
    // The peripheral role depends on BlueZ calling *back* into this process, so
    // the server side has to work as well as the client side.
    connection.export(
        "/dev/webbluetooth/Probe",
        std::sync::Arc::new(|call: &Message| match call.member.as_deref() {
            Some("Echo") => Ok(vec![call
                .arg(0)
                .cloned()
                .unwrap_or(Value::Str(String::new()))]),
            _ => Err((
                "org.freedesktop.DBus.Error.UnknownMethod".into(),
                "no".into(),
            )),
        }),
    );

    let reply = connection
        .call_blocking(
            Message::call(
                &connection.unique_name(),
                "/dev/webbluetooth/Probe",
                "dev.webbluetooth.Probe",
                "Echo",
            )
            .with_body(vec![Value::Str("round trip".into())]),
            Duration::from_secs(5),
        )
        .expect("calling our own exported object failed");

    assert_eq!(reply.arg(0).and_then(Value::as_str), Some("round trip"));
}
