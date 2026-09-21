//! The GATT tree: server, service, characteristic, descriptor.
//!
//! Each type is a handle into a device's attribute table, valid only while the
//! connection that produced it lasts. A handle records the generation it was
//! minted in; a disconnect or a `didModifyServices:` bumps that generation and
//! every stale handle then fails rather than messaging a freed `CBService`.
//! Which failure depends on what went wrong: `NetworkError` while the device
//! is still disconnected, and `InvalidStateError` once it is back, because
//! reconnecting fixes the first and only rediscovery fixes the second.

use crate::backend::attribute_uuid;
use crate::backend::Handle;
use crate::blocklist;
use crate::error::{Error, Result};
use crate::uuid::{BluetoothUuid, IntoUuid};
use futures_core::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
pub use webbluetooth_core::gatt::{CharacteristicProperties, WriteType};

/// The largest value the spec permits in a single write.
const MAX_ATTRIBUTE_LENGTH: usize = 512;

/// `BluetoothRemoteGATTServer`.
#[derive(Clone)]
pub struct RemoteGattServer {
    pub(crate) inner: Arc<crate::session::Session>,
    pub(crate) id: String,
}

impl RemoteGattServer {
    /// Watch for the service set changing — the specification's
    /// `serviceadded`, `servicechanged` and `serviceremoved`.
    ///
    /// Yields once per change. Every handle obtained before the change is
    /// stale afterwards — `Network` until the device is back, `InvalidState`
    /// after that — so a caller should re-discover rather than retry.
    ///
    /// One stream rather than three events: the platforms report *that* the
    /// set changed, not which service, so splitting it would mean inventing
    /// detail. Reported where the platform says so — CoreBluetooth's
    /// `didModifyServices:` and Android's `onServiceChanged`.
    pub fn watch_services_changed(&self) -> crate::ServiceEvents {
        crate::ServiceEvents {
            rx: self.inner.watch_services_changed(&self.id),
        }
    }

    /// The device this server belongs to — `BluetoothRemoteGATTServer.device`.
    pub fn device(&self) -> crate::BluetoothDevice {
        crate::BluetoothDevice {
            inner: self.inner.clone(),
            id: self.id.clone(),
        }
    }

    /// Whether the link is currently up.
    pub fn connected(&self) -> bool {
        self.inner.is_connected(&self.id)
    }

    /// Open a connection. Resolves once CoreBluetooth reports the link is up.
    ///
    /// Unlike a socket connect there is no built-in timeout: CoreBluetooth will
    /// wait indefinitely for a peripheral to come into range. Wrap this in
    /// [`crate::timeout`] if that is not what you want.
    /// # Example
    ///
    /// ```no_run
    /// # use webbluetooth::uuid::services;
    /// # async fn example(device: webbluetooth::BluetoothDevice) -> webbluetooth::Result<()> {
    /// let gatt = device.gatt();
    /// gatt.connect().await?;
    ///
    /// let service = gatt.get_primary_service(services::BATTERY_SERVICE).await?;
    /// # let _ = service;
    /// # Ok(()) }
    /// ```
    pub async fn connect(&self) -> Result<()> {
        self.inner.backend().connect(&self.id).await
    }

    /// Close the connection. Every handle into this device becomes stale.
    pub fn disconnect(&self) {
        self.inner.disconnect(&self.id);
    }

