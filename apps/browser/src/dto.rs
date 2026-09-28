//! The wire between the injected shim and the bridge.
//!
//! Field names are the specification's, so that a `RequestDeviceOptions`
//! dictionary can be handed over almost as the page wrote it. The shim does
//! one transformation first: every UUID argument is resolved to its canonical
//! 128-bit lowercase form in JavaScript, because `BluetoothUUID.getService`
//! and friends are synchronous and namespaced — `current_time` is a different
//! UUID depending on whether a service or a characteristic was asked for, and
//! only the call site knows which.

use serde::{Deserialize, Serialize};
use webbluetooth::filter::Advertisement;
use webbluetooth::CharacteristicProperties;

// ---- in ------------------------------------------------------------------

/// `navigator.bluetooth.requestDevice(options)`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestOptions {
    /// Absent and present-but-empty are different: the specification rejects
    /// an empty `filters` array, and `None` means the key was not given.
    pub filters: Option<Vec<Filter>>,
    pub exclusion_filters: Option<Vec<Filter>>,
    pub optional_services: Vec<String>,
    pub optional_manufacturer_data: Vec<u16>,
    pub accept_all_devices: bool,
}

/// One entry of `filters` or `exclusionFilters`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    pub services: Vec<String>,
    pub name: Option<String>,
    pub name_prefix: Option<String>,
    pub manufacturer_data: Vec<ManufacturerDataFilter>,
    pub service_data: Vec<ServiceDataFilter>,
}

/// `{ companyIdentifier, dataPrefix, mask }`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManufacturerDataFilter {
    pub company_identifier: u16,
    #[serde(default)]
    pub data_prefix: Option<Vec<u8>>,
    #[serde(default)]
    pub mask: Option<Vec<u8>>,
}

/// `{ service, dataPrefix, mask }`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDataFilter {
    pub service: String,
    #[serde(default)]
    pub data_prefix: Option<Vec<u8>>,
    #[serde(default)]
    pub mask: Option<Vec<u8>>,
}

/// `navigator.bluetooth.requestLEScan(options)`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LeScanOptions {
    pub filters: Option<Vec<Filter>>,
    pub keep_repeated_devices: bool,
    pub accept_all_advertisements: bool,
}

// ---- out -----------------------------------------------------------------

/// `BluetoothDevice`. The identifier is the library's per-host one, never a
/// hardware address — Apple does not expose those, and the specification does
/// not want them exposed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
}

/// `BluetoothRemoteGATTService`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub handle: String,
    pub uuid: String,
    pub is_primary: bool,
    pub device_id: String,
}

/// `BluetoothRemoteGATTCharacteristic`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Characteristic {
    pub handle: String,
    pub uuid: String,
    pub service_handle: String,
    pub properties: Properties,
}

/// `BluetoothCharacteristicProperties`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Properties {
    pub broadcast: bool,
    pub read: bool,
    pub write_without_response: bool,
    pub write: bool,
    pub notify: bool,
    pub indicate: bool,
    pub authenticated_signed_writes: bool,
    pub reliable_write: bool,
    pub writable_auxiliaries: bool,
}

impl From<CharacteristicProperties> for Properties {
    fn from(p: CharacteristicProperties) -> Self {
        Self {
            broadcast: p.broadcast(),
            read: p.read(),
            write_without_response: p.write_without_response(),
            write: p.write(),
            notify: p.notify(),
            indicate: p.indicate(),
            authenticated_signed_writes: p.authenticated_signed_writes(),
            reliable_write: p.reliable_write(),
            writable_auxiliaries: p.writable_auxiliaries(),
        }
    }
}

/// `BluetoothRemoteGATTDescriptor`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    pub handle: String,
    pub uuid: String,
    pub characteristic_handle: String,
}

/// `BluetoothLEScan`, as the page sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeScan {
    pub id: String,
    pub keep_repeated_devices: bool,
    pub accept_all_advertisements: bool,
}

/// `BluetoothAdvertisingEvent`, for `watchAdvertisements()` and for the
/// chooser's list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvertisingEvent {
    pub device_id: String,
    pub name: Option<String>,
    pub rssi: Option<i32>,
    pub tx_power: Option<i16>,
    pub appearance: Option<u16>,
    pub uuids: Vec<String>,
    /// Company identifier to payload. JSON object keys are strings, so the
    /// identifier is rendered in decimal and the shim parses it back into the
    /// integer-keyed `Map` the specification defines.
    pub manufacturer_data: std::collections::BTreeMap<String, Vec<u8>>,
    pub service_data: std::collections::BTreeMap<String, Vec<u8>>,
}

impl AdvertisingEvent {
    /// Build one from a sighting.
    pub fn new(device_id: &str, name: Option<String>, adv: &Advertisement) -> Self {
        Self {
            device_id: device_id.to_string(),
            name: name.or_else(|| adv.local_name.clone()),
            rssi: (adv.rssi != webbluetooth::filter::UNAVAILABLE_RSSI).then_some(adv.rssi),
            tx_power: adv.tx_power,
            appearance: adv.appearance,
            uuids: adv.service_uuids.iter().map(|u| u.to_string()).collect(),
            manufacturer_data: adv
                .manufacturer_data
                .iter()
                .map(|(company, data)| (company.to_string(), data.clone()))
                .collect(),
            service_data: adv
                .service_data
                .iter()
                .map(|(uuid, data)| (uuid.to_string(), data.clone()))
                .collect(),
        }
    }
}
