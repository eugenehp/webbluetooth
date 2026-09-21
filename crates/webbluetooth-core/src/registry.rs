//! The device registry, shared by every backend.
//!
//! Each platform reaches a peripheral differently — a retained `CBPeripheral`,
//! a D-Bus object path, a Bluetooth address and an ATT socket — but what is
//! *remembered* about a granted device is the same everywhere: its name, the
//! services the grant covers, whether the link is up, and which generation of
//! handles is still valid.
//!
//! Keeping that here rather than three times over matters most for the two
//! checks that enforce the security model. [`DeviceRegistry::check_allowed`] is
//! what makes an unrequested service a `SecurityError`, and
//! [`DeviceRegistry::check_generation`] is what stops a handle outliving the
//! connection that produced it. A fix to either should not need finding in
//! three files.

use crate::error::{Error, Result};
use crate::uuid::BluetoothUuid;
use futures_channel::mpsc;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// One granted device. `T` is whatever the backend needs to reach it.
/// What `request_device` granted access to.
///
/// Both halves are permission, not preference: a service outside `services` is
/// a `SecurityError`, and manufacturer data from a company outside
/// `manufacturer_data` is withheld from advertisement events rather than
/// reported. They travel together because they are granted together, in one
/// call, and are meaningless apart from it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grant {
    /// The services this grant permits. Anything else is refused, and the
    /// blocklist overrides it.
    pub services: BTreeSet<BluetoothUuid>,
    /// Company identifiers whose advertisement data may be seen.
    pub manufacturer_data: Vec<u16>,
}

impl Grant {
    /// A grant of nothing: no service reachable, no manufacturer data seen.
    ///
    /// Build one up for [`Bluetooth::adopt_device`], which is granting access
    /// rather than matching a device and so has no use for the filters a
    /// `RequestDeviceOptions` also carries.
    ///
    /// [`Bluetooth::adopt_device`]: https://docs.rs/webbluetooth
    pub fn new() -> Self {
        Self::default()
    }

    /// Permit access to a service.
    ///
    /// Refuses a blocklisted service here rather than at first use. The
    /// refusal is not new — `get_primary_service` enforces the blocklist
    /// whatever the grant says — but a grant that cannot be exercised is worth
    /// reporting when it is written, which is what
    /// [`RequestDeviceOptions::validate`](crate::filter::RequestDeviceOptions::validate)
    /// does for the other path.
    pub fn service(mut self, uuid: impl crate::uuid::IntoUuid) -> crate::error::Result<Self> {
        let uuid = uuid.into_uuid()?;
        if crate::blocklist::is_blocked(&uuid) {
            return Err(crate::error::Error::Security(format!(
                "service {uuid} is on the GATT blocklist"
            )));
        }
        self.services.insert(uuid);
        Ok(self)
    }

    /// Permit access to several services at once.
    pub fn services<U: crate::uuid::IntoUuid>(
        mut self,
        uuids: impl IntoIterator<Item = U>,
    ) -> crate::error::Result<Self> {
        for uuid in uuids {
            self = self.service(uuid)?;
        }
        Ok(self)
    }

    /// Permit reading advertisement data from these company identifiers.
    pub fn manufacturer_data(mut self, ids: impl IntoIterator<Item = u16>) -> Self {
        for company in ids {
            if !self.manufacturer_data.contains(&company) {
                self.manufacturer_data.push(company);
            }
        }
        self
    }

    /// Refuse a grant that names something the blocklist forbids.
    ///
    /// The check that matters happens at use — `get_primary_service` consults
    /// the blocklist whatever the grant says — so this is about reporting:
    /// a grant built from a `RequestDeviceOptions` has been through
    /// [`RequestDeviceOptions::validate`](crate::filter::RequestDeviceOptions::validate),
    /// and one built by hand should not be held to a lower standard.
    pub fn validate(&self) -> crate::error::Result<()> {
        for uuid in &self.services {
            if crate::blocklist::is_blocked(uuid) {
                return Err(crate::error::Error::Security(format!(
                    "service {uuid} is on the GATT blocklist"
                )));
            }
        }
        Ok(())
    }