    /// The primary service with this UUID.
    ///
    /// `SecurityError` if the service was not listed in the `filters` or
    /// `optional_services` of the request that produced this device — the same
    /// rule a browser applies.
    pub async fn get_primary_service(&self, uuid: impl IntoUuid) -> Result<RemoteGattService> {
        let uuid = uuid.into_uuid()?;
        self.guard_service(&uuid)?;
        let found = self.inner.discover_services(&self.id, Some(&uuid)).await?;
        let handle = found
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("no primary service {uuid} on this device")))?;
        self.service_from(handle, uuid)
    }

    /// Every primary service, or every one matching `uuid`.
    ///
    /// Services outside the grant are omitted rather than reported — a caller
    /// must not learn what else the device offers.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example(gatt: webbluetooth::RemoteGattServer) -> webbluetooth::Result<()> {
    /// // Every service this device granted access to. Passing a UUID narrows
    /// // it to that one; `None` is "all of them", which is still only the
    /// // ones the request asked for.
    /// for service in gatt.get_primary_services(None).await? {
    ///     println!("{}", service.uuid());
    /// }
    /// # Ok(()) }
    /// ```
    /// `None` means every one of them. Anything [`IntoUuid`] accepts narrows
    /// it — bare, without a `Some`: `get_primary_services("heart_rate")`.
    /// `Some(services::HEART_RATE)` works too, because an assigned number is
    /// a [`BluetoothUuid`].
    ///
    pub async fn get_primary_services(
        &self,
        uuid: impl webbluetooth_core::uuid::IntoOptionalUuid,
    ) -> Result<Vec<RemoteGattService>> {
        let uuid = uuid.into_optional_uuid()?;
        if let Some(u) = &uuid {
            self.guard_service(u)?;
        }
        let allowed = self.inner.allowed_services(&self.id)?;
        let found = self
            .inner
            .discover_services(&self.id, uuid.as_ref())
            .await?;

        let mut out = Vec::new();
        for handle in found {
            let Ok(u) = attribute_uuid(&handle) else {
                continue;
            };
            if !allowed.contains(&u) || blocklist::is_blocked(&u) {
                continue;
            }
            out.push(self.service_from(handle, u)?);
        }
        if out.is_empty() {
            return Err(Error::NotFound(
                "no accessible primary services on this device".into(),
            ));
        }
        Ok(out)
    }

    fn guard_service(&self, uuid: &BluetoothUuid) -> Result<()> {
        if blocklist::is_blocked(uuid) {
            return Err(Error::Security(format!(
                "service {uuid} is on the GATT blocklist"
            )));
        }
        self.inner.check_allowed(&self.id, uuid)
    }

    fn service_from(&self, handle: Handle, uuid: BluetoothUuid) -> Result<RemoteGattService> {
        Ok(RemoteGattService {
            is_primary: self.inner.service_is_primary(&handle),
            inner: self.inner.clone(),
            device_id: self.id.clone(),
            generation: self.inner.generation(&self.id)?,
            handle,
            uuid,
        })
    }
}

/// `BluetoothRemoteGATTService`.
#[derive(Clone)]
pub struct RemoteGattService {
    inner: Arc<crate::session::Session>,
    device_id: String,

    handle: Handle,
    uuid: BluetoothUuid,
    generation: u64,
    is_primary: bool,
}

