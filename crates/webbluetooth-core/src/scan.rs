//! Fan-out for the one radio scan a controller can run.
//!
//! Every platform here scans globally: CoreBluetooth has one
//! `scanForPeripheralsWithServices:` per central, BlueZ one `StartDiscovery`
//! per adapter, WinRT one watcher, Android one `BluetoothLeScanner`. The radio
//! is single, but the *interest* in it is not — the specification has three
//! things that all want advertisements at once:
//!
//! * `requestDevice()`, while the chooser deliberates,
//! * `requestLEScan()`, for as long as the caller keeps it,
//! * `watchAdvertisements()`, on a device that is already granted.
//!
//! So the scan is reference-counted and its sightings are published to every
//! interested party. The first watcher starts the radio and the last one stops
//! it, which means a `requestDevice` no longer cancels a scan somebody else is
//! relying on.
//!
//! Filters are combined rather than intersected: the radio scan has to be at
//! least as wide as the widest watcher, and each watcher then applies its own
//! filter to what arrives. A watcher with no service filter therefore forces an
//! unfiltered scan, which is the only way it can see what it asked for.

use crate::chooser::Candidate;
use crate::filter::{Advertisement, RequestDeviceOptions};
use crate::registry::Grant;
use crate::uuid::BluetoothUuid;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// What one watcher is interested in.
// `Request` and `choose` are unused on the web: `requestDevice` there opens
// the browser's own picker, so there is no scan to run and no candidate list
// for a chooser to be offered.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
/// How many advertisements a watcher queues before the oldest are dropped.
///
/// Smaller than a notification backlog on purpose. Advertisements arrive far
/// faster and age far worse: by the time a consumer is 256 behind, the early
/// ones describe a room that has moved on. Keeping more would cost memory to
/// deliver information that is already wrong.
const ADVERTISEMENT_BACKLOG: usize = 256;

/// Why something is watching the radio.
///
/// The hub keeps one scan for all of them and hands each watcher only what it
/// asked for, so a `request_device` finishing cannot stop somebody else's
/// `watchAdvertisements`.
pub enum Want {
    /// A `requestDevice` chooser. Duplicates are wanted: they keep RSSI live
    /// while the chooser is deciding.
    Request(RequestDeviceOptions),
    /// `requestLEScan`.
    Scan {
        /// What the caller asked for, which is also what it may be told.
        options: RequestDeviceOptions,
        /// Whether repeated sightings of the same device are wanted.
        keep_repeated: bool,
    },
    /// `watchAdvertisements` on one granted device — every packet from it, but
    /// reported through the grant that device was given.
    /// One device's advertisements — `watchAdvertisements`.
    Device {
        /// Which device.
        id: String,
        /// What the holder may be told about it.
        grant: Grant,
    },
}

impl Want {
    fn options(&self) -> Option<&RequestDeviceOptions> {
        match self {
            Self::Request(options) => Some(options),
            Self::Scan { options, .. } => Some(options),
            Self::Device { .. } => None,
        }
    }

    /// The grant that advertisements reported to this watcher pass through,
    /// or `None` if they are reported whole.
    ///
    /// A grant is a permission, not a preference. The specification's
    /// *fire an advertisementreceived event* narrows the event to the services
    /// and company identifiers the caller was granted, so a site cannot learn
    /// what else a device is broadcasting; asking for none means seeing none.
    ///
    /// It applies to the two paths that fire that event — `watchAdvertisements`
    /// and `requestLEScan` — and not to the chooser. The chooser is the picker
    /// the user is looking at while deciding, which is the one place the
    /// unfiltered view is the point: it has to show what is on the air, and
    /// there is no grant yet to filter by, because granting is what the user
    /// is in the middle of doing.
    fn grant(&self) -> Option<Grant> {
        match self {
            Self::Request(_) => None,
            Self::Scan { options, .. } => Some(options.grant()),
            Self::Device { grant, .. } => Some(grant.clone()),
        }
    }

    fn wants_duplicates(&self) -> bool {
        match self {
            Self::Request(_) | Self::Device { .. } => true,
            Self::Scan { keep_repeated, .. } => *keep_repeated,
        }
    }

    /// Does this sighting belong to this watcher?
    fn accepts(&self, id: &str, name: Option<&str>, advertisement: &Advertisement) -> bool {
        match self {
            Self::Device { id: wanted, .. } => wanted == id,
            Self::Request(options) | Self::Scan { options, .. } => {
                options.matches(name, advertisement)
            }
        }
    }
}

