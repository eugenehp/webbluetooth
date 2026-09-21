//! Report exactly why Bluetooth is or is not usable, and what to do about it.
//!
//! ```sh
//! cargo run -p webbluetooth --example doctor
//! ```
//!
//! Run this first. Almost every "it finds no devices" report is a permission
//! problem rather than anything to do with the radio — and the permission model
//! is completely different on each platform, so the advice is too.

use futures_executor::block_on;
use webbluetooth::{Authorization, Availability, Bluetooth};

fn main() {
    println!("webbluetooth doctor\n");

    println!(
        "  platform       {}{}",
        platform(),
        board().map(|b| format!(" — {b}")).unwrap_or_default()
    );
    println!(
        "  peripheral     {}",
        if webbluetooth::PERIPHERAL_ROLE {
            "available"
        } else {
            "unavailable on this platform"
        }
    );

    print!("  permission     ");
    let auth = webbluetooth::authorization();
    match auth {
        Authorization::Allowed => println!("✓ allowed"),
        Authorization::NotDetermined => println!("? not determined"),
        Authorization::Denied => println!("✗ denied"),
        Authorization::Restricted => println!("✗ restricted by policy"),
    }

    for (label, ok, note) in platform_checks() {
        println!("  {label:<14} {} {note}", if ok { "✓" } else { "✗" });
    }

    print!("  adapter        ");
    let bluetooth = Bluetooth::new();
    let availability = block_on(bluetooth.availability());
    match availability {
        Ok(()) => println!("✓ powered on and ready"),
        Err(Availability::PoweredOff) => println!("✗ powered off"),
        Err(Availability::Unauthorized) => println!("✗ unauthorized"),
        Err(Availability::Unsupported) => println!("✗ no Bluetooth LE support"),
        Err(Availability::Resetting) => println!("… resetting"),
        Err(Availability::Unknown) => println!("✗ state never reported"),
    }

    println!();
    if availability.is_ok() {
        println!("Ready. Try:  cargo run -p webbluetooth --example scan");
    } else {
        advice(auth, availability);
    }
}

/// The board this is running on, where the kernel names one.
///
/// A Raspberry Pi says so in the device tree, and it matters: its Bluetooth
/// controller is on a UART rather than USB, which gives it failure modes no
/// other Linux machine has.
#[cfg(target_os = "linux")]
fn board() -> Option<String> {
    board_from(&std::fs::read("/proc/device-tree/model").ok()?)
}

/// Parse a device-tree string property.
///
/// These are NUL-terminated, and the terminator is part of the file — reading
/// one without trimming it gives a string that compares equal to nothing and
/// prints a stray byte.
#[cfg(target_os = "linux")]
fn board_from(bytes: &[u8]) -> Option<String> {
    let model = String::from_utf8_lossy(bytes);
    let model = model.trim_end_matches('\0').trim();
    (!model.is_empty()).then(|| model.to_string())
}

#[cfg(not(target_os = "linux"))]
fn board() -> Option<String> {
    None
}

/// Is this a Raspberry Pi?
#[cfg(target_os = "linux")]
fn is_raspberry_pi() -> bool {
    board().is_some_and(|b| b.contains("Raspberry Pi"))
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS (CoreBluetooth)"
    } else if cfg!(target_os = "ios") {
        "iOS (CoreBluetooth)"
    } else if cfg!(target_os = "tvos") {
        "tvOS (CoreBluetooth)"
    } else if cfg!(target_os = "watchos") {
        "watchOS (CoreBluetooth)"
    } else if cfg!(target_os = "visionos") {
        "visionOS (CoreBluetooth)"
    } else if cfg!(target_os = "linux") {
        "Linux (BlueZ over D-Bus)"
    } else {
        "unsupported"
    }
}

// ── Apple: the gate is TCC, and it reads an embedded Info.plist ─────────────

#[cfg(target_vendor = "apple")]
fn platform_checks() -> Vec<(&'static str, bool, String)> {
    // Where the usage string has to live differs by platform, so checking the
    // wrong place reports a failure that is not one. macOS runs unbundled
    // binaries and reads the key from the running image's `__TEXT,__info_plist`
    // section. Every other Apple platform only runs bundled apps, and reads it
    // from the bundle — the section is neither present nor wanted there.
    #[cfg(target_os = "macos")]
    {
        let present = has_info_plist();
        vec![(
            "Info.plist",
            present,
            if present {
                "NSBluetoothAlwaysUsageDescription in __TEXT,__info_plist".into()
            } else {
                "missing — macOS will deny Bluetooth without prompting".into()
            },
        )]
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (present, note) = bundle_usage_description();
        vec![("Info.plist", present, note)]
    }
}

