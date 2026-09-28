//! Compile-time Bluetooth protocol capability reporting.
//!
//! This is deliberately descriptive: a capability being `false` means the
//! current build does not expose that protocol. It does not claim that a
//! controller or operating system could never support it.

/// A protocol family that can be queried from [`ProtocolCapabilities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// Bluetooth Low Energy GATT client/server operations.
    BleGatt,
    /// Bluetooth Classic BR/EDR transport.
    Classic,
    /// RFCOMM byte streams.
    Rfcomm,
    /// Bluetooth Classic L2CAP channels.
    ClassicL2cap,
    /// BLE/Classic L2CAP exposed by the public transport layer.
    L2cap,
    /// SDP service discovery.
    Sdp,
    /// Profile implementations such as A2DP, AVRCP, HFP, HID, and PAN.
    Profiles,
    /// Bluetooth Mesh.
    Mesh,
    /// Raw ACL packet transport.
    RawAcl,
    /// Raw SCO/eSCO packet transport.
    RawSco,
    /// LE Isochronous Channels and LE Audio.
    LeAudio,
}

/// Protocol capabilities for the current target and Cargo feature set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolCapabilities {
    ble_gatt: bool,
    classic: bool,
    rfcomm: bool,
    l2cap: bool,
    classic_l2cap: bool,
    sdp: bool,
    profiles: bool,
    mesh: bool,
    raw_acl: bool,
    raw_sco: bool,
    le_audio: bool,
}

impl ProtocolCapabilities {
    /// Construct the capabilities for this build.
    pub const fn current() -> Self {
        Self {
            ble_gatt: true,
            classic: cfg!(feature = "classic"),
            rfcomm: cfg!(all(
                feature = "rfcomm",
                any(
                    target_os = "linux",
                    target_os = "android",
                    target_os = "windows"
                )
            )),
            l2cap: !cfg!(any(target_os = "windows", target_arch = "wasm32")),
            classic_l2cap: cfg!(feature = "classic-l2cap"),
            sdp: cfg!(feature = "classic"),
            profiles: cfg!(feature = "profiles"),
            mesh: cfg!(feature = "mesh"),
            raw_acl: cfg!(all(feature = "raw-acl", target_os = "linux")),
            raw_sco: cfg!(all(feature = "raw-sco", target_os = "linux")),
            le_audio: cfg!(feature = "le-audio"),
        }
    }

    /// Whether a protocol family is exposed by this build.
    pub const fn supports(self, protocol: Protocol) -> bool {
        match protocol {
            Protocol::BleGatt => self.ble_gatt,
            Protocol::L2cap => self.l2cap,
            Protocol::Classic => self.classic,
            Protocol::Rfcomm => self.rfcomm,
            Protocol::ClassicL2cap => self.classic_l2cap,
            Protocol::Sdp => self.sdp,
            Protocol::Profiles => self.profiles,
            Protocol::Mesh => self.mesh,
            Protocol::RawAcl => self.raw_acl,
            Protocol::RawSco => self.raw_sco,
            Protocol::LeAudio => self.le_audio,
        }
    }

    /// Whether BLE GATT is enabled.
    pub const fn ble_gatt(self) -> bool {
        self.ble_gatt
    }

    /// Whether public L2CAP channels are enabled.
    pub const fn l2cap(self) -> bool {
        self.l2cap
    }

    /// Whether Classic BR/EDR is enabled.
    pub const fn classic(self) -> bool {
        self.classic
    }
    /// Whether RFCOMM is enabled.
    pub const fn rfcomm(self) -> bool {
        self.rfcomm
    }
    /// Whether Classic L2CAP is enabled.
    pub const fn classic_l2cap(self) -> bool {
        self.classic_l2cap
    }
    /// Whether SDP support is enabled.
    pub const fn sdp(self) -> bool {
        self.sdp
    }
    /// Whether profile APIs are enabled.
    pub const fn profiles(self) -> bool {
        self.profiles
    }
    /// Whether Mesh support is enabled.
    pub const fn mesh(self) -> bool {
        self.mesh
    }
    /// Whether raw ACL is enabled.
    pub const fn raw_acl(self) -> bool {
        self.raw_acl
    }
    /// Whether raw SCO/eSCO is enabled.
    pub const fn raw_sco(self) -> bool {
        self.raw_sco
    }
    /// Whether LE Audio support is enabled.
    pub const fn le_audio(self) -> bool {
        self.le_audio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_capabilities_match_feature_flags() {
        let capabilities = ProtocolCapabilities::current();
        assert!(capabilities.ble_gatt());
        assert_eq!(
            capabilities.l2cap(),
            !cfg!(any(target_os = "windows", target_arch = "wasm32"))
        );
        assert_eq!(capabilities.classic(), cfg!(feature = "classic"));
        assert_eq!(
            capabilities.rfcomm(),
            cfg!(all(
                feature = "rfcomm",
                any(
                    target_os = "linux",
                    target_os = "android",
                    target_os = "windows"
                )
            ))
        );
        assert_eq!(
            capabilities.classic_l2cap(),
            cfg!(feature = "classic-l2cap")
        );
        assert_eq!(capabilities.sdp(), cfg!(feature = "classic"));
        assert_eq!(capabilities.profiles(), cfg!(feature = "profiles"));
        assert_eq!(capabilities.mesh(), cfg!(feature = "mesh"));
        assert_eq!(
            capabilities.raw_acl(),
            cfg!(all(feature = "raw-acl", target_os = "linux"))
        );
        assert_eq!(
            capabilities.raw_sco(),
            cfg!(all(feature = "raw-sco", target_os = "linux"))
        );
        assert_eq!(capabilities.le_audio(), cfg!(feature = "le-audio"));

        assert!(capabilities.supports(Protocol::BleGatt));
        assert_eq!(
            capabilities.supports(Protocol::Classic),
            cfg!(feature = "classic")
        );
        assert_eq!(
            capabilities.supports(Protocol::Rfcomm),
            cfg!(all(
                feature = "rfcomm",
                any(
                    target_os = "linux",
                    target_os = "android",
                    target_os = "windows"
                )
            ))
        );
        assert_eq!(
            capabilities.supports(Protocol::ClassicL2cap),
            cfg!(feature = "classic-l2cap")
        );
        assert_eq!(
            capabilities.supports(Protocol::L2cap),
            !cfg!(any(target_os = "windows", target_arch = "wasm32"))
        );
        assert_eq!(
            capabilities.supports(Protocol::Sdp),
            cfg!(feature = "classic")
        );
        assert_eq!(
            capabilities.supports(Protocol::Profiles),
            cfg!(feature = "profiles")
        );
        assert_eq!(
            capabilities.supports(Protocol::Mesh),
            cfg!(feature = "mesh")
        );
        assert_eq!(
            capabilities.supports(Protocol::RawAcl),
            cfg!(all(feature = "raw-acl", target_os = "linux"))
        );
        assert_eq!(
            capabilities.supports(Protocol::RawSco),
            cfg!(all(feature = "raw-sco", target_os = "linux"))
        );
        assert_eq!(
            capabilities.supports(Protocol::LeAudio),
            cfg!(feature = "le-audio")
        );
    }
}
