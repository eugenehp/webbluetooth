//! What a peripheral publishes, independent of how.
//!
//! Service, characteristic and descriptor definitions, the permission and error
//! vocabularies, and the advertising payload. None of this touches a platform
//! API — the engines in `apple.rs` and `android.rs` turn it into
//! `CBMutableService` or `BluetoothGattService` respectively.
//!
//! What each platform *accepts* differs, so validation is delegated: see
//! `validate_characteristic` and `validate_descriptor` in whichever engine is
//! compiled.
//!
//! Names like `Request::Read` and `Peripheral::publish` in the prose below
//! belong to `webbluetooth::peripheral`, above this: the adapters are what
//! turn these definitions into a running GATT server, and they cannot be
//! linked from here without pointing a crate at the one that depends on it.

use crate::error::{Error, Result};
use crate::gatt::CharacteristicProperties as Properties;
use crate::uuid::{BluetoothUuid, IntoUuid};

/// What a peripheral may permit a central to do with an attribute.
///
/// The Bluetooth permission bits. CoreBluetooth calls these
/// `CBAttributePermissions`; Android splits the same ideas across
/// `BluetoothGattCharacteristic.PERMISSION_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions(pub u32);

impl Permissions {
    /// A central may read it.
    pub const READABLE: u32 = 0x01;
    /// A central may write it.
    pub const WRITEABLE: u32 = 0x02;
    /// Reads require an encrypted link.
    pub const READ_ENCRYPTION_REQUIRED: u32 = 0x04;
    /// Writes require an encrypted link.
    pub const WRITE_ENCRYPTION_REQUIRED: u32 = 0x08;

    /// Whether one of the bits above is set.
    #[inline]
    pub fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
    /// Whether [`Self::READABLE`] is set.
    #[inline]
    pub fn readable(self) -> bool {
        self.has(Self::READABLE)
    }
    /// Whether [`Self::WRITEABLE`] is set.
    #[inline]
    pub fn writeable(self) -> bool {
        self.has(Self::WRITEABLE)
    }
    /// Any permission that would let a central write.
    #[inline]
    pub fn any_write(self) -> bool {
        self.has(Self::WRITEABLE) || self.has(Self::WRITE_ENCRYPTION_REQUIRED)
    }
}

/// The status a request is answered with.
///
/// These are the Bluetooth ATT error codes, so a central sees exactly the
/// failure the protocol defines rather than a generic one. Both platforms
/// transmit the same numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AttError {
    /// The operation succeeded. Not an error; answering with it is the normal reply.
    Success = 0x00,
    /// The attribute handle is not valid on this server.
    InvalidHandle = 0x01,
    /// The attribute cannot be read.
    ReadNotPermitted = 0x02,
    /// The attribute cannot be written.
    WriteNotPermitted = 0x03,
    /// The request was malformed.
    InvalidPdu = 0x04,
    /// The link is not authenticated enough for this attribute.
    InsufficientAuthentication = 0x05,
    /// The server does not support this request. What a dropped request answers.
    RequestNotSupported = 0x06,
    /// The offset is past the end of the attribute.
    InvalidOffset = 0x07,
    /// The client is not authorised for this attribute.
    InsufficientAuthorization = 0x08,
    /// Too many queued prepare-writes.
    PrepareQueueFull = 0x09,
    /// No attribute in the requested range.
    AttributeNotFound = 0x0A,
    /// The attribute cannot be read or written with a blob request.
    AttributeNotLong = 0x0B,
    /// The encryption key is too short for this attribute.
    InsufficientEncryptionKeySize = 0x0C,
    /// The value's length is wrong for this attribute.
    InvalidAttributeValueLength = 0x0D,
    /// The request failed for a reason with no better code. The catch-all.
    UnlikelyError = 0x0E,
    /// The link must be encrypted first.
    InsufficientEncryption = 0x0F,
    /// The attribute type is not a supported grouping type.
    UnsupportedGroupType = 0x10,
    /// The server is out of resources to complete the request.
    InsufficientResources = 0x11,
}

/// One write within a `WriteRequest`.
#[derive(Debug, Clone)]
pub struct Write {
    /// Which characteristic is being written.
    pub characteristic: BluetoothUuid,
    /// Where in the attribute the bytes go.
    pub offset: usize,
    /// The bytes.
    pub value: Vec<u8>,
}