impl RemoteGattService {
    /// This service's UUID — `BluetoothRemoteGATTService.uuid`.
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.uuid
    }

    /// The device this service is on — `BluetoothRemoteGATTService.device`.
    pub fn device(&self) -> crate::BluetoothDevice {
        crate::BluetoothDevice {
            inner: self.inner.clone(),
            id: self.device_id.clone(),
        }
    }

    /// Whether this is a primary service rather than an included one.
    pub fn is_primary(&self) -> bool {
        self.is_primary
    }

    /// The characteristic with this UUID.
    pub async fn get_characteristic(
        &self,
        uuid: impl IntoUuid,
    ) -> Result<RemoteGattCharacteristic> {
        let uuid = uuid.into_uuid()?;
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        if blocklist::is_blocked(&uuid) {
            return Err(Error::Security(format!(
                "characteristic {uuid} is on the GATT blocklist"
            )));
        }
        let found = self
            .inner
            .discover_characteristics(&self.device_id, &self.handle, Some(&uuid))
            .await?;
        let handle = found.into_iter().next().ok_or_else(|| {
            Error::NotFound(format!("no characteristic {uuid} in service {}", self.uuid))
        })?;
        self.characteristic_from(handle, uuid)
    }

    /// Every characteristic, or every one matching `uuid`.
    /// `None` means every one of them. Anything [`IntoUuid`] accepts narrows
    /// it — bare, without a `Some`: `get_primary_services("heart_rate")`.
    /// `Some(services::HEART_RATE)` works too, because an assigned number is
    /// a [`BluetoothUuid`].
    ///
    pub async fn get_characteristics(
        &self,
        uuid: impl webbluetooth_core::uuid::IntoOptionalUuid,
    ) -> Result<Vec<RemoteGattCharacteristic>> {
        let uuid = uuid.into_optional_uuid()?;
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        let found = self
            .inner
            .discover_characteristics(&self.device_id, &self.handle, uuid.as_ref())
            .await?;
        let mut out = Vec::new();
        for handle in found {
            let Ok(u) = attribute_uuid(&handle) else {
                continue;
            };
            if blocklist::is_blocked(&u) {
                continue;
            }
            out.push(self.characteristic_from(handle, u)?);
        }
        if out.is_empty() {
            return Err(Error::NotFound(format!(
                "no accessible characteristics in service {}",
                self.uuid
            )));
        }
        Ok(out)
    }

    /// The included service with this UUID — `getIncludedService()`.
    ///
    /// An included service is one a primary service points at with an Include
    /// declaration, so that a composite service can reuse a definition rather
    /// than restate it. The access rules are the same as for a primary
    /// service: the allowlist decides, and the blocklist overrides it.
    pub async fn get_included_service(&self, uuid: impl IntoUuid) -> Result<RemoteGattService> {
        let uuid = uuid.into_uuid()?;
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        self.guard_included(&uuid)?;
        let found = self
            .inner
            .discover_included_services(&self.device_id, &self.handle, Some(&uuid))
            .await?;
        let handle = found.into_iter().next().ok_or_else(|| {
            Error::NotFound(format!(
                "no included service {uuid} in service {}",
                self.uuid
            ))
        })?;
        self.included_from(handle, uuid)
    }

    /// Every included service, or every one matching `uuid`.
    ///
    /// Services outside the grant are omitted rather than reported, exactly as
    /// for primary services — being reachable through an Include declaration
    /// does not widen what a caller was granted.
    /// `None` means every one of them. Anything [`IntoUuid`] accepts narrows
    /// it — bare, without a `Some`: `get_primary_services("heart_rate")`.
    /// `Some(services::HEART_RATE)` works too, because an assigned number is
    /// a [`BluetoothUuid`].
    ///
    pub async fn get_included_services(
        &self,
        uuid: impl webbluetooth_core::uuid::IntoOptionalUuid,
    ) -> Result<Vec<RemoteGattService>> {
        let uuid = uuid.into_optional_uuid()?;
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        if let Some(u) = &uuid {
            self.guard_included(u)?;
        }
        let allowed = self.inner.allowed_services(&self.device_id)?;
        let found = self
            .inner
            .discover_included_services(&self.device_id, &self.handle, uuid.as_ref())
            .await?;

        let mut out = Vec::new();
        for handle in found {
            let Ok(u) = attribute_uuid(&handle) else {
                continue;
            };
            if !allowed.contains(&u) || blocklist::is_blocked(&u) {
                continue;
            }
            out.push(self.included_from(handle, u)?);
        }
        if out.is_empty() {
            return Err(Error::NotFound(format!(
                "no accessible included services in service {}",
                self.uuid
            )));
        }
        Ok(out)
    }

    fn guard_included(&self, uuid: &BluetoothUuid) -> Result<()> {
        if blocklist::is_blocked(uuid) {
            return Err(Error::Security(format!(
                "service {uuid} is on the GATT blocklist"
            )));
        }
        self.inner.check_allowed(&self.device_id, uuid)
    }

    fn included_from(&self, handle: Handle, uuid: BluetoothUuid) -> Result<RemoteGattService> {
        Ok(RemoteGattService {
            // An included service is by definition not primary, whatever the
            // platform reports for the handle.
            is_primary: false,
            inner: self.inner.clone(),
            device_id: self.device_id.clone(),
            generation: self.generation,
            handle,
            uuid,
        })
    }

    fn characteristic_from(
        &self,
        handle: Handle,
        uuid: BluetoothUuid,
    ) -> Result<RemoteGattCharacteristic> {
        Ok(RemoteGattCharacteristic {
            properties: self.inner.characteristic_properties(&handle),
            inner: self.inner.clone(),
            device_id: self.device_id.clone(),
            service_uuid: self.uuid,
            service_handle: self.handle.clone(),
            generation: self.generation,
            handle,
            uuid,
        })
    }
}

impl std::fmt::Debug for RemoteGattService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteGattService")
            .field("uuid", &self.uuid.as_str())
            .field("is_primary", &self.is_primary)
            .finish()
    }
}

/// `BluetoothRemoteGATTCharacteristic`.
#[derive(Clone)]
pub struct RemoteGattCharacteristic {
    inner: Arc<crate::session::Session>,
    device_id: String,
    service_uuid: BluetoothUuid,
    /// The service's own handle, so [`Self::service`] can hand back the
    /// service rather than just its UUID — which is what the specification's
    /// `.service` is.
    service_handle: Handle,
    handle: Handle,
    uuid: BluetoothUuid,
    generation: u64,
    properties: CharacteristicProperties,
}

