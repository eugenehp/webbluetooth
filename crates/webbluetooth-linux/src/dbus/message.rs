//! D-Bus message framing.
//!
//! ```text
//!   byte   endianness ('l' little, 'B' big)
//!   byte   message type
//!   byte   flags
//!   byte   protocol version (1)
//!   u32    body length
//!   u32    serial
//!   a(yv)  header fields — path, interface, member, signature, …
//!   ---- padded to 8 ----
//!   body
//! ```
//!
//! The header array's padding is measured from the start of the *message*, so
//! it is encoded into the same buffer as the fixed part rather than separately.
//! The body may be encoded on its own, because it always begins at a multiple
//! of 8 and every D-Bus alignment divides 8.

use super::codec::{CodecError, Decoder, Encoder};
use super::value::Value;

/// Header field codes.
mod field {
    pub const PATH: u8 = 1;
    pub const INTERFACE: u8 = 2;
    pub const MEMBER: u8 = 3;
    pub const ERROR_NAME: u8 = 4;
    pub const REPLY_SERIAL: u8 = 5;
    pub const DESTINATION: u8 = 6;
    pub const SENDER: u8 = 7;
    pub const SIGNATURE: u8 = 8;
    pub const UNIX_FDS: u8 = 9;
}

/// `NO_REPLY_EXPECTED`.
pub const FLAG_NO_REPLY_EXPECTED: u8 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// The four kinds of D-Bus message, as the header's type byte encodes them.
pub enum MessageType {
    MethodCall = 1,
    MethodReturn = 2,
    Error = 3,
    Signal = 4,
}

impl MessageType {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::MethodCall,
            2 => Self::MethodReturn,
            3 => Self::Error,
            4 => Self::Signal,
            _ => return None,
        })
    }
}

/// One D-Bus message.
#[derive(Debug, Clone)]
pub struct Message {
    pub message_type: MessageType,
    pub flags: u8,
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error_name: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub sender: Option<String>,
    pub body: Vec<Value>,
    /// How many file descriptors travelled beside this message.
    ///
    /// A `h` value in the body is an index into them, not a descriptor, and
    /// they arrive out of band — see [`super::fds`]. Declared here so the
    /// transport knows how many to claim for this message and no more.
    pub unix_fds: u32,
}

impl Message {
    fn empty(message_type: MessageType) -> Self {
        Self {
            message_type,
            flags: 0,
            serial: 0,
            path: None,
            interface: None,
            member: None,
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: None,
            body: Vec::new(),
            unix_fds: 0,
        }
    }

    /// A method call.
    pub fn call(destination: &str, path: &str, interface: &str, member: &str) -> Self {
        let mut m = Self::empty(MessageType::MethodCall);
        m.destination = Some(destination.into());
        m.path = Some(path.into());
        m.interface = Some(interface.into());
        m.member = Some(member.into());
        m
    }

    /// A signal.
    pub fn signal(path: &str, interface: &str, member: &str) -> Self {
        let mut m = Self::empty(MessageType::Signal);
        m.path = Some(path.into());
        m.interface = Some(interface.into());
        m.member = Some(member.into());
        m
    }

    /// A successful reply to `call`.
    pub fn reply_to(call: &Message) -> Self {
        let mut m = Self::empty(MessageType::MethodReturn);
        m.reply_serial = Some(call.serial);
        m.destination = call.sender.clone();
        m
    }

    /// An error reply to `call`.
    pub fn error_to(call: &Message, name: &str, text: &str) -> Self {
        let mut m = Self::empty(MessageType::Error);
        m.reply_serial = Some(call.serial);
        m.destination = call.sender.clone();
        m.error_name = Some(name.into());
        m.body = vec![Value::Str(text.into())];
        m
    }

    /// Attach a body.
    pub fn with_body(mut self, body: Vec<Value>) -> Self {
        self.body = body;
        self
    }

    /// The body's signature, derived from the values themselves.
    pub fn signature(&self) -> String {
        self.body.iter().map(Value::signature).collect()
    }

    /// `body[i]`, if present.
    pub fn arg(&self, i: usize) -> Option<&Value> {
        self.body.get(i)
    }

