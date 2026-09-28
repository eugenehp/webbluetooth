//! The Web Bluetooth surface, as commands the injected shim calls.
//!
//! Every command re-derives the caller's origin from the content webview's
//! *current* URL. Nothing is trusted from the page: not the origin, not a
//! device identifier, not a handle. The shim is ordinary page script and a
//! site can replace it, so the shim is a convenience for honest pages and
//! never a control — the checks are all here.

use std::collections::BTreeSet;

use futures_util::{FutureExt, StreamExt};
use serde_json::json;
use tauri::{AppHandle, Manager, State, Webview};
use webbluetooth::uuid::{services, BluetoothUuid};
use webbluetooth::{DataPrefix, DeviceFilter, LeScanOptions, RequestDeviceOptions};

use crate::chooser::{ChooserUi, Prompt};
use crate::dto;
use crate::error::{JsError, Result};
use crate::origin;
use crate::scanning;
use crate::shell;
use crate::state::{Browser, Caller};

// ---- plumbing ------------------------------------------------------------

/// Deliver an event to the page.
///
/// `eval` rather than Tauri's event plugin: this has to work on an arbitrary
/// remote origin, and it avoids granting web content any command it does not
/// need. It is also not subject to the page's Content-Security-Policy, which
/// a `connect-src`-restricted site would otherwise use to cut the channel.
pub fn push(webview: &Webview, kind: &str, detail: serde_json::Value) {
    let Ok(json) = serde_json::to_string(&json!({ "type": kind, "detail": detail })) else {
        return;
    };
    let _ = webview.eval(format!(
        "window.__WEBBLUETOOTH_SHIM__&&window.__WEBBLUETOOTH_SHIM__.dispatch({json})"
    ));
}

/// Who is calling, or a refusal.
///
/// Three things are checked. The webview has to be one that renders web
/// content — the capability already restricts these commands to those, and
/// this says so a second time where it is cheap. The origin has to be a
/// secure context, which is where the specification puts Web Bluetooth. And
/// the tab is carried alongside, because a handle belongs to one document:
/// permissions are the origin's, lifetimes are the tab's.
fn caller(webview: &Webview) -> Result<Caller> {
    if !shell::is_content(webview.label()) {
        return Err(JsError::security("not reachable from this webview"));
    }
    let url = webview
        .url()
        .map_err(|e| JsError::invalid_state(format!("the page has no URL: {e}")))?;

    let origin = origin::of(&url)
        .ok_or_else(|| JsError::security("Web Bluetooth is not available on an opaque origin"))?;
    if !origin::is_secure(&url) {
        return Err(JsError::security(format!(
            "Web Bluetooth requires a secure context; {origin} is not one"
        )));
    }
    Ok(Caller {
        origin,
        webview: webview.label().to_string(),
    })
}

/// A UUID the shim has already canonicalised.
///
/// The shim resolves assigned names in JavaScript, because
/// `BluetoothUUID.getService` is synchronous and namespaced. Anything that
/// still fails here came from a page bypassing the shim, so `TypeError` — what
/// the specification throws for a malformed UUID — is the honest answer.
fn uuid(value: &str) -> Result<BluetoothUuid> {
    BluetoothUuid::parse(value).map_err(|_| JsError::type_error(format!("invalid UUID: {value:?}")))
}

fn optional_uuid(value: &Option<String>) -> Result<Option<BluetoothUuid>> {
    value.as_deref().map(uuid).transpose()
}

/// An assigned name if the registry has one, else the short form.
fn label(uuid: &BluetoothUuid) -> String {
    if let Some((name, _)) = services::ALL.iter().find(|(_, u)| u == uuid) {
        return (*name).to_string();
    }
    match uuid.as_u16() {
        Some(short) => format!("0x{short:04x}"),
        None => uuid.to_string(),
    }
}

// ---- the adapter ---------------------------------------------------------

/// `navigator.bluetooth.getAvailability()`.
#[tauri::command]
pub async fn wb_availability(webview: Webview, state: State<'_, Browser>) -> Result<bool> {
    caller(&webview)?;
    Ok(matches!(
        state.bluetooth.availability().await,
        // "Is there a radio this process could use", not "is it ready this
        // instant" — a powered-off adapter still answers yes, and turning it
        // on does not change what the page is allowed to do.
        Ok(())
            | Err(webbluetooth::Availability::PoweredOff)
            | Err(webbluetooth::Availability::Resetting)
            | Err(webbluetooth::Availability::Unknown)
    ))
}

