//! The controllers themselves.
//!
//! Web Bluetooth has no concept of choosing a radio: a browser has one
//! Bluetooth, and `navigator.bluetooth` is it. That is the right model for a
//! web page and the wrong one for a machine with two dongles, a test rig
//! driving both ends of a link, or a Raspberry Pi whose built-in controller is
//! worse than the USB one next to it.
//!
//! So this sits alongside the standard rather than inside it. The default is
//! unchanged — `Bluetooth::new` takes whichever controller the
//! platform considers first — and nothing here has to be called to use the
//! rest of the crate.
//!
//! What a platform can say about its radios varies more than most things in
//! this crate, and the honest answer is sometimes "there is one and I cannot
//! describe it":
//!
//! | platform | enumerate | select | address |
//! |---|---|---|---|
//! | BlueZ | yes | yes | yes |
//! | `linux-hci` | yes | yes | yes |
//! | Windows | yes | yes | yes |
//! | CoreBluetooth | one | no | no |
//! | Android | one | no | redacted |
//!
//! CoreBluetooth exposes no controller identity at all — a `CBCentralManager`
//! is the radio, and there is no API to ask which one or to pick another.
//! Android has `getDefaultAdapter()` and no way to enumerate; since Android 6
//! `getAddress()` returns a fixed `02:00:00:00:00:00` to every caller that is
//! not the system, so reporting it would be reporting a placeholder.
//!
//! Rather than pretend, those platforms report exactly one adapter and
//! `Bluetooth::select_adapter` refuses anything else by name.

/// One Bluetooth controller.
///
/// Obtained from `Bluetooth::adapters`. The [`id`](Self::id) is what
/// `Bluetooth::select_adapter` takes, and is whatever the platform
/// uses to name a radio: `hci0` on Linux, a device interface path on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterInfo {
    /// What `Bluetooth::select_adapter` takes to choose this controller.
    pub id: String,
    /// A human-readable name, where the platform has one.
    pub name: Option<String>,
    /// The controller's own Bluetooth address, where the platform exposes it.
    pub address: Option<String>,
    /// Whether the radio is on.
    pub powered: bool,
    /// Whether this is the controller a plain `Bluetooth::new` uses.
    pub is_default: bool,
}

impl AdapterInfo {
    /// What `Bluetooth::select_adapter` takes to choose this one.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// A human-readable name, where the platform has one.
    ///
    /// BlueZ reports the adapter's `Alias`, which is what the machine
    /// advertises as. `None` where the platform does not name its radios.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The controller's own Bluetooth address.
    ///
    /// `None` where the platform withholds it: CoreBluetooth never exposes it,
    /// and Android returns a fixed placeholder to ordinary apps rather than
    /// the real address, which is reported as `None` rather than as that
    /// placeholder.
    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    /// Whether the radio is switched on.
    pub fn is_powered(&self) -> bool {
        self.powered
    }

    /// Whether this is the controller a plain `Bluetooth::new` uses.
    pub fn is_default(&self) -> bool {
        self.is_default
    }
}

/// What a link actually negotiated.
///
/// The counterpart to `Bluetooth`'s connection *priority*: priority
/// asks for a profile, this reports what came back. Not part of Web
/// Bluetooth, which has neither.
///
/// The units are the Bluetooth ones rather than milliseconds, because that is
/// what the controller reports and converting would lose precision: the
/// interval is in 1.25 ms steps and the timeout in 10 ms steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionParameters {
    /// Connection interval, in 1.25 ms steps.
    pub interval: u16,
    /// Peripheral latency: connection events the peer may skip.
    pub latency: u16,
    /// Supervision timeout, in 10 ms steps.
    pub timeout: u16,
}

impl ConnectionParameters {
    /// Connection interval, in units of 1.25 ms.
    pub fn interval(&self) -> u16 {
        self.interval
    }

    /// How many connection events the peripheral may skip.
    pub fn latency(&self) -> u16 {
        self.latency
    }

    /// Supervision timeout, in units of 10 ms.
    pub fn timeout(&self) -> u16 {
        self.timeout
    }

    /// Connection interval in milliseconds.
    pub fn interval_millis(&self) -> f32 {
        self.interval as f32 * 1.25
    }

    /// Supervision timeout in milliseconds.
    pub fn timeout_millis(&self) -> u32 {
        self.timeout as u32 * 10
    }
}