impl RemoteGattCharacteristic {
    /// The service this characteristic belongs to —
    /// `BluetoothRemoteGATTCharacteristic.service`.
    pub fn service(&self) -> RemoteGattService {
        RemoteGattService {
            is_primary: self.inner.service_is_primary(&self.service_handle),
            inner: self.inner.clone(),
            device_id: self.device_id.clone(),
            generation: self.generation,
            handle: self.service_handle.clone(),
            uuid: self.service_uuid,
        }
    }

    /// This characteristic's UUID — `BluetoothRemoteGATTCharacteristic.uuid`.
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.uuid
    }

    /// The UUID of the service this characteristic belongs to.
    pub fn service_uuid(&self) -> &BluetoothUuid {
        &self.service_uuid
    }

    /// What the peer says this characteristic supports.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example(c: webbluetooth::RemoteGattCharacteristic) -> webbluetooth::Result<()> {
    /// if c.properties().notify() {
    ///     let _stream = c.start_notifications().await?;
    /// } else if c.properties().read() {
    ///     let _value = c.read_value().await?;
    /// }
    /// # Ok(()) }
    /// ```
    pub fn properties(&self) -> CharacteristicProperties {
        self.properties
    }

    /// The last value read or notified, without going to the device.
    pub fn value(&self) -> Option<Vec<u8>> {
        self.inner.cached_value(&self.handle)
    }

    /// Read the current value from the device.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use webbluetooth::uuid::characteristics;
    /// # async fn example(service: webbluetooth::RemoteGattService) -> webbluetooth::Result<()> {
    /// let level = service.get_characteristic(characteristics::BATTERY_LEVEL).await?;
    /// let value = level.read_value().await?;
    /// println!("{}%", value[0]);
    /// # Ok(()) }
    /// ```
    pub async fn read_value(&self) -> Result<Vec<u8>> {
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        if blocklist::reads_blocked(&self.uuid) {
            return Err(Error::Security(format!(
                "reading {} is blocklisted",
                self.uuid
            )));
        }
        if !self.properties.read() {
            return Err(Error::NotSupported(format!(
                "characteristic {} does not support reading",
                self.uuid
            )));
        }
        self.inner
            .read_characteristic(&self.device_id, &self.handle)
            .await
    }

    /// Write and wait for the peer to acknowledge —
    /// `writeValueWithResponse()`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example(c: webbluetooth::RemoteGattCharacteristic) -> webbluetooth::Result<()> {
    /// // Fails if the peer refuses it. The unacknowledged form would not.
    /// c.write_value_with_response(&[0x01]).await?;
    /// # Ok(()) }
    /// ```
    pub async fn write_value_with_response(&self, value: &[u8]) -> Result<()> {
        self.guard_write(value)?;
        if !self.properties.write() {
            return Err(Error::NotSupported(format!(
                "characteristic {} does not support acknowledged writes",
                self.uuid
            )));
        }
        self.inner
            .write_characteristic(
                &self.device_id,
                &self.handle,
                value,
                WriteType::WithResponse,
            )
            .await
    }

    /// Write without waiting for acknowledgement —
    /// `writeValueWithoutResponse()`.
    ///
    /// Returns as soon as the write is handed to CoreBluetooth. There is no
    /// delivery confirmation and no flow control: a burst can be dropped
    /// silently. Check [`RemoteGattCharacteristic::max_write_length`] and pace
    /// large transfers.
    pub async fn write_value_without_response(&self, value: &[u8]) -> Result<()> {
        self.guard_write(value)?;
        if !self.properties.write_without_response() {
            return Err(Error::NotSupported(format!(
                "characteristic {} does not support unacknowledged writes",
                self.uuid
            )));
        }
        self.inner
            .write_characteristic(
                &self.device_id,
                &self.handle,
                value,
                WriteType::WithoutResponse,
            )
            .await
    }

    /// The most bytes one write can carry, given the negotiated ATT MTU.
    pub fn max_write_length(&self, with_response: bool) -> Result<usize> {
        let t = if with_response {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        self.inner.max_write_len(&self.device_id, t)
    }

    /// Subscribe to notifications or indications.
    ///
    /// Returns a stream of values. The stream ends when the device disconnects.
    /// Dropping it does not stop the subscription on the peer — call
    /// [`RemoteGattCharacteristic::stop_notifications`] for that.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use futures_util::StreamExt;
    /// # use webbluetooth::uuid::characteristics;
    /// # async fn example(service: webbluetooth::RemoteGattService) -> webbluetooth::Result<()> {
    /// let measurement = service
    ///     .get_characteristic(characteristics::HEART_RATE_MEASUREMENT)
    ///     .await?;
    ///
    /// let mut beats = measurement.start_notifications().await?;
    /// while let Some(value) = beats.next().await {
    ///     println!("{} bpm", value[1]);
    /// }
    /// // Dropping the stream is what stops the subscription.
    /// # Ok(()) }
    /// ```
    pub async fn start_notifications(&self) -> Result<Notifications> {
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        if !self.properties.notify() && !self.properties.indicate() {
            return Err(Error::NotSupported(format!(
                "characteristic {} supports neither notifications nor indications",
                self.uuid
            )));
        }
        // Subscribe before enabling, so a notification that races the
        // acknowledgement is not lost.
        let rx = self.inner.subscribe(&self.handle);
        self.inner
            .set_notify(&self.device_id, &self.handle, true)
            .await?;
        Ok(Notifications {
            rx,
            characteristic: self.clone(),
        })
    }

    /// Stop notifications on the peer.
    pub async fn stop_notifications(&self) -> Result<()> {
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        self.inner
            .set_notify(&self.device_id, &self.handle, false)
            .await?;
        Ok(())
    }

    /// Whether the peer currently has notifications enabled.
    pub fn is_notifying(&self) -> bool {
        self.inner.is_notifying(&self.handle)
    }

    /// The descriptor with this UUID.
    pub async fn get_descriptor(&self, uuid: impl IntoUuid) -> Result<RemoteGattDescriptor> {
        let uuid = uuid.into_uuid()?;
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        let found = self
            .inner
            .discover_descriptors(&self.device_id, &self.handle, Some(&uuid))
            .await?;
        let handle = found.into_iter().next().ok_or_else(|| {
            Error::NotFound(format!(
                "no descriptor {uuid} on characteristic {}",
                self.uuid
            ))
        })?;
        Ok(self.descriptor_from(handle, uuid))
    }

    /// Every descriptor on this characteristic.
    pub async fn get_descriptors(&self) -> Result<Vec<RemoteGattDescriptor>> {
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        let found = self
            .inner
            .discover_descriptors(&self.device_id, &self.handle, None)
            .await?;
        Ok(found
            .into_iter()
            .filter_map(|h| {
                let u = attribute_uuid(&h).ok()?;
                Some(self.descriptor_from(h, u))
            })
            .collect())
    }

    fn descriptor_from(&self, handle: Handle, uuid: BluetoothUuid) -> RemoteGattDescriptor {
        RemoteGattDescriptor {
            inner: self.inner.clone(),
            device_id: self.device_id.clone(),
            characteristic_uuid: self.uuid,
            characteristic_handle: self.handle.clone(),
            service_uuid: self.service_uuid,
            service_handle: self.service_handle.clone(),
            generation: self.generation,
            handle,
            uuid,
        }
    }

    fn guard_write(&self, value: &[u8]) -> Result<()> {
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        if blocklist::writes_blocked(&self.uuid) {
            return Err(Error::Security(format!(
                "writing {} is blocklisted",
                self.uuid
            )));
        }
        if value.len() > MAX_ATTRIBUTE_LENGTH {
            return Err(Error::InvalidModification(format!(
                "value is {} bytes; a GATT attribute holds at most {MAX_ATTRIBUTE_LENGTH}",
                value.len()
            )));
        }
        Ok(())
    }
}

