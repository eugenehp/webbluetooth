//! Who may touch what.
//!
//! One radio session serves every site, so the per-origin permission model a
//! browser is defined by has to live here rather than in the library. The
//! library enforces a grant *per session*: `request_device` records the
//! services a request asked for, and `get_primary_service` refuses anything
//! else. With one session shared by every origin that grant is the **union**
//! of what every site has ever been given — correct as an outer bound, far too
//! generous as an answer to "may this page reach this service".
//!
//! So the table below is the real one, and the library's is a second fence
//! behind it. An origin gets a device only by going through the chooser, and
//! gets a service on that device only if the request that produced it named
//! the service in `filters` or `optionalServices`.
//!
//! Handles are opaque strings rather than anything the page could construct.
//! They are sequential, which is guessable — so guessing is not what they
//! defend against: every lookup checks the handle's origin against the calling
//! webview's current origin, and a guessed handle belonging to another origin
//! is refused. A page can only reach handles it was given.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use futures_util::future::{BoxFuture, FutureExt, Shared};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Manager};
use webbluetooth::uuid::BluetoothUuid;
use webbluetooth::{
    Bluetooth, BluetoothDevice, RemoteGattCharacteristic, RemoteGattDescriptor, RemoteGattService,
};

use crate::error::{JsError, Result};
use crate::permissions::{PermissionStore, Stored, StoredDevice, StoredOrigin};

/// A site, as `https://example.com` — scheme, host and non-default port.
///
/// This is the permission key, and it is always derived from the content
/// webview's *current* URL rather than from anything the page sends.
pub type Origin = String;

/// Who is asking: an origin, and the tab it is asking from.
///
/// Both matter and they answer different questions. The origin decides what
/// may be reached — permissions belong to a site and outlive any one
/// document. The tab decides what dies when: a navigation in one tab must
/// not take down another tab's connections, and Chromium draws the same line
/// with `FrameConnectedBluetoothDevices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub origin: Origin,
    /// The content webview's label.
    pub webview: String,
}

/// What one origin has been granted.
#[derive(Default)]
struct OriginGrants {
    /// Whether `requestLEScan()` has been allowed, refused, or not yet asked.
    /// Separate from `devices` because it is a different question: every
    /// advertisement in the room, rather than one device a person pointed at.
    scanning: Option<bool>,
    /// Device identifier to the services that origin may reach on it. Empty
    /// set means a device was granted with no services — legal, and it means
    /// GATT is reachable but nothing on it is.
    devices: BTreeMap<String, BTreeSet<BluetoothUuid>>,
}

struct ServiceEntry {
    origin: Origin,
    webview: String,
    device_id: String,
    service: RemoteGattService,
}

struct CharacteristicEntry {
    origin: Origin,
    webview: String,
    device_id: String,
    service_handle: String,
    characteristic: RemoteGattCharacteristic,
}

/// One device an origin holds, as the permissions manager shows it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantedDevice {
    pub id: String,
    /// From the live device if it has been adopted, else the name it had
    /// when the grant was made — a remembered grant should not show as a
    /// bare identifier just because the device is out of range.
    pub name: Option<String>,
    pub connected: bool,
    /// How many services the grant covers. The count is what answers "how
    /// much did I give away"; the UUIDs themselves are not worth showing.
    pub services: usize,
}

/// Everything one origin holds.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginSummary {
    pub origin: String,
    /// `None` for never asked, which reads differently from a remembered
    /// refusal.
    pub scanning: Option<bool>,
    pub devices: Vec<GrantedDevice>,
}

struct DescriptorEntry {
    origin: Origin,
    webview: String,
    device_id: String,
    descriptor: RemoteGattDescriptor,
}

