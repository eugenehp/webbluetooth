//! The Android harness: real BLE traffic, in an app process.
//!
//! ```sh
//! ./scripts/android/apk.sh peripheral   # one emulator advertises
//! ./scripts/android/apk.sh central      # another connects to it
//! ```
//!
//! `scripts/android/art.sh` proves the generated classes load and bind in ART,
//! but it runs under `dalvikvm`, which has no `Context` — so it never reaches
//! `BluetoothManager` and never moves a packet. This does: it is loaded by an
//! `Activity` that holds the `Context` and the runtime scan and connect
//! grants, which is the only way Android will let any of it happen.
//!
//! Both ends are this crate. The peripheral publishes the same fixture as
//! `scripts/qemu/peer.py` and `examples/ios-harness.rs`, so the central half is
//! the same conversation tested everywhere else.

// A `cdylib` and so deliberately without a `main`: the entry points are
// `JNI_OnLoad` and the `Java_..._nativeRun` symbol, called by ART.

#[cfg(target_os = "android")]
mod harness {
    use futures_executor::block_on;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::OnceLock;
    use std::time::Duration;
    use webbluetooth::peripheral::{Advertising, Characteristic, Peripheral, Request, Service};
    use webbluetooth::prelude::*;
    use webbluetooth::{Bluetooth, DeviceFilter, RequestDeviceOptions};
    use webbluetooth_android::jni::{Env, JClass, JObject, JString, Vm, JNI_VERSION_1_6};
    use webbluetooth_android::runtime::Runtime;

