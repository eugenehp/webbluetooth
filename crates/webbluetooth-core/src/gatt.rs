//! The two attribute value-types the backends and the GATT tree share.
//!
//! They live here rather than beside `RemoteGattCharacteristic` because a
//! backend produces them — every one of the six converts its platform's
//! spelling of the properties byte on the way in — while the GATT tree only
//! reads them. Anything both sides name has to sit below both.

/// What a characteristic supports — `BluetoothCharacteristicProperties`.
///
/// The bit values are the Bluetooth ones, which CoreBluetooth uses directly and
/// BlueZ spells as a list of flag strings; each backend converts on the way in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CharacteristicProperties(pub u32);

impl CharacteristicProperties {
    /// Broadcast in an advertisement, via the Server Characteristic Configuration descriptor.
    pub const BROADCAST: u32 = 0x001;
    /// `readValue()` is permitted.
    pub const READ: u32 = 0x002;
    /// `writeValueWithoutResponse()` is permitted — no ATT acknowledgement.
    pub const WRITE_WITHOUT_RESPONSE: u32 = 0x004;
    /// `writeValueWithResponse()` is permitted.
    pub const WRITE: u32 = 0x008;
    /// The peer can push values, unacknowledged.
    pub const NOTIFY: u32 = 0x010;
    /// The peer can push values, each one acknowledged.
    pub const INDICATE: u32 = 0x020;
    /// Signed writes without a response are permitted.
    pub const AUTHENTICATED_SIGNED_WRITES: u32 = 0x040;
    /// A Characteristic Extended Properties descriptor exists.
    ///
    /// Says the descriptor is there, not what is in it — the two bits below
    /// are what is in it.
    pub const EXTENDED_PROPERTIES: u32 = 0x080;
    /// Notifications require an encrypted link. CoreBluetooth's own bit; not in the ATT properties byte.
    pub const NOTIFY_ENCRYPTION_REQUIRED: u32 = 0x100;
    /// Indications require an encrypted link. CoreBluetooth's own bit.
    pub const INDICATE_ENCRYPTION_REQUIRED: u32 = 0x200;
    // The two bits below are *not* in the characteristic's properties byte.
    // They live in the Characteristic Extended Properties descriptor, which
    // `EXTENDED_PROPERTIES` merely says exists. These values continue past
    // CoreBluetooth's, which stops at 0x200, so nothing collides.
    /// Queued writes are permitted — a bit *inside* the Extended Properties descriptor.
    pub const RELIABLE_WRITE: u32 = 0x400;
    /// The User Description descriptor is writable — also inside Extended Properties.
    pub const WRITABLE_AUXILIARIES: u32 = 0x800;

    /// Whether one of the bits above is set.
    #[inline]
    pub fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
    /// Whether [`Self::BROADCAST`] is set — `broadcast`.
    #[inline]
    pub fn broadcast(self) -> bool {
        self.has(Self::BROADCAST)
    }
    /// Whether [`Self::READ`] is set — `read`.
    #[inline]
    pub fn read(self) -> bool {
        self.has(Self::READ)
    }
    /// Whether [`Self::WRITE_WITHOUT_RESPONSE`] is set — `writeWithoutResponse`.
    #[inline]
    pub fn write_without_response(self) -> bool {
        self.has(Self::WRITE_WITHOUT_RESPONSE)
    }
    /// Whether [`Self::WRITE`] is set — `write`.
    #[inline]
    pub fn write(self) -> bool {
        self.has(Self::WRITE)
    }
    /// Whether [`Self::NOTIFY`] is set — `notify`.
    #[inline]
    pub fn notify(self) -> bool {
        self.has(Self::NOTIFY)
    }
    /// Whether [`Self::INDICATE`] is set — `indicate`.
    #[inline]
    pub fn indicate(self) -> bool {
        self.has(Self::INDICATE)
    }
    /// Whether [`Self::AUTHENTICATED_SIGNED_WRITES`] is set — `authenticatedSignedWrites`.
    #[inline]
    pub fn authenticated_signed_writes(self) -> bool {
        self.has(Self::AUTHENTICATED_SIGNED_WRITES)
    }
    /// Whether an Extended Properties descriptor exists.
    ///
    /// Not the same question as [`Self::reliable_write`]: this says the
    /// descriptor is there, not what is in it.
    #[inline]
    pub fn extended_properties(self) -> bool {
        self.has(Self::EXTENDED_PROPERTIES)
    }

    /// `reliableWrite` — queued writes are applied atomically.
    ///
    /// Reported only where the platform distinguishes it. BlueZ and WinRT list
    /// it as its own flag; CoreBluetooth and Android expose nothing beyond
    /// "there is an Extended Properties descriptor", so on those this is
    /// `false` even when the peer supports it, and
    /// [`Self::extended_properties`] is what there is to go on.
    #[inline]
    pub fn reliable_write(self) -> bool {
        self.has(Self::RELIABLE_WRITE)
    }

    /// `writableAuxiliaries` — the Characteristic User Description descriptor
    /// is writable. Same platform caveat as [`Self::reliable_write`].
    #[inline]
    pub fn writable_auxiliaries(self) -> bool {
        self.has(Self::WRITABLE_AUXILIARIES)
    }
}

/// Whether a write waits for the peer to acknowledge it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteType {
    /// Wait for the peer's ATT acknowledgement, and report a failure.
    WithResponse,
    /// Send and carry on. Faster, and the peer never says whether it arrived.
    WithoutResponse,
}
