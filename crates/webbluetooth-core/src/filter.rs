//! `requestDevice` options, and matching them against an advertisement.

use crate::blocklist;
use crate::error::{Error, Result};
use crate::uuid::{BluetoothUuid, IntoUuid};
use std::collections::{BTreeSet, HashMap};

/// One advertising packet, in Web Bluetooth's shape.
///
/// Converted from the `NSDictionary` CoreBluetooth supplies, with every UUID
/// canonicalised and the manufacturer blob split on its leading company
/// identifier the way the spec's `manufacturerData` map is keyed.
/// The RSSI value that means "no reading", which every transport spells `127`.
/// The RSSI a platform reports when it has none.
///
/// 127 is the Bluetooth "RSSI not available" value, and is what a sighting
/// carries when the transport did not measure one.
pub const UNAVAILABLE_RSSI: i32 = 127;

/// One advertising packet, as the specification's `BluetoothAdvertisingEvent`
/// reports it.
///
/// What is present depends on what the peer sent and on what the transport
/// exposes — each field says so where it differs.
#[derive(Debug, Clone, Default)]
pub struct Advertisement {
    /// The name in this packet, which can differ from the cached GAP name.
    pub local_name: Option<String>,
    /// Advertised transmit power in dBm, if present.
    pub tx_power: Option<i16>,
    /// The GAP Appearance value, if the device advertised one — a 16-bit code
    /// for what kind of thing it is (a heart-rate belt, a keyboard).
    ///
    /// Reported where the transport exposes it: a raw HCI scan reads AD type
    /// `0x19`, and BlueZ has an `Appearance` property. CoreBluetooth does not
    /// expose it in an advertisement at all, so it is `None` there.
    pub appearance: Option<u16>,
    /// Whether the peripheral said it accepts connections.
    pub is_connectable: Option<bool>,
    /// Services advertised in this packet.
    pub service_uuids: Vec<BluetoothUuid>,
    /// Services advertised in the iOS/macOS "overflow" area — visible only to
    /// an Apple scanner, and not part of the Web Bluetooth model.
    pub overflow_service_uuids: Vec<BluetoothUuid>,
    /// Services the peripheral is soliciting.
    pub solicited_service_uuids: Vec<BluetoothUuid>,
    /// Payload per company identifier, the identifier itself stripped.
    pub manufacturer_data: HashMap<u16, Vec<u8>>,
    /// Payload per service.
    pub service_data: HashMap<BluetoothUuid, Vec<u8>>,
    /// Received signal strength in dBm. [`UNAVAILABLE_RSSI`] means no reading.
    pub rssi: i32,
}

impl Advertisement {
    /// Fold a later packet from the same device into this one.
    ///
    /// A device does not necessarily say everything in one packet. The common
    /// case is a name that does not fit alongside the service UUIDs, so it
    /// goes in the scan response instead — which arrives as a *separate*
    /// report. Judging each report on its own then loses whichever half the
    /// filter did not match: an `ADV_IND` carrying UUIDs but no name looks
    /// nameless, and the `SCAN_RSP` carrying the name has no UUIDs, so a
    /// service filter rejects it outright and the name is never seen again.
    ///
    /// Platforms with a daemon merge these before we see them. On a raw HCI
    /// socket the merging is ours to do.
    // Only the raw-HCI backend needs this — every other transport merges
    // reports before handing them over — but it stays compiled everywhere so
    // its tests run on any host, which is where the behaviour is pinned down.
    #[cfg_attr(not(all(target_os = "linux", feature = "linux-hci")), allow(dead_code))]
    pub fn merge(&mut self, newer: Advertisement) {
        // Newer is better for anything that can change between packets.
        if newer.local_name.is_some() {
            self.local_name = newer.local_name;
        }
        if newer.tx_power.is_some() {
            self.tx_power = newer.tx_power;
        }
        if newer.appearance.is_some() {
            self.appearance = newer.appearance;
        }
        // A scan response is not itself connectable, so it must not be allowed
        // to retract what the advertisement already established.
        if newer.is_connectable == Some(true) || self.is_connectable.is_none() {
            self.is_connectable = newer.is_connectable.or(self.is_connectable);
        }
        // RSSI is a live measurement: the latest reading wins, unless the
        // newer report had none to give.
        if newer.rssi != UNAVAILABLE_RSSI {
            self.rssi = newer.rssi;
        }

        // Sets accumulate. A device advertising more UUIDs than fit in one
        // packet splits them, and dropping either half would hide it.
        for uuid in newer.service_uuids {
            if !self.service_uuids.contains(&uuid) {
                self.service_uuids.push(uuid);
            }
        }
        for uuid in newer.overflow_service_uuids {
            if !self.overflow_service_uuids.contains(&uuid) {
                self.overflow_service_uuids.push(uuid);
            }
        }
        for uuid in newer.solicited_service_uuids {
            if !self.solicited_service_uuids.contains(&uuid) {
                self.solicited_service_uuids.push(uuid);
            }
        }
        self.manufacturer_data.extend(newer.manufacturer_data);
        self.service_data.extend(newer.service_data);
    }
}