/// The shim announcing itself. Useful when a page is not behaving and it is
/// not obvious whether the injection happened at all.
#[tauri::command]
pub fn wb_shim_hello(webview: Webview) -> Result<serde_json::Value> {
    let who = caller(&webview)?;
    Ok(json!({ "origin": who.origin }))
}

// ---- choosing a device ---------------------------------------------------

/// `navigator.bluetooth.requestDevice(options)`.
#[tauri::command]
pub async fn wb_request_device(
    app: AppHandle,
    webview: Webview,
    state: State<'_, Browser>,
    options: dto::RequestOptions,
) -> Result<dto::Device> {
    let who = caller(&webview)?;
    let (request, wants) = build_request(&options)?;
    request.validate()?;

    // Read the grant the request would produce before handing it over: the
    // call consumes the options.
    let granted: BTreeSet<BluetoothUuid> = request.allowed_services();

    // `AutoPick` is only ever registered by a self-test run that was pointed
    // at a device by name; see `selftest::DEVICE_ENV`. In every other run the
    // state does not exist and this is the picker window, always.
    let chooser: Box<dyn webbluetooth::DeviceChooser> =
        match app.try_state::<crate::selftest::AutoPick>() {
            Some(auto) => Box::new(auto.chooser()),
            None => Box::new(ChooserUi::new(
                app.clone(),
                Prompt {
                    origin: who.origin.clone(),
                    wants,
                    accept_all: options.accept_all_devices,
                },
            )),
        };

    let device = state
        .bluetooth
        .request_device_with(request, chooser.as_ref())
        .await?;

    let dto = dto::Device {
        id: device.id().to_string(),
        name: device.name(),
    };
    state.grant(&who.origin, device, granted);
    shell::refresh_chrome(&webview.app_handle().clone());
    Ok(dto)
}

/// Translate the page's dictionary, and describe it for the picker.
fn build_request(options: &dto::RequestOptions) -> Result<(RequestDeviceOptions, Vec<String>)> {
    let mut request = RequestDeviceOptions::new();
    let mut wants = Vec::new();

    if options.accept_all_devices {
        request = request.accept_all_devices();
    }

    if let Some(filters) = &options.filters {
        for filter in filters {
            let (built, described) = build_filter(filter)?;
            request = request.filter(built);
            wants.push(described);
        }
    }
    if let Some(filters) = &options.exclusion_filters {
        for filter in filters {
            let (built, described) = build_filter(filter)?;
            request = request.exclusion_filter(built);
            wants.push(format!("not {described}"));
        }
    }

    for service in &options.optional_services {
        request = request.optional_service(uuid(service)?)?;
    }
    if !options.optional_manufacturer_data.is_empty() {
        request = request.optional_manufacturer_data(options.optional_manufacturer_data.clone());
    }

    Ok((request, wants))
}

fn build_filter(filter: &dto::Filter) -> Result<(DeviceFilter, String)> {
    let mut built = DeviceFilter::new();
    let mut parts = Vec::new();

    for service in &filter.services {
        let parsed = uuid(service)?;
        built = built.service(parsed)?;
        parts.push(format!("offers {}", label(&parsed)));
    }
    if let Some(name) = &filter.name {
        built = built.name(name.clone());
        parts.push(format!("is named {name:?}"));
    }
    if let Some(prefix) = &filter.name_prefix {
        built = built.name_prefix(prefix.clone());
        parts.push(format!("has a name starting {prefix:?}"));
    }
    for entry in &filter.manufacturer_data {
        built = built.manufacturer_data(
            entry.company_identifier,
            data_prefix(&entry.data_prefix, &entry.mask)?,
        );
        parts.push(format!(
            "advertises data from company 0x{:04x}",
            entry.company_identifier
        ));
    }
    for entry in &filter.service_data {
        let parsed = uuid(&entry.service)?;
        built = built.service_data(parsed, data_prefix(&entry.data_prefix, &entry.mask)?)?;
        parts.push(format!("advertises data for {}", label(&parsed)));
    }

    if parts.is_empty() {
        // The specification rejects a filter with no members; say so here
        // rather than letting it through as "matches everything".
        return Err(JsError::type_error(
            "a filter must have at least one of services, name, namePrefix, \
             manufacturerData or serviceData",
        ));
    }
    let described = parts.join(" and ");
    Ok((built, described))
}