impl std::fmt::Debug for RemoteGattCharacteristic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteGattCharacteristic")
            .field("uuid", &self.uuid.as_str())
            .field("properties", &self.properties)
            .finish()
    }
}

/// `BluetoothRemoteGATTDescriptor`.
#[derive(Clone)]
pub struct RemoteGattDescriptor {
    inner: Arc<crate::session::Session>,
    device_id: String,
    characteristic_uuid: BluetoothUuid,
    /// Enough of the parent to rebuild it for [`Self::characteristic`].
    characteristic_handle: Handle,
    service_uuid: BluetoothUuid,
    service_handle: Handle,
    handle: Handle,
    uuid: BluetoothUuid,
    generation: u64,
}

impl RemoteGattDescriptor {
    /// The characteristic this descriptor belongs to —
    /// `BluetoothRemoteGATTDescriptor.characteristic`.
    pub fn characteristic(&self) -> RemoteGattCharacteristic {
        RemoteGattCharacteristic {
            properties: self
                .inner
                .characteristic_properties(&self.characteristic_handle),
            inner: self.inner.clone(),
            device_id: self.device_id.clone(),
            service_uuid: self.service_uuid,
            service_handle: self.service_handle.clone(),
            generation: self.generation,
            handle: self.characteristic_handle.clone(),
            uuid: self.characteristic_uuid,
        }
    }

