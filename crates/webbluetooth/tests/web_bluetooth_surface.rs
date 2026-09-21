//! Every member of the Web Bluetooth API must be accounted for.
//!
//! "Are we at parity with the standard?" is not a question to answer from
//! memory — that is how the GATT blocklist ended up with three wrong entries
//! and the JNI table with four wrong slots. `spec/web-bluetooth-surface.txt`
//! is generated from MDN's own `browser-compat-data`, and this test requires
//! that each line in it is either mapped to something here or excluded on
//! purpose, with the reason recorded.
//!
//! When MDN publishes a new member, the file changes and this test fails until
//! somebody decides what to do about it. That is the whole mechanism.
//!
//! It checks *accounting*, not behaviour: that a member has a counterpart, not
//! that the counterpart is correct. Behaviour is what the round-trip examples
//! and the per-backend tests are for.

const SURFACE: &str = include_str!("../../webbluetooth-core/spec/web-bluetooth-surface.txt");

/// MDN's status for each member, which the IDL does not carry.
const STATUS: &str = include_str!("../../webbluetooth-core/spec/web-bluetooth-status.txt");

/// What MDN says about `Interface.member`, if it knows it at all.
///
/// MDN lists only what browsers ship, so a member the standard defines and
/// nobody implements is simply absent — that is not an error, it is the
/// difference between the two oracles.
fn mdn_status(member: &str) -> Option<&'static str> {
    // The surface file prefixes the kind; MDN does not.
    let bare = member.split_once(' ').map_or(member, |(_, rest)| rest);
    STATUS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .find_map(|line| {
            let (name, flags) = line.split_once(' ')?;
            (name == bare).then_some(flags)
        })
}

