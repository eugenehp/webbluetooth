//! A D-Bus client and server, spoken directly.
//!
//! D-Bus is a wire protocol over a Unix socket, so this talks it rather than
//! binding `libdbus`: [`codec`] marshals, [`message`] frames, [`connection`]
//! authenticates and dispatches. No C library, no bindings, no build step.
//!
//! [`codec`] and [`value`] are pure byte manipulation and build everywhere, so
//! the fiddly part — alignment and signature-directed decoding — is unit-tested
//! on any host rather than only inside a container.

pub mod codec;
pub mod value;

#[cfg(unix)]
pub mod connection;
pub mod fds;
#[cfg(unix)]
pub mod message;

pub use codec::{CodecError, Decoder, Encoder};
pub use value::Value;

#[cfg(unix)]
pub use connection::Connection;
#[cfg(unix)]
pub use message::{Message, MessageType};