fn data_prefix(prefix: &Option<Vec<u8>>, mask: &Option<Vec<u8>>) -> Result<DataPrefix> {
    let bytes = prefix.clone().unwrap_or_default();
    Ok(match mask {
        // `with_mask` rejects a mask of a different length, which is the
        // specification's rule too.
        Some(mask) => DataPrefix::with_mask(bytes, mask.clone())?,
        None => DataPrefix::new(bytes),
    })
}

/// `navigator.bluetooth.getDevices()` — what this origin already holds.
#[tauri::command]
pub async fn wb_get_devices(
    webview: Webview,
    state: State<'_, Browser>,
) -> Result<Vec<dto::Device>> {
    let who = caller(&webview)?;
    // Grants outlive the process now, so this can be asked before the
    // remembered devices have been adopted. Waiting is the difference
    // between a page seeing its permissions and seeing an empty list that
    // fills in a moment later.
    state.ready().await;
    Ok(state
        .granted_devices(&who.origin)
        .into_iter()
        .map(|device| dto::Device {
            id: device.id().to_string(),
            name: device.name(),
        })
        .collect())
}

/// `BluetoothDevice.forget()` — give up this origin's permission.
#[tauri::command]
pub fn wb_forget_device(
    webview: Webview,
    state: State<'_, Browser>,
    device_id: String,
) -> Result<()> {
    let who = caller(&webview)?;
    // Not an error if it was never granted: `forget()` returns void and the
    // end state is the same either way.
    state.revoke(&who.origin, &device_id);
    shell::refresh_chrome(&webview.app_handle().clone());
    Ok(())
}

/// `BluetoothDevice.watchAdvertisements()`.
#[tauri::command]
pub async fn wb_watch_advertisements(
    webview: Webview,
    state: State<'_, Browser>,
    device_id: String,
) -> Result<()> {
    let who = caller(&webview)?;
    let device = state.device(&who.origin, &device_id)?;
    let mut sightings = device.watch_advertisements().await?;

    let target = webview.clone();
    let id = device_id.clone();
    let task = tauri::async_runtime::spawn(async move {
        while let Some(sighting) = sightings.next().await {
            let event = dto::AdvertisingEvent::new(&id, sighting.name, &sighting.advertisement);
            push(
                &target,
                "advertisementreceived",
                serde_json::to_value(event).unwrap_or(serde_json::Value::Null),
            );
        }
    });
    state.set_advertisement_task(&who, &device_id, task);
    Ok(())
}

/// `navigator.bluetooth.requestLEScan(options)`.
///
/// A scan is a standing grant on everything the radio can hear, so it gets
/// its own prompt rather than riding on a device grant — see [`scanning`].
/// The answer is remembered per origin; a refusal is remembered too, so a
/// site that was told no cannot re-ask on a loop.
#[tauri::command]
pub async fn wb_request_le_scan(
    app: AppHandle,
    webview: Webview,
    state: State<'_, Browser>,
    options: dto::LeScanOptions,
) -> Result<dto::LeScan> {
    let who = caller(&webview)?;

    let mut request = LeScanOptions::new();
    let mut wants = Vec::new();
    if options.accept_all_advertisements {
        request = LeScanOptions::accept_all_advertisements();
    }
    if let Some(filters) = &options.filters {
        for filter in filters {
            let (built, described) = build_filter(filter)?;
            request = request.filter(built);
            wants.push(described);
        }
    }
    request = request.keep_repeated_devices(options.keep_repeated_devices);
    request.validate()?;

    match state.scanning_decision(&who.origin) {
        Some(true) => {}
        Some(false) => {
            return Err(JsError::new(
                "NotAllowedError",
                "scanning was refused for this site",
            ))
        }
        None => {
            let allowed = scanning::ask(
                &app,
                scanning::ScanAsk {
                    origin: who.origin.clone(),
                    wants,
                    accept_all: options.accept_all_advertisements,
                },
            )
            .await;
            state.set_scanning_decision(&who.origin, allowed);
            shell::refresh_chrome(&webview.app_handle().clone());
            if !allowed {
                return Err(JsError::new(
                    "NotAllowedError",
                    "the user refused scanning for this site",
                ));
            }
        }
    }

    let mut sightings = state.bluetooth.request_le_scan(request).await?;

    let target = webview.clone();
    // The identifier has to exist before the task can label its events with
    // it, and the task has to exist before it can be registered. One channel
    // breaks the knot.
    let (id_tx, id_rx) = futures_channel::oneshot::channel::<String>();
    let task = tauri::async_runtime::spawn(async move {
        let Ok(scan_id) = id_rx.await else {
            return;
        };
        while let Some(sighting) = sightings.next().await {
            let event =
                dto::AdvertisingEvent::new(&sighting.id, sighting.name, &sighting.advertisement);
            let Ok(mut value) = serde_json::to_value(event) else {
                continue;
            };
            if let Some(object) = value.as_object_mut() {
                // What tells the shim to fire this at `navigator.bluetooth`
                // rather than at a device: an LE scan reports devices the
                // page has no grant for and no `BluetoothDevice` of its own.
                object.insert("scanId".into(), serde_json::Value::String(scan_id.clone()));
            }
            push(&target, "advertisementreceived", value);
        }
    });

    let id = state.insert_scan(&who, task);
    let _ = id_tx.send(id.clone());

    Ok(dto::LeScan {
        id,
        keep_repeated_devices: options.keep_repeated_devices,
        accept_all_advertisements: options.accept_all_advertisements,
    })
}