impl Advertisement {
    /// Narrow this advertisement to what a holder of `grant` may be told.
    ///
    /// This is the reporting half of the specification's *fire an
    /// advertisementreceived event*: the event carries only the service UUIDs
    /// and service data the caller was granted, and only the manufacturer data
    /// for company identifiers it asked for. `None` reports the services and
    /// company identifiers whole — the chooser's view, which has to show what
    /// is actually on the air.
    ///
    /// The manufacturer blocklist applies either way. It is not part of any
    /// grant and cannot be opted out of: an iBeacon frame is a location fix,
    /// and no permission a caller can hold makes reading one off a passing
    /// stranger's phone acceptable.
    pub(crate) fn restrict_to(&mut self, grant: Option<&crate::registry::Grant>) {
        if let Some(grant) = grant {
            let allowed = |u: &BluetoothUuid| grant.services.contains(u);
            self.service_uuids.retain(allowed);
            self.overflow_service_uuids.retain(allowed);
            self.solicited_service_uuids.retain(allowed);
            self.service_data.retain(|uuid, _| allowed(uuid));
            self.manufacturer_data
                .retain(|company, _| grant.manufacturer_data.contains(company));
        }
        self.manufacturer_data
            .retain(|company, data| !blocklist::manufacturer_data_blocked(*company, data));
    }
}

/// The longest a Bluetooth Device Name can be, and so the longest a `name` or
/// `name_prefix` filter can usefully be.
const MAX_DEVICE_NAME: usize = 248;

/// A prefix-and-mask test over a byte payload, as the spec defines it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataPrefix {
    prefix: Vec<u8>,
    mask: Option<Vec<u8>>,
}

impl DataPrefix {
    /// Match bytes that begin with `prefix`.
    pub fn new(prefix: impl Into<Vec<u8>>) -> Self {
        Self {
            prefix: prefix.into(),
            mask: None,
        }
    }

    /// Match only the bits set in `mask`. Must be the same length as the prefix.
    pub fn with_mask(prefix: impl Into<Vec<u8>>, mask: impl Into<Vec<u8>>) -> Result<Self> {
        let (prefix, mask) = (prefix.into(), mask.into());
        if prefix.len() != mask.len() {
            return Err(Error::InvalidModification(format!(
                "dataPrefix is {} bytes but mask is {} — they must be equal",
                prefix.len(),
                mask.len()
            )));
        }
        Ok(Self {
            prefix,
            mask: Some(mask),
        })
    }

    /// The mask actually applied: an absent mask means every bit matters, so
    /// it stands in for a run of `0xFF` as long as the prefix.
    fn effective_mask(&self) -> Vec<u8> {
        self.mask
            .clone()
            .unwrap_or_else(|| vec![0xFF; self.prefix.len()])
    }

    /// Whether every payload this filter can match is also matched by `other`.
    ///
    /// The specification calls this a *strict subset*, and uses it to decide
    /// whether a caller's manufacturer-data filter is merely a narrower way of
    /// asking for something the blocklist already forbids. Being narrower
    /// means two things at once: covering at least the bytes `other` covers,
    /// and agreeing with it on every bit `other` examines.
    pub(crate) fn strict_subset_of(&self, other: &DataPrefix) -> bool {
        if self.prefix.len() < other.prefix.len() {
            return false;
        }
        let (mine, theirs) = (self.effective_mask(), other.effective_mask());
        (0..other.prefix.len()).all(|i| {
            // Every bit `other` looks at, this filter must also look at,
            mine[i] & theirs[i] == theirs[i]
                // and must agree on.
                && self.prefix[i] & theirs[i] == other.prefix[i] & theirs[i]
        })
    }