    /// This descriptor's UUID — `BluetoothRemoteGATTDescriptor.uuid`.
    pub fn uuid(&self) -> &BluetoothUuid {
        &self.uuid
    }

    /// The UUID of the characteristic this descriptor belongs to.
    pub fn characteristic_uuid(&self) -> &BluetoothUuid {
        &self.characteristic_uuid
    }

    /// Read the descriptor's value.
    pub async fn read_value(&self) -> Result<Vec<u8>> {
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        if blocklist::reads_blocked(&self.uuid) {
            return Err(Error::Security(format!(
                "reading {} is blocklisted",
                self.uuid
            )));
        }
        self.inner
            .read_descriptor(&self.device_id, &self.handle)
            .await
    }

    /// Write the descriptor's value.
    ///
    /// Note that the Client Characteristic Configuration descriptor is
    /// write-blocklisted: enabling notifications by writing it directly would
    /// bypass [`RemoteGattCharacteristic::start_notifications`], so use that
    /// instead.
    pub async fn write_value(&self, value: &[u8]) -> Result<()> {
        self.inner
            .check_generation(&self.device_id, self.generation)?;
        if blocklist::writes_blocked(&self.uuid) {
            return Err(Error::Security(format!(
                "writing {} is blocklisted — use start_notifications() to subscribe",
                self.uuid
            )));
        }
        if value.len() > MAX_ATTRIBUTE_LENGTH {
            return Err(Error::InvalidModification(format!(
                "value is {} bytes; a GATT attribute holds at most {MAX_ATTRIBUTE_LENGTH}",
                value.len()
            )));
        }
        self.inner
            .write_descriptor(&self.device_id, &self.handle, value)
            .await
    }
}

impl std::fmt::Debug for RemoteGattDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteGattDescriptor")
            .field("uuid", &self.uuid.as_str())
            .finish()
    }
}

/// A stream of characteristic values, from
/// [`RemoteGattCharacteristic::start_notifications`].
pub struct Notifications {
    rx: crate::backlog::Receiver<Vec<u8>>,
    /// Where the values come from.
    ///
    /// The specification delivers these as events, and an event carries its
    /// `target` — so in JavaScript `event.target.uuid` is always at hand and
    /// a listener over several characteristics can tell them apart. A bare
    /// stream of `Vec<u8>` lost that, which is only invisible on a device with
    /// one interesting characteristic.
    characteristic: RemoteGattCharacteristic,
}

impl Notifications {
    /// The characteristic these values come from — the specification's
    /// `event.target`.
    pub fn characteristic(&self) -> &RemoteGattCharacteristic {
        &self.characteristic
    }

    /// That characteristic's UUID.
    ///
    /// The reason to reach for this is usually a merged stream: several
    /// subscriptions selected together deliver values that no longer say what
    /// they are, and — since [`lost`](Self::lost) is per subscription — a gap
    /// could not be attributed to a characteristic either.
    pub fn uuid(&self) -> &BluetoothUuid {
        self.characteristic.uuid()
    }

    /// Pair every value with the UUID it came from.
    ///
    /// For merging subscriptions: tag each one, then select over them, and the
    /// result is a single stream that still says which characteristic each
    /// value belongs to.
    ///
    /// ```no_run
    /// # use futures_util::{stream::select_all, StreamExt};
    /// # async fn example(
    /// #     eeg: webbluetooth::RemoteGattCharacteristic,
    /// #     ppg: webbluetooth::RemoteGattCharacteristic,
    /// # ) -> webbluetooth::Result<()> {
    /// let mut sensors = select_all([
    ///     eeg.start_notifications().await?.tagged().boxed(),
    ///     ppg.start_notifications().await?.tagged().boxed(),
    /// ]);
    ///
    /// while let Some((uuid, value)) = sensors.next().await {
    ///     println!("{uuid}: {} bytes", value.len());
    /// }
    /// # Ok(()) }
    /// ```
    pub fn tagged(self) -> Tagged {
        Tagged { inner: self }
    }

