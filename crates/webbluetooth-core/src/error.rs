//! Errors, named after the `DOMException`s the Web Bluetooth spec throws.
//!
//! Keeping the spec's names — rather than inventing Rust-flavoured ones — means
//! code ported from JavaScript matches failures the same way, and the reason a
//! given operation failed is looked up in the same place.

use std::fmt;

/// The result of any Web Bluetooth operation.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a Web Bluetooth operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No device matched, or the chooser declined to pick one. The spec throws
    /// this when the user dismisses the device picker.
    NotFound(String),

    /// The operation is not permitted: a blocklisted UUID, or a service that
    /// was never listed in `filters` / `optional_services`.
    Security(String),

    /// The object is no longer usable — the device disconnected, or the
    /// peripheral re-advertised a different service set and invalidated every
    /// handle into the old one.
    InvalidState(String),

    /// The GATT operation itself failed: the link dropped mid-request, the
    /// peer rejected it, or `connect()` never completed.
    Network(String),

    /// The characteristic or descriptor does not support what was asked of it —
    /// reading one with no `read` property, writing one with no `write`.
    NotSupported(String),

    /// A value was out of range, such as a write longer than the ATT MTU
    /// allows.
    InvalidModification(String),

    /// Bluetooth is unavailable on this system, or this process is not
    /// permitted to use it. Carries the underlying [`Availability`].
    NotAvailable(Availability),

    /// An operation exceeded its deadline.
    Timeout(String),

    /// The operation was cancelled — the future was dropped, or the adapter
    /// reset underneath it.
    Aborted(String),
}

/// Whether this process may use Bluetooth at all.
///
/// On Apple platforms this is the TCC verdict. Linux has no such gate, so the
/// backend reports whether `bluetoothd` is reachable over D-Bus, which is the
/// closest equivalent: a denial there is a policy decision too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorization {
    /// Never asked. On Apple platforms, also what a process with no
    /// `NSBluetoothAlwaysUsageDescription` sees — and it is never prompted.
    NotDetermined,
    /// Refused by policy rather than by the user, such as an MDM profile.
    Restricted,
    /// The user said no.
    Denied,
    /// Permitted.
    Allowed,
}

/// Why Bluetooth is not currently usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// This machine has no Bluetooth LE radio, or its stack does not support it.
    Unsupported,
    /// The radio is powered off.
    PoweredOff,
    /// The process has no Bluetooth entitlement — on macOS and iOS this means
    /// no `NSBluetoothAlwaysUsageDescription`, or the user declined. Note that
    /// the system reports this *instead of* prompting when the process has no
    /// usage description at all.
    Unauthorized,
    /// The adapter is resetting; try again shortly.
    Resetting,
    /// State is not yet known. `CBCentralManager` reports this until its first
    /// `centralManagerDidUpdateState:`, which is why every entry point waits
    /// for that callback before doing anything.
    Unknown,
}

impl Error {
    /// The `DOMException` name the Web Bluetooth spec uses for this failure.
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "NotFoundError",
            Self::Security(_) => "SecurityError",
            Self::InvalidState(_) => "InvalidStateError",
            Self::Network(_) => "NetworkError",
            Self::NotSupported(_) => "NotSupportedError",
            Self::InvalidModification(_) => "InvalidModificationError",
            Self::NotAvailable(_) => "NotFoundError",
            Self::Timeout(_) => "TimeoutError",
            Self::Aborted(_) => "AbortError",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = match self {
            Self::NotFound(m)
            | Self::Security(m)
            | Self::InvalidState(m)
            | Self::Network(m)
            | Self::NotSupported(m)
            | Self::InvalidModification(m)
            | Self::Timeout(m)
            | Self::Aborted(m) => m.clone(),
            Self::NotAvailable(a) => a.to_string(),
        };
        write!(f, "{}: {detail}", self.name())
    }
}

impl std::fmt::Display for Availability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::Unsupported => "this system has no Bluetooth LE support",
            Self::PoweredOff => "Bluetooth is turned off",
            Self::Unauthorized => {
                "this process is not authorised to use Bluetooth — on Apple \
                 platforms it needs NSBluetoothAlwaysUsageDescription in its \
                 Info.plist"
            }
            Self::Resetting => "the Bluetooth adapter is resetting",
            Self::Unknown => "Bluetooth state is not yet known",
        };
        f.write_str(detail)
    }
}

impl std::error::Error for Error {}

/// `availability()` hands an [`Availability`] back on its own rather than
/// wrapped, because an unusable radio is a state rather than a failed call.
/// That is worth keeping — and it should not also mean a caller cannot put it
/// where every other failure goes. Without this, reporting one costs a manual
/// translation into `anyhow`, `Box<dyn Error>` or a crate's own error type at
/// each call site, which is exactly the boilerplate the trait exists to
/// prevent.
impl std::error::Error for Availability {}

impl From<Availability> for Error {
    /// The spec resolves a request against an unavailable radio as
    /// `NotFoundError`, which is what [`Error::NotAvailable`] reports — so
    /// `?` on an availability check inside a function returning [`Error`]
    /// produces the same failure the operation itself would have.
    fn from(availability: Availability) -> Self {
        Self::NotAvailable(availability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `availability()` hands an [`Availability`] back on its own, so it has
    /// to read as a sentence without an [`Error`] around it. These messages
    /// used to live inside `Error`'s `Display` and nowhere else.
    #[test]
    fn every_availability_explains_itself() {
        let mut seen = std::collections::BTreeSet::new();
        for a in [
            Availability::Unsupported,
            Availability::PoweredOff,
            Availability::Unauthorized,
            Availability::Resetting,
            Availability::Unknown,
        ] {
            let said = a.to_string();
            assert!(!said.is_empty(), "{a:?} says nothing");
            // They are embedded — `Error` prints "{name}: {detail}", and the
            // examples print "Bluetooth unavailable: {why}" — so a trailing
            // full stop would land in the middle of somebody's sentence.
            assert!(
                !said.ends_with('.'),
                "{a:?}: {said:?} ends a sentence it is inside of"
            );
            assert!(
                seen.insert(said.clone()),
                "{a:?} says the same as another: {said:?}"
            );
        }
    }

    /// The two must not drift apart now that one delegates to the other.
    #[test]
    fn an_unavailable_error_carries_the_same_explanation() {
        let why = Availability::PoweredOff;
        let wrapped = Error::NotAvailable(why).to_string();
        assert!(
            wrapped.contains(&why.to_string()),
            "{wrapped:?} does not contain {:?}",
            why.to_string()
        );
        // And still names the DOMException the specification defines.
        assert!(wrapped.starts_with("NotFoundError"), "got {wrapped:?}");
    }
}