    /// The text of an error reply.
    pub fn error_text(&self) -> Option<String> {
        self.body
            .first()
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    /// Encode to the wire.
    pub fn serialize(&self, serial: u32) -> Vec<u8> {
        let mut body_encoder = Encoder::new();
        body_encoder.body(&self.body);
        let body = body_encoder.finish();

        let mut fields: Vec<Value> = Vec::new();
        let mut push = |code: u8, v: Value| {
            fields.push(Value::Struct(vec![
                Value::Byte(code),
                Value::Variant(Box::new(v)),
            ]));
        };
        if let Some(p) = &self.path {
            push(field::PATH, Value::ObjectPath(p.clone()));
        }
        if let Some(i) = &self.interface {
            push(field::INTERFACE, Value::Str(i.clone()));
        }
        if let Some(m) = &self.member {
            push(field::MEMBER, Value::Str(m.clone()));
        }
        if let Some(e) = &self.error_name {
            push(field::ERROR_NAME, Value::Str(e.clone()));
        }
        if let Some(r) = self.reply_serial {
            push(field::REPLY_SERIAL, Value::Uint32(r));
        }
        if let Some(d) = &self.destination {
            push(field::DESTINATION, Value::Str(d.clone()));
        }
        if let Some(s) = &self.sender {
            push(field::SENDER, Value::Str(s.clone()));
        }
        if !self.body.is_empty() {
            push(field::SIGNATURE, Value::Signature(self.signature()));
        }

        let mut encoder = Encoder::new();
        encoder.byte(b'l');
        encoder.byte(self.message_type as u8);
        encoder.byte(self.flags);
        encoder.byte(1);
        encoder.u32(body.len() as u32);
        encoder.u32(serial);
        encoder.value(&Value::Array {
            element: "(yv)".into(),
            items: fields,
        });
        encoder.align(8);

        let mut out = encoder.finish();
        out.extend_from_slice(&body);
        out
    }

    /// How long a message starting at `buf` is, or `None` if the fixed header
    /// has not arrived yet. Used to know how much more to read.
    pub fn framed_length(buf: &[u8]) -> Option<usize> {
        if buf.len() < 16 {
            return None;
        }
        let little_endian = buf[0] == b'l';
        let read_u32 = |at: usize| {
            let a = [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]];
            (if little_endian {
                u32::from_le_bytes(a)
            } else {
                u32::from_be_bytes(a)
            }) as usize
        };
        let body_len = read_u32(4);
        let fields_len = read_u32(12);
        // 12 fixed + 4 array length + fields, padded to 8, then body.
        let header_end = 16 + fields_len;
        let padded = header_end.div_ceil(8) * 8;
        Some(padded + body_len)
    }

    /// Decode one complete message.
    pub fn parse(buf: &[u8]) -> Result<Self, CodecError> {
        if buf.len() < 16 {
            return Err(CodecError::Truncated {
                wanted: 16,
                available: buf.len(),
            });
        }
        let little_endian = buf[0] == b'l';
        let message_type =
            MessageType::from_u8(buf[1]).ok_or(CodecError::BadTypeCode(buf[1] as char))?;
        let flags = buf[2];

        let mut decoder = Decoder::at(buf, 4, little_endian);
        let body_len = decoder.u32()? as usize;
        let serial = decoder.u32()?;
        let fields = decoder.value("a(yv)")?;

        let mut message = Self::empty(message_type);
        message.flags = flags;
        message.serial = serial;
        let mut signature = String::new();

        for entry in fields.as_array().unwrap_or(&[]) {
            let Some(parts) = entry.as_array() else {
                continue;
            };
            let (Some(code), Some(v)) = (parts.first().and_then(Value::as_u64), parts.get(1))
            else {
                continue;
            };
            match code as u8 {
                field::PATH => message.path = v.as_str().map(str::to_owned),
                field::INTERFACE => message.interface = v.as_str().map(str::to_owned),
                field::MEMBER => message.member = v.as_str().map(str::to_owned),
                field::ERROR_NAME => message.error_name = v.as_str().map(str::to_owned),
                field::REPLY_SERIAL => message.reply_serial = v.as_u64().map(|n| n as u32),
                field::DESTINATION => message.destination = v.as_str().map(str::to_owned),
                field::SENDER => message.sender = v.as_str().map(str::to_owned),
                field::SIGNATURE => signature = v.as_str().unwrap_or_default().to_owned(),
                field::UNIX_FDS => message.unix_fds = v.as_u64().unwrap_or(0) as u32,
                _ => {}
            }
        }

        // The body starts at the next multiple of 8 after the header array.
        let body_start = decoder.position().div_ceil(8) * 8;
        if !signature.is_empty() {
            let end = body_start + body_len;
            if end > buf.len() {
                return Err(CodecError::Truncated {
                    wanted: body_len,
                    available: buf.len().saturating_sub(body_start),
                });
            }
            let mut body_decoder = Decoder::new(&buf[body_start..end], little_endian);
            message.body = body_decoder.body(&signature)?;
        }
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_call_round_trips() {
        let call = Message::call(
            "org.bluez",
            "/org/bluez/hci0",
            "org.freedesktop.DBus.Properties",
            "Get",
        )
        .with_body(vec![
            Value::Str("org.bluez.Adapter1".into()),
            Value::Str("Powered".into()),
        ]);

        let bytes = call.serialize(7);
        assert_eq!(
            Message::framed_length(&bytes),
            Some(bytes.len()),
            "framing disagrees"
        );

        let back = Message::parse(&bytes).unwrap();
        assert_eq!(back.message_type, MessageType::MethodCall);
        assert_eq!(back.serial, 7);
        assert_eq!(back.destination.as_deref(), Some("org.bluez"));
        assert_eq!(back.path.as_deref(), Some("/org/bluez/hci0"));
        assert_eq!(
            back.interface.as_deref(),
            Some("org.freedesktop.DBus.Properties")
        );
        assert_eq!(back.member.as_deref(), Some("Get"));
        assert_eq!(back.body.len(), 2);
        assert_eq!(back.arg(1).unwrap().as_str(), Some("Powered"));
    }

    #[test]
    fn a_body_less_message_round_trips() {
        let m = Message::call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "Hello",
        );
        let bytes = m.serialize(1);
        assert_eq!(Message::framed_length(&bytes), Some(bytes.len()));
        let back = Message::parse(&bytes).unwrap();
        assert!(back.body.is_empty());
        assert_eq!(back.member.as_deref(), Some("Hello"));
    }