struct Watcher {
    id: u64,
    want: Want,
    /// Resolved once here rather than per packet, since it cannot change.
    grant: Option<Grant>,
    tx: crate::backlog::Sender<Candidate>,
    /// Devices already delivered, for a watcher that does not want repeats.
    delivered: HashSet<String>,
}

/// The registry of everyone watching the scan.
pub struct ScanHub {
    watchers: Mutex<Vec<Watcher>>,
    next_id: AtomicU64,
}

impl Default for ScanHub {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanHub {
    /// A hub with no watchers and the radio off.
    pub fn new() -> Self {
        Self {
            watchers: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Register a watcher.
    ///
    /// The `bool` is whether the radio scan has to be (re)started: either
    /// because this is the first watcher, or because it widened the filter.
    pub fn add(&self, want: Want) -> (u64, crate::backlog::Receiver<Candidate>, bool) {
        // The busiest stream in the crate: a radio in a crowded room reports
        // hundreds of advertisements a second, from a callback thread that
        // cannot be made to wait. Bounded, keeping the newest, because an
        // advertisement is a statement about *now* — one from four seconds ago
        // is not worth the memory it is sitting in, and the device that sent
        // it will say so again shortly if it is still there.
        let (tx, rx) =
            crate::backlog::bounded(ADVERTISEMENT_BACKLOG, crate::backlog::Overflow::KeepNewest);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut watchers = self.watchers.lock().unwrap();
        let before = Self::radio_filter(&watchers);
        watchers.push(Watcher {
            id,
            grant: want.grant(),
            want,
            tx,
            delivered: HashSet::new(),
        });
        let after = Self::radio_filter(&watchers);
        (id, rx, before != after || watchers.len() == 1)
    }

    /// Remove a watcher, returning whether the radio scan should now stop.
    pub fn remove(&self, id: u64) -> bool {
        let mut watchers = self.watchers.lock().unwrap();
        watchers.retain(|w| w.id != id);
        watchers.is_empty()
    }

    /// Drop every watcher, ending their streams.
    ///
    /// For when the platform says the scan has failed: nothing further will
    /// arrive, so a chooser waiting on the stream should see it end rather than
    /// wait out its whole window for traffic that is not coming.
    ///
    /// Not every platform has such a signal — CoreBluetooth has no
    /// scan-failed callback at all — so this is unused on some targets.
    #[allow(dead_code)]
    pub fn close_all(&self) {
        self.watchers.lock().unwrap().clear();
    }

    /// Whether `watchAdvertisements` is running for this device —
    /// `BluetoothDevice.watchingAdvertisements`.
    pub fn is_watching_device(&self, id: &str) -> bool {
        self.watchers
            .lock()
            .unwrap()
            .iter()
            .any(|w| matches!(&w.want, Want::Device { id: watched, .. } if watched == id))
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    /// Whether anything still wants the radio scanning.
    pub fn is_watching(&self) -> bool {
        !self.watchers.lock().unwrap().is_empty()
    }

    /// Whether a given watcher is still registered — how a handed-out scan
    /// reports whether it is still live.
    pub fn contains(&self, id: u64) -> bool {
        self.watchers.lock().unwrap().iter().any(|w| w.id == id)
    }

    /// The service UUIDs the radio scan should ask for.
    ///
    /// Unused on a transport whose scan cannot be filtered at all — a raw HCI
    /// socket reports whatever the radio hears.
    ///
    /// Empty means "everything", and one watcher that needs everything makes it
    /// empty for all of them — a narrower scan would hide what that watcher
    /// asked for. The per-watcher filter still applies on the way out, so a
    /// wider radio scan costs traffic, never correctness.
    #[allow(dead_code)]
    pub fn scan_services(&self) -> Vec<BluetoothUuid> {
        Self::radio_filter(&self.watchers.lock().unwrap())
    }

    fn radio_filter(watchers: &[Watcher]) -> Vec<BluetoothUuid> {
        let mut union: Vec<BluetoothUuid> = Vec::new();
        for watcher in watchers {
            let Some(options) = watcher.want.options() else {
                // `watchAdvertisements` names a device, not a service.
                return Vec::new();
            };
            let services = options.scan_services();
            if services.is_empty() {
                return Vec::new();
            }
            for uuid in services {
                if !union.contains(&uuid) {
                    union.push(uuid);
                }
            }
        }
        union.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        union
    }

    /// Whether the radio scan should report repeat sightings.
    ///
    /// Also unused where the scan cannot be told either way.
    #[allow(dead_code)]
    pub fn wants_duplicates(&self) -> bool {
        self.watchers
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.want.wants_duplicates())
    }

    /// Offer one sighting to every watcher that wants it.
    ///
    /// A watcher whose receiver has been dropped is left in place; `remove` is
    /// what deregisters, so that a caller who stops polling briefly does not
    /// silently lose its subscription.
    pub fn publish(&self, id: &str, name: Option<&str>, advertisement: &Advertisement) {
        let mut watchers = self.watchers.lock().unwrap();
        for watcher in watchers.iter_mut() {
            if !watcher.want.accepts(id, name, advertisement) {
                continue;
            }
            if !watcher.want.wants_duplicates() && !watcher.delivered.insert(id.to_owned()) {
                continue;
            }
            // Filtering happens after matching, deliberately: what a watcher
            // may be *told* is narrower than what it may be matched on. A
            // filter for a service the caller then cannot see is still a
            // legitimate way to find a device.
            //
            // Two watchers of the same device can hold different grants, so
            // the same packet is legitimately reported differently to each.
            let mut advertisement = advertisement.clone();
            advertisement.restrict_to(watcher.grant.as_ref());

            let _ = watcher.tx.send(Candidate {
                id: id.to_owned(),
                name: name.map(str::to_owned),
                advertisement,
            });
        }
    }
}

/// Run a `requestDevice` against the hub.
///
/// Every backend used to carry its own copy of this: register a watcher, start
/// the radio if this watcher needs it, let the chooser decide, then stop the
/// radio only if nobody else is still watching. The copies drifted — two said
/// "restarting" where the others said "stopping", and one named the variable
/// differently — and each was a place the reference counting could be got
/// wrong independently. What is genuinely per-platform is only what happens
/// *after* a device is chosen, which stays in the backend.
///
/// `set_scanning` starts or stops the platform's scan. `on_joined` runs
/// instead, when an existing scan is already wide enough to serve this
/// watcher — BlueZ replays its device cache there, because `bluetoothd` does
/// not re-announce devices it already knows.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub async fn choose(
    hub: &ScanHub,
    options: RequestDeviceOptions,
    chooser: &dyn crate::chooser::DeviceChooser,
    set_scanning: impl Fn(bool) -> crate::error::Result<()>,
    on_joined: impl Fn(),
) -> crate::error::Result<String> {
    options.validate()?;
    let (watcher, from_scan, restart) = hub.add(Want::Request(options));

    if restart {
        if let Err(e) = set_scanning(true) {
            hub.remove(watcher);
            return Err(e);
        }
    } else {
        on_joined();
    }

    let chosen = chooser
        .choose(crate::chooser::Candidates { inner: from_scan })
        .await;

    if hub.remove(watcher) {
        let _ = set_scanning(false);
    }

    chosen.ok_or_else(|| crate::error::Error::NotFound("no device was chosen".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::DeviceFilter;
    use crate::uuid::services;

    /// The next sighting a watcher has been offered, if any.
    fn next(rx: &mut crate::backlog::Receiver<Candidate>) -> Option<Candidate> {
        rx.try_recv()
    }

    fn advertisement(uuids: &[u16]) -> Advertisement {
        Advertisement {
            service_uuids: uuids.iter().map(|u| BluetoothUuid::from_u16(*u)).collect(),
            ..Default::default()
        }
    }

    fn requesting(service: u16) -> Want {
        Want::Request(
            RequestDeviceOptions::new().filter(
                DeviceFilter::new()
                    .service(BluetoothUuid::from_u16(service))
                    .unwrap(),
            ),
        )
    }

    #[test]
    fn the_first_watcher_starts_the_radio_and_the_last_stops_it() {
        let hub = ScanHub::new();
        let (first, _a, start) = hub.add(requesting(0x180F));
        assert!(start, "the first watcher has to start the scan");
        let (second, _b, _) = hub.add(requesting(0x180F));

        assert!(!hub.remove(first), "another watcher still wants the scan");
        assert!(hub.remove(second), "the last one stops it");
        assert!(!hub.is_watching());
    }

    /// The case that motivated reference counting: a `requestDevice` used to
    /// stop the radio on its way out, which would cancel a scan someone else
    /// was still reading.
    #[test]
    fn a_finished_request_does_not_stop_somebody_elses_scan() {
        let hub = ScanHub::new();
        let (scan, mut stream, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new().accept_all_devices(),
            keep_repeated: true,
        });
        let (request, _r, _) = hub.add(requesting(0x180F));

        assert!(!hub.remove(request));
        hub.publish("aa", Some("Sensor"), &advertisement(&[0x180F]));
        assert!(next(&mut stream).is_some(), "the scan still runs");
        assert!(hub.contains(scan));
    }

    #[test]
    fn one_unfiltered_watcher_widens_the_radio_scan_for_everyone() {
        let hub = ScanHub::new();
        let (_a, _s, _) = hub.add(requesting(0x180F));
        assert_eq!(
            hub.scan_services(),
            vec![BluetoothUuid::from_u16(0x180F)],
            "a single service filter narrows the scan"
        );

        // `watchAdvertisements` names a device, so the radio cannot be filtered
        // by service without risking hiding it.
        let (_b, _t, restart) = hub.add(Want::Device {
            id: "aa".into(),
            grant: Grant::default(),
        });
        assert!(restart, "widening the filter has to restart the radio");
        assert!(hub.scan_services().is_empty());
    }

    #[test]
    fn filters_are_combined_not_intersected() {
        let hub = ScanHub::new();
        hub.add(requesting(0x180F));
        hub.add(requesting(0x180D));
        assert_eq!(
            hub.scan_services(),
            vec![
                BluetoothUuid::from_u16(0x180D),
                BluetoothUuid::from_u16(0x180F)
            ],
            "the radio must be at least as wide as the widest watcher"
        );
    }

    #[test]
    fn a_watcher_only_sees_what_its_own_filter_accepts() {
        let hub = ScanHub::new();
        let (_a, mut battery, _) = hub.add(requesting(0x180F));
        let (_b, mut heart, _) = hub.add(requesting(0x180D));

        hub.publish("aa", None, &advertisement(&[0x180F]));
        assert!(next(&mut battery).is_some());
        assert!(next(&mut heart).is_none(), "not this watcher's device");
    }

    #[test]
    fn a_device_knows_whether_it_is_being_watched() {
        let hub = ScanHub::new();
        assert!(!hub.is_watching_device("mine"));

        let (id, _stream, _) = hub.add(Want::Device {
            id: "mine".into(),
            grant: Grant::default(),
        });
        assert!(hub.is_watching_device("mine"));
        // A scan is not a watch on any particular device.
        assert!(!hub.is_watching_device("theirs"));

        hub.remove(id);
        assert!(!hub.is_watching_device("mine"), "the watch ended");
    }

    #[test]
    fn watching_a_device_ignores_every_other_device() {
        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Device {
            id: "mine".into(),
            grant: Grant::default(),
        });

        hub.publish("theirs", Some("Someone Else"), &advertisement(&[]));
        assert!(next(&mut stream).is_none());

        hub.publish("mine", Some("Mine"), &advertisement(&[]));
        let event = next(&mut stream).expect("the watched device");
        assert_eq!(event.id, "mine");
    }

    #[test]
    fn a_scan_without_keep_repeated_reports_each_device_once() {
        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new().accept_all_devices(),
            keep_repeated: false,
        });
        assert!(!hub.wants_duplicates());