/// `BluetoothLEScan.stop()`.
#[tauri::command]
pub fn wb_stop_le_scan(webview: Webview, state: State<'_, Browser>, scan_id: String) -> Result<()> {
    let who = caller(&webview)?;
    state.stop_scan(&who, &scan_id);
    Ok(())
}

/// Stop watching — what aborting `watchAdvertisements()`'s signal does.
#[tauri::command]
pub fn wb_unwatch_advertisements(
    webview: Webview,
    state: State<'_, Browser>,
    device_id: String,
) -> Result<()> {
    let who = caller(&webview)?;
    // Check the grant even though stopping is harmless: answering for a
    // device this origin does not hold would confirm it exists.
    state.device(&who.origin, &device_id)?;
    state.clear_advertisement_task(&who, &device_id);
    Ok(())
}

// ---- GATT ----------------------------------------------------------------

/// `gatt.connect()`.
#[tauri::command]
pub async fn wb_gatt_connect(
    webview: Webview,
    state: State<'_, Browser>,
    device_id: String,
) -> Result<()> {
    let who = caller(&webview)?;
    let device = state.device(&who.origin, &device_id)?;
    device.gatt().connect().await?;
    state.note_connected(&who, &device_id);

    let mut drops = device.watch_disconnect();
    let target = webview.clone();
    let id = device_id.clone();
    let app = webview.app_handle().clone();
    let task = tauri::async_runtime::spawn(async move {
        while drops.next().await.is_some() {
            push(&target, "gattserverdisconnected", json!({ "deviceId": id }));
            shell::refresh_chrome(&app);
        }
    });
    state.set_disconnect_task(&who, &device_id, task);

    // A peer that re-advertises a different service set invalidates every
    // handle into the old one. Dropping them here is what turns a stale read
    // into an `InvalidStateError` instead of a read of whatever now occupies
    // that position; the page is told so it can re-discover.
    let mut changes = device.gatt().watch_services_changed();
    let target = webview.clone();
    let id = device_id.clone();
    // The command's `State` borrow cannot outlive it, so the task reaches
    // the same state through the handle instead.
    let handle = webview.app_handle().clone();
    let task = tauri::async_runtime::spawn(async move {
        while changes.next().await.is_some() {
            handle.state::<Browser>().invalidate_device(&id);
            push(&target, "serviceschanged", json!({ "deviceId": id }));
        }
    });
    state.set_service_watcher(&who, &device_id, task);

    shell::refresh_chrome(&webview.app_handle().clone());
    Ok(())
}

/// `gatt.disconnect()`.
#[tauri::command]
pub fn wb_gatt_disconnect(
    webview: Webview,
    state: State<'_, Browser>,
    device_id: String,
) -> Result<()> {
    let who = caller(&webview)?;
    // Not straight to the radio: another tab may still be holding this
    // device, and the link is shared.
    state.device(&who.origin, &device_id)?;
    state.disconnect(&who, &device_id);
    shell::refresh_chrome(&webview.app_handle().clone());
    Ok(())
}

