//! `webbluetooth::prelude::*` and nothing else.
//!
//! The prelude's documentation says it is "everything a program needs in
//! scope, in one import", and that the reason it exists is that a caller
//! should not have to depend on `futures-util` to use this crate. That claim
//! was true for reading a notification stream and false for writing a
//! `DeviceChooser`: `choose` returns a `BoxFuture`, which was named in the
//! trait's signature and exported nowhere, so implementing the crate's own
//! extension point meant adding `futures-util` to your own manifest. Both
//! chooser examples in this repository do exactly that.
//!
//! A claim about what a caller needs in scope can only be checked from the
//! position of a caller, which is what this file is: one import, and then the
//! programs somebody actually writes. It compiles or the claim is wrong.
//!
//! The bodies never run — there is no radio in a test — so the assertions are
//! the signatures.

#![allow(
    dead_code,
    unreachable_code,
    unused_variables,
    clippy::diverging_sub_expression
)]

use webbluetooth::prelude::*;

/// A chooser, which is the case that was broken.
struct Nearest;

impl DeviceChooser for Nearest {
    fn choose(&self, mut candidates: Candidates) -> BoxFuture<'static, Option<String>> {
        Box::pin(async move {
            let mut best: Option<Candidate> = None;
            while let Some(candidate) = candidates.next().await {
                if candidate.rssi() > best.as_ref().and_then(Candidate::rssi) {
                    best = Some(candidate);
                }
            }
            best.map(|candidate| candidate.id)
        })
    }
}

/// Connect to something and read from it — the README's first program.
///
/// `Box<dyn Error>` rather than `webbluetooth::Result`, deliberately: the
/// prelude leaves `Result` out to avoid shadowing `std`'s, so this is the
/// shape a caller's own error type takes. It also means `?` has to work on
/// both of this crate's failures — including `Availability`, which is not an
/// `Error` variant.
async fn the_first_program() -> Result<(), Box<dyn std::error::Error>> {
    let bluetooth: Bluetooth = Bluetooth::with_chooser(Nearest);
    bluetooth.availability().await?;

    let device: BluetoothDevice = bluetooth
        .request_device(
            RequestDeviceOptions::new()
                .filter(DeviceFilter::new().name_prefix("Device"))
                .optional_service(0x180fu16)?,
        )
        .await?;

    let gatt: RemoteGattServer = device.gatt();
    gatt.connect().await?;

    let service: RemoteGattService = gatt.get_primary_service("battery_service").await?;
    let characteristic: RemoteGattCharacteristic =
        service.get_characteristic("battery_level").await?;
    let uuid: BluetoothUuid = *characteristic.uuid();

    let mut values = characteristic.start_notifications().await?;
    while let Some(value) = values.next().await {
        println!("{uuid}: {value:02x?}");
    }
    Ok(())
}

/// Scan, then adopt a sighting — the path that has no chooser in it.
async fn the_scanning_program() -> Result<(), Box<dyn std::error::Error>> {
    let bluetooth = Bluetooth::shared();

    let mut scan = bluetooth
        .request_le_scan(
            LeScanOptions::new()
                .filter(DeviceFilter::new().name_prefix("Device"))
                .keep_repeated_devices(true),
        )
        .await?;

    // No `break`: a scanning program runs until the stream ends, and an
    // unconditional break makes this `while let` a `clippy::never_loop`.
    while let Some(sighting) = scan.next().await {
        let grant: Grant = Grant::new().service(0x180fu16)?;
        let device = bluetooth.adopt_candidate(&sighting, grant).await?;
        device.gatt().connect().await?;
    }
    Ok(())
}

/// Filtering on advertised data, which is the one that needs `DataPrefix`.
fn the_filters() {
    let _ = RequestDeviceOptions::new()
        .filter(DeviceFilter::new().manufacturer_data(0x004c, DataPrefix::new([0x02, 0x15])))
        .exclusion_filter(DeviceFilter::new().name("not this one"));
}

#[test]
fn it_all_compiles_from_one_import() {
    // The assertions are the signatures above. This exists so `cargo test`
    // reports the file, and so a future reader sees it is deliberate.
}
