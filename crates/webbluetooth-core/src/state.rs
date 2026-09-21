//! What the adapter is doing, and what that means for a caller.
//!
//! Every backend needs this enum and every backend needs the same mapping from
//! it to [`Availability`], so both live here once. They used to be declared
//! four times and matched on six times, which is four places for the enum to
//! drift and six for the mapping to disagree.

use crate::error::{Availability, Error, Result};

/// Mirrors `CBManagerState`, which is the most detailed of the platforms'
/// answers; the others widen into it.
///
/// This is the union of what the platforms can report, so on any one target
/// some variants are never constructed — Android reads its adapter state
/// synchronously and is never `Unknown`; Windows never reports `Unauthorized`
/// because WinRT fails the call instead. That is a fact about each platform
/// rather than dead code, and every variant is still matched in
/// [`Self::availability`].
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerState {
    /// Not yet reported, or reported as resetting.
    Unknown,
    /// This machine has no Bluetooth LE.
    Unsupported,
    /// This process may not use it.
    Unauthorized,
    /// The radio is off.
    PoweredOff,
    /// Usable.
    PoweredOn,
}

impl ManagerState {
    /// Whether Bluetooth can be used, and if not, why.
    pub fn availability(self) -> std::result::Result<(), Availability> {
        match self {
            Self::PoweredOn => Ok(()),
            Self::PoweredOff => Err(Availability::PoweredOff),
            Self::Unauthorized => Err(Availability::Unauthorized),
            Self::Unsupported => Err(Availability::Unsupported),
            Self::Unknown => Err(Availability::Unknown),
        }
    }

    /// The same question, as the error an operation should fail with.
    pub fn require_powered_on(self) -> Result<()> {
        self.availability().map_err(Error::NotAvailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_powered_on_is_usable() {
        assert!(ManagerState::PoweredOn.availability().is_ok());
        for state in [
            ManagerState::PoweredOff,
            ManagerState::Unauthorized,
            ManagerState::Unsupported,
            ManagerState::Unknown,
        ] {
            assert!(state.availability().is_err(), "{state:?} is not usable");
        }
    }

    /// The distinction matters to a caller: "turn Bluetooth on" and "this
    /// machine has none" call for different responses, so the reason has to
    /// survive the mapping rather than collapsing into one error.
    #[test]
    fn the_reason_survives() {
        assert_eq!(
            ManagerState::PoweredOff.availability(),
            Err(Availability::PoweredOff)
        );
        assert_eq!(
            ManagerState::Unsupported.availability(),
            Err(Availability::Unsupported)
        );
        assert!(matches!(
            ManagerState::Unauthorized.require_powered_on(),
            Err(Error::NotAvailable(Availability::Unauthorized))
        ));
    }
}