        hub.publish("aa", None, &advertisement(&[]));
        hub.publish("aa", None, &advertisement(&[]));
        assert!(next(&mut stream).is_some());
        assert!(next(&mut stream).is_none(), "the repeat is suppressed");

        // A different device is still new.
        hub.publish("bb", None, &advertisement(&[]));
        assert!(next(&mut stream).is_some());
    }

    #[test]
    fn a_chooser_gets_duplicates_so_its_rssi_stays_live() {
        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(requesting(0x180F));
        assert!(hub.wants_duplicates());
        hub.publish("aa", None, &advertisement(&[0x180F]));
        hub.publish("aa", None, &advertisement(&[0x180F]));
        assert!(next(&mut stream).is_some());
        assert!(next(&mut stream).is_some(), "repeats are wanted");
    }

    /// A failed scan has to end the streams, or a chooser waits out its whole
    /// window for traffic that will never arrive.
    #[test]
    fn closing_the_hub_ends_every_stream() {
        let hub = ScanHub::new();
        let (id, mut stream, _) = hub.add(requesting(0x180F));
        hub.close_all();

        assert!(!hub.is_watching());
        assert!(!hub.contains(id));
        // The sender is gone, so the stream is finished rather than merely idle.
        assert!(stream.try_recv().is_none());
    }