    /// Everything either grant allows.
    ///
    /// A permission only ever widens: the specification unions a new request
    /// with what was already allowed, so asking for less the second time does
    /// not revoke anything.
    pub fn union(&self, other: &Self) -> Self {
        let mut manufacturer_data = self.manufacturer_data.clone();
        for company in &other.manufacturer_data {
            if !manufacturer_data.contains(company) {
                manufacturer_data.push(*company);
            }
        }
        Self {
            services: self.services.union(&other.services).cloned().collect(),
            manufacturer_data,
        }
    }
}

impl From<BTreeSet<BluetoothUuid>> for Grant {
    /// A grant of services alone, which is what state restoration carries:
    /// the specification's grant does not survive process death, so a restored
    /// session is given the allowlist the caller declared up front and no
    /// manufacturer data at all.
    fn from(services: BTreeSet<BluetoothUuid>) -> Self {
        Self {
            services,
            manufacturer_data: Vec::new(),
        }
    }
}

/// One granted device, and whatever the backend keeps beside it.
pub struct Device<T> {
    /// Its name, if the platform reported one.
    pub name: Option<String>,
    /// What `request_device` granted. Reaching anything else is a
    /// `SecurityError`, as in a browser.
    pub allowed: Grant,
    /// Whether the backend believes the link is up.
    pub connected: bool,
    /// Bumped whenever every handle into this device becomes stale.
    pub generation: u64,
    /// Serialises GATT operations, as the spec's per-device queue does.
    ///
    /// Unused on the web, where the browser runs that queue itself.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub gatt: Arc<futures_util::lock::Mutex<()>>,
    /// Whatever the backend needs to reach the device. The web backend needs
    /// nothing: the browser holds the device and this side holds only its id.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub inner: T,
}

/// Every device this session has been granted, and what it may reach.
///
/// Shared by all six backends: the allowlist, the generation counter that
/// invalidates stale handles, and the disconnect and service-changed watchers
/// are the same problem everywhere.
pub struct DeviceRegistry<T> {
    devices: Mutex<HashMap<String, Device<T>>>,
    listeners: Mutex<HashMap<String, Vec<mpsc::UnboundedSender<()>>>>,
    /// Watchers of the service set, kept apart from the disconnect watchers so
    /// a device that reconnects and re-discovers does not look like a drop.
    service_listeners: Mutex<HashMap<String, Vec<mpsc::UnboundedSender<()>>>>,
}

impl<T> Default for DeviceRegistry<T> {
    fn default() -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
            listeners: Mutex::new(HashMap::new()),
            service_listeners: Mutex::new(HashMap::new()),
        }
    }
}

impl<T> DeviceRegistry<T> {
    /// Grant access to a device.
    /// Record a grant, merging it with anything this device was already
    /// granted.
    ///
    /// Merging, not replacing, because that is what the specification says:
    /// *"add the contents of `allowedDevice.allowedServices` to
    /// `grantedServiceUUIDs`"*. A second `requestDevice` for the same device
    /// with narrower options must not take away what the first one granted —
    /// a caller still holding a service from the earlier grant would start
    /// getting `SecurityError` for something it was legitimately given.
    ///
    /// The generation is carried forward for a harder reason. It counts how
    /// many times every handle into this device became stale, and
    /// `check_generation` compares a handle's copy against it. Resetting it to
    /// zero would let a handle captured before a disconnect match again
    /// afterwards, so a re-grant would quietly revive objects that point at
    /// attributes the peer may have rearranged. It only ever goes forwards.
    pub fn insert(
        &self,
        id: &str,
        name: Option<String>,
        allowed: Grant,
        connected: bool,
        inner: T,
    ) {
        let mut devices = self.devices.lock().unwrap();
        let previous = devices.get(id);
        let generation = previous.map(|d| d.generation).unwrap_or(0);
        // The lock is kept too: a fresh one would let an operation already in
        // flight run beside the next, which is the thing it exists to prevent.
        let gatt = previous
            .map(|d| d.gatt.clone())
            .unwrap_or_else(|| Arc::new(futures_util::lock::Mutex::new(())));
        let allowed = match previous {
            Some(d) => d.allowed.union(&allowed),
            None => allowed,
        };

        devices.insert(
            id.to_owned(),
            Device {
                name,
                allowed,
                connected,
                generation,
                gatt,
                inner,
            },
        );
    }