/// What the system preserved for a peripheral across a relaunch.
#[derive(Debug, Clone, Default)]
pub struct RestoredPeripheral {
    /// The local name that was being advertised.
    pub advertised_local_name: Option<String>,
    /// The services that were being advertised.
    pub advertised_services: Vec<BluetoothUuid>,
    /// How many services were published.
    pub service_count: usize,
}

/// A descriptor to publish alongside a characteristic.
#[derive(Debug, Clone)]
pub struct Descriptor {
    /// Which descriptor this is. The CCCD is not yours to declare.
    pub uuid: BluetoothUuid,
    /// Its value, which a descriptor must have.
    pub value: Vec<u8>,
    /// Whether the value is text. CoreBluetooth types a descriptor's value by
    /// UUID — `NSString` for a user description, `NSData` for a presentation
    /// format — so it must be told which. Android takes bytes either way.
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    pub is_string: bool,
}

impl Descriptor {
    /// A Characteristic User Description (`0x2901`) — a human-readable label.
    pub fn user_description(text: impl Into<String>) -> Self {
        Self {
            uuid: BluetoothUuid::from_u16(0x2901),
            value: text.into().into_bytes(),
            is_string: true,
        }
    }

    /// A Characteristic Presentation Format descriptor (`0x2904`).
    ///
    /// Seven bytes: format, exponent, unit (16-bit), namespace, description
    /// (16-bit).
    pub fn presentation_format(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            uuid: BluetoothUuid::from_u16(0x2904),
            value: bytes.into(),
            is_string: false,
        }
    }

    /// The two rules every platform enforces.
    ///
    /// Both were written out four times before they were written here, once
    /// per adapter, differing only in which stack the message blamed for
    /// managing the CCCD. They all manage it.
    pub fn validate(&self) -> Result<()> {
        if self.uuid == cccd_uuid() {
            return Err(Error::NotSupported(
                "the client characteristic configuration descriptor is added automatically \
                 when a characteristic can notify — every platform here manages it, and \
                 declaring it would publish a second one"
                    .into(),
            ));
        }
        if self.value.is_empty() {
            return Err(Error::NotSupported("a descriptor must have a value".into()));
        }
        Ok(())
    }
}

/// The client characteristic configuration descriptor.
///
/// Named once here because all four adapters need it and all four used to
/// define it: the engine adds it to any characteristic that can notify, so it
/// is never the caller's to declare.
pub fn cccd_uuid() -> BluetoothUuid {
    crate::uuid::descriptors::CLIENT_CHARACTERISTIC_CONFIGURATION
}

/// A characteristic to publish.
#[derive(Debug, Clone)]
pub struct Characteristic {
    /// Which characteristic this is.
    pub uuid: BluetoothUuid,
    /// [`crate::CharacteristicProperties`] bits: what a central may do.
    pub properties: u32,
    /// [`Permissions`] bits: what a central must have done to be allowed to.
    pub permissions: u32,
    /// A fixed value, answered by the stack without waking the application.
    ///
    /// CoreBluetooth requires a characteristic with one to be read-only,
    /// because a cached value can never change; the other platforms do not,
    /// and the check is applied where Apple publishes.
    pub value: Option<Vec<u8>>,
    /// Descriptors published alongside it.
    pub descriptors: Vec<Descriptor>,
}

impl Characteristic {
    /// Begin defining a characteristic. It has no properties until you add some.
    pub fn new(uuid: impl IntoUuid) -> Result<Self> {
        Ok(Self {
            uuid: uuid.into_uuid()?,
            properties: 0,
            permissions: 0,
            value: None,
            descriptors: Vec::new(),
        })
    }

    /// Allow reads. Answered by `Request::Read` unless a fixed
    /// [`Characteristic::value`] is set.
    pub fn read(mut self) -> Self {
        self.properties |= Properties::READ;
        self.permissions |= Permissions::READABLE;
        self
    }

    /// Allow acknowledged writes.
    pub fn write(mut self) -> Self {
        self.properties |= Properties::WRITE;
        self.permissions |= Permissions::WRITEABLE;
        self
    }

    /// Allow unacknowledged writes.
    pub fn write_without_response(mut self) -> Self {
        self.properties |= Properties::WRITE_WITHOUT_RESPONSE;
        self.permissions |= Permissions::WRITEABLE;
        self
    }