    /// `optionalManufacturerData` is a permission, not a preference.
    ///
    /// The specification filters an advertisement event down to the companies
    /// the caller asked for, so a site cannot learn what else a device is
    /// broadcasting. This crate accepted the option and ignored it, which is
    /// worse than not offering it: a caller could reasonably believe the data
    /// they were handed had been filtered.
    #[test]
    fn manufacturer_data_is_filtered_to_what_was_granted() {
        const APPLE: u16 = 0x004C;
        const OTHER: u16 = 0x00E0;

        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new()
                .accept_all_devices()
                .optional_manufacturer_data([APPLE]),
            keep_repeated: true,
        });

        let mut advertisement = advertisement(&[]);
        advertisement.manufacturer_data.insert(APPLE, vec![1, 2, 3]);
        advertisement.manufacturer_data.insert(OTHER, vec![9, 9]);
        hub.publish("aa", None, &advertisement);

        let seen = next(&mut stream).expect("the device matches");
        assert_eq!(
            seen.advertisement.manufacturer_data.get(&APPLE),
            Some(&vec![1, 2, 3]),
            "what was asked for comes through"
        );
        assert!(
            !seen.advertisement.manufacturer_data.contains_key(&OTHER),
            "a company that was not asked for must not be reported"
        );
    }

    /// The event carries three grant-filtered fields, not one: service UUIDs
    /// and service data are narrowed the same way manufacturer data is.
    #[test]
    fn services_and_service_data_are_filtered_to_the_grant() {
        const GRANTED: u16 = 0x180D; // heart rate
        const PRIVATE: u16 = 0x180F; // battery, not asked for

        let (granted, private) = (
            BluetoothUuid::from_u16(GRANTED),
            BluetoothUuid::from_u16(PRIVATE),
        );

        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new()
                .filter(DeviceFilter::new().service(granted).unwrap()),
            keep_repeated: true,
        });

        let mut advertisement = advertisement(&[GRANTED, PRIVATE]);
        advertisement.service_data.insert(granted, vec![0x50]);
        advertisement.service_data.insert(private, vec![0x63]);
        advertisement.solicited_service_uuids.push(private);
        hub.publish("aa", None, &advertisement);

        let seen = next(&mut stream).expect("the device matches on the granted service");
        assert_eq!(
            seen.advertisement.service_uuids,
            vec![granted],
            "an ungranted service must not be reported"
        );
        assert!(
            seen.advertisement.solicited_service_uuids.is_empty(),
            "nor solicited under one"
        );
        assert_eq!(seen.advertisement.service_data.len(), 1);
        assert!(seen.advertisement.service_data.contains_key(&granted));
    }

    /// The chooser is the exception, and deliberately: it is what the user is
    /// looking at while deciding, so it has to show what is on the air. There
    /// is also no grant to filter by yet — granting is the thing in progress.
    #[test]
    fn the_chooser_sees_the_advertisement_whole() {
        const APPLE: u16 = 0x004C;
        let battery = BluetoothUuid::from_u16(0x180F);

        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Request(
            RequestDeviceOptions::new().accept_all_devices(),
        ));

        let mut advertisement = advertisement(&[0x180F]);
        advertisement.service_data.insert(battery, vec![0x63]);
        advertisement.manufacturer_data.insert(APPLE, vec![0x09]);
        advertisement
            .manufacturer_data
            .insert(0x00E0, vec![0x02, 0x15]);
        hub.publish("aa", None, &advertisement);

        let seen = next(&mut stream).expect("accept_all_devices matches");
        assert_eq!(seen.advertisement.service_uuids, vec![battery]);
        assert_eq!(seen.advertisement.service_data.len(), 1);
        assert_eq!(
            seen.advertisement.manufacturer_data.len(),
            2,
            "the picker needs to show what is actually being broadcast"
        );
    }

    /// The blocklist is not part of any grant, so the chooser does not escape
    /// it either. No permission makes reading a stranger's beacon acceptable.
    #[test]
    fn the_chooser_does_not_escape_the_manufacturer_blocklist() {
        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Request(
            RequestDeviceOptions::new().accept_all_devices(),
        ));

        let mut advertisement = advertisement(&[]);
        advertisement
            .manufacturer_data
            .insert(0x004C, vec![0x02, 0x15, 0xAB]);
        hub.publish("aa", None, &advertisement);

        let seen = next(&mut stream).expect("accept_all_devices matches");
        assert!(seen.advertisement.manufacturer_data.is_empty());
    }

    /// Filtering happens after matching: a caller may find a device by a
    /// service it is then not told about. Narrowing the match too would make
    /// `optionalServices` a precondition for its own filter.
    #[test]
    fn a_device_still_matches_on_what_the_grant_hides() {
        const SECRET: u16 = 0x180F;
        let hub = ScanHub::new();
        // Granted nothing, because an exclusion filter grants nothing.
        let (_id, mut stream, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new().accept_all_devices(),
            keep_repeated: true,
        });

        hub.publish("aa", None, &advertisement(&[SECRET]));
        let seen = next(&mut stream).expect("the device is still reported");
        assert!(
            seen.advertisement.service_uuids.is_empty(),
            "but without the service it was found by"
        );
    }

    /// Granting a company does not grant its blocklisted data: the two filters
    /// are independent, and the stricter one wins.
    #[test]
    fn a_grant_does_not_override_the_manufacturer_blocklist() {
        const APPLE: u16 = 0x004C;

        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new()
                .accept_all_devices()
                .optional_manufacturer_data([APPLE]),
            keep_repeated: true,
        });

        let mut advertisement = advertisement(&[]);
        advertisement
            .manufacturer_data
            .insert(APPLE, vec![0x02, 0x15, 0xAB]); // an iBeacon frame
        hub.publish("aa", None, &advertisement);

        let seen = next(&mut stream).expect("the device still matches");
        assert!(
            seen.advertisement.manufacturer_data.is_empty(),
            "asking for Apple's data must not yield a proximity beacon"
        );
    }

    /// Asking for none means seeing none, rather than seeing everything.
    #[test]
    fn manufacturer_data_is_withheld_when_none_was_asked_for() {
        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new().accept_all_devices(),
            keep_repeated: true,
        });

        let mut advertisement = advertisement(&[]);
        advertisement.manufacturer_data.insert(0x004C, vec![1]);
        hub.publish("aa", None, &advertisement);

        let seen = next(&mut stream).expect("the device matches");
        assert!(seen.advertisement.manufacturer_data.is_empty());
    }

    /// Two watchers of the same device can hold different grants, so the same
    /// packet is reported differently to each.
    #[test]
    fn each_watcher_sees_only_its_own_grant() {
        const APPLE: u16 = 0x004C;
        const OTHER: u16 = 0x00E0;

        let hub = ScanHub::new();
        let (_a, mut permissive, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new()
                .accept_all_devices()
                .optional_manufacturer_data([APPLE, OTHER]),
            keep_repeated: true,
        });
        let (_b, mut narrow, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new()
                .accept_all_devices()
                .optional_manufacturer_data([OTHER]),
            keep_repeated: true,
        });

        let mut advertisement = advertisement(&[]);
        advertisement.manufacturer_data.insert(APPLE, vec![1]);
        advertisement.manufacturer_data.insert(OTHER, vec![2]);
        hub.publish("aa", None, &advertisement);

        assert_eq!(
            next(&mut permissive)
                .unwrap()
                .advertisement
                .manufacturer_data
                .len(),
            2
        );
        let narrow = next(&mut narrow).unwrap().advertisement.manufacturer_data;
        assert_eq!(narrow.len(), 1);
        assert!(narrow.contains_key(&OTHER));
    }

    #[test]
    fn the_allowlist_still_decides_what_a_scan_may_see() {
        // A scan is not a grant: `requestLEScan` filters do not widen access to
        // services, they only choose which advertisements are reported.
        let hub = ScanHub::new();
        let (_id, mut stream, _) = hub.add(Want::Scan {
            options: RequestDeviceOptions::new()
                .filter(DeviceFilter::new().name("Sensor"))
                .optional_service(services::BATTERY_SERVICE)
                .unwrap(),
            keep_repeated: true,
        });
        hub.publish("aa", Some("Sensor"), &advertisement(&[]));
        assert!(next(&mut stream).is_some());
        hub.publish("bb", Some("Other"), &advertisement(&[]));
        assert!(next(&mut stream).is_none());
    }
}