    /// Whether `data` begins with this prefix, under the mask if there is one.
    pub fn matches(&self, data: &[u8]) -> bool {
        if data.len() < self.prefix.len() {
            return false;
        }
        match &self.mask {
            None => data.starts_with(&self.prefix),
            Some(mask) => self
                .prefix
                .iter()
                .zip(mask)
                .zip(data)
                .all(|((p, m), d)| (p & m) == (d & m)),
        }
    }
}

/// One clause of `filters`. A device matches a filter when it satisfies every
/// member the filter sets; it matches the request when it satisfies any filter.
#[derive(Debug, Clone, Default)]
pub struct DeviceFilter {
    services: Vec<BluetoothUuid>,
    name: Option<String>,
    name_prefix: Option<String>,
    manufacturer_data: Vec<(u16, DataPrefix)>,
    service_data: Vec<(BluetoothUuid, DataPrefix)>,
}

impl DeviceFilter {
    /// A filter that matches nothing yet. Add at least one condition.
    pub fn new() -> Self {
        Self::default()
    }

    /// Require every one of these services in the advertisement.
    pub fn services<U: IntoUuid>(mut self, uuids: impl IntoIterator<Item = U>) -> Result<Self> {
        for u in uuids {
            self.services.push(u.into_uuid()?);
        }
        Ok(self)
    }

    /// Require one service. Convenience over [`DeviceFilter::services`].
    pub fn service(self, uuid: impl IntoUuid) -> Result<Self> {
        self.services([uuid])
    }

    /// Require an exact device name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Require a device-name prefix.
    pub fn name_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.name_prefix = Some(prefix.into());
        self
    }

    /// Require manufacturer data from `company_identifier` matching `prefix`.
    pub fn manufacturer_data(mut self, company_identifier: u16, prefix: DataPrefix) -> Self {
        self.manufacturer_data.push((company_identifier, prefix));
        self
    }

    /// Require service data for `service` matching `prefix`.
    pub fn service_data(mut self, service: impl IntoUuid, prefix: DataPrefix) -> Result<Self> {
        self.service_data.push((service.into_uuid()?, prefix));
        Ok(self)
    }

    /// The spec rejects `{}` — a filter with no members would match everything,
    /// which is what `accept_all_devices` is for.
    fn validate(&self) -> Result<()> {
        if self.services.is_empty()
            && self.name.is_none()
            && self.name_prefix.is_none()
            && self.manufacturer_data.is_empty()
            && self.service_data.is_empty()
        {
            return Err(Error::InvalidModification(
                "a filter must constrain something; use accept_all_devices() to match everything"
                    .into(),
            ));
        }
        // 248 bytes is the longest a Bluetooth Device Name can be, so a
        // filter longer than that could not match anything — the spec rejects
        // it rather than letting it silently never fire.
        if let Some(n) = &self.name {
            if n.len() > MAX_DEVICE_NAME {
                return Err(Error::InvalidModification(format!(
                    "name is {} bytes; a Bluetooth device name holds at most {MAX_DEVICE_NAME}",
                    n.len()
                )));
            }
        }
        if let Some(p) = &self.name_prefix {
            if p.is_empty() {
                return Err(Error::InvalidModification(
                    "name_prefix must not be empty".into(),
                ));
            }
            if p.len() > MAX_DEVICE_NAME {
                return Err(Error::InvalidModification(format!(
                    "name_prefix is {} bytes; a Bluetooth device name holds at most \
                     {MAX_DEVICE_NAME}",
                    p.len()
                )));
            }
        }
        for u in &self.services {
            if blocklist::is_blocked(u) {
                return Err(Error::Security(format!(
                    "service {u} is on the GATT blocklist"
                )));
            }
        }
        for (company, prefix) in &self.manufacturer_data {
            if blocklist::manufacturer_filter_blocked(*company, prefix) {
                return Err(Error::InvalidModification(format!(
                    "manufacturer data filter for company {company:#06x} is on the blocklist"
                )));
            }
        }
        // Filtering on a blocklisted service's *data* is still filtering on
        // it: without this, `serviceData` would be a way to ask which devices
        // expose the very services the blocklist exists to hide.
        for (service, _) in &self.service_data {
            if blocklist::is_blocked(service) {
                return Err(Error::Security(format!(
                    "service data filter for {service} is on the GATT blocklist"
                )));
            }
        }
        Ok(())
    }

    /// Does this advertisement satisfy every member of the filter?
    pub fn matches(&self, name: Option<&str>, adv: &Advertisement) -> bool {
        if !self.services.iter().all(|want| {
            adv.service_uuids.contains(want) || adv.overflow_service_uuids.contains(want)
        }) {
            return false;
        }
        // A device has two names and they need not agree: the cached GAP name
        // read from the Device Name characteristic, and the Local Name in the
        // advertisement. Preferring one hides the other — an iPad advertising
        // "Rust iPad" reports a GAP name of "iPad" as soon as anything has
        // connected to it once, and a filter for the advertised name then stops
        // matching a device that is still advertising it. Either name counts.
        let names = [name, adv.local_name.as_deref()];
        let any_name = |f: &dyn Fn(&str) -> bool| names.iter().flatten().any(|n| f(n));

        if let Some(want) = &self.name {
            if !any_name(&|n| n == want.as_str()) {
                return false;
            }
        }
        if let Some(prefix) = &self.name_prefix {
            if !any_name(&|n| n.starts_with(prefix.as_str())) {
                return false;
            }
        }
        for (company, prefix) in &self.manufacturer_data {
            match adv.manufacturer_data.get(company) {
                Some(data) if prefix.matches(data) => {}
                _ => return false,
            }
        }
        for (service, prefix) in &self.service_data {
            match adv.service_data.get(service) {
                Some(data) if prefix.matches(data) => {}
                _ => return false,
            }
        }
        true
    }
}