/// Is `NSBluetoothAlwaysUsageDescription` in the app bundle's `Info.plist`?
///
/// The bundle is the directory holding the executable, so this reads the file
/// rather than asking `NSBundle` — the answer is the same and it needs nothing
/// from Foundation.
#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
fn bundle_usage_description() -> (bool, String) {
    const KEY: &str = "NSBluetoothAlwaysUsageDescription";
    let Ok(exe) = std::env::current_exe() else {
        return (false, "cannot locate the executable".into());
    };
    let Some(plist) = exe.parent().map(|d| d.join("Info.plist")) else {
        return (false, "executable has no parent directory".into());
    };
    match std::fs::read(&plist) {
        Ok(bytes) if String::from_utf8_lossy(&bytes).contains(KEY) => {
            (true, format!("{KEY} in the app bundle"))
        }
        Ok(_) => (false, format!("{KEY} missing from {}", plist.display())),
        Err(e) => (false, format!("{}: {e}", plist.display())),
    }
}

/// Is `NSBluetoothAlwaysUsageDescription` in this image's `__TEXT,__info_plist`?
#[cfg(target_os = "macos")]
fn has_info_plist() -> bool {
    unsafe extern "C" {
        fn _dyld_get_image_header(index: u32) -> *const MachHeader64;
        fn getsectiondata(
            mh: *const MachHeader64,
            segname: *const std::ffi::c_char,
            sectname: *const std::ffi::c_char,
            size: *mut u64,
        ) -> *const u8;
    }
    #[repr(C)]
    struct MachHeader64 {
        _fields: [u32; 8],
    }

    unsafe {
        let header = _dyld_get_image_header(0);
        if header.is_null() {
            return false;
        }
        let mut size: u64 = 0;
        let data = getsectiondata(
            header,
            c"__TEXT".as_ptr(),
            c"__info_plist".as_ptr(),
            &mut size,
        );
        if data.is_null() || size == 0 {
            return false;
        }
        let needle = b"NSBluetoothAlwaysUsageDescription";
        std::slice::from_raw_parts(data, size as usize)
            .windows(needle.len())
            .any(|w| w == needle)
    }
}

// ── Linux: the gate is D-Bus policy, and the adapter is an object on the bus ─

