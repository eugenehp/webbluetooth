//! Picking up a Bluetooth session the system interrupted.
//!
//! Only iOS relaunches a terminated process to finish Bluetooth work, but the
//! types are portable: every backend reports what it recovered the same way,
//! and the five that never recover anything report `None`.

use crate::uuid::{BluetoothUuid, IntoUuid};
use crate::Result;

/// Opt in to Bluetooth state preservation and restoration.
///
/// The system preserves a manager's peripherals and scan across process death
/// and hands them back through `willRestoreState:` on the next launch.
///
/// # This only does something on iOS
///
/// `CBCentralManagerOptionRestoreIdentifierKey` is declared on macOS too, and
/// passing it is harmless there, but **only iOS relaunches a terminated process
/// to finish Bluetooth work** — and only one that declares the
/// `bluetooth-central` (or `bluetooth-peripheral`) background mode in its
/// `Info.plist`. On every other platform this is inert: no callback arrives and
/// `Bluetooth::restored_session` stays `None`.
///
/// # Grants do not survive
///
/// A Web Bluetooth grant is per-session, and the system does not preserve which
/// services a user allowed. So a restored session's allowlist is whatever you
/// declare here, up front — restored devices get exactly these services and
/// nothing else.
#[derive(Debug, Clone)]
pub struct Restoration {
    /// The restoration identifier, which must be identical on every launch.
    pub identifier: String,
    /// What a restored session is permitted to use. A grant does not survive
    /// a relaunch, so this is the whole allowlist.
    pub allowed_services: Vec<BluetoothUuid>,
}

impl Restoration {
    /// The restoration identifier. Must be **identical on every launch**, or
    /// the system cannot match the manager it preserved.
    pub fn new(identifier: impl Into<String>) -> Self {
        Self {
            identifier: identifier.into(),
            allowed_services: Vec::new(),
        }
    }

    /// Permit a restored session to use this service.
    pub fn allow_service(mut self, uuid: impl IntoUuid) -> Result<Self> {
        self.allowed_services.push(uuid.into_uuid()?);
        Ok(self)
    }

    /// Permit a restored session to use several services.
    pub fn allow_services<U: IntoUuid>(
        mut self,
        uuids: impl IntoIterator<Item = U>,
    ) -> Result<Self> {
        for u in uuids {
            self.allowed_services.push(u.into_uuid()?);
        }
        Ok(self)
    }
}

/// What a backend found waiting for it when the system brought it back.
///
/// Written once here rather than once per backend: the shape is the same
/// everywhere because it is a description of an interrupted scan, not of a
/// platform.
#[derive(Debug, Clone, Default)]
pub struct RestoredScan {
    /// Devices that were connected or connecting when the process ended.
    pub device_ids: Vec<String>,
    /// The services the interrupted scan was filtering on.
    pub scan_services: Vec<BluetoothUuid>,
    /// Whether that scan allowed duplicate advertisements.
    pub scan_allow_duplicates: bool,
}