/// What `BluetoothDevice::pair` did.
///
/// Three outcomes rather than a bare `Result<()>`, because "nothing happened"
/// is a real answer here and it is not a failure: Apple pairs when an
/// encrypted attribute is touched and offers no way to ask for it in advance.
/// Collapsing that into `Ok(())` would tell a caller a bond exists when it
/// does not, and into an error would send them looking for a problem that is
/// not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pairing {
    /// The ceremony ran and the device is now bonded.
    Paired,
    /// It already was, so nothing was asked of the user.
    AlreadyPaired,
    /// This platform pairs by itself, when an encrypted attribute is first
    /// read or written. Nothing was done; carry on and the system will ask the
    /// user if it needs to.
    Implicit,
}

/// A physical layer an LE link can run at.
///
/// Not part of Web Bluetooth, which has no notion of one. It is here because
/// it is the largest single throughput lever available on a connection: 2M
/// doubles the symbol rate, which roughly doubles what fits in a connection
/// event. Coded trades the other way, buying range at a quarter or an eighth
/// of the rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phy {
    /// 1 Msym/s. Every LE device supports this; it is the only one that
    /// existed before Bluetooth 5.
    Le1M,
    /// 2 Msym/s. Bluetooth 5, and optional — a peer may simply not have it,
    /// in which case asking for it leaves the link where it was.
    Le2M,
    /// Long range. Bluetooth 5, optional, and slower by design.
    LeCoded,
}

/// What a link is actually running at, in each direction.
///
/// The two can differ: a sensor that sends a great deal and receives almost
/// nothing has no reason to run both ways at the same rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionPhy {
    /// The PHY this side transmits on.
    pub tx: Phy,
    /// The PHY this side receives on.
    pub rx: Phy,
}

impl ConnectionPhy {
    /// The PHY this device transmits on.
    pub fn tx(&self) -> Phy {
        self.tx
    }

    /// The PHY it receives on.
    pub fn rx(&self) -> Phy {
        self.rx
    }
}

/// The single adapter a platform that cannot enumerate reports.
///
/// Named rather than empty so that `Bluetooth::adapters` has the same
/// shape everywhere: one entry, selectable by this id, and refusing any other.
pub const DEFAULT_ADAPTER: &str = "default";

/// Whether `address` is the placeholder Android hands out instead of the real
/// controller address.
///
/// Since Android 6 `BluetoothAdapter.getAddress()` returns a constant to any
/// app without `LOCAL_MAC_ADDRESS_PERMISSION`, which is a system permission.
/// Every device returns the same one, so passing it through would be worse
/// than saying nothing.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn is_placeholder_address(address: &str) -> bool {
    address.eq_ignore_ascii_case("02:00:00:00:00:00")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Android's placeholder is not an address, and reporting it as one would
    /// have every Android device claim the same radio.
    #[test]
    fn the_android_placeholder_is_not_reported_as_an_address() {
        assert!(is_placeholder_address("02:00:00:00:00:00"));
        assert!(is_placeholder_address(
            "02:00:00:00:00:00".to_uppercase().as_str()
        ));
        assert!(!is_placeholder_address("AA:BB:CC:DD:EE:FF"));
    }

    /// The units are the controller's, and the conversions have to match the
    /// specification's: 1.25 ms per interval step, 10 ms per timeout step.
    #[test]
    fn connection_parameters_convert_in_the_units_the_controller_uses() {
        // 24 * 1.25 = 30 ms, the interval an iPhone typically settles on.
        let p = ConnectionParameters {
            interval: 24,
            latency: 0,
            timeout: 500,
        };
        assert_eq!(p.interval(), 24);
        assert_eq!(p.interval_millis(), 30.0);
        // 500 * 10 = 5 s, the usual supervision timeout.
        assert_eq!(p.timeout_millis(), 5_000);
        assert_eq!(p.latency(), 0);
    }

    #[test]
    fn an_adapter_reports_what_it_was_given() {
        let a = AdapterInfo {
            id: "hci1".into(),
            name: Some("rig".into()),
            address: Some("AA:BB:CC:DD:EE:FF".into()),
            powered: true,
            is_default: false,
        };
        assert_eq!(a.id(), "hci1");
        assert_eq!(a.name(), Some("rig"));
        assert_eq!(a.address(), Some("AA:BB:CC:DD:EE:FF"));
        assert!(a.is_powered());
        assert!(!a.is_default());
    }
}

/// How aggressively to maintain a connection.
///
/// **Not part of Web Bluetooth**, and a hint rather than an instruction: the
/// peripheral has the final say on the connection interval, and only some
/// platforms can even ask. See
/// `BluetoothDevice::request_connection_priority`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPriority {
    /// Whatever the platform considers a reasonable default.
    Balanced,
    /// A shorter interval: lower latency, more power, and fewer simultaneous
    /// connections the controller can keep.
    High,
    /// A longer interval: less power, higher latency.
    LowPower,
}