    #[test]
    fn an_error_reply_round_trips() {
        let call = Message::call("org.bluez", "/", "org.bluez.Adapter1", "StartDiscovery");
        let mut call = call;
        call.serial = 42;
        call.sender = Some(":1.5".into());

        let err = Message::error_to(&call, "org.bluez.Error.NotReady", "adapter is down");
        let bytes = err.serialize(9);
        let back = Message::parse(&bytes).unwrap();
        assert_eq!(back.message_type, MessageType::Error);
        assert_eq!(back.reply_serial, Some(42));
        assert_eq!(back.error_name.as_deref(), Some("org.bluez.Error.NotReady"));
        assert_eq!(back.error_text().as_deref(), Some("adapter is down"));
        assert_eq!(back.destination.as_deref(), Some(":1.5"));
    }

    #[test]
    fn a_signal_with_a_nested_body_round_trips() {
        // PropertiesChanged is the shape every notification arrives in.
        let sig = Message::signal(
            "/org/bluez/hci0/dev_AA/service0001/char0002",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
        )
        .with_body(vec![
            Value::Str("org.bluez.GattCharacteristic1".into()),
            Value::dict([("Value".into(), Value::bytes(&[0x5B]))]),
            Value::string_array([]),
        ]);

        let bytes = sig.serialize(3);
        assert_eq!(Message::framed_length(&bytes), Some(bytes.len()));
        let back = Message::parse(&bytes).unwrap();
        assert_eq!(back.message_type, MessageType::Signal);
        let changed = back.arg(1).unwrap();
        assert_eq!(changed.get("Value").unwrap().as_bytes(), Some(vec![0x5B]));
    }

    #[test]
    fn framing_needs_the_whole_fixed_header() {
        assert_eq!(Message::framed_length(&[]), None);
        assert_eq!(Message::framed_length(&[b'l', 1, 0, 1]), None);
    }

    #[test]
    fn a_truncated_message_errors_rather_than_panicking() {
        let m = Message::call("a", "/b", "c", "d").with_body(vec![Value::Str("x".into())]);
        let bytes = m.serialize(1);
        for cut in 0..bytes.len() {
            // Every prefix must be an error, never a panic.
            let _ = Message::parse(&bytes[..cut]);
        }
    }

    #[test]
    fn serial_is_taken_from_the_argument_not_the_struct() {
        // The connection assigns serials, so the field on the struct is ignored.
        let m = Message::call("a", "/b", "c", "d");
        assert_eq!(Message::parse(&m.serialize(99)).unwrap().serial, 99);
    }
}