    /// Read something out of a device's record.
    pub fn get<R>(&self, id: &str, f: impl FnOnce(&Device<T>) -> R) -> Result<R> {
        let devices = self.devices.lock().unwrap();
        let device = devices
            .get(id)
            .ok_or_else(|| Error::InvalidState(format!("device {id} is no longer known")))?;
        Ok(f(device))
    }

    /// Modify a device's record. Silently does nothing if it is gone.
    pub fn update(&self, id: &str, f: impl FnOnce(&mut Device<T>)) {
        if let Some(device) = self.devices.lock().unwrap().get_mut(id) {
            f(device);
        }
    }

    /// Find a device by something only the backend can recognise — a peripheral
    /// pointer, an object path — and modify it.
    ///
    /// Only the Apple backend needs this: its events carry a `CBPeripheral`
    /// rather than an id, so the device has to be found by identity.
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    pub fn update_where(
        &self,
        matches: impl Fn(&Device<T>) -> bool,
        f: impl FnOnce(&mut Device<T>),
    ) -> Option<String> {
        let mut devices = self.devices.lock().unwrap();
        let found = devices.iter_mut().find(|(_, d)| matches(d));
        let (id, device) = found?;
        let id = id.clone();
        f(device);
        Some(id)
    }

    /// The id of a device matching a backend-specific predicate.
    ///
    /// Unused where a backend's events already carry the device id rather than
    /// a platform handle — `linux-hci`, Windows and the web all do.
    #[cfg_attr(
        any(
            all(target_os = "linux", feature = "linux-hci"),
            target_os = "windows",
            target_arch = "wasm32",
        ),
        allow(dead_code)
    )]
    pub fn find(&self, matches: impl Fn(&Device<T>) -> bool) -> Option<String> {
        self.devices
            .lock()
            .unwrap()
            .iter()
            .find(|(_, d)| matches(d))
            .map(|(id, _)| id.clone())
    }

    /// The device's name, if it has one.
    pub fn name(&self, id: &str) -> Option<String> {
        self.get(id, |d| d.name.clone()).ok().flatten()
    }

    /// Whether the device is recorded as connected.
    pub fn is_connected(&self, id: &str) -> bool {
        self.get(id, |d| d.connected).unwrap_or(false)
    }

    /// The device's current generation, which a handle records when minted.
    pub fn generation(&self, id: &str) -> Result<u64> {
        self.get(id, |d| d.generation)
    }

    /// Every granted device's id.
    pub fn ids(&self) -> Vec<String> {
        self.devices.lock().unwrap().keys().cloned().collect()
    }

    /// Revoke a grant, returning the record so the backend can tear down its
    /// side of it.
    pub fn remove(&self, id: &str) -> Option<Device<T>> {
        self.listeners.lock().unwrap().remove(id);
        self.devices.lock().unwrap().remove(id)
    }

    /// The per-device allowlist from `request_device`.
    pub fn check_allowed(&self, id: &str, uuid: &BluetoothUuid) -> Result<()> {
        if self.get(id, |d| d.allowed.services.contains(uuid))? {
            Ok(())
        } else {
            Err(Error::Security(format!(
                "service {uuid} was not requested — list it in filters or optional_services"
            )))
        }
    }

    /// What this device was granted access to.
    pub fn allowed_services(&self, id: &str) -> Result<BTreeSet<BluetoothUuid>> {
        self.get(id, |d| d.allowed.services.clone())
    }

    /// The company identifiers this device's grant covers.
    ///
    /// Empty means none were asked for, which is the common case and means no
    /// manufacturer data is reported at all.
    /// Everything `id` was granted, or an empty grant if it is not known.
    ///
    /// An unknown device getting an empty grant rather than a full one is the
    /// safe direction: it reports nothing rather than everything.
    pub fn grant(&self, id: &str) -> Grant {
        self.get(id, |d| d.allowed.clone()).unwrap_or_default()
    }

    /// Fail if handles minted at `generation` are stale, or the link is down.
    /// Whether a handle taken at `generation` may still be used on `id`.
    ///
    /// The two failures are distinct, and the specification orders them: it
    /// checks `gatt.connected` before it checks whether the object still
    /// represents anything. The distinction is worth keeping because it tells
    /// the caller what to do about it — a disconnection is something to
    /// reconnect from and retry, whereas a stale handle survives reconnecting
    /// and has to be rediscovered. Collapsing them into one error loses that.
    pub fn check_generation(&self, id: &str, generation: u64) -> Result<()> {
        // An id nobody has heard of is a stale object, not a lost connection,
        // so this comes first.
        let current = self.generation(id)?;
        if !self.is_connected(id) {
            return Err(Error::Network("the device is not connected".into()));
        }
        if current != generation {
            return Err(Error::InvalidState(
                "this GATT object is stale — the device disconnected or changed its services"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Take this device's GATT lock, held across one operation.
    ///
    /// Unused on the web: `navigator.bluetooth` serialises GATT operations
    /// itself, so taking a second lock here would only add contention.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub async fn gatt_lock(&self, id: &str) -> Result<futures_util::lock::OwnedMutexGuard<()>> {
        Ok(self.get(id, |d| d.gatt.clone())?.lock_owned().await)
    }

    /// Watch for this device's service set changing — the specification's
    /// `serviceadded`, `servicechanged` and `serviceremoved` between them.
    ///
    /// One signal rather than three, because that is what the platforms give:
    /// each reports that the set changed and that existing handles are stale,
    /// not which service it was. Inventing the distinction would mean
    /// reporting detail nobody actually has.
    pub fn watch_services_changed(&self, id: &str) -> mpsc::UnboundedReceiver<()> {
        let (tx, rx) = mpsc::unbounded();
        self.service_listeners
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_default()
            .push(tx);
        rx
    }

    /// The service set changed: invalidate every handle into the device and
    /// tell anyone watching.
    ///
    /// Only the platforms that detect it call this — CoreBluetooth's
    /// `didModifyServices:` and Android's `onServiceChanged`. BlueZ, WinRT and
    /// a raw ATT socket give no such signal, so on those targets this is
    /// compiled and never reached; `watch_services_changed` there returns a
    /// stream that stays silent rather than one that does not exist.
    /// Every handle into this device is now stale, but the link is still up.
    ///
    /// Called by every backend, each from whatever its platform calls this:
    /// CoreBluetooth's `didModifyServices:`, Android's `onServiceChanged`,
    /// BlueZ's GATT objects appearing and vanishing, WinRT's
    /// `GattServicesChanged`, the browser's `serviceschanged`, and — for
    /// `linux-hci`, which has no stack to inherit it from — an indication on
    /// the peer's own Service Changed characteristic.
    pub fn mark_services_changed(&self, id: &str) {
        self.update(id, |d| d.generation += 1);
        let mut listeners = self.service_listeners.lock().unwrap();
        if let Some(subscribers) = listeners.get_mut(id) {
            subscribers.retain(|tx| tx.unbounded_send(()).is_ok());
        }
    }

    /// A stream that yields once each time the device disconnects.
    pub fn watch_disconnect(&self, id: &str) -> mpsc::UnboundedReceiver<()> {
        let (tx, rx) = mpsc::unbounded();
        self.listeners
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_default()
            .push(tx);
        rx
    }

    /// Mark a device disconnected: drop the link, invalidate every handle into
    /// it, and tell anyone watching.
    pub fn mark_disconnected(&self, id: &str) {
        self.update(id, |d| {
            d.connected = false;
            d.generation += 1;
        });
        let mut listeners = self.listeners.lock().unwrap();
        if let Some(subscribers) = listeners.get_mut(id) {
            subscribers.retain(|tx| tx.unbounded_send(()).is_ok());
            if subscribers.is_empty() {
                listeners.remove(id);
            }
        }
    }
}

/// The methods every backend forwards to its registry, unchanged.
///
/// Ten one-line functions, written out six times, byte for byte — the backends
/// differ in how they reach a radio and not at all in how they answer "is this
/// device connected". Expanded inside each `impl Inner` rather than put behind
/// a trait: exactly one backend is compiled into any build, so a trait would
/// buy dynamic dispatch nobody needs, which is the same reasoning that keeps
/// `Inner` a concrete type.
///
/// Every type is named absolutely, so this does not depend on which of them a
/// given backend happens to have imported.
#[macro_export]
macro_rules! forward_to_registry {
    () => {
        pub fn device_name(&self, id: &str) -> Option<String> {
            self.devices.name(id)
        }

        pub fn is_connected(&self, id: &str) -> bool {
            self.devices.is_connected(id)
        }

        pub fn generation(&self, id: &str) -> $crate::error::Result<u64> {
            self.devices.generation(id)
        }

        pub fn granted_devices(&self) -> Vec<String> {
            self.devices.ids()
        }

        pub fn check_allowed(
            &self,
            id: &str,
            uuid: &$crate::uuid::BluetoothUuid,
        ) -> $crate::error::Result<()> {
            self.devices.check_allowed(id, uuid)
        }

        pub fn allowed_services(
            &self,
            id: &str,
        ) -> $crate::error::Result<std::collections::BTreeSet<$crate::uuid::BluetoothUuid>> {
            self.devices.allowed_services(id)
        }

        pub fn check_generation(&self, id: &str, generation: u64) -> $crate::error::Result<()> {
            self.devices.check_generation(id, generation)
        }

        pub fn watch_disconnect(
            &self,
            id: &str,
        ) -> $crate::futures_channel::mpsc::UnboundedReceiver<()> {
            self.devices.watch_disconnect(id)
        }

        pub fn watch_services_changed(
            &self,
            id: &str,
        ) -> $crate::futures_channel::mpsc::UnboundedReceiver<()> {
            self.devices.watch_services_changed(id)
        }

        pub fn grant(&self, id: &str) -> $crate::registry::Grant {
            self.devices.grant(id)
        }
    };
}

pub use crate::forward_to_registry;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uuid::services;
    use futures_executor::block_on;

    fn registry() -> DeviceRegistry<u32> {
        let registry = DeviceRegistry::default();
        registry.insert(
            "AA:BB",
            Some("Sensor".into()),
            BTreeSet::from([services::HEART_RATE]).into(),
            true,
            7u32,
        );
        registry
    }

    #[test]
    fn an_unrequested_service_is_a_security_error() {
        let registry = registry();
        assert!(registry
            .check_allowed("AA:BB", &services::HEART_RATE)
            .is_ok());
        let refused = registry.check_allowed("AA:BB", &services::BATTERY_SERVICE);
        assert!(
            matches!(refused, Err(Error::Security(_))),
            "got {refused:?}"
        );
    }

    #[test]
    fn handles_go_stale_when_the_generation_moves() {
        let registry = registry();
        assert!(registry.check_generation("AA:BB", 0).is_ok());

        registry.mark_disconnected("AA:BB");

        // Both reasons now apply, and the specification orders them: being
        // disconnected is reported ahead of the handle being stale, because
        // reconnecting is what the caller has to do first either way.
        assert!(matches!(
            registry.check_generation("AA:BB", 0),
            Err(Error::Network(_))
        ));
        assert!(matches!(
            registry.check_generation("AA:BB", 1),
            Err(Error::Network(_))
        ));
        assert!(!registry.is_connected("AA:BB"));
    }

    /// Reconnecting clears the disconnection but not the staleness: handles
    /// taken before it have to be rediscovered, and say so.
    #[test]
    fn a_handle_from_before_a_reconnect_is_stale_not_disconnected() {
        let registry = registry();
        let before = registry.generation("AA:BB").unwrap();

        registry.mark_disconnected("AA:BB");
        registry.update("AA:BB", |d| d.connected = true);

        assert!(matches!(
            registry.check_generation("AA:BB", before),
            Err(Error::InvalidState(_))
        ));
        let now = registry.generation("AA:BB").unwrap();
        assert!(registry.check_generation("AA:BB", now).is_ok());
    }

    #[test]
    fn disconnecting_notifies_watchers() {
        let registry = registry();
        let mut watcher = registry.watch_disconnect("AA:BB");
        registry.mark_disconnected("AA:BB");
        assert!(block_on(futures_util::StreamExt::next(&mut watcher)).is_some());
    }

    #[test]
    fn an_unknown_device_is_an_invalid_state_not_a_panic() {
        let registry: DeviceRegistry<u32> = DeviceRegistry::default();
        assert!(matches!(
            registry.generation("nope"),
            Err(Error::InvalidState(_))
        ));
        assert!(matches!(
            registry.check_generation("nope", 0),
            Err(Error::InvalidState(_))
        ));
        assert!(registry.name("nope").is_none());
        assert!(!registry.is_connected("nope"));
    }

    #[test]
    fn the_gatt_lock_serialises_operations() {
        let registry = registry();
        block_on(async {
            let first = registry.gatt_lock("AA:BB").await.unwrap();
            // A second attempt must not be ready while the first is held.
            let second = registry.gatt_lock("AA:BB");
            let pending = futures_util::poll!(std::pin::pin!(second));
            assert!(pending.is_pending(), "the lock let two operations through");
            drop(first);
        });
    }

    #[test]
    fn devices_can_be_found_by_backend_specific_identity() {
        let registry = registry();
        // A backend looks its own payload up — a pointer, a path, an address.
        assert_eq!(registry.find(|d| d.inner == 7), Some("AA:BB".into()));
        assert_eq!(registry.find(|d| d.inner == 99), None);

        registry.update_where(|d| d.inner == 7, |d| d.name = Some("Renamed".into()));
        assert_eq!(registry.name("AA:BB").as_deref(), Some("Renamed"));
    }

    /// A second `requestDevice` for the same device must not take away what
    /// the first one granted.
    ///
    /// The specification unions: *"add the contents of
    /// `allowedDevice.allowedServices` to `grantedServiceUUIDs`"*. Replacing
    /// instead would revoke a service silently — a caller still holding it
    /// starts getting `SecurityError` for something it was given.
    #[test]
    fn a_narrower_second_grant_does_not_revoke_the_first() {
        let registry = DeviceRegistry::<()>::default();
        let battery = BluetoothUuid::from_u16(0x180F);
        let info = BluetoothUuid::from_u16(0x180A);

        registry.insert(
            "dev",
            None,
            Grant {
                services: [battery, info].into_iter().collect(),
                manufacturer_data: vec![0x004C],
            },
            false,
            (),
        );
        // The same device again, asking for less.
        registry.insert(
            "dev",
            None,
            Grant {
                services: [battery].into_iter().collect(),
                manufacturer_data: vec![],
            },
            false,
            (),
        );

        assert!(
            registry.check_allowed("dev", &info).is_ok(),
            "the first grant's service is still allowed"
        );
        assert!(registry.check_allowed("dev", &battery).is_ok());
        assert_eq!(
            registry.grant("dev").manufacturer_data,
            vec![0x004C],
            "and so is the company it was granted"
        );
    }

    /// A re-grant must not revive handles that went stale.
    ///
    /// `generation` counts how many times every handle into a device became
    /// invalid. A handle carries the value it was made at, and
    /// `check_generation` compares them. If a re-grant reset the counter, a
    /// handle captured before a disconnect would match again afterwards and be
    /// treated as live — pointing at attributes the peer is free to have
    /// rearranged while it was away.
    #[test]
    fn a_re_grant_does_not_revive_stale_handles() {
        let registry = DeviceRegistry::<()>::default();
        let grant = || Grant {
            services: [BluetoothUuid::from_u16(0x180F)].into_iter().collect(),
            manufacturer_data: vec![],
        };

        registry.insert("dev", None, grant(), true, ());
        let captured = registry.generation("dev").unwrap();
        assert!(registry.check_generation("dev", captured).is_ok());

        // The link drops: everything handed out before is now stale.
        registry.mark_disconnected("dev");

        // The device is granted again and reconnects.
        registry.insert("dev", None, grant(), true, ());
        assert_eq!(
            registry
                .check_generation("dev", captured)
                .unwrap_err()
                .to_string(),
            Error::InvalidState(
                "this GATT object is stale — the device disconnected or changed its services"
                    .into()
            )
            .to_string(),
            "the handle from before the disconnect must stay stale"
        );
        // And a handle made now is fine.
        let fresh = registry.generation("dev").unwrap();
        assert!(registry.check_generation("dev", fresh).is_ok());
    }

    #[test]
    fn forgetting_removes_the_grant_and_its_watchers() {
        let registry = registry();
        let _watcher = registry.watch_disconnect("AA:BB");
        assert!(registry.remove("AA:BB").is_some());
        assert!(registry.ids().is_empty());
        assert!(registry.remove("AA:BB").is_none());
    }
}