/// Options for `Bluetooth::request_device`.
///
/// Mirrors `RequestDeviceOptions`: either a non-empty `filters` list or
/// `accept_all_devices`, never both, plus the `optional_services` that widen
/// the per-device service allowlist without themselves being required.
#[derive(Debug, Clone, Default)]
pub struct RequestDeviceOptions {
    filters: Vec<DeviceFilter>,
    exclusion_filters: Vec<DeviceFilter>,
    optional_services: Vec<BluetoothUuid>,
    optional_manufacturer_data: Vec<u16>,
    accept_all_devices: bool,
}

impl RequestDeviceOptions {
    /// An empty request. It needs either a filter or `accept_all_devices`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a filter. A device matching any filter is a candidate.
    pub fn filter(mut self, filter: DeviceFilter) -> Self {
        self.filters.push(filter);
        self
    }

    /// Add an exclusion filter. A device matching any of these is rejected even
    /// if it matched a `filter`.
    pub fn exclusion_filter(mut self, filter: DeviceFilter) -> Self {
        self.exclusion_filters.push(filter);
        self
    }

    /// Permit access to a service without requiring the device to advertise it.
    ///
    /// **This is the allowlist.** A service reached through
    /// `get_primary_service` that appears neither here nor in a filter is a
    /// `SecurityError`, exactly as in a browser.
    pub fn optional_service(mut self, uuid: impl IntoUuid) -> Result<Self> {
        self.optional_services.push(uuid.into_uuid()?);
        Ok(self)
    }

    /// Permit access to several services at once.
    pub fn optional_services<U: IntoUuid>(
        mut self,
        uuids: impl IntoIterator<Item = U>,
    ) -> Result<Self> {
        for u in uuids {
            self.optional_services.push(u.into_uuid()?);
        }
        Ok(self)
    }

    /// Permit reading manufacturer data from these company identifiers.
    pub fn optional_manufacturer_data(mut self, ids: impl IntoIterator<Item = u16>) -> Self {
        self.optional_manufacturer_data.extend(ids);
        self
    }

    /// Consider every device in range, rather than filtering.
    ///
    /// Mutually exclusive with [`RequestDeviceOptions::filter`].
    pub fn accept_all_devices(mut self) -> Self {
        self.accept_all_devices = true;
        self
    }

    /// Check the combination the way the spec's algorithm does, before any
    /// scanning starts.
    pub fn validate(&self) -> Result<()> {
        match (self.accept_all_devices, self.filters.is_empty()) {
            (true, false) => {
                return Err(Error::InvalidModification(
                    "accept_all_devices() and filter() are mutually exclusive".into(),
                ))
            }
            (false, true) => {
                return Err(Error::InvalidModification(
                    "provide at least one filter(), or call accept_all_devices()".into(),
                ))
            }
            _ => {}
        }
        for f in self.filters.iter().chain(&self.exclusion_filters) {
            f.validate()?;
        }
        for u in &self.optional_services {
            if blocklist::is_blocked(u) {
                return Err(Error::Security(format!(
                    "optional service {u} is on the GATT blocklist"
                )));
            }
        }
        Ok(())
    }

    /// Every service this request grants access to: the union of each filter's
    /// required services and `optional_services`.
    /// Everything this request would grant: the services and the company
    /// identifiers whose advertisement data may be seen.
    pub fn grant(&self) -> crate::registry::Grant {
        crate::registry::Grant {
            services: self.allowed_services(),
            manufacturer_data: self.optional_manufacturer_data.clone(),
        }
    }