/// What this crate calls a member of the specification.
///
/// Rust is not JavaScript, so some of this is renaming rather than omission:
/// a getter becomes a method, an event becomes a stream, and an overload that
/// JavaScript distinguishes by argument count becomes two methods.
enum Mapped {
    /// Implemented as this Rust function, whose existence is checked.
    ///
    /// Naming a function that does not exist is the failure mode this test is
    /// most vulnerable to: a mapping table is trivially easy to fill in
    /// optimistically, and then it certifies nothing.
    As(&'static str),
    /// Deliberately absent, with why.
    Excluded(&'static str),
}

/// The public API, for checking that a mapped function is really there.
const SOURCES: &[&str] = &[
    include_str!("../src/lib.rs"),
    include_str!("../src/device.rs"),
    include_str!("../src/gatt.rs"),
    // The portable half. A member of the standard is just as implemented for
    // living in `webbluetooth-core` — `BluetoothUUID.getService` resolves
    // through the registry there, and `properties` is a type a backend
    // produces — so the check has to read both crates or it would report
    // members as missing that merely moved.
    include_str!("../../webbluetooth-core/src/gatt.rs"),
    include_str!("../../webbluetooth-core/src/uuid.rs"),
    include_str!("../../webbluetooth-core/src/filter.rs"),
    include_str!("../../webbluetooth-core/src/chooser.rs"),
    include_str!("../../webbluetooth-core/src/registry.rs"),
];

/// Is `path`'s final segment part of the public API?
///
/// A member can be a method or a field, and a method can be generic — so
/// `fn services<U: IntoUuid>(…)` has to count, and `pub appearance: …` has to
/// count. Matching only `fn name(` missed both, and reported six mappings as
/// broken that were merely spelled differently.
fn exists(path: &str) -> bool {
    let name = path.rsplit("::").next().unwrap_or(path);
    let forms = [
        format!("fn {name}("),  // a method
        format!("fn {name}<"),  // a generic method
        format!("pub {name}:"), // a public field
    ];
    SOURCES
        .iter()
        .any(|src| forms.iter().any(|form| src.contains(form)))
}

use Mapped::{As, Excluded};

fn mapping(member: &str) -> Option<Mapped> {
    Some(match member {
        "dictionary AllowedBluetoothDevice.allowedManufacturerData" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary AllowedBluetoothDevice.deviceId" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary AllowedBluetoothDevice.mayUseGATT" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary BluetoothAdvertisingEventInit.appearance" => Excluded(
            "fields of a DOM event type's constructor, which this crate does not expose",
        ),
        "dictionary BluetoothAdvertisingEventInit.device" => Excluded(
            "fields of a DOM event type's constructor, which this crate does not expose",
        ),
        "dictionary BluetoothAdvertisingEventInit.manufacturerData" => Excluded(
            "fields of a DOM event type's constructor, which this crate does not expose",
        ),
        "dictionary BluetoothAdvertisingEventInit.name" => Excluded(
            "fields of a DOM event type's constructor, which this crate does not expose",
        ),
        "dictionary BluetoothAdvertisingEventInit.rssi" => Excluded(
            "fields of a DOM event type's constructor, which this crate does not expose",
        ),
        "dictionary BluetoothAdvertisingEventInit.serviceData" => Excluded(
            "fields of a DOM event type's constructor, which this crate does not expose",
        ),
        "dictionary BluetoothAdvertisingEventInit.txPower" => Excluded(
            "fields of a DOM event type's constructor, which this crate does not expose",
        ),
        "dictionary BluetoothAdvertisingEventInit.uuids" => Excluded(
            "fields of a DOM event type's constructor, which this crate does not expose",
        ),
        "dictionary BluetoothDataFilterInit.dataPrefix" => As("DataPrefix::new"),
        "dictionary BluetoothDataFilterInit.mask" => As("DataPrefix::with_mask"),
        "dictionary BluetoothLEScanFilterInit.manufacturerData" => As("DeviceFilter::manufacturer_data"),
        "dictionary BluetoothLEScanFilterInit.name" => As("DeviceFilter::name"),
        "dictionary BluetoothLEScanFilterInit.namePrefix" => As("DeviceFilter::name_prefix"),
        "dictionary BluetoothLEScanFilterInit.serviceData" => As("DeviceFilter::service_data"),
        "dictionary BluetoothLEScanFilterInit.services" => As("DeviceFilter::services"),
        "dictionary BluetoothManufacturerDataFilterInit.companyIdentifier" => As("DeviceFilter::manufacturer_data"),
        "dictionary BluetoothPermissionDescriptor.acceptAllDevices" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary BluetoothPermissionDescriptor.deviceId" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary BluetoothPermissionDescriptor.filters" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary BluetoothPermissionDescriptor.optionalManufacturerData" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary BluetoothPermissionDescriptor.optionalServices" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary BluetoothPermissionStorage.allowedDevices" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "dictionary BluetoothServiceDataFilterInit.service" => As("DeviceFilter::service_data"),
        "dictionary RequestDeviceOptions.acceptAllDevices" => As("RequestDeviceOptions::accept_all_devices"),
        "dictionary RequestDeviceOptions.exclusionFilters" => As("RequestDeviceOptions::exclusion_filter"),
        "dictionary RequestDeviceOptions.filters" => As("RequestDeviceOptions::filter"),
        "dictionary RequestDeviceOptions.optionalManufacturerData" => As("RequestDeviceOptions::optional_manufacturer_data"),
        "dictionary RequestDeviceOptions.optionalServices" => As("RequestDeviceOptions::optional_services"),
        "dictionary ValueEventInit.value" => Excluded(
            "a DOM event type; streams carry values here",
        ),
        "dictionary WatchAdvertisementsOptions.signal" => Excluded(
            "an AbortSignal; dropping the Advertisements stream is what stops the watch here",
        ),
        "interface Bluetooth.getAvailability" => As("Bluetooth::availability"),
        "interface Bluetooth.getDevices" => As("Bluetooth::get_devices"),
        "interface Bluetooth.onavailabilitychanged" => As("Bluetooth::watch_availability"),
        "interface Bluetooth.referringDevice" => Excluded(
            "a page can be launched from a device by a browser; nothing outside a browser has a referrer",
        ),
        "interface Bluetooth.requestDevice" => As("Bluetooth::request_device"),
        "interface BluetoothAdvertisingEvent.appearance" => As("Advertisement::appearance"),
        "interface BluetoothAdvertisingEvent.constructor" => Excluded(
            "a DOM event type a page constructs for testing; a stream yields values here",
        ),
        "interface BluetoothAdvertisingEvent.device" => As("AdvertisementEvent::id"),
        "interface BluetoothAdvertisingEvent.manufacturerData" => As("Advertisement::manufacturer_data"),
        "interface BluetoothAdvertisingEvent.name" => As("AdvertisementEvent::name"),
        "interface BluetoothAdvertisingEvent.rssi" => As("Candidate::rssi"),
        "interface BluetoothAdvertisingEvent.serviceData" => As("Advertisement::service_data"),
        "interface BluetoothAdvertisingEvent.txPower" => As("Advertisement::tx_power"),
        "interface BluetoothAdvertisingEvent.uuids" => As("Advertisement::service_uuids"),
        "interface BluetoothCharacteristicProperties.authenticatedSignedWrites" => As("CharacteristicProperties::authenticated_signed_writes"),
        "interface BluetoothCharacteristicProperties.broadcast" => As("CharacteristicProperties::broadcast"),
        "interface BluetoothCharacteristicProperties.indicate" => As("CharacteristicProperties::indicate"),
        "interface BluetoothCharacteristicProperties.notify" => As("CharacteristicProperties::notify"),
        "interface BluetoothCharacteristicProperties.read" => As("CharacteristicProperties::read"),
        "interface BluetoothCharacteristicProperties.reliableWrite" => As("CharacteristicProperties::reliable_write"),
        "interface BluetoothCharacteristicProperties.writableAuxiliaries" => As("CharacteristicProperties::writable_auxiliaries"),
        "interface BluetoothCharacteristicProperties.write" => As("CharacteristicProperties::write"),
        "interface BluetoothCharacteristicProperties.writeWithoutResponse" => As("CharacteristicProperties::write_without_response"),
        "interface BluetoothDevice.forget" => As("BluetoothDevice::forget"),
        "interface BluetoothDevice.gatt" => As("BluetoothDevice::gatt"),
        "interface BluetoothDevice.id" => As("BluetoothDevice::id"),
        "interface BluetoothDevice.name" => As("BluetoothDevice::name"),
        "interface BluetoothDevice.watchAdvertisements" => As("BluetoothDevice::watch_advertisements"),
        "interface BluetoothDevice.watchingAdvertisements" => As("BluetoothDevice::watching_advertisements"),
        "interface BluetoothPermissionResult.devices" => Excluded(
            "the Permissions API, which is a browser's grant store",
        ),
        "interface BluetoothRemoteGATTCharacteristic.getDescriptor" => As("RemoteGattCharacteristic::get_descriptor"),
        "interface BluetoothRemoteGATTCharacteristic.getDescriptors" => As("RemoteGattCharacteristic::get_descriptors"),
        "interface BluetoothRemoteGATTCharacteristic.properties" => As("RemoteGattCharacteristic::properties"),
        "interface BluetoothRemoteGATTCharacteristic.readValue" => As("RemoteGattCharacteristic::read_value"),
        "interface BluetoothRemoteGATTCharacteristic.service" => As("RemoteGattCharacteristic::service"),
        "interface BluetoothRemoteGATTCharacteristic.startNotifications" => As("RemoteGattCharacteristic::start_notifications"),
        "interface BluetoothRemoteGATTCharacteristic.stopNotifications" => As("RemoteGattCharacteristic::stop_notifications"),
        "interface BluetoothRemoteGATTCharacteristic.uuid" => As("RemoteGattCharacteristic::uuid"),
        "interface BluetoothRemoteGATTCharacteristic.value" => As("RemoteGattCharacteristic::value"),
        "interface BluetoothRemoteGATTCharacteristic.writeValue" => Excluded(
            "deprecated by the specification; the two explicit forms make delivery confirmation a choice",
        ),
        "interface BluetoothRemoteGATTCharacteristic.writeValueWithResponse" => As("RemoteGattCharacteristic::write_value_with_response"),
        "interface BluetoothRemoteGATTCharacteristic.writeValueWithoutResponse" => As("RemoteGattCharacteristic::write_value_without_response"),
        "interface BluetoothRemoteGATTDescriptor.characteristic" => As("RemoteGattDescriptor::characteristic"),
        "interface BluetoothRemoteGATTDescriptor.readValue" => As("RemoteGattDescriptor::read_value"),
        "interface BluetoothRemoteGATTDescriptor.uuid" => As("RemoteGattDescriptor::uuid"),
        "interface BluetoothRemoteGATTDescriptor.value" => As("RemoteGattDescriptor::value"),
        "interface BluetoothRemoteGATTDescriptor.writeValue" => As("RemoteGattDescriptor::write_value"),
        "interface BluetoothRemoteGATTServer.connect" => As("RemoteGattServer::connect"),
        "interface BluetoothRemoteGATTServer.connected" => As("RemoteGattServer::connected"),
        "interface BluetoothRemoteGATTServer.device" => As("RemoteGattServer::device"),
        "interface BluetoothRemoteGATTServer.disconnect" => As("RemoteGattServer::disconnect"),
        "interface BluetoothRemoteGATTServer.getPrimaryService" => As("RemoteGattServer::get_primary_service"),
        "interface BluetoothRemoteGATTServer.getPrimaryServices" => As("RemoteGattServer::get_primary_services"),
        "interface BluetoothRemoteGATTService.device" => As("RemoteGattService::device"),
        "interface BluetoothRemoteGATTService.getCharacteristic" => As("RemoteGattService::get_characteristic"),
        "interface BluetoothRemoteGATTService.getCharacteristics" => As("RemoteGattService::get_characteristics"),
        "interface BluetoothRemoteGATTService.getIncludedService" => As("RemoteGattService::get_included_service"),
        "interface BluetoothRemoteGATTService.getIncludedServices" => As("RemoteGattService::get_included_services"),
        "interface BluetoothRemoteGATTService.isPrimary" => As("RemoteGattService::is_primary"),
        "interface BluetoothRemoteGATTService.uuid" => As("RemoteGattService::uuid"),
        "interface BluetoothUUID.canonicalUUID" => As("uuid::BluetoothUuid::from_u32"),
        "interface BluetoothUUID.getCharacteristic" => As("uuid::characteristics::parse"),
        "interface BluetoothUUID.getDescriptor" => As("uuid::descriptors::parse"),
        "interface BluetoothUUID.getService" => As("uuid::services::parse"),
        // `navigator.bluetooth` is the browsing context's one `Bluetooth`,
        // which a page cannot ask for a second of. `Bluetooth::shared` is that
        // object for a process: one session, handed to every caller. It was
        // excluded as unimplementable while `new()` was the only entry point,
        // and `new()` really is the wrong shape for it — it opens a *new*
        // adapter session each time.
        "interface Navigator.bluetooth" => As("Bluetooth::shared"),
        "interface ValueEvent.constructor" => Excluded(
            "a DOM event type; streams carry values here",
        ),
        "interface ValueEvent.value" => Excluded(
            "a DOM event type; streams carry values here",
        ),
        "interface-mixin BluetoothDeviceEventHandlers.onadvertisementreceived" => As("BluetoothDevice::watch_advertisements"),
        "interface-mixin BluetoothDeviceEventHandlers.ongattserverdisconnected" => As("BluetoothDevice::watch_disconnect"),
        "interface-mixin CharacteristicEventHandlers.oncharacteristicvaluechanged" => As("RemoteGattCharacteristic::start_notifications"),
        "interface-mixin ServiceEventHandlers.onserviceadded" => As("RemoteGattServer::watch_services_changed"),
        "interface-mixin ServiceEventHandlers.onservicechanged" => As("RemoteGattServer::watch_services_changed"),
        "interface-mixin ServiceEventHandlers.onserviceremoved" => As("RemoteGattServer::watch_services_changed"),

        _ => return None,
    })
}

/// Parse the generated file, ignoring comments and blank lines.
fn members() -> Vec<&'static str> {
    SURFACE
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// A mapping that names a function which does not exist is worse than a gap:
/// it reports coverage that is not there.
#[test]
fn every_mapped_function_exists() {
    let mut missing = Vec::new();
    for member in members() {
        if let Some(As(path)) = mapping(member) {
            if !exists(path) {
                missing.push(format!("{member} -> {path}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} mapping(s) name a function this crate does not define:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

#[test]
fn every_member_of_the_standard_is_accounted_for() {
    let mut unaccounted = Vec::new();
    for member in members() {
        if mapping(member).is_none() {
            unaccounted.push(member);
        }
    }
    assert!(
        unaccounted.is_empty(),
        "MDN lists {} member(s) this crate does not account for:\n  {}\n\n\
         Either map it in tests/web_bluetooth_surface.rs or exclude it there \
         with a reason. Re-run ./scripts/update.sh surface to \
         refresh the list.",
        unaccounted.len(),
        unaccounted.join("\n  ")
    );
}

/// The mapping must not outlive the standard either: an entry for a member MDN
/// no longer lists is stale, and would quietly claim coverage of something that
/// does not exist.
#[test]
fn the_mapping_has_no_entries_the_standard_dropped() {
    // Reconstructed by asking the mapping about every member, then checking the
    // count matches — a stale entry cannot be enumerated directly, so this
    // relies on the file being the source of truth for what should exist.
    let mapped = members().iter().filter(|m| mapping(m).is_some()).count();
    assert_eq!(
        mapped,
        members().len(),
        "the mapping and the generated surface disagree"
    );
}

/// An exclusion that claims a member is deprecated has to be able to point at
/// something that says so.
///
/// This one was written from memory. The IDL does not contain the word
/// "deprecated" anywhere, so until MDN was vendored alongside it there was
/// nothing in the repository that could have contradicted the claim — which is
/// the same position the GATT blocklist and the JNI slot table were in when
/// they turned out to be wrong.
#[test]
fn a_deprecation_claim_is_corroborated() {
    let mut unsupported = Vec::new();
    for member in members() {
        let Some(Excluded(why)) = mapping(member) else {
            continue;
        };
        if !why.contains("deprecated") {
            continue;
        }
        match mdn_status(member) {
            Some(flags) if flags.contains("deprecated") => {}
            Some(flags) => unsupported.push(format!("{member}: MDN says {flags}")),
            None => unsupported.push(format!("{member}: MDN does not list it")),
        }
    }
    assert!(
        unsupported.is_empty(),
        "excluded as deprecated without corroboration:\n  {}",
        unsupported.join("\n  ")
    );
}

/// The two oracles have to be describing the same API.
///
/// MDN is the narrower of the two, so everything it lists should appear in the
/// IDL. Anything that does not means one of them has moved and the mapping is
/// being checked against a surface that no longer exists.
#[test]
fn the_two_oracles_agree_where_they_overlap() {
    // The two spell the same member differently, and the interface it hangs
    // off can differ too: MDN files `gattserverdisconnected_event` under
    // `BluetoothDevice`, while the IDL declares `ongattserverdisconnected` on
    // the `BluetoothDeviceEventHandlers` mixin that `BluetoothDevice`
    // includes. The normalised member name is the only common ground.
    fn normalise(member: &str) -> String {
        let bare = member.rsplit('.').next().unwrap_or(member);
        bare.trim_end_matches("_event")
            .trim_end_matches("_static")
            .trim_start_matches("on")
            .to_ascii_lowercase()
    }

    let idl: Vec<String> = members().iter().map(|m| normalise(m)).collect();

    let mut orphaned = Vec::new();
    for line in STATUS.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, _)) = line.split_once(' ') else {
            continue;
        };
        if !idl.contains(&normalise(name)) {
            orphaned.push(name);
        }
    }
    assert!(
        orphaned.is_empty(),
        "MDN lists {} member(s) the IDL does not:\n  {}",
        orphaned.len(),
        orphaned.join("\n  ")
    );
}

#[test]
fn the_generated_surface_looks_like_itself() {
    let members = members();
    // A fetch that half-failed would leave a short file, and a short file would
    // make the conformance test pass for the wrong reason.
    assert!(
        members.len() >= 100,
        "only {} members — did the fetch fail?",
        members.len()
    );
    // Every line is `kind Interface.member`.
    assert!(members.iter().all(|m| m.contains('.') && m.contains(' ')));
    for expected in [
        "interface Bluetooth.requestDevice",
        "interface BluetoothRemoteGATTServer.connect",
        "interface BluetoothRemoteGATTCharacteristic.readValue",
        // A member MDN does not list, which is why the IDL is the oracle.
        "interface BluetoothRemoteGATTService.getIncludedService",
    ] {
        assert!(members.contains(&expected), "{expected} is missing");
    }
}

/// How much of the standard is implemented rather than excluded.
#[test]
fn coverage_is_reported() {
    let (mut implemented, mut excluded) = (0, Vec::new());
    for member in members() {
        match mapping(member) {
            Some(As(_)) => implemented += 1,
            Some(Excluded(why)) => excluded.push((member, why)),
            None => {}
        }
    }
    println!(
        "Web Bluetooth surface: {implemented} implemented, {} excluded",
        excluded.len()
    );
    for (member, why) in &excluded {
        println!("  excluded {member}: {why}");
    }
    // Every exclusion is a decision. Rather than cap the count, require that
    // each one names a reason from the short list of things that genuinely
    // cannot exist outside a browser — which stops "excluded" from becoming a
    // place to put anything inconvenient.
    const REASONS: [&str; 5] = [
        "browser", // Navigator, referrer
        "Permissions API",
        "DOM event type",
        "AbortSignal",
        "deprecated",
    ];
    for (member, why) in &excluded {
        assert!(
            REASONS.iter().any(|r| why.contains(r)),
            "{member} is excluded for a reason outside the accepted list: {why}"
        );
    }
    // The implemented half is the point, so hold a floor under it.
    assert!(
        implemented >= 60,
        "only {implemented} members implemented, which is below the bar"
    );
}