#[cfg(target_os = "linux")]
fn platform_checks() -> Vec<(&'static str, bool, String)> {
    use webbluetooth_linux::bluez::{interfaces, Bluez, Event, EventSink};

    struct Discard;
    impl EventSink for Discard {
        fn emit(&self, _: Event) {}
    }

    let address = std::env::var("WEBBLUETOOTH_DBUS_ADDRESS")
        .or_else(|_| std::env::var("DBUS_SYSTEM_BUS_ADDRESS"))
        .unwrap_or_else(|_| "unix:path=/var/run/dbus/system_bus_socket".into());

    let mut checks = Vec::new();

    // On a Pi the controller is attached over a UART by a systemd unit, so it
    // can be absent for reasons that have nothing to do with D-Bus.
    if is_raspberry_pi() {
        let controllers: Vec<String> = std::fs::read_dir("/sys/class/bluetooth")
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        checks.push((
            "controller",
            !controllers.is_empty(),
            if controllers.is_empty() {
                "nothing in /sys/class/bluetooth — the UART controller is not attached".into()
            } else {
                controllers.join(", ")
            },
        ));

        // `dtoverlay=disable-bt` removes the controller outright, and is the
        // most common reason a Pi has no Bluetooth at all.
        for config in ["/boot/firmware/config.txt", "/boot/config.txt"] {
            let Ok(text) = std::fs::read_to_string(config) else {
                continue;
            };
            let disabled = text
                .lines()
                .map(str::trim)
                .any(|l| !l.starts_with('#') && l.contains("disable-bt"));
            checks.push((
                "config.txt",
                !disabled,
                if disabled {
                    format!("{config} has dtoverlay=disable-bt — remove it and reboot")
                } else {
                    format!("{config}: Bluetooth not disabled")
                },
            ));
            break;
        }
    }

    match Bluez::new(std::sync::Arc::new(Discard)) {
        Ok(bluez) => {
            checks.push(("system bus", true, address));
            match bluez.adapter() {
                Some(path) => {
                    checks.push(("bluetoothd", true, "org.bluez is on the bus".into()));
                    let powered = bluez.is_powered();
                    checks.push((
                        "adapter",
                        powered,
                        format!("{path}{}", if powered { "" } else { " (Powered = false)" }),
                    ));
                }
                None => {
                    let reachable = !bluez.paths_with(interfaces::ADAPTER).is_empty();
                    checks.push((
                        "bluetoothd",
                        reachable,
                        "org.bluez answered, but published no adapter".into(),
                    ));
                }
            }
        }
        Err(e) => {
            checks.push(("system bus", false, format!("{address} — {e}")));
        }
    }
    checks
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn platform_checks() -> Vec<(&'static str, bool, String)> {
    Vec::new()
}

// ── Advice ──────────────────────────────────────────────────────────────────

#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
fn advice(auth: Authorization, _availability: Result<(), Availability>) {
    let (plist, _) = bundle_usage_description();
    match (auth, plist) {
        (Authorization::Denied, _) => {
            println!("Bluetooth was denied for this app.");
            println!("Re-enable it in Settings ▸ Privacy & Security ▸ Bluetooth.");
        }
        (_, false) => {
            println!("The app bundle's Info.plist has no NSBluetoothAlwaysUsageDescription,");
            println!("so the system denies Bluetooth without ever prompting. Add the key to");
            println!("the bundle's Info.plist and reinstall.");
        }
        (_, true) => {
            println!("The usage description is present but no grant exists yet.");
            println!();
            println!("The prompt is only shown to an app that is in the foreground with a");
            println!("window on screen. A bundle launched for its console output alone never");
            println!("becomes active, so it is never asked and the manager stays unauthorised.");
            println!("Give the app a UI, launch it, and approve the prompt once.");
        }
    }
}

#[cfg(target_os = "macos")]
fn advice(auth: Authorization, _availability: Result<(), Availability>) {
    match (auth, has_info_plist()) {
        (Authorization::Denied, _) => {
            println!("Bluetooth was denied for this binary.");
            println!("Re-enable it in System Settings ▸ Privacy & Security ▸ Bluetooth.");
        }
        (_, false) => {
            println!("This binary has no embedded Info.plist, so macOS denies Bluetooth without");
            println!("ever prompting. Link one in from a build script — see the README, under");
            println!("\"Authorization\" — or run it from an app bundle that carries the key.");
        }
        (_, true) => {
            println!("The Info.plist is present but no grant exists yet.");
            println!();
            println!("The prompt is attributed to the *responsible process* — the terminal or");
            println!("IDE that launched this one. Under a wrapper with no Bluetooth grant of its");
            println!("own, the request is denied silently. Run it straight from Terminal.app:");
            println!();
            println!("    open -a Terminal && cargo run -p webbluetooth --example doctor");
        }
    }
}

#[cfg(target_os = "linux")]
fn advice(_auth: Authorization, availability: Result<(), Availability>) {
    match availability {
        Err(Availability::Unauthorized) => {
            println!("Could not reach org.bluez on the system bus. Either bluetoothd is not");
            println!("running, or D-Bus policy does not allow this user to talk to it:");
            println!();
            println!("    systemctl status bluetooth");
            println!("    sudo usermod -aG bluetooth \"$USER\"   # then log out and back in");
            if is_raspberry_pi() {
                println!();
                println!("On Raspberry Pi OS the default user is usually already in that group,");
                println!("so a denial here more often means bluetoothd is masked or stopped.");
                println!("If you have no daemon at all, build with --features linux-hci, which");
                println!("talks to the controller directly and needs CAP_NET_RAW only to scan.");
            }
        }
        Err(Availability::Unsupported) => {
            println!("bluetoothd is reachable but published no adapter — there is no Bluetooth");
            println!("controller visible to this kernel.");
            println!();
            println!("    hciconfig -a          # or: bluetoothctl list");
            if is_raspberry_pi() {
                println!();
                println!("On a Raspberry Pi the onboard controller is attached over a UART, not");
                println!("USB, so there are three ways for it to be missing:");
                println!();
                println!("  1. dtoverlay=disable-bt in config.txt removes it. Delete the line,");
                println!("     then reboot.");
                println!("  2. The attach service is not running:");
                println!("         sudo systemctl status hciuart     # bluetooth-dev on Pi 5");
                println!("         sudo systemctl enable --now hciuart");
                println!("  3. On a Pi 3 or Zero W the controller shares the PL011 UART with the");
                println!("     serial console. Enabling that console on /dev/ttyAMA0 takes the");
                println!("     UART away from Bluetooth — use raspi-config to turn the console");
                println!("     off, or accept the reduced speed of the miniuart-bt overlay.");
            } else {
                println!();
                println!("In a container this is expected: pass a controller through with");
                println!("--net=host --privileged -v /var/run/dbus:/var/run/dbus, and note that");
                println!("Docker Desktop on macOS or Windows cannot do so at all — its VM has no");
                println!("Bluetooth hardware.");
            }
        }
        Err(Availability::PoweredOff) => {
            println!("The adapter is present but powered off:");
            println!();
            println!("    bluetoothctl power on");
        }
        _ => println!("Bluetooth is unavailable for an unknown reason."),
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn advice(_auth: Authorization, _availability: Result<(), Availability>) {
    println!("This platform is not supported.");
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::board_from;

    #[test]
    fn device_tree_strings_are_nul_terminated() {
        // Exactly what /proc/device-tree/model contains on a Pi 4.
        assert_eq!(
            board_from(b"Raspberry Pi 4 Model B Rev 1.4\0").as_deref(),
            Some("Raspberry Pi 4 Model B Rev 1.4")
        );
        assert_eq!(
            board_from(b"Raspberry Pi Zero W Rev 1.1\0").as_deref(),
            Some("Raspberry Pi Zero W Rev 1.1")
        );
    }

    #[test]
    fn an_absent_or_empty_model_is_none() {
        assert_eq!(board_from(b""), None);
        assert_eq!(board_from(b"\0"), None);
        assert_eq!(board_from(b"   \0"), None);
    }

    #[test]
    fn a_non_pi_board_is_reported_but_not_treated_as_a_pi() {
        let other = board_from(b"Some Other SBC\0").unwrap();
        assert_eq!(other, "Some Other SBC");
        assert!(!other.contains("Raspberry Pi"));
    }
}