    /// Every service this request would grant access to: the union of each
    /// filter's required services and `optional_services`.
    pub fn allowed_services(&self) -> BTreeSet<BluetoothUuid> {
        self.filters
            .iter()
            .flat_map(|f| f.services.iter())
            .chain(self.optional_services.iter())
            .cloned()
            .collect()
    }

    /// The services to hand `scanForPeripheralsWithServices:`.
    ///
    /// Empty means an unfiltered scan, which is what `accept_all_devices` and
    /// any name-only filter require — CoreBluetooth can only pre-filter by
    /// service UUID, so every other criterion is applied to the results here.
    /// An unfiltered scan on macOS also misses peripherals that advertise only
    /// in the overflow area, which is why a service filter is preferable when
    /// one is possible.
    // Unused where the platform has no controller-side service filter: the
    // `linux-hci` backend scans raw, and WinRT's watcher filters by payload
    // rather than by UUID list. Both match in Rust instead.
    #[cfg_attr(
        any(all(target_os = "linux", feature = "linux-hci"), target_os = "windows"),
        allow(dead_code)
    )]
    pub fn scan_services(&self) -> Vec<BluetoothUuid> {
        if self.accept_all_devices || self.filters.iter().any(|f| f.services.is_empty()) {
            return Vec::new();
        }
        let mut seen = BTreeSet::new();
        for f in &self.filters {
            seen.extend(f.services.iter().cloned());
        }
        seen.into_iter().collect()
    }

    /// Is this device a candidate?
    pub fn matches(&self, name: Option<&str>, adv: &Advertisement) -> bool {
        if self.exclusion_filters.iter().any(|f| f.matches(name, adv)) {
            return false;
        }
        self.accept_all_devices || self.filters.iter().any(|f| f.matches(name, adv))
    }
}

impl From<&RequestDeviceOptions> for crate::registry::Grant {
    /// What this request would grant, for the calls that take a grant alone.
    ///
    /// The filters are not part of it: they decide which device is matched,
    /// and a grant is about what may be reached once one is.
    fn from(options: &RequestDeviceOptions) -> Self {
        options.grant()
    }
}

