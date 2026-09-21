//! One process, both roles at once.
//!
//! `CBCentralManager` and `CBPeripheralManager` are independent objects, so a
//! process can scan and advertise simultaneously. This crate gives each its own
//! runtime-synthesised delegate class, its own delegate instance, its own sink
//! registry and its own private dispatch queue, so the two never touch.
//!
//! These tests need no Bluetooth grant: they assert the managers coexist and
//! both deliver their first state callback, whatever that state turns out to
//! be. On a machine where Bluetooth is denied, both report `Unauthorized` — and
//! that still proves the delegate plumbing runs side by side.

use futures_executor::block_on;
use webbluetooth::Bluetooth;

#[cfg(peripheral_role)]
use webbluetooth::Peripheral;

#[test]
fn a_central_and_a_peripheral_coexist_in_one_process() {
    #[cfg(peripheral_role)]
    {
        let bluetooth = Bluetooth::new();
        let (peripheral, _requests) = Peripheral::new();

        // Both managers must report a settled state. A shared registry or a
        // shared queue would show up here as one of them never being called.
        let central_state = block_on(bluetooth.availability());
        let peripheral_state = block_on(peripheral.availability());

        assert!(
            !matches!(central_state, Err(webbluetooth::Availability::Unknown)),
            "the central never reported a state"
        );
        assert!(
            !matches!(peripheral_state, Err(webbluetooth::Availability::Unknown)),
            "the peripheral never reported a state"
        );
        // Same radio, so the two roles must agree about it.
        assert_eq!(
            central_state.is_ok(),
            peripheral_state.is_ok(),
            "the two roles disagree about the adapter: {central_state:?} vs {peripheral_state:?}"
        );
    }
}

#[test]
fn many_managers_of_both_kinds_coexist() {
    // The sinks are keyed by delegate object address, so N instances of each
    // must all be served. This is what would break if the registry were keyed
    // by class instead.
    let centrals: Vec<Bluetooth> = (0..3).map(|_| Bluetooth::new()).collect();
    for (i, b) in centrals.iter().enumerate() {
        assert!(
            !matches!(
                block_on(b.availability()),
                Err(webbluetooth::Availability::Unknown)
            ),
            "central {i} never reported a state"
        );
    }

    #[cfg(peripheral_role)]
    {
        let peripherals: Vec<_> = (0..3).map(|_| Peripheral::new()).collect();
        for (i, (p, _rx)) in peripherals.iter().enumerate() {
            assert!(
                !matches!(
                    block_on(p.availability()),
                    Err(webbluetooth::Availability::Unknown)
                ),
                "peripheral {i} never reported a state"
            );
        }
    }
}

/// The availability stream owns a polling thread, so dropping it has to stop
/// that thread rather than leave it running for the life of the process.
#[test]
fn dropping_the_availability_stream_stops_its_poller() {
    use futures_util::StreamExt;

    let bluetooth = webbluetooth::Bluetooth::new();
    let mut events = bluetooth.watch_availability();

    // The first value is the state at the time of the call, so it arrives
    // without waiting for anything to change.
    let first = futures_executor::block_on(events.next());
    assert!(
        first.is_some(),
        "the stream should report the current state"
    );

    drop(events);
    // Nothing to assert about the thread directly; what matters is that this
    // returns rather than hanging, and that the process can exit.
}

#[test]
fn the_peripheral_role_matches_the_platform_constant() {
    // `PERIPHERAL_ROLE` is what cross-platform code branches on, so it must
    // agree with whether the module is actually there. A const block makes the
    // disagreement a compile error rather than a test failure.
    #[cfg(peripheral_role)]
    const {
        assert!(webbluetooth::PERIPHERAL_ROLE)
    };
    #[cfg(not(peripheral_role))]
    const {
        assert!(!webbluetooth::PERIPHERAL_ROLE)
    };
}