/// `gatt.connected`.
#[tauri::command]
pub fn wb_gatt_connected(
    webview: Webview,
    state: State<'_, Browser>,
    device_id: String,
) -> Result<bool> {
    let who = caller(&webview)?;
    Ok(state.device(&who.origin, &device_id)?.gatt().connected())
}

/// `getPrimaryService` and `getPrimaryServices`.
#[tauri::command]
pub async fn wb_primary_services(
    webview: Webview,
    state: State<'_, Browser>,
    device_id: String,
    service: Option<String>,
) -> Result<Vec<dto::Service>> {
    let who = caller(&webview)?;
    let device = state.device(&who.origin, &device_id)?;
    let wanted = optional_uuid(&service)?;

    if let Some(wanted) = &wanted {
        // Refuse before going near the radio, so that a service this origin
        // was never granted cannot even be probed for existence.
        if !state.may_use_service(&who.origin, &device_id, wanted) {
            return Err(JsError::security(format!(
                "this origin was not granted access to service {wanted}"
            )));
        }
    }

    let found = device.gatt().get_primary_services(wanted).await?;
    let allowed = state.allowed_services(&who.origin, &device_id);

    // The library filtered by the *session* grant, which is the union over
    // every origin. Narrow it to this one.
    let mut out = Vec::new();
    for service in found {
        if !allowed.contains(service.uuid()) {
            continue;
        }
        out.push(dto::Service {
            uuid: service.uuid().to_string(),
            is_primary: service.is_primary(),
            device_id: device_id.clone(),
            handle: state.insert_service(&who, &device_id, service),
        });
    }
    if out.is_empty() {
        return Err(JsError::not_found(
            "no accessible primary services on this device",
        ));
    }
    Ok(out)
}

/// `getIncludedService` and `getIncludedServices`.
#[tauri::command]
pub async fn wb_included_services(
    webview: Webview,
    state: State<'_, Browser>,
    service_handle: String,
    service: Option<String>,
) -> Result<Vec<dto::Service>> {
    let who = caller(&webview)?;
    let (device_id, parent) = state.service(&who, &service_handle)?;
    let wanted = optional_uuid(&service)?;

    if let Some(wanted) = &wanted {
        if !state.may_use_service(&who.origin, &device_id, wanted) {
            return Err(JsError::security(format!(
                "this origin was not granted access to service {wanted}"
            )));
        }
    }

    let found = parent.get_included_services(wanted).await?;
    let allowed = state.allowed_services(&who.origin, &device_id);

    let mut out = Vec::new();
    for service in found {
        if !allowed.contains(service.uuid()) {
            continue;
        }
        out.push(dto::Service {
            uuid: service.uuid().to_string(),
            is_primary: service.is_primary(),
            device_id: device_id.clone(),
            handle: state.insert_service(&who, &device_id, service),
        });
    }
    if out.is_empty() {
        return Err(JsError::not_found("no accessible included services"));
    }
    Ok(out)
}

/// `getCharacteristic` and `getCharacteristics`.
#[tauri::command]
pub async fn wb_characteristics(
    webview: Webview,
    state: State<'_, Browser>,
    service_handle: String,
    characteristic: Option<String>,
) -> Result<Vec<dto::Characteristic>> {
    let who = caller(&webview)?;
    let (device_id, service) = state.service(&who, &service_handle)?;

    // The grant can be revoked between getting the service and using it.
    if !state.may_use_service(&who.origin, &device_id, service.uuid()) {
        return Err(JsError::security(
            "this origin's access to that service has been revoked",
        ));
    }

    let found = service
        .get_characteristics(optional_uuid(&characteristic)?)
        .await?;
    if found.is_empty() {
        return Err(JsError::not_found("no such characteristic on this service"));
    }

    Ok(found
        .into_iter()
        .map(|characteristic| dto::Characteristic {
            uuid: characteristic.uuid().to_string(),
            properties: characteristic.properties().into(),
            service_handle: service_handle.clone(),
            handle: state.insert_characteristic(&who, &device_id, &service_handle, characteristic),
        })
        .collect())
}