    /// How many notifications have been dropped because this stream was not
    /// being read fast enough.
    ///
    /// A peer can notify once per connection interval — every 7.5 ms at the
    /// fastest — and its callback thread cannot be made to wait, so a consumer
    /// that falls behind has to lose something. The queue keeps the newest and
    /// counts the rest here.
    ///
    /// Cumulative, so two readings can be compared: if this has moved between
    /// one value and the next, the difference is exactly the size of the gap
    /// between them. Anything reassembling a sequence — a firmware image, a
    /// log download — should check it rather than trust the bytes to be
    /// contiguous.
    ///
    /// ```no_run
    /// # async fn example(mut notifications: webbluetooth::Notifications) {
    /// use futures_util::StreamExt;
    /// let mut seen = notifications.lost();
    /// while let Some(value) = notifications.next().await {
    ///     if notifications.lost() != seen {
    ///         seen = notifications.lost();
    ///         // A gap: start the transfer again rather than stitch it.
    ///     }
    ///     let _ = value;
    /// }
    /// # }
    /// ```
    pub fn lost(&self) -> u64 {
        self.rx.lost()
    }

    /// How many notifications are queued and unread.
    ///
    /// Sitting near the queue's capacity means a consumer that is not keeping
    /// up — visible before [`lost`](Self::lost) starts moving.
    pub fn depth(&self) -> usize {
        self.rx.depth()
    }
}

/// [`Notifications`] with each value paired with the UUID it came from —
/// [`Notifications::tagged`].
///
/// A named type rather than `impl Stream` on purpose. Tagging exists so that
/// several subscriptions can be merged and still say what each value is, and
/// merging is what takes the stream by value: `select_all` owns what it is
/// given and hands back only what the item type carries. An opaque return type
/// would have taken [`lost`](Self::lost) with it, so the merged set could
/// report that notifications had been dropped but not which characteristic
/// dropped them — which is the position the bare `Vec<u8>` stream was in, one
/// step along.
///
/// `SelectAll::iter()` reaches these, so a merged set stays diagnosable.
pub struct Tagged {
    inner: Notifications,
}

impl Tagged {
    /// The characteristic these values come from.
    pub fn characteristic(&self) -> &RemoteGattCharacteristic {
        self.inner.characteristic()
    }

    /// That characteristic's UUID — the same one each item carries.
    pub fn uuid(&self) -> &BluetoothUuid {
        self.inner.uuid()
    }

    /// How many notifications this subscription has dropped.
    ///
    /// See [`Notifications::lost`]. Per subscription, which is why it is worth
    /// having beside the UUID.
    pub fn lost(&self) -> u64 {
        self.inner.lost()
    }

    /// How many are queued and unread on this subscription.
    pub fn depth(&self) -> usize {
        self.inner.depth()
    }
}

impl Stream for Tagged {
    type Item = (BluetoothUuid, Vec<u8>);

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Copied before the stream is borrowed mutably; a `BluetoothUuid` is
        // 36 bytes of ASCII and `Copy`, so this is not worth avoiding.
        let uuid = *self.inner.uuid();
        Pin::new(&mut self.inner)
            .poll_next(cx)
            .map(|value| value.map(|value| (uuid, value)))
    }
}

impl std::fmt::Debug for Tagged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tagged")
            .field("uuid", &self.uuid().as_str())
            .field("lost", &self.lost())
            .finish_non_exhaustive()
    }
}

impl Stream for Notifications {
    type Item = Vec<u8>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

// The other handles in this file print their identity; these two were simply
// missed, which is visible the moment a caller puts one in a struct they want
// to `#[derive(Debug)]`.
impl std::fmt::Debug for RemoteGattServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteGattServer")
            .field("device", &self.id)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Notifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifications")
            .field("uuid", &self.uuid().as_str())
            .field("lost", &self.lost())
            .finish_non_exhaustive()
    }
}