impl From<RequestDeviceOptions> for crate::registry::Grant {
    fn from(options: RequestDeviceOptions) -> Self {
        options.grant()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uuid::services;

    fn adv_with_services(uuids: &[BluetoothUuid]) -> Advertisement {
        Advertisement {
            service_uuids: uuids.to_vec(),
            ..Default::default()
        }
    }

    /// A device has a cached GAP name and an advertised Local Name, and they
    /// need not agree. Observed on an iPad advertising "Rust iPad": once
    /// anything had connected to it, CoreBluetooth reported its GAP name
    /// "iPad", and a filter for the advertised name stopped matching a device
    /// that was still advertising it.
    /// The case that actually bit: bluetoothd puts the service UUIDs in the
    /// advertisement and the name in the scan response. Judged separately, the
    /// first looks nameless and the second fails a service filter, so the
    /// device is found without a name — or not at all.
    #[test]
    fn a_scan_response_completes_the_advertisement() {
        let mut merged = Advertisement::default();
        merged.merge(Advertisement {
            service_uuids: vec![BluetoothUuid::from_u16(0x180F)],
            is_connectable: Some(true),
            rssi: -61,
            ..Default::default()
        });
        // The scan response: a name, no UUIDs, and not itself connectable.
        merged.merge(Advertisement {
            local_name: Some("Rust Linux".into()),
            is_connectable: Some(false),
            rssi: -60,
            ..Default::default()
        });

        assert_eq!(merged.local_name.as_deref(), Some("Rust Linux"));
        assert_eq!(merged.service_uuids, vec![BluetoothUuid::from_u16(0x180F)]);
        // A scan response must not retract connectability.
        assert_eq!(merged.is_connectable, Some(true));
        // The newest reading wins.
        assert_eq!(merged.rssi, -60);

        assert!(DeviceFilter::new()
            .service(services::BATTERY_SERVICE)
            .unwrap()
            .matches(None, &merged));
        assert!(DeviceFilter::new()
            .name("Rust Linux")
            .matches(None, &merged));
    }

    #[test]
    fn merging_accumulates_uuids_and_keeps_the_last_reading() {
        let mut merged = Advertisement {
            service_uuids: vec![BluetoothUuid::from_u16(0x180D)],
            rssi: UNAVAILABLE_RSSI,
            ..Default::default()
        };
        merged.merge(Advertisement {
            service_uuids: vec![BluetoothUuid::from_u16(0x180F)],
            rssi: UNAVAILABLE_RSSI,
            ..Default::default()
        });
        assert_eq!(merged.service_uuids.len(), 2, "both packets' UUIDs");
        // Neither packet had a reading, so there still is none.
        assert_eq!(merged.rssi, UNAVAILABLE_RSSI);

        // A duplicate UUID is not added twice.
        merged.merge(Advertisement {
            service_uuids: vec![BluetoothUuid::from_u16(0x180F)],
            rssi: -70,
            ..Default::default()
        });
        assert_eq!(merged.service_uuids.len(), 2);
        assert_eq!(merged.rssi, -70);
    }

    #[test]
    fn either_name_satisfies_a_name_filter() {
        let advertised = Advertisement {
            local_name: Some("Rust iPad".into()),
            ..Default::default()
        };
        let filter = DeviceFilter::new().name("Rust iPad");
        // Matches on the advertised name while the GAP name says otherwise…
        assert!(filter.matches(Some("iPad"), &advertised));
        // …and on the GAP name when the advertisement carries none.
        assert!(DeviceFilter::new()
            .name("iPad")
            .matches(Some("iPad"), &Advertisement::default()));
        // A name neither of them carries still does not match.
        assert!(!DeviceFilter::new()
            .name("Somebody Else")
            .matches(Some("iPad"), &advertised));
    }

    #[test]
    fn either_name_satisfies_a_name_prefix() {
        let advertised = Advertisement {
            local_name: Some("Rust iPad".into()),
            ..Default::default()
        };
        assert!(DeviceFilter::new()
            .name_prefix("Rust")
            .matches(Some("iPad"), &advertised));
        assert!(!DeviceFilter::new()
            .name_prefix("Rust")
            .matches(Some("iPad"), &Advertisement::default()));
    }

    #[test]
    fn rejects_filters_together_with_accept_all() {
        let opts = RequestDeviceOptions::new()
            .filter(DeviceFilter::new().name("x"))
            .accept_all_devices();
        assert!(matches!(
            opts.validate(),
            Err(Error::InvalidModification(_))
        ));
    }

    #[test]
    fn rejects_no_filters_and_no_accept_all() {
        assert!(matches!(
            RequestDeviceOptions::new().validate(),
            Err(Error::InvalidModification(_))
        ));
    }

    #[test]
    fn rejects_an_empty_filter() {
        let opts = RequestDeviceOptions::new().filter(DeviceFilter::new());
        assert!(matches!(
            opts.validate(),
            Err(Error::InvalidModification(_))
        ));
    }

    #[test]
    fn rejects_blocklisted_services_before_scanning() {
        let f = DeviceFilter::new()
            .service(services::HUMAN_INTERFACE_DEVICE)
            .unwrap();
        let opts = RequestDeviceOptions::new().filter(f);
        assert!(matches!(opts.validate(), Err(Error::Security(_))));

        let opts = RequestDeviceOptions::new()
            .accept_all_devices()
            .optional_service(services::HUMAN_INTERFACE_DEVICE)
            .unwrap();
        assert!(matches!(opts.validate(), Err(Error::Security(_))));
    }

    #[test]
    fn a_filter_requires_all_its_services() {
        let f = DeviceFilter::new()
            .services([services::BATTERY_SERVICE, services::HEART_RATE])
            .unwrap();
        assert!(f.matches(
            None,
            &adv_with_services(&[services::BATTERY_SERVICE, services::HEART_RATE])
        ));
        assert!(!f.matches(None, &adv_with_services(&[services::BATTERY_SERVICE])));
    }

    #[test]
    fn filters_are_disjunctive_but_members_are_conjunctive() {
        let opts = RequestDeviceOptions::new()
            .filter(
                DeviceFilter::new()
                    .service(services::BATTERY_SERVICE)
                    .unwrap(),
            )
            .filter(DeviceFilter::new().name_prefix("Polar"));
        // Matches the second filter only.
        assert!(opts.matches(Some("Polar H10"), &adv_with_services(&[])));
        // Matches the first filter only.
        assert!(opts.matches(
            Some("Anything"),
            &adv_with_services(&[services::BATTERY_SERVICE])
        ));
        assert!(!opts.matches(Some("Anything"), &adv_with_services(&[])));

        // Within one filter, every member must hold.
        let strict = RequestDeviceOptions::new().filter(
            DeviceFilter::new()
                .service(services::BATTERY_SERVICE)
                .unwrap()
                .name_prefix("Polar"),
        );
        assert!(!strict.matches(
            Some("Wahoo"),
            &adv_with_services(&[services::BATTERY_SERVICE])
        ));
        assert!(strict.matches(
            Some("Polar H10"),
            &adv_with_services(&[services::BATTERY_SERVICE])
        ));
    }

    #[test]
    fn exclusion_filters_win() {
        let opts = RequestDeviceOptions::new()
            .accept_all_devices()
            .exclusion_filter(DeviceFilter::new().name_prefix("Bad"));
        assert!(opts.matches(Some("Good device"), &Advertisement::default()));
        assert!(!opts.matches(Some("Bad device"), &Advertisement::default()));
    }

    /// `serviceData` is a second way to name a service, so it needs the same
    /// blocklist check the `services` member gets.
    #[test]
    fn a_service_data_filter_may_not_name_a_blocklisted_service() {
        let hid = crate::uuid::services::HUMAN_INTERFACE_DEVICE;
        let refused = DeviceFilter::new()
            .service_data(hid, DataPrefix::new(vec![1]))
            .unwrap()
            .validate();
        assert!(
            matches!(refused, Err(Error::Security(_))),
            "got {refused:?}"
        );

        let battery = BluetoothUuid::from_u16(0x180F);
        assert!(DeviceFilter::new()
            .service_data(battery, DataPrefix::new(vec![1]))
            .unwrap()
            .validate()
            .is_ok());
    }

    /// A name longer than a device name can be would never match, so it is
    /// rejected rather than left to silently never fire.
    #[test]
    fn a_name_filter_longer_than_a_device_name_is_refused() {
        let long = "a".repeat(MAX_DEVICE_NAME + 1);
        assert!(DeviceFilter::new().name(&long).validate().is_err());
        assert!(DeviceFilter::new().name_prefix(&long).validate().is_err());

        // The limit itself is allowed; it is the length a name may be.
        let exact = "a".repeat(MAX_DEVICE_NAME);
        assert!(DeviceFilter::new().name(&exact).validate().is_ok());
        assert!(DeviceFilter::new().name_prefix(&exact).validate().is_ok());
    }

    /// The limit is on encoded bytes, not characters — 248 emoji are far more
    /// than 248 bytes, and would never fit in a device name.
    #[test]
    fn the_name_limit_counts_bytes_not_characters() {
        let emoji = "😀".repeat(100); // 400 bytes, 100 characters
        assert_eq!(emoji.chars().count(), 100);
        assert!(emoji.len() > MAX_DEVICE_NAME);
        assert!(DeviceFilter::new().name(&emoji).validate().is_err());
    }

    /// The blocklist has to be reachable from `requestDevice`, not merely
    /// correct in its own module.
    #[test]
    fn a_request_may_not_filter_for_blocklisted_manufacturer_data() {
        let refused = RequestDeviceOptions::new()
            .filter(DeviceFilter::new().manufacturer_data(0x004C, DataPrefix::new(vec![0x02])))
            .validate();
        assert!(
            matches!(refused, Err(Error::InvalidModification(_))),
            "an iBeacon filter must be refused, got {refused:?}"
        );

        let allowed = RequestDeviceOptions::new()
            .filter(DeviceFilter::new().manufacturer_data(0x004C, DataPrefix::new(vec![0x09])))
            .validate();
        assert!(allowed.is_ok(), "other Apple data is filterable");
    }

    #[test]
    fn strict_subset_needs_a_prefix_at_least_as_long() {
        let short = DataPrefix::new(vec![0x02]);
        let long = DataPrefix::new(vec![0x02, 0x15]);
        assert!(long.strict_subset_of(&short));
        assert!(!short.strict_subset_of(&long));
    }

    #[test]
    fn manufacturer_data_matches_with_and_without_a_mask() {
        let mut adv = Advertisement::default();
        adv.manufacturer_data.insert(0x004C, vec![0x02, 0x15, 0xAB]);

        let plain =
            DeviceFilter::new().manufacturer_data(0x004C, DataPrefix::new(vec![0x02, 0x15]));
        assert!(plain.matches(None, &adv));

        let wrong_company =
            DeviceFilter::new().manufacturer_data(0x0059, DataPrefix::new(vec![0x02]));
        assert!(!wrong_company.matches(None, &adv));

        // Mask out the low nibble of the first byte: 0x03 & 0xF0 == 0x02 & 0xF0.
        let masked = DeviceFilter::new().manufacturer_data(
            0x004C,
            DataPrefix::with_mask(vec![0x03, 0x15], vec![0xF0, 0xFF]).unwrap(),
        );
        assert!(masked.matches(None, &adv));
    }

    #[test]
    fn mask_length_must_equal_prefix_length() {
        assert!(DataPrefix::with_mask(vec![1, 2, 3], vec![0xFF]).is_err());
    }

    #[test]
    fn allowlist_is_the_union_of_filters_and_optional_services() {
        let opts = RequestDeviceOptions::new()
            .filter(
                DeviceFilter::new()
                    .service(services::BATTERY_SERVICE)
                    .unwrap(),
            )
            .optional_service(services::DEVICE_INFORMATION)
            .unwrap();
        let allowed = opts.allowed_services();
        assert!(allowed.contains(&services::BATTERY_SERVICE));
        assert!(allowed.contains(&services::DEVICE_INFORMATION));
        assert!(!allowed.contains(&services::HEART_RATE));
    }

    #[test]
    fn a_name_only_filter_forces_an_unfiltered_scan() {
        // CoreBluetooth can only pre-filter by service UUID.
        let by_name = RequestDeviceOptions::new().filter(DeviceFilter::new().name_prefix("Polar"));
        assert!(by_name.scan_services().is_empty());

        let by_service = RequestDeviceOptions::new()
            .filter(DeviceFilter::new().service(services::HEART_RATE).unwrap());
        assert_eq!(by_service.scan_services().len(), 1);
    }
}

// ── Reading a request back ──────────────────────────────────────────────────
//
// The builders are write-only by design: `DeviceFilter::name` sets a name, so
// it cannot also return one. That cost nothing while every reader lived in
// this crate, and stopped being free when the backends moved out — the browser
// backend has to re-encode a request to hand it to the page, because the page
// is the only chooser it has.
//
// So the fields are readable through a borrowed view, rather than through
// getters that would have to be named something other than what they read.

/// One filter's fields, for a backend that has to re-encode them.
#[derive(Debug, Clone, Copy)]
pub struct FilterParts<'a> {
    /// Services the advertisement must contain, all of them.
    pub services: &'a [BluetoothUuid],
    /// An exact name to require.
    pub name: Option<&'a str>,
    /// A name prefix to require.
    pub name_prefix: Option<&'a str>,
    /// Manufacturer data to require, by company identifier.
    pub manufacturer_data: &'a [(u16, DataPrefix)],
    /// Service data to require, by service.
    pub service_data: &'a [(BluetoothUuid, DataPrefix)],
}