#[derive(Default)]
struct Inner {
    grants: HashMap<Origin, OriginGrants>,
    /// Devices the chooser has handed out, by identifier. The library hands
    /// back a `BluetoothDevice` bound to the session that found it, and there
    /// is no way to rebuild one from an identifier, so they are kept.
    devices: HashMap<String, BluetoothDevice>,
    services: HashMap<String, ServiceEntry>,
    characteristics: HashMap<String, CharacteristicEntry>,
    descriptors: HashMap<String, DescriptorEntry>,
    /// Names from the permission file, for devices not yet adopted — a
    /// remembered grant should not be displayed as a bare identifier just
    /// because the device is out of range.
    remembered_names: HashMap<String, Option<String>>,
    /// Everything below is keyed by `(tab, thing)` rather than by the thing
    /// alone. Two tabs may watch the same device, and each needs its own
    /// forwarder pointed at its own webview; closing one must not silence
    /// the other.
    ///
    /// Characteristic handle to the task pumping its notifications into the
    /// page. Aborting the task is what `stopNotifications` does locally; the
    /// library is told separately.
    notifications: HashMap<(String, String), JoinHandle<()>>,
    /// Task forwarding advertisements for one device to one tab.
    advertisements: HashMap<(String, String), JoinHandle<()>>,
    /// Task forwarding disconnect events for one device to one tab.
    disconnects: HashMap<(String, String), JoinHandle<()>>,
    /// Task watching one device's service set on behalf of one tab.
    service_watchers: HashMap<(String, String), JoinHandle<()>>,
    /// Scan identifier to who started it and the task pumping it. Aborting
    /// the task drops the library's `LeScan`, which is what stops the radio.
    scans: HashMap<String, (Caller, JoinHandle<()>)>,
    /// Which tabs believe they hold a GATT connection to each device.
    ///
    /// `gatt.disconnect()` is a property of the *link*, not of the page, so
    /// one tab hanging up would drop a device another tab is still using.
    /// The radio is only told to disconnect when the last holder lets go —
    /// this is Chromium's `FrameConnectedBluetoothDevices`, one level up.
    connections: HashMap<String, BTreeSet<String>>,
    next_handle: u64,
}

/// Abort and forget every task in `map` whose key matches.
///
/// A free function rather than a method because the maps are three separate
/// fields of the same struct: borrowing them together in one array is what
/// the borrow checker will not have, and taking one at a time is clearer
/// than the alternatives anyway.
fn abort_matching(
    map: &mut HashMap<(String, String), JoinHandle<()>>,
    doomed: impl Fn(&(String, String)) -> bool,
) {
    let keys: Vec<(String, String)> = map.keys().filter(|key| doomed(key)).cloned().collect();
    for key in keys {
        if let Some(task) = map.remove(&key) {
            task.abort();
        }
    }
}

/// Everything the commands share: the radio, and who may use it.
pub struct Browser {
    /// One session for the whole browser, the way a browser has exactly one
    /// `navigator.bluetooth` — see [`Bluetooth::new`] on why a second one is
    /// a bug rather than an option.
    pub bluetooth: Bluetooth,
    inner: Mutex<Inner>,
    store: PermissionStore,
    /// Resolves once remembered devices have been adopted, or once that has
    /// been tried and failed. Commands that answer questions about grants
    /// wait on it, so a page asking `getDevices()` the instant it loads does
    /// not see an empty list that fills in a moment later.
    restored: Shared<BoxFuture<'static, ()>>,
    done: Mutex<Option<futures_channel::oneshot::Sender<()>>>,
}

impl Browser {
    /// Open the adapter and read back what earlier runs granted.
    ///
    /// Returns immediately; the radio settles on its own, and the remembered
    /// devices are adopted by [`Self::spawn_restore`] once it has.
    pub fn new(app: &AppHandle) -> Self {
        let store = PermissionStore::new(app);
        let stored = store.load();

        let mut inner = Inner::default();
        for (origin, remembered) in stored.origins {
            let grants = inner.grants.entry(origin).or_default();
            grants.scanning = remembered.scanning;
            for (id, device) in remembered.devices {
                let services: BTreeSet<BluetoothUuid> = device
                    .services
                    .iter()
                    .filter_map(|uuid| BluetoothUuid::parse(uuid).ok())
                    .collect();
                grants.devices.insert(id.clone(), services);
                inner.remembered_names.insert(id, device.name);
            }
        }

        let (tx, rx) = futures_channel::oneshot::channel();
        Self {
            // The chooser is supplied per call by `request_device_with`, so
            // that it knows which origin is asking. This default is never
            // reached.
            bluetooth: Bluetooth::new(),
            inner: Mutex::new(inner),
            store,
            restored: rx.map(|_| ()).boxed().shared(),
            done: Mutex::new(Some(tx)),
        }
    }