/// `getDescriptor` and `getDescriptors`.
#[tauri::command]
pub async fn wb_descriptors(
    webview: Webview,
    state: State<'_, Browser>,
    characteristic_handle: String,
    descriptor: Option<String>,
) -> Result<Vec<dto::Descriptor>> {
    let who = caller(&webview)?;
    let (device_id, _, characteristic) = state.characteristic(&who, &characteristic_handle)?;

    let wanted = optional_uuid(&descriptor)?;
    let found = match &wanted {
        Some(uuid) => vec![characteristic.get_descriptor(*uuid).await?],
        None => characteristic.get_descriptors().await?,
    };
    if found.is_empty() {
        return Err(JsError::not_found(
            "no such descriptor on this characteristic",
        ));
    }

    Ok(found
        .into_iter()
        .map(|descriptor| dto::Descriptor {
            uuid: descriptor.uuid().to_string(),
            characteristic_handle: characteristic_handle.clone(),
            handle: state.insert_descriptor(&who, &device_id, descriptor),
        })
        .collect())
}

/// `characteristic.readValue()`.
#[tauri::command]
pub async fn wb_read_characteristic(
    webview: Webview,
    state: State<'_, Browser>,
    handle: String,
) -> Result<Vec<u8>> {
    let who = caller(&webview)?;
    let (_, _, characteristic) = state.characteristic(&who, &handle)?;
    Ok(characteristic.read_value().await?)
}

/// `writeValueWithResponse()` and `writeValueWithoutResponse()`.
#[tauri::command]
pub async fn wb_write_characteristic(
    webview: Webview,
    state: State<'_, Browser>,
    handle: String,
    value: Vec<u8>,
    with_response: bool,
) -> Result<()> {
    let who = caller(&webview)?;
    let (_, _, characteristic) = state.characteristic(&who, &handle)?;
    if with_response {
        characteristic.write_value_with_response(&value).await?;
    } else {
        characteristic.write_value_without_response(&value).await?;
    }
    Ok(())
}

/// `characteristic.startNotifications()`.
#[tauri::command]
pub async fn wb_start_notifications(
    webview: Webview,
    state: State<'_, Browser>,
    handle: String,
) -> Result<()> {
    let who = caller(&webview)?;
    let (_, _, characteristic) = state.characteristic(&who, &handle)?;
    let mut values = characteristic.start_notifications().await?;

    let target = webview.clone();
    let id = handle.clone();
    let task = tauri::async_runtime::spawn(async move {
        // Coalesce bursts. Every value costs one `eval` into the page, and a
        // characteristic pushing at connection-interval rates would otherwise
        // spend the whole budget on round trips. Nothing waits for a batch to
        // fill: the first value is awaited normally and only values *already*
        // queued behind it are swept up, so a sensor reporting once a second
        // is delivered exactly as promptly as before.
        const MAX_BATCH: usize = 64;
        while let Some(first) = values.next().await {
            let mut batch = vec![first];
            let mut ended = false;
            while batch.len() < MAX_BATCH {
                match values.next().now_or_never() {
                    Some(Some(value)) => batch.push(value),
                    Some(None) => {
                        ended = true;
                        break;
                    }
                    None => break,
                }
            }
            push(
                &target,
                "characteristicvaluechanged",
                json!({ "handle": id, "values": batch }),
            );
            if ended {
                break;
            }
        }
    });
    state.set_notification_task(&who, &handle, task);
    Ok(())
}

/// `characteristic.stopNotifications()`.
#[tauri::command]
pub async fn wb_stop_notifications(
    webview: Webview,
    state: State<'_, Browser>,
    handle: String,
) -> Result<()> {
    let who = caller(&webview)?;
    let (_, _, characteristic) = state.characteristic(&who, &handle)?;
    // Stop forwarding first: the library may deliver one more value while the
    // CCCD write is in flight, and the page has already unsubscribed.
    state.clear_notification_task(&who, &handle);
    characteristic.stop_notifications().await?;
    Ok(())
}

/// `descriptor.readValue()`.
#[tauri::command]
pub async fn wb_read_descriptor(
    webview: Webview,
    state: State<'_, Browser>,
    handle: String,
) -> Result<Vec<u8>> {
    let who = caller(&webview)?;
    Ok(state.descriptor(&who, &handle)?.read_value().await?)
}

/// `descriptor.writeValue()`.
#[tauri::command]
pub async fn wb_write_descriptor(
    webview: Webview,
    state: State<'_, Browser>,
    handle: String,
    value: Vec<u8>,
) -> Result<()> {
    let who = caller(&webview)?;
    state.descriptor(&who, &handle)?.write_value(&value).await?;
    Ok(())
}
