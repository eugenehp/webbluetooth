//! Events the page pushes: notifications, disconnections, advertisements.
//!
//! The request side of the boundary is a question with an answer. This is the
//! other half — things the browser reports because a listener was registered,
//! with no request outstanding to attach them to.
//!
//! The shim calls [`wbt_event`] and it is routed to whatever the backend
//! installed. A single sink rather than one per kind, because the backend
//! wants them in order: a characteristic value that arrives after a
//! disconnection must not overtake it.

use crate::codec::Reader;
use crate::host::EventKind;
use std::cell::RefCell;

/// A decoded event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A characteristic notified or indicated a new value.
    CharacteristicValue {
        device: String,
        service: String,
        characteristic: String,
        value: Vec<u8>,
    },
    /// The link dropped.
    Disconnected { device: String },
    /// The device's service set changed and every handle into it is stale.
    ServiceChanged { device: String },
    /// An advertisement from a device being watched.
    Advertisement {
        device: String,
        name: Option<String>,
        rssi: i32,
        tx_power: Option<i32>,
        appearance: Option<u32>,
        uuids: Vec<String>,
        manufacturer_data: Vec<(u16, Vec<u8>)>,
        service_data: Vec<(String, Vec<u8>)>,
    },
    /// `navigator.bluetooth`'s availability changed.
    AvailabilityChanged { available: bool },
}

/// Where a decoded event is delivered.
type Sink = Box<dyn Fn(Event)>;