    /// Wait until remembered devices have been adopted.
    pub async fn ready(&self) {
        self.restored.clone().await;
    }

    /// Turn remembered identifiers back into usable devices.
    ///
    /// `adopt_device` needs neither a scan nor the device to be in range, but
    /// it does need the adapter, so this runs in the background rather than
    /// holding up the first window. A device the platform can no longer
    /// resolve is left in the store: it may simply be a machine the user has
    /// not seen since, and forgetting it would revoke a permission nobody
    /// asked to revoke.
    pub fn spawn_restore(app: &AppHandle) {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = handle.state::<Browser>();
            let wanted = state.remembered_devices();
            if !wanted.is_empty() {
                // Adopting before the radio is up fails for every device.
                let _ = state.bluetooth.availability().await;
            }
            for (id, services) in wanted {
                match state.bluetooth.adopt_device(&id, services).await {
                    Ok(device) => {
                        state.lock().devices.insert(id, device);
                    }
                    Err(error) => {
                        eprintln!("could not restore the grant for {id}: {error}");
                    }
                }
            }
            state.finish_restore();
        });
    }

    /// Release everything waiting on [`Self::ready`].
    fn finish_restore(&self) {
        let taken = self.done.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(tx) = taken {
            let _ = tx.send(());
        }
    }

    /// Every remembered device, with the union of what any origin may reach
    /// on it — which is what the library's own session grant has to be.
    fn remembered_devices(&self) -> Vec<(String, BTreeSet<BluetoothUuid>)> {
        let inner = self.lock();
        let mut union: BTreeMap<String, BTreeSet<BluetoothUuid>> = BTreeMap::new();
        for grants in inner.grants.values() {
            for (id, services) in &grants.devices {
                union
                    .entry(id.clone())
                    .or_default()
                    .extend(services.clone());
            }
        }
        union.into_iter().collect()
    }

    /// Write the current grants out.
    fn persist(&self) {
        let stored = {
            let inner = self.lock();
            let mut stored = Stored::default();
            for (origin, grants) in &inner.grants {
                let mut out = StoredOrigin {
                    scanning: grants.scanning,
                    ..StoredOrigin::default()
                };
                for (id, services) in &grants.devices {
                    out.devices.insert(
                        id.clone(),
                        StoredDevice {
                            name: inner
                                .devices
                                .get(id)
                                .and_then(|device| device.name())
                                .or_else(|| inner.remembered_names.get(id).cloned().flatten()),
                            services: services.iter().map(|u| u.to_string()).collect(),
                        },
                    );
                }
                stored.origins.insert(origin.clone(), out);
            }
            stored
        };
        self.store.save(&stored);
    }

    /// Everything remembered, for the permission manager.
    pub fn everything(&self) -> Vec<OriginSummary> {
        let inner = self.lock();
        inner
            .grants
            .iter()
            .map(|(origin, grants)| OriginSummary {
                origin: origin.clone(),
                scanning: grants.scanning,
                devices: grants
                    .devices
                    .iter()
                    .map(|(id, services)| {
                        let device = inner.devices.get(id);
                        GrantedDevice {
                            id: id.clone(),
                            name: device
                                .and_then(|d| d.name())
                                .or_else(|| inner.remembered_names.get(id).cloned().flatten()),
                            connected: device.is_some_and(|d| d.gatt().connected()),
                            services: services.len(),
                        }
                    })
                    .collect(),
            })
            .collect()
    }

    /// Drop everything one origin holds — devices and the scanning answer.
    pub fn revoke_origin(&self, origin: &str) {
        let ids: Vec<String> = self
            .lock()
            .grants
            .get(origin)
            .map(|g| g.devices.keys().cloned().collect())
            .unwrap_or_default();
        for id in ids {
            self.revoke(origin, &id);
        }
        self.lock().grants.remove(origin);
        self.persist();
    }

    /// Forget what was decided about scanning, so the next call asks again.
    pub fn clear_scanning_decision(&self, origin: &str) {
        {
            let mut inner = self.lock();
            if let Some(grants) = inner.grants.get_mut(origin) {
                grants.scanning = None;
                if grants.devices.is_empty() {
                    inner.grants.remove(origin);
                }
            }
        }
        self.persist();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding this would poison it for the rest of the run,
        // turning one failed command into a dead browser. Nothing in here can
        // panic except an allocation failure, so recovering is the honest
        // choice.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn handle(inner: &mut Inner, prefix: char) -> String {
        inner.next_handle += 1;
        format!("{prefix}{}", inner.next_handle)
    }

    // ---- grants -----------------------------------------------------------

    /// Record what the chooser just granted: this device, these services.
    ///
    /// Re-requesting the same device from the same origin widens the service
    /// set rather than replacing it, which is what a browser does — a second
    /// `requestDevice` with different `optionalServices` does not revoke the
    /// first.
    pub fn grant(&self, origin: &str, device: BluetoothDevice, services: BTreeSet<BluetoothUuid>) {
        let mut inner = self.lock();
        let id = device.id().to_string();
        inner.devices.insert(id.clone(), device);
        inner
            .grants
            .entry(origin.to_string())
            .or_default()
            .devices
            .entry(id)
            .or_default()
            .extend(services);
        drop(inner);
        self.persist();
    }

    /// The device, if this origin may use it.
    pub fn device(&self, origin: &str, device_id: &str) -> Result<BluetoothDevice> {
        let inner = self.lock();
        if !inner
            .grants
            .get(origin)
            .is_some_and(|g| g.devices.contains_key(device_id))
        {
            // Deliberately the same answer whether the device exists and is
            // another origin's, or does not exist at all. A page must not be
            // able to probe what other sites have been granted.
            return Err(JsError::security(
                "this origin has not been granted access to that device",
            ));
        }
        inner
            .devices
            .get(device_id)
            .cloned()
            .ok_or_else(|| JsError::invalid_state("that device is no longer available"))
    }

    /// The devices this origin has been granted — `getDevices()`.
    pub fn granted_devices(&self, origin: &str) -> Vec<BluetoothDevice> {
        let inner = self.lock();
        let Some(grants) = inner.grants.get(origin) else {
            return Vec::new();
        };
        grants
            .devices
            .keys()
            .filter_map(|id| inner.devices.get(id).cloned())
            .collect()
    }

    /// Whether this origin may reach this service on this device.
    pub fn may_use_service(&self, origin: &str, device_id: &str, uuid: &BluetoothUuid) -> bool {
        self.lock()
            .grants
            .get(origin)
            .and_then(|g| g.devices.get(device_id))
            .is_some_and(|services| services.contains(uuid))
    }

    /// The services this origin may reach on this device.
    pub fn allowed_services(&self, origin: &str, device_id: &str) -> BTreeSet<BluetoothUuid> {
        self.lock()
            .grants
            .get(origin)
            .and_then(|g| g.devices.get(device_id))
            .cloned()
            .unwrap_or_default()
    }

    /// Drop this origin's grant on a device — `BluetoothDevice.forget()`, and
    /// the toolbar's revoke control.
    ///
    /// The library's own grant is left alone unless no origin holds one any
    /// more: `forget` there is session-wide, and one site giving a device up
    /// must not take it away from another.
    pub fn revoke(&self, origin: &str, device_id: &str) {
        let mut inner = self.lock();
        if let Some(grants) = inner.grants.get_mut(origin) {
            grants.devices.remove(device_id);
            // `scanning` is a separate permission and outlives the devices;
            // only drop the origin entirely when it holds nothing at all.
            if grants.devices.is_empty() && grants.scanning.is_none() {
                inner.grants.remove(origin);
            }
        }
        // Which tabs were using it under this origin, before the handles go.
        let tabs: BTreeSet<String> = inner
            .services
            .values()
            .filter(|e| e.origin == origin && e.device_id == device_id)
            .map(|e| e.webview.clone())
            .collect();
        self.drop_handles_for(&mut inner, |o, d| o == origin && d == device_id);
        for tab in tabs {
            Self::release_connection(&mut inner, &tab, device_id);
        }

        let still_granted = inner
            .grants
            .values()
            .any(|g| g.devices.contains_key(device_id));
        if !still_granted {
            let gone = |(_, device): &(String, String)| device == device_id;
            abort_matching(&mut inner.advertisements, gone);
            abort_matching(&mut inner.disconnects, gone);
            abort_matching(&mut inner.service_watchers, gone);
            inner.connections.remove(device_id);
            inner.remembered_names.remove(device_id);
            if let Some(device) = inner.devices.remove(device_id) {
                device.gatt().disconnect();
                device.forget();
            }
        }
        drop(inner);
        self.persist();
    }

    /// Everything one origin holds, for the toolbar's permission display.
    pub fn origin_devices(&self, origin: &str) -> Vec<(String, Option<String>, bool)> {
        let inner = self.lock();
        let Some(grants) = inner.grants.get(origin) else {
            return Vec::new();
        };
        grants
            .devices
            .keys()
            .filter_map(|id| {
                let device = inner.devices.get(id)?;
                Some((id.clone(), device.name(), device.gatt().connected()))
            })
            .collect()
    }

    /// What this origin has already been told about scanning, if anything.
    pub fn scanning_decision(&self, origin: &str) -> Option<bool> {
        self.lock()
            .grants
            .get(origin)
            .and_then(|grants| grants.scanning)
    }

    /// Remember what the user said about scanning for this origin.
    pub fn set_scanning_decision(&self, origin: &str, allowed: bool) {
        self.lock()
            .grants
            .entry(origin.to_string())
            .or_default()
            .scanning = Some(allowed);
        self.persist();
    }

    /// Claim the right to watch one device's service set.
    pub fn set_service_watcher(&self, who: &Caller, device_id: &str, task: JoinHandle<()>) -> bool {
        let mut inner = self.lock();
        let key = (who.webview.clone(), device_id.to_string());
        if inner.service_watchers.contains_key(&key) {
            task.abort();
            return false;
        }
        inner.service_watchers.insert(key, task);
        true
    }

    /// Throw away every handle into one device.
    ///
    /// What a changed service set means: the peer re-advertised a different
    /// tree and the old handles address nothing. Holding them would turn a
    /// clear `InvalidStateError` into a read of whatever now sits at that
    /// position.
    pub fn invalidate_device(&self, device_id: &str) {
        let mut inner = self.lock();
        let doomed = device_id.to_string();
        self.drop_handles_for(&mut inner, |_, device| device == doomed);
    }

    /// Register a running scan and hand back its identifier.
    pub fn insert_scan(&self, who: &Caller, task: JoinHandle<()>) -> String {
        let mut inner = self.lock();
        let id = Self::handle(&mut inner, 'l');
        inner.scans.insert(id.clone(), (who.clone(), task));
        id
    }

    /// Stop a scan this origin started. Stopping an unknown or foreign scan
    /// is not an error: `BluetoothLEScan.stop()` returns void and the end
    /// state is the same either way.
    pub fn stop_scan(&self, who: &Caller, id: &str) {
        let mut inner = self.lock();
        if inner.scans.get(id).is_some_and(|(owner, _)| owner == who) {
            if let Some((_, task)) = inner.scans.remove(id) {
                task.abort();
            }
        }
    }

    // ---- handles ----------------------------------------------------------

    /// Hand a service to the page and remember whose it is.
    pub fn insert_service(
        &self,
        who: &Caller,
        device_id: &str,
        service: RemoteGattService,
    ) -> String {
        let mut inner = self.lock();
        let handle = Self::handle(&mut inner, 's');
        inner.services.insert(
            handle.clone(),
            ServiceEntry {
                origin: who.origin.clone(),
                webview: who.webview.clone(),
                device_id: device_id.to_string(),
                service,
            },
        );
        handle
    }

    /// Resolve a service handle, refusing one that belongs to another origin.
    pub fn service(&self, who: &Caller, handle: &str) -> Result<(String, RemoteGattService)> {
        let inner = self.lock();
        let entry = inner
            .services
            .get(handle)
            .filter(|e| e.origin == who.origin && e.webview == who.webview)
            .ok_or_else(|| JsError::invalid_state("that service handle is no longer valid"))?;
        Ok((entry.device_id.clone(), entry.service.clone()))
    }

    /// Hand a characteristic to the page.
    pub fn insert_characteristic(
        &self,
        who: &Caller,
        device_id: &str,
        service_handle: &str,
        characteristic: RemoteGattCharacteristic,
    ) -> String {
        let mut inner = self.lock();
        let handle = Self::handle(&mut inner, 'c');
        inner.characteristics.insert(
            handle.clone(),
            CharacteristicEntry {
                origin: who.origin.clone(),
                webview: who.webview.clone(),
                device_id: device_id.to_string(),
                service_handle: service_handle.to_string(),
                characteristic,
            },
        );
        handle
    }

    /// Resolve a characteristic handle: `(device id, service handle, object)`.
    pub fn characteristic(
        &self,
        who: &Caller,
        handle: &str,
    ) -> Result<(String, String, RemoteGattCharacteristic)> {
        let inner = self.lock();
        let entry = inner
            .characteristics
            .get(handle)
            .filter(|e| e.origin == who.origin && e.webview == who.webview)
            .ok_or_else(|| {
                JsError::invalid_state("that characteristic handle is no longer valid")
            })?;
        Ok((
            entry.device_id.clone(),
            entry.service_handle.clone(),
            entry.characteristic.clone(),
        ))
    }

    /// Hand a descriptor to the page.
    pub fn insert_descriptor(
        &self,
        who: &Caller,
        device_id: &str,
        descriptor: RemoteGattDescriptor,
    ) -> String {
        let mut inner = self.lock();
        let handle = Self::handle(&mut inner, 'd');
        inner.descriptors.insert(
            handle.clone(),
            DescriptorEntry {
                origin: who.origin.clone(),
                webview: who.webview.clone(),
                device_id: device_id.to_string(),
                descriptor,
            },
        );
        handle
    }

    /// Resolve a descriptor handle.
    pub fn descriptor(&self, who: &Caller, handle: &str) -> Result<RemoteGattDescriptor> {
        let inner = self.lock();
        inner
            .descriptors
            .get(handle)
            .filter(|e| e.origin == who.origin && e.webview == who.webview)
            .map(|e| e.descriptor.clone())
            .ok_or_else(|| JsError::invalid_state("that descriptor handle is no longer valid"))
    }

    // ---- background tasks -------------------------------------------------

    /// Remember the task forwarding one characteristic's notifications,
    /// replacing and aborting any task already running for it.
    pub fn set_notification_task(&self, who: &Caller, handle: &str, task: JoinHandle<()>) {
        let mut inner = self.lock();
        let key = (who.webview.clone(), handle.to_string());
        if let Some(previous) = inner.notifications.insert(key, task) {
            previous.abort();
        }
    }

    /// Stop forwarding one characteristic's notifications.
    pub fn clear_notification_task(&self, who: &Caller, handle: &str) {
        let key = (who.webview.clone(), handle.to_string());
        if let Some(task) = self.lock().notifications.remove(&key) {
            task.abort();
        }
    }

    /// Remember the task forwarding one device's advertisements.
    ///
    /// Returns `false` if one was already running, in which case the caller's
    /// task is redundant — `watchAdvertisements()` twice is not an error.
    pub fn set_advertisement_task(
        &self,
        who: &Caller,
        device_id: &str,
        task: JoinHandle<()>,
    ) -> bool {
        let mut inner = self.lock();
        let key = (who.webview.clone(), device_id.to_string());
        if inner.advertisements.contains_key(&key) {
            task.abort();
            return false;
        }
        inner.advertisements.insert(key, task);
        true
    }

    /// Stop forwarding one device's advertisements.
    pub fn clear_advertisement_task(&self, who: &Caller, device_id: &str) {
        let key = (who.webview.clone(), device_id.to_string());
        if let Some(task) = self.lock().advertisements.remove(&key) {
            task.abort();
        }
    }

    /// Claim the right to forward one device's disconnect events.
    ///
    /// Returns `false` and abandons `task` if a forwarder is already running:
    /// `connect()` twice must not deliver every `gattserverdisconnected`
    /// twice.
    pub fn set_disconnect_task(&self, who: &Caller, device_id: &str, task: JoinHandle<()>) -> bool {
        let mut inner = self.lock();
        let key = (who.webview.clone(), device_id.to_string());
        if inner.disconnects.contains_key(&key) {
            task.abort();
            return false;
        }
        inner.disconnects.insert(key, task);
        true
    }

    /// Note that a tab believes it holds a connection to a device.
    pub fn note_connected(&self, who: &Caller, device_id: &str) {
        self.lock()
            .connections
            .entry(device_id.to_string())
            .or_default()
            .insert(who.webview.clone());
    }

    /// Give up one tab's claim on a device, and say whether that was the
    /// last one — which is the only point at which the radio should be told
    /// to hang up.
    fn release_connection(inner: &mut Inner, webview: &str, device_id: &str) -> bool {
        let Some(holders) = inner.connections.get_mut(device_id) else {
            return false;
        };
        holders.remove(webview);
        if holders.is_empty() {
            inner.connections.remove(device_id);
            return true;
        }
        false
    }

    /// `gatt.disconnect()` from one tab.
    ///
    /// Only reaches the radio when no other tab still holds the device. A
    /// page is entitled to believe its own `connected` went false either
    /// way; the shim sets that locally.
    pub fn disconnect(&self, who: &Caller, device_id: &str) {
        let mut inner = self.lock();
        if Self::release_connection(&mut inner, &who.webview, device_id) {
            if let Some(device) = inner.devices.get(device_id) {
                device.gatt().disconnect();
            }
        }
    }

    // ---- page lifetime ----------------------------------------------------

    /// The page went away: it navigated, reloaded, or the window closed.
    ///
    /// Everything the *document* held goes with it — handles, notification
    /// pumps, GATT connections — exactly as it would in a browser, where a
    /// `BluetoothRemoteGATTServer` does not survive a navigation. The grants
    /// do not: a permission belongs to the origin and outlives any one
    /// document, which is what stops a reload re-prompting.
    pub fn page_unloaded(&self, webview: &str) {
        let mut inner = self.lock();

        // Only this tab's. Another tab's notification pump has nothing to do
        // with this document going away, and killing it would make a
        // background tab go silent whenever a foreground one navigated.
        let mine = |key: &(String, String)| key.0 == webview;
        abort_matching(&mut inner.notifications, mine);
        abort_matching(&mut inner.advertisements, mine);
        abort_matching(&mut inner.disconnects, mine);
        abort_matching(&mut inner.service_watchers, mine);

        let scans: Vec<_> = inner
            .scans
            .iter()
            .filter(|(_, (owner, _))| owner.webview == webview)
            .map(|(id, _)| id.clone())
            .collect();
        for id in scans {
            if let Some((_, task)) = inner.scans.remove(&id) {
                task.abort();
            }
        }

        inner.services.retain(|_, e| e.webview != webview);
        inner.characteristics.retain(|_, e| e.webview != webview);
        inner.descriptors.retain(|_, e| e.webview != webview);

        // Hang up only what nobody else is holding.
        let held: Vec<String> = inner
            .connections
            .iter()
            .filter(|(_, holders)| holders.contains(webview))
            .map(|(device, _)| device.clone())
            .collect();
        for device_id in held {
            if Self::release_connection(&mut inner, webview, &device_id) {
                if let Some(device) = inner.devices.get(&device_id) {
                    device.gatt().disconnect();
                }
            }
        }
    }

    /// Forget every handle matching `doomed`, and stop anything pumping into
    /// one of them. A live notification task holding a dropped characteristic
    /// would otherwise keep delivering events for a device the page can no
    /// longer name.
    fn drop_handles_for(&self, inner: &mut Inner, doomed: impl Fn(&str, &str) -> bool) {
        inner
            .services
            .retain(|_, e| !doomed(&e.origin, &e.device_id));
        inner
            .descriptors
            .retain(|_, e| !doomed(&e.origin, &e.device_id));

        let mut stopped = Vec::new();
        inner.characteristics.retain(|handle, e| {
            let keep = !doomed(&e.origin, &e.device_id);
            if !keep {
                stopped.push((e.webview.clone(), handle.clone()));
            }
            keep
        });
        for key in stopped {
            if let Some(task) = inner.notifications.remove(&key) {
                task.abort();
            }
        }
    }
}