    /// The fixture, identical to the one every other peer here publishes.
    const SERVICE: &str = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";
    const LEVEL: &str = "6e400003-b5a3-f393-e0a9-e50e24dcca9e";
    const CONTROL: &str = "6e400002-b5a3-f393-e0a9-e50e24dcca9e";

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(
            priority: i32,
            tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char,
        ) -> i32;
    }

    /// Write one line to logcat, now.
    ///
    /// The peripheral role waits minutes for a central, so a report returned
    /// at the end tells you nothing while it matters — and nothing at all if
    /// the process is killed first.
    fn log(line: &str) {
        const INFO: i32 = 4;
        if let Ok(text) = std::ffi::CString::new(line) {
            unsafe { __android_log_write(INFO, c"wbharness".as_ptr(), text.as_ptr()) };
        }
    }

    static VM: OnceLock<usize> = OnceLock::new();
    static BATTERY: AtomicU8 = AtomicU8::new(87);

    /// ART calls this on `System.loadLibrary`, and it is the only place a
    /// native library is handed the `JavaVM`.
    #[no_mangle]
    pub extern "C" fn JNI_OnLoad(vm: Vm, _reserved: *mut c_void) -> i32 {
        let _ = VM.set(vm.0 as usize);
        JNI_VERSION_1_6
    }

    #[no_mangle]
    pub extern "C" fn Java_io_webbluetooth_harness_Harness_nativeRun(
        env: Env,
        _class: JClass,
        context: JObject,
        role: JString,
    ) -> JString {
        let role = env.get_string(role).unwrap_or_else(|| "central".into());
        let report = match run(env, context, &role) {
            Ok(report) => report,
            Err(message) => format!("!! {message}"),
        };
        env.new_string(&report)
    }

    fn run(env: Env, context: JObject, role: &str) -> Result<String, String> {
        let raw = VM.get().ok_or("JNI_OnLoad never ran")?;
        let vm = Vm(*raw as *mut _);

        // The Context is what the backend needs to reach BluetoothManager;
        // everything below fails without it.
        let runtime =
            Runtime::init(vm, context).map_err(|e| format!("Runtime::init failed: {e}"))?;
        let _ = env;

        // Report the underlying reason before going further. The backend
        // collapses every failure here into `Availability::Unsupported`, which
        // reads as "no Bluetooth LE support" even when the real problem is a
        // missing method or a pending JNI exception.
        if let Err(e) = webbluetooth_android::ble::Adapter::open(runtime) {
            return Err(format!("Adapter::open: {e}"));
        }

        match role {
            "peripheral" => block_on(peripheral()),
            _ => block_on(central()),
        }
    }

    /// Publish the fixture and answer whatever a central asks of it.
    async fn peripheral() -> Result<String, String> {
        let (peripheral, mut requests) = Peripheral::new();
        if let Err(why) = peripheral.availability().await {
            return Err(format!("Bluetooth unavailable: {why}"));
        }

        let level = Characteristic::new(LEVEL)
            .map_err(|e| e.to_string())?
            .read()
            .notify();
        let control = Characteristic::new(CONTROL)
            .map_err(|e| e.to_string())?
            .write()
            .write_without_response();
        let published = peripheral
            .publish(
                Service::new(SERVICE)
                    .map_err(|e| e.to_string())?
                    .characteristic(level)
                    .characteristic(control),
            )
            .await
            .map_err(|e| e.to_string())?;
        log(&format!("published {}", published.uuid()));

        peripheral
            // No local name. Android's `AdvertiseData` cannot carry a custom
            // one — `local_name` becomes `setIncludeDeviceName(true)`, which
            // uses the *adapter's* name — and an emulator is called
            // "sdk_gphone64_arm64". Eighteen characters plus a 128-bit service
            // UUID is well past the 31 bytes an advertisement holds, and
            // Android refuses the whole thing. The central filters on the
            // service, which is what identifies this peer anyway.
            .start_advertising(
                Advertising::new()
                    .service(SERVICE)
                    .map_err(|e| e.to_string())?,
            )
            .await
            .map_err(|e| e.to_string())?;
        log("HARNESS READY — advertising the fixture service");

        let value = published
            .characteristic(LEVEL)
            .ok_or("the level characteristic vanished")?
            .clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(3));
            let next = BATTERY.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
            if next == 0 {
                BATTERY.store(100, Ordering::Relaxed);
            }
            if !value.subscribers().is_empty() {
                let _ = value.try_notify(&[next]);
            }
        });

        // Report as it happens: the caller only sees the string at the end, so
        // anything interesting has to accumulate here.
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            let Some(request) = requests.next().await else {
                break;
            };
            match request {
                Request::Read(read) => {
                    let value = BATTERY.load(Ordering::Relaxed);
                    log(&format!("read  {} -> {value}", read.characteristic()));
                    read.respond(&[value]).map_err(|e| e.to_string())?;
                }
                Request::Write(write) => {
                    for w in write.writes() {
                        log(&format!("write {} <- {:02x?}", w.characteristic, w.value));
                    }
                    write.accept();
                }
                Request::Subscribed { characteristic, .. } => {
                    log(&format!("subscribe {characteristic}"));
                }
                Request::Unsubscribed { characteristic, .. } => {
                    log(&format!("unsubscribe {characteristic}"));
                    break;
                }
                _ => {}
            }
        }
        Ok("done".into())
    }

    /// Find the fixture, then read, subscribe and write to it.
    async fn central() -> Result<String, String> {
        let bluetooth = Bluetooth::with_chooser(webbluetooth::chooser::StrongestSignal::new(
            Duration::from_secs(8),
        ));
        if let Err(why) = bluetooth.availability().await {
            return Err(format!("Bluetooth unavailable: {why}"));
        }
        log(&format!("scanning for {SERVICE}"));

        let device = bluetooth
            .request_device(
                RequestDeviceOptions::new().filter(
                    DeviceFilter::new()
                        .service(SERVICE)
                        .map_err(|e| e.to_string())?,
                ),
            )
            .await
            .map_err(|e| e.to_string())?;
        let gatt = device.gatt();
        gatt.connect().await.map_err(|e| e.to_string())?;
        log(&format!(
            "connected to {}\n",
            device.name().unwrap_or_else(|| device.id().into())
        ));

        let service = gatt
            .get_primary_service(SERVICE)
            .await
            .map_err(|e| e.to_string())?;
        let level = service
            .get_characteristic(LEVEL)
            .await
            .map_err(|e| e.to_string())?;

        let value = level.read_value().await.map_err(|e| e.to_string())?;
        log(&format!("read     {:?}", value.first()));

        let mut ticks = level
            .start_notifications()
            .await
            .map_err(|e| e.to_string())?;
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while seen.len() < 2 && std::time::Instant::now() < deadline {
            match ticks.next().await {
                Some(v) => seen.push(v.first().copied().unwrap_or_default()),
                None => break,
            }
        }
        log(&format!("notify   {seen:?}"));

        let control = service
            .get_characteristic(CONTROL)
            .await
            .map_err(|e| e.to_string())?;
        control
            .write_value_with_response(&[0x2a])
            .await
            .map_err(|e| e.to_string())?;
        log("write    control point accepted 0x2a");

        log(if seen.len() >= 2 {
            "HARNESS READY — round-trip complete"
        } else {
            "!! no notifications arrived"
        });
        gatt.disconnect();
        Ok("done".into())
    }
}
