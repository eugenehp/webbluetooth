//! Run webbluetooth on a real iOS device, as a real app.
//!
//! ```sh
//! ./scripts/ios.sh peripheral   # the iPad advertises a Battery Service
//! ./scripts/ios.sh central      # the iPad connects to one
//! ```
//!
//! iOS only shows the Bluetooth prompt to an app that is in the foreground with
//! a window on screen. A bundle launched purely for its console output never
//! becomes active, so it is never asked, its manager stays unauthorised, and a
//! harness that is only a `main` can test nothing. This one boots a real
//! `UIApplication` first, then does the work on a background thread — whatever
//! it prints comes back over the device console.
//!
//! The application delegate is synthesised at runtime, like every other
//! delegate in this workspace: there is no Objective-C source here either.

#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!(
        "ios-harness is iOS-only. On this platform use the peripheral, battery \
         or scan examples directly."
    );
}

#[cfg(target_os = "ios")]
fn main() {
    ios::boot()
}

#[cfg(target_os = "ios")]
mod ios {
    use futures_executor::block_on;
    use std::ffi::c_char;
    use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, AtomicUsize, Ordering};
    use std::time::Duration;
    use webbluetooth::peripheral::{
        Advertising, Characteristic, Descriptor, Peripheral, Request, Service,
    };
    use webbluetooth::prelude::*;
    use webbluetooth::uuid::{characteristics, services};
    use webbluetooth::{Bluetooth, DeviceFilter, RequestDeviceOptions};
    use webbluetooth_apple::objc::{self, ClassBuilder, Id, Sel, NIL};
    use webbluetooth_apple::{msg_send, msg_send_t, msg_send_void};

    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" {
        fn UIApplicationMain(argc: i32, argv: *mut *mut c_char, principal: Id, delegate: Id)
            -> i32;
    }

    // arm64 returns and passes these in the floating-point registers, so a
    // correctly-typed `objc_msgSend` handles them without any special case.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    /// The window, which must outlive every frame the app draws.
    static WINDOW: AtomicUsize = AtomicUsize::new(0);
    /// The value served for battery reads and pushed on notify.
    static BATTERY: AtomicU8 = AtomicU8::new(87);
    /// Set once `applicationDidBecomeActive:` has been seen.
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    /// The PSM the system assigned, served to whoever reads `PSM_CHAR`.
    static PSM: AtomicU16 = AtomicU16::new(0);

    const DELEGATE: &std::ffi::CStr = c"WBRustHarnessDelegate";

    // A private service, not the assigned Battery Service.
    //
    // On iOS an app's services are added to the *device's* GATT database,
    // alongside the ones iOS publishes itself. Reusing 0x180F puts two Battery
    // Services on one server, and a central asking for "the" battery service
    // gets whichever comes first — in practice iOS's own, whose level needs an
    // encrypted link. A private UUID keeps the fixture unambiguous.
    const SERVICE: &str = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";
    const LEVEL: &str = "6e400003-b5a3-f393-e0a9-e50e24dcca9e";
    const CONTROL: &str = "6e400002-b5a3-f393-e0a9-e50e24dcca9e";
    const PSM_CHAR: &str = "6e400004-b5a3-f393-e0a9-e50e24dcca9e";

    pub fn boot() -> ! {
        unsafe {
            if objc::class(DELEGATE).is_null() {
                ClassBuilder::new(c"UIResponder", DELEGATE)
                    .expect("allocate the delegate class")
                    .method(
                        c"application:didFinishLaunchingWithOptions:",
                        did_finish_launching as *const _,
                        // BOOL, self, _cmd, UIApplication*, NSDictionary*
                        c"B@:@@",
                    )
                    .method(
                        c"applicationDidBecomeActive:",
                        did_become_active as *const _,
                        // void, self, _cmd, UIApplication*
                        c"v@:@",
                    )
                    .conforms(c"UIApplicationDelegate")
                    .register();
            }
            UIApplicationMain(
                0,
                core::ptr::null_mut(),
                NIL,
                objc::nsstring("WBRustHarnessDelegate"),
            );
        }
        unreachable!("UIApplicationMain does not return")
    }

    extern "C" fn did_finish_launching(_this: Id, _cmd: Sel, _app: Id, _options: Id) -> bool {
        unsafe { present_window() };
        // The role blocks, so it cannot run on this thread — the main thread
        // has to keep servicing the run loop or the watchdog kills the app.
        //
        // Starting here rather than in `applicationDidBecomeActive:` is
        // deliberate: that callback was not observed to fire under a
        // `devicectl` launch, and waiting for it meant never starting at all.
        // The prompt does not need the manager to be created while active —
        // it is presented once the app becomes active, however much later.
        std::thread::spawn(run_role);
        true
    }

    /// Only a signal that the app reached the foreground.
    extern "C" fn did_become_active(_this: Id, _cmd: Sel, _app: Id) {
        if !ACTIVE.swap(true, Ordering::SeqCst) {
            println!("   app is active");
        }
    }

    /// Block until the Bluetooth prompt has been answered, or give up.
    ///
    /// `availability()` settles for five seconds and then reports whatever it
    /// has, which on a first run is `Unknown` — the state stays unknown until
    /// someone taps the prompt, and nobody taps it in five seconds. A manager
    /// must already exist when this is called, since creating one is what
    /// raises the prompt.
    fn await_grant() {
        use webbluetooth::Authorization;
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let mut asked = false;
        while webbluetooth::authorization() == Authorization::NotDetermined {
            if !asked {
                println!(
                    "   ⏳ approve the Bluetooth prompt on the device (app active: {})",
                    ACTIVE.load(Ordering::SeqCst)
                );
                asked = true;
            }
            if std::time::Instant::now() > deadline {
                println!("   … no answer after 120s; carrying on unauthorised");
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        println!("   permission {:?}", webbluetooth::authorization());
    }

    /// A plain window with a root view controller.
    ///
    /// Nothing is drawn in it. It exists because an app with no window never
    /// becomes active, and an inactive app is never offered the prompt.
    unsafe fn present_window() {
        let screen: Id = msg_send![objc::require_class(c"UIScreen"), mainScreen];
        let bounds: CGRect = msg_send_t![CGRect; screen, bounds];
        let window: Id = msg_send![
            msg_send![objc::require_class(c"UIWindow"), alloc],
            initWithFrame: bounds
        ];
        let controller: Id = msg_send![objc::require_class(c"UIViewController"), new];
        msg_send_void![window, setRootViewController: controller];
        msg_send_void![window, makeKeyAndVisible];
        WINDOW.store(window as usize, Ordering::SeqCst);
    }

    fn run_role() {
        let role = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "peripheral".into());
        println!("── webbluetooth iOS harness: {role} ──");
        println!("   permission {:?}", webbluetooth::authorization());
        let outcome = match role.as_str() {
            "central" => block_on(central()),
            _ => block_on(peripheral()),
        };
        match outcome {
            Ok(()) => println!("── done ──"),
            Err(e) => println!("!! {e}"),
        }
    }

    /// Advertise a Battery Service and answer whatever a central asks of it.
    async fn peripheral() -> Result<(), String> {
        let (peripheral, mut requests) = Peripheral::new();
        await_grant();
        if let Err(why) = peripheral.availability().await {
            return Err(format!("Bluetooth unavailable: {why}"));
        }

        let level = Characteristic::new(LEVEL)
            .map_err(|e| e.to_string())?
            .read()
            .notify()
            .descriptor(Descriptor::user_description("Battery level, percent"));
        let control = Characteristic::new(CONTROL)
            .map_err(|e| e.to_string())?
            .write()
            .write_without_response();
        // A PSM is not discoverable: nothing in GATT advertises one, and the
        // far side has to be told the number. So the number goes in a
        // characteristic, which is how real peers do it too.
        let psm = Characteristic::new(PSM_CHAR)
            .map_err(|e| e.to_string())?
            .read()
            .descriptor(Descriptor::user_description("L2CAP PSM, little-endian u16"));

        let published = peripheral
            .publish(
                Service::new(SERVICE)
                    .map_err(|e| e.to_string())?
                    .characteristic(level)
                    .characteristic(control)
                    .characteristic(psm),
            )
            .await
            .map_err(|e| e.to_string())?;
        println!("   published {}", published.uuid());

        // Not part of Web Bluetooth, and the one thing the round trip never
        // exercised over a real radio until now.
        let psm = peripheral
            .publish_l2cap_channel(false)
            .await
            .map_err(|e| e.to_string())?;
        PSM.store(psm, Ordering::Relaxed);
        println!("   l2cap psm {psm:#06x}");

        peripheral
            .start_advertising(
                Advertising::new()
                    .local_name("Rust iPad")
                    .service(SERVICE)
                    .map_err(|e| e.to_string())?,
            )
            .await
            .map_err(|e| e.to_string())?;
        println!("HARNESS READY — advertising as \"Rust iPad\"");

        let battery = published.characteristic(LEVEL).expect("just published");
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let next = BATTERY.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
            if next == 0 {
                BATTERY.store(100, Ordering::Relaxed);
            }
            if !battery.subscribers().is_empty() && battery.try_notify(&[next]) {
                println!("   notified {next}%");
            }
        });

        let psm_uuid = BluetoothUuid::parse(PSM_CHAR).map_err(|e| e.to_string())?;

        while let Some(request) = requests.next().await {
            match request {
                // Compared as a UUID, not as text. `characteristic()` is
                // canonical lowercase and `PSM_CHAR` happens to be written
                // that way, so `as_str() == PSM_CHAR` works — until somebody
                // writes the constant in the uppercase that CBUUID hands back,
                // and then it silently never matches and the channel never
                // opens. Parsing both sides removes the question.
                Request::Read(read) if read.characteristic() == &psm_uuid => {
                    let psm = PSM.load(Ordering::Relaxed);
                    println!("   read  psm → {psm:#06x}");
                    read.respond(&psm.to_le_bytes())
                        .map_err(|e| e.to_string())?;
                }
                Request::Read(read) => {
                    let value = BATTERY.load(Ordering::Relaxed);
                    println!("   read  {} → {value}%", read.characteristic());
                    read.respond(&[value]).map_err(|e| e.to_string())?;
                }
                Request::ChannelOpened(channel) => {
                    println!("   l2cap opened by {}", channel.peer_id());
                    // Echo, on its own thread: the channel outlives this match
                    // arm and the request loop must not stop to serve it.
                    std::thread::spawn(move || {
                        block_on(async move {
                            let Some(mut incoming) = channel.take_incoming() else {
                                return;
                            };
                            while let Some(chunk) = incoming.next().await {
                                println!("   l2cap echo {} bytes", chunk.len());
                                if channel.send(&chunk).is_err() {
                                    break;
                                }
                            }
                            println!("   l2cap closed");
                        });
                    });
                }
                Request::Write(write) => {
                    for w in write.writes() {
                        println!("   write {} ← {:02x?}", w.characteristic, w.value);
                    }
                    write.accept();
                }
                Request::Subscribed {
                    central,
                    characteristic,
                } => {
                    println!(
                        "   subscribe {characteristic} by {} (mtu {})",
                        central.id(),
                        central.max_notification_length()
                    );
                }
                Request::Unsubscribed { characteristic, .. } => {
                    println!("   unsubscribe {characteristic}");
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Connect to the nearest Battery Service and read it.
    async fn central() -> Result<(), String> {
        let bluetooth = Bluetooth::with_chooser(webbluetooth::chooser::StrongestSignal::default());
        await_grant();
        if let Err(why) = bluetooth.availability().await {
            return Err(format!("Bluetooth unavailable: {why}"));
        }
        println!("   scanning for a Battery Service…");
        let device = bluetooth
            .request_device(
                RequestDeviceOptions::new().filter(
                    DeviceFilter::new()
                        .service(services::BATTERY_SERVICE)
                        .map_err(|e| e.to_string())?,
                ),
            )
            .await
            .map_err(|e| e.to_string())?;

        let gatt = device.gatt();
        gatt.connect().await.map_err(|e| e.to_string())?;
        println!(
            "   connected to {}",
            device.name().unwrap_or_else(|| device.id().into())
        );

        let service = gatt
            .get_primary_service(services::BATTERY_SERVICE)
            .await
            .map_err(|e| e.to_string())?;
        let level = service
            .get_characteristic(characteristics::BATTERY_LEVEL)
            .await
            .map_err(|e| e.to_string())?;
        let value = level.read_value().await.map_err(|e| e.to_string())?;
        println!("HARNESS READY — battery {:?}%", value.first());
        Ok(())
    }
}