    /// Allow subscription. CoreBluetooth adds the Client Characteristic
    /// Configuration descriptor for you.
    pub fn notify(mut self) -> Self {
        self.properties |= Properties::NOTIFY;
        self
    }

    /// Allow acknowledged subscription.
    pub fn indicate(mut self) -> Self {
        self.properties |= Properties::INDICATE;
        self
    }

    /// Require an encrypted link for reads and writes.
    pub fn require_encryption(mut self) -> Self {
        if self.permissions & Permissions::READABLE != 0 {
            self.permissions |= Permissions::READ_ENCRYPTION_REQUIRED;
        }
        if self.permissions & Permissions::WRITEABLE != 0 {
            self.permissions |= Permissions::WRITE_ENCRYPTION_REQUIRED;
        }
        self
    }

    /// Serve a fixed value from CoreBluetooth's cache, with no read requests
    /// reaching your code.
    ///
    /// Forces read-only: `Peripheral::publish` rejects a characteristic that
    /// combines a fixed value with `write`, `notify` or `indicate`, because
    /// CoreBluetooth raises on that.
    pub fn value(mut self, value: impl Into<Vec<u8>>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Attach a descriptor.
    /// Publish a descriptor alongside this characteristic.
    pub fn descriptor(mut self, descriptor: Descriptor) -> Self {
        self.descriptors.push(descriptor);
        self
    }

    /// The rules every platform enforces on a characteristic, and on the
    /// descriptors under it. A platform may add its own where it publishes.
    pub fn validate(&self) -> Result<()> {
        if self.properties == 0 {
            return Err(Error::NotSupported(format!(
                "characteristic {} has no properties — add read(), write() or notify()",
                self.uuid
            )));
        }
        for d in &self.descriptors {
            d.validate()?;
        }
        Ok(())
    }
}

// A platform may have rules of its own on top of these — CoreBluetooth
// refuses a cached value on anything writeable or notifying, where BlueZ does
// not care — and applies them where it publishes. What is here is what holds
// everywhere, so that a definition rejected on one platform is not quietly
// accepted on another.

/// A service to publish.
#[derive(Debug, Clone)]
pub struct Service {
    /// Which service this is.
    pub uuid: BluetoothUuid,
    /// Primary rather than included in another service.
    pub primary: bool,
    /// Its characteristics. A service needs at least one.
    pub characteristics: Vec<Characteristic>,
}

impl Service {
    /// Begin defining a primary service.
    pub fn new(uuid: impl IntoUuid) -> Result<Self> {
        Ok(Self {
            uuid: uuid.into_uuid()?,
            primary: true,
            characteristics: Vec::new(),
        })
    }

    /// Make this a secondary (included) service rather than a primary one.
    pub fn secondary(mut self) -> Self {
        self.primary = false;
        self
    }

    /// Add a characteristic.
    /// Add a characteristic to this service.
    pub fn characteristic(mut self, characteristic: Characteristic) -> Self {
        self.characteristics.push(characteristic);
        self
    }

    /// The rules every platform enforces on a service and everything in it.
    pub fn validate(&self) -> Result<()> {
        if self.characteristics.is_empty() {
            return Err(Error::NotSupported(format!(
                "service {} has no characteristics",
                self.uuid
            )));
        }
        for c in &self.characteristics {
            c.validate()?;
        }
        Ok(())
    }
}

/// What to put in the advertisement.
///
/// Apple honours only a local name and a service UUID list in the peripheral
/// role — no manufacturer data, no service data. The packet holds 28 bytes for
/// everything; service UUIDs that do not fit go to the overflow area, where
/// only an Apple device scanning for them explicitly will see them.
#[derive(Debug, Clone, Default)]
pub struct Advertising {
    /// The name to advertise, if any.
    pub local_name: Option<String>,
    /// Service UUIDs to advertise, which is what a central filters on.
    pub services: Vec<BluetoothUuid>,
}

impl Advertising {
    /// Advertise nothing in particular. Add a name or a service.
    pub fn new() -> Self {
        Self::default()
    }

    /// The name centrals will show.
    pub fn local_name(mut self, name: impl Into<String>) -> Self {
        self.local_name = Some(name.into());
        self
    }

    /// Advertise a service UUID, so centrals filtering for it find this device.
    pub fn service(mut self, uuid: impl IntoUuid) -> Result<Self> {
        self.services.push(uuid.into_uuid()?);
        Ok(self)
    }
}