/// A whole request's fields. See [`FilterParts`].
#[derive(Debug, Clone, Copy)]
pub struct RequestParts<'a> {
    /// Matching any one of these makes a device a candidate.
    pub filters: &'a [DeviceFilter],
    /// Matching any one of these disqualifies it, whatever else matched.
    pub exclusion_filters: &'a [DeviceFilter],
    /// Services the chosen device grants access to beyond those filtered on.
    pub optional_services: &'a [BluetoothUuid],
    /// Company identifiers whose advertisement data may be reported.
    pub optional_manufacturer_data: &'a [u16],
    /// Whether every device is a candidate. Cannot be combined with filters.
    pub accept_all_devices: bool,
}

impl DeviceFilter {
    /// This filter's fields, borrowed.
    pub fn parts(&self) -> FilterParts<'_> {
        FilterParts {
            services: &self.services,
            name: self.name.as_deref(),
            name_prefix: self.name_prefix.as_deref(),
            manufacturer_data: &self.manufacturer_data,
            service_data: &self.service_data,
        }
    }
}

impl RequestDeviceOptions {
    /// This request's fields, borrowed.
    pub fn parts(&self) -> RequestParts<'_> {
        RequestParts {
            filters: &self.filters,
            exclusion_filters: &self.exclusion_filters,
            optional_services: &self.optional_services,
            optional_manufacturer_data: &self.optional_manufacturer_data,
            accept_all_devices: self.accept_all_devices,
        }
    }
}

impl DataPrefix {
    /// The bytes a payload has to start with.
    pub fn bytes(&self) -> &[u8] {
        &self.prefix
    }

    /// The mask applied before comparing, if there is one.
    pub fn mask(&self) -> Option<&[u8]> {
        self.mask.as_deref()
    }
}
