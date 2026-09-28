//! Failures, in the shape a page expects to catch.

use serde::Serialize;

/// What a rejected command hands back to the shim.
///
/// The shim turns this into a `DOMException` carrying the same `name`, which
/// is the object the specification says each of these failures raises. Sites
/// branch on `err.name`, so getting the name right matters more than the
/// message: a heart-rate page that retries on `NetworkError` but gives up on
/// `SecurityError` behaves correctly here for the same reason it does in
/// Chrome.
#[derive(Debug, Clone, Serialize)]
pub struct JsError {
    /// The `DOMException` name — `"NotFoundError"`, `"SecurityError"`, …
    pub name: String,
    /// Human-readable detail. Never load-bearing for a caller.
    pub message: String,
}

impl JsError {
    /// A failure with an explicit `DOMException` name.
    pub fn new(name: &str, message: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            message: message.into(),
        }
    }

    /// The permission model refused this — a device or service this origin was
    /// never granted.
    pub fn security(message: impl Into<String>) -> Self {
        Self::new("SecurityError", message)
    }

    /// Nothing matched: no such device, no such attribute, or the user
    /// dismissed the chooser.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("NotFoundError", message)
    }

    /// A handle that no longer refers to anything.
    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::new("InvalidStateError", message)
    }

    /// Bad arguments. `TypeError` rather than a `DOMException`, which the shim
    /// honours by constructing an actual `TypeError`.
    pub fn type_error(message: impl Into<String>) -> Self {
        Self::new("TypeError", message)
    }
}

impl From<webbluetooth::Error> for JsError {
    fn from(error: webbluetooth::Error) -> Self {
        // `Display` writes "{name}: {detail}" so that a logged error stands on
        // its own. Here the name travels in its own field, so the prefix would
        // just be said twice.
        let name = error.name();
        let rendered = error.to_string();
        let message = rendered
            .strip_prefix(&format!("{name}: "))
            .unwrap_or(&rendered)
            .to_string();
        Self {
            name: name.to_string(),
            message,
        }
    }
}

/// Every command returns this.
pub type Result<T> = std::result::Result<T, JsError>;