thread_local! {
    /// Where decoded events go. `None` until a backend installs a sink.
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Route events to `sink` from here on, replacing any previous one.
pub fn set_sink(sink: impl Fn(Event) + 'static) {
    SINK.with(|s| *s.borrow_mut() = Some(Box::new(sink)));
}

/// Stop routing events.
pub fn clear_sink() {
    SINK.with(|s| *s.borrow_mut() = None);
}

fn deliver(event: Event) {
    // The sink is taken out for the call: it may start a request, which can
    // settle synchronously and deliver another event, which would otherwise
    // borrow this again.
    let sink = SINK.with(|s| s.borrow_mut().take());
    if let Some(sink) = sink {
        sink(event);
        SINK.with(|s| {
            let mut slot = s.borrow_mut();
            // Only put it back if nothing replaced it while it was out.
            if slot.is_none() {
                *slot = Some(sink);
            }
        });
    }
}

/// Decode one event. Separate from [`wbt_event`] so it can be tested without
/// raw pointers.
pub fn decode(kind: u32, body: &[u8]) -> Option<Event> {
    let mut r = Reader::new(body);
    let event = match EventKind::from_u32(kind)? {
        EventKind::CharacteristicValue => Event::CharacteristicValue {
            device: r.str()?,
            service: r.str()?,
            characteristic: r.str()?,
            value: r.bytes()?,
        },
        EventKind::GattServerDisconnected => Event::Disconnected { device: r.str()? },
        EventKind::ServiceChanged => Event::ServiceChanged { device: r.str()? },
        EventKind::AdvertisementReceived => Event::Advertisement {
            device: r.str()?,
            name: r.option_str()?,
            rssi: r.i32()?,
            tx_power: r.bool()?.then(|| r.i32()).flatten(),
            appearance: r.bool()?.then(|| r.u32()).flatten(),
            uuids: r.list(|r| r.str())?,
            manufacturer_data: r.list(|r| Some((r.u16()?, r.bytes()?)))?,
            service_data: r.list(|r| Some((r.str()?, r.bytes()?)))?,
        },
        EventKind::AvailabilityChanged => Event::AvailabilityChanged {
            available: r.bool()?,
        },
    };
    // Trailing bytes mean the two sides disagree about the shape, which is a
    // bug to surface rather than something to read half of.
    r.is_empty().then_some(event)
}

/// Deliver an event from the host.
///
/// # Safety
/// `ptr` must be valid for `len` bytes for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn wbt_event(kind: u32, ptr: *const u8, len: usize) {
    let body = if ptr.is_null() || len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len) }
    };
    // A malformed event is dropped. The alternative is trapping, which takes
    // the whole page down over one bad message.
    if let Some(event) = decode(kind, body) {
        deliver(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Writer;
    use std::cell::RefCell as Cell2;
    use std::rc::Rc;

    #[test]
    fn a_notification_round_trips() {
        let mut w = Writer::new();
        w.str("dev-1")
            .str("0000180f-0000-1000-8000-00805f9b34fb")
            .str("00002a19-0000-1000-8000-00805f9b34fb")
            .bytes(&[0x63]);

        assert_eq!(
            decode(EventKind::CharacteristicValue as u32, &w.finish()),
            Some(Event::CharacteristicValue {
                device: "dev-1".into(),
                service: "0000180f-0000-1000-8000-00805f9b34fb".into(),
                characteristic: "00002a19-0000-1000-8000-00805f9b34fb".into(),
                value: vec![0x63],
            })
        );
    }

    #[test]
    fn an_advertisement_round_trips_with_its_optional_fields() {
        let mut w = Writer::new();
        w.str("dev-2")
            .option_str(Some("Thermometer"))
            .u32(-55i32 as u32)
            .bool(true)
            .u32(-4i32 as u32)
            .bool(false)
            .list(&["0000180d-0000-1000-8000-00805f9b34fb"], |w, s| {
                w.str(s);
            })
            .list(&[(0x004Cu16, vec![9u8])], |w, (c, d)| {
                w.u16(*c).bytes(d);
            })
            .list(
                &[("0000180f-0000-1000-8000-00805f9b34fb", vec![0x63u8])],
                |w, (u, d)| {
                    w.str(u).bytes(d);
                },
            );

        let got = decode(EventKind::AdvertisementReceived as u32, &w.finish());
        assert_eq!(
            got,
            Some(Event::Advertisement {
                device: "dev-2".into(),
                name: Some("Thermometer".into()),
                rssi: -55,
                tx_power: Some(-4),
                appearance: None,
                uuids: vec!["0000180d-0000-1000-8000-00805f9b34fb".into()],
                manufacturer_data: vec![(0x004C, vec![9])],
                service_data: vec![("0000180f-0000-1000-8000-00805f9b34fb".into(), vec![0x63])],
            })
        );
    }

    #[test]
    fn the_simple_events_round_trip() {
        let mut w = Writer::new();
        w.str("dev-3");
        let body = w.finish();
        assert_eq!(
            decode(EventKind::GattServerDisconnected as u32, &body),
            Some(Event::Disconnected {
                device: "dev-3".into()
            })
        );
        assert_eq!(
            decode(EventKind::ServiceChanged as u32, &body),
            Some(Event::ServiceChanged {
                device: "dev-3".into()
            })
        );

        let mut w = Writer::new();
        w.bool(true);
        assert_eq!(
            decode(EventKind::AvailabilityChanged as u32, &w.finish()),
            Some(Event::AvailabilityChanged { available: true })
        );
    }

    /// The bytes come from JavaScript. A malformed event must be dropped, not
    /// read past the end of and not trapped on.
    #[test]
    fn a_malformed_event_is_rejected() {
        assert_eq!(decode(EventKind::GattServerDisconnected as u32, &[]), None);
        // Truncated mid-string.
        assert_eq!(
            decode(EventKind::CharacteristicValue as u32, &[5, 0, 0, 0, b'a']),
            None
        );
        // Unknown kind.
        assert_eq!(decode(999, &[]), None);
    }

    /// Trailing bytes mean the two sides disagree about the layout; reading
    /// the prefix and carrying on would hide that.
    #[test]
    fn an_event_with_bytes_left_over_is_rejected() {
        let mut w = Writer::new();
        w.str("dev-4").u32(0xDEAD);
        assert_eq!(
            decode(EventKind::GattServerDisconnected as u32, &w.finish()),
            None
        );
    }

    #[test]
    fn events_reach_the_installed_sink() {
        let seen = Rc::new(Cell2::new(Vec::new()));
        let out = seen.clone();
        set_sink(move |e| out.borrow_mut().push(e));

        let mut w = Writer::new();
        w.str("dev-5");
        let body = w.finish();
        unsafe {
            wbt_event(
                EventKind::GattServerDisconnected as u32,
                body.as_ptr(),
                body.len(),
            )
        };

        assert_eq!(
            seen.borrow().as_slice(),
            &[Event::Disconnected {
                device: "dev-5".into()
            }]
        );
        clear_sink();
    }

    /// With no sink installed, an event is dropped rather than queued — a page
    /// that never asked for notifications should not accumulate them.
    #[test]
    fn an_event_with_no_sink_is_dropped() {
        clear_sink();
        let mut w = Writer::new();
        w.str("dev-6");
        let body = w.finish();
        unsafe { wbt_event(EventKind::ServiceChanged as u32, body.as_ptr(), body.len()) };
        // Nothing to assert but the absence of a panic.
    }
}
