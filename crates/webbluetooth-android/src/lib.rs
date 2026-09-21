//! Android BLE from pure Rust, with no dependencies.
//!
//! ## License
//! MIT — Copyright © 2025 [Eugene Hauptmann](https://github.com/eugenehp)
// The adapter: this platform expressed as the portable model.
// Public so `webbluetooth` can reach it, and not an API anyone else
// should call: the facade is what a caller holds. Hidden from the docs
// for the same reason, which also keeps the same 38 adapter methods from
// being documented six times over.
#[doc(hidden)]
pub mod backend;
// The peripheral-role adapter: a GATT server expressed as the portable model.
#[cfg(target_os = "android")]
pub mod peripheral_backend;

pub mod ble;
pub mod bluetooth;
pub mod dex;
pub mod jni;
pub mod l2cap;
pub mod runtime;

use dex::DexBuilder;

/// The package the generated classes live in.
pub const PACKAGE: &str = "dev/webbluetooth";

/// Every callback class this crate synthesises, as `(name, dex)`.
///
/// Each one subclasses an abstract Android callback and declares its overrides
/// `native`, so `RegisterNatives` can bind them to Rust functions. The method
/// signatures are the ones Android declares — a mismatch does not fail at
/// registration, it fails the first time the framework tries to call in.
pub fn dex_classes() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("gatt", gatt_callback_dex()),
        ("scan", scan_callback_dex()),
        ("server", gatt_server_callback_dex()),
        ("advertise", advertise_callback_dex()),
    ]
}

/// `BluetoothGattCallback` — the GATT client.
pub fn gatt_callback_dex() -> Vec<u8> {
    // Built from the native table rather than beside it.
    //
    // `RegisterNatives` rejects the whole table if a single name is not
    // declared by the class, so the two have to agree exactly. They used to be
    // two lists with a test counting them, which catches a mismatch only after
    // someone writes one — and is fixed by editing the count. Deriving one
    // from the other means there is nothing to keep in step.
    let mut builder = DexBuilder::new(
        &format!("{PACKAGE}/GattCallback"),
        "android/bluetooth/BluetoothGattCallback",
    );
    for (name, descriptor, _) in crate::bluetooth::gatt_natives() {
        builder = builder.native_method(name, descriptor);
    }
    builder.build()
}

/// `ScanCallback` — LE discovery.
pub fn scan_callback_dex() -> Vec<u8> {
    DexBuilder::new(
        &format!("{PACKAGE}/ScanCallback"),
        "android/bluetooth/le/ScanCallback",
    )
    .native_method("onScanResult", "(ILandroid/bluetooth/le/ScanResult;)V")
    .native_method("onBatchScanResults", "(Ljava/util/List;)V")
    .native_method("onScanFailed", "(I)V")
    .build()
}

/// `BluetoothGattServerCallback` — the peripheral role.
pub fn gatt_server_callback_dex() -> Vec<u8> {
    const DEVICE: &str = "Landroid/bluetooth/BluetoothDevice;";
    const CHARACTERISTIC: &str = "Landroid/bluetooth/BluetoothGattCharacteristic;";
    const DESCRIPTOR: &str = "Landroid/bluetooth/BluetoothGattDescriptor;";
    const SERVICE: &str = "Landroid/bluetooth/BluetoothGattService;";
    DexBuilder::new(
        &format!("{PACKAGE}/GattServerCallback"),
        "android/bluetooth/BluetoothGattServerCallback",
    )
    .native_method("onConnectionStateChange", &format!("({DEVICE}II)V"))
    .native_method("onServiceAdded", &format!("(I{SERVICE})V"))
    .native_method(
        "onCharacteristicReadRequest",
        &format!("({DEVICE}II{CHARACTERISTIC})V"),
    )
    .native_method(
        "onCharacteristicWriteRequest",
        &format!("({DEVICE}I{CHARACTERISTIC}ZZI[B)V"),
    )
    .native_method(
        "onDescriptorReadRequest",
        &format!("({DEVICE}II{DESCRIPTOR})V"),
    )
    .native_method(
        "onDescriptorWriteRequest",
        &format!("({DEVICE}I{DESCRIPTOR}ZZI[B)V"),
    )
    .native_method("onNotificationSent", &format!("({DEVICE}I)V"))
    .native_method("onMtuChanged", &format!("({DEVICE}I)V"))
    .build()
}

/// `AdvertiseCallback` — advertising.
pub fn advertise_callback_dex() -> Vec<u8> {
    DexBuilder::new(
        &format!("{PACKAGE}/AdvertiseCallback"),
        "android/bluetooth/le/AdvertiseCallback",
    )
    .native_method(
        "onStartSuccess",
        "(Landroid/bluetooth/le/AdvertiseSettings;)V",
    )
    .native_method("onStartFailure", "(I)V")
    .build()
}
