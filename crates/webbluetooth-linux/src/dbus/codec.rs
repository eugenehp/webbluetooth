//! Marshalling values to and from the D-Bus wire format.
//!
//! Two rules account for nearly all of the difficulty:
//!
//! 1. **Every value is padded to its natural alignment**, and the offset that
//!    alignment is measured against is the start of the *message*, not the
//!    start of the value being written. An encoder therefore has to carry a
//!    position, not just a buffer. (The body may be encoded from zero, because
//!    it always begins on an 8-byte boundary and every alignment divides 8.)
//!
//! 2. **The bytes carry no type tags.** A `u32` on the wire is four bytes and
//!    nothing else; only the signature says what it is. Decoding is therefore
//!    signature-directed, and a signature that disagrees with the bytes yields
//!    silent nonsense rather than an error — which is why [`Decoder`] bounds-
//!    checks every read instead of trusting lengths from the peer.
//!
//! The exception is `v`, a variant, which carries its own signature inline.
//! BlueZ wraps every property in one, so most real payloads are variants all
//! the way down.

use super::value::{alignment, complete_types, type_length, Value};
use std::fmt;

/// A malformed or unexpected message.
#[derive(Debug, Clone, PartialEq)]
pub enum CodecError {
    /// The peer's length field ran past the end of the buffer.
    Truncated { wanted: usize, available: usize },
    /// A type code that is not in the specification.
    BadTypeCode(char),
    /// A signature that does not parse.
    BadSignature(String),
    /// A string field that was not valid UTF-8.
    BadUtf8,
    /// An array claimed a length that is not a whole number of elements.
    BadArrayLength(usize),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { wanted, available } => {
                write!(
                    f,
                    "message truncated: wanted {wanted} bytes, {available} available"
                )
            }
            Self::BadTypeCode(c) => write!(f, "unknown D-Bus type code {c:?}"),
            Self::BadSignature(s) => write!(f, "malformed signature {s:?}"),
            Self::BadUtf8 => f.write_str("string field was not valid UTF-8"),
            Self::BadArrayLength(n) => {
                write!(f, "array length {n} is not a whole number of elements")
            }
        }
    }
}

impl std::error::Error for CodecError {}

type Result<T> = std::result::Result<T, CodecError>;

// ── Encoding ────────────────────────────────────────────────────────────────

/// Writes values in little-endian, tracking position so padding is correct.
#[derive(Debug, Default)]
pub struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Start encoding at a non-zero offset, for a buffer that continues one
    /// that has already been written.
    pub fn at_offset(offset: usize) -> Self {
        Self {
            buf: vec![0; offset],
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The bytes written, discarding `skip` leading bytes of offset padding.
    pub fn finish_from(self, skip: usize) -> Vec<u8> {
        self.buf[skip.min(self.buf.len())..].to_vec()
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    /// Pad with zeroes until the position is a multiple of `n`.
    pub fn align(&mut self, n: usize) {
        while !self.buf.len().is_multiple_of(n) {
            self.buf.push(0);
        }
    }

    pub fn byte(&mut self, v: u8) {
        self.buf.push(v)
    }

    pub fn u16(&mut self, v: u16) {
        self.align(2);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.align(4);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.align(8);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// `s` / `o`: 32-bit length, bytes, then a NUL that is not counted.
    pub fn string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    /// `g`: 8-bit length, bytes, NUL. Signatures are short by definition.
    pub fn signature(&mut self, s: &str) {
        self.buf.push(s.len() as u8);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    /// Write one value.
    pub fn value(&mut self, v: &Value) {
        match v {
            Value::Byte(b) => self.byte(*b),
            Value::Bool(b) => self.u32(u32::from(*b)),
            Value::Int16(n) => self.u16(*n as u16),
            Value::Uint16(n) => self.u16(*n),
            Value::Int32(n) => self.u32(*n as u32),
            Value::Uint32(n) => self.u32(*n),
            Value::UnixFd(n) => self.u32(*n),
            Value::Int64(n) => self.u64(*n as u64),
            Value::Uint64(n) => self.u64(*n),
            Value::Double(d) => self.u64(d.to_bits()),
            Value::Str(s) | Value::ObjectPath(s) => self.string(s),
            Value::Signature(s) => self.signature(s),

            Value::Array { element, items } => {
                // The length counts the element bytes only, so it cannot be
                // written until the padding that follows it is known. Reserve
                // four bytes, write the elements, then backfill.
                self.align(4);
                let length_at = self.buf.len();
                self.buf.extend_from_slice(&0u32.to_le_bytes());
                let elem_align = element.as_bytes().first().map_or(1, |c| alignment(*c));
                self.align(elem_align);
                let body_start = self.buf.len();
                for item in items {
                    self.value(item);
                }
                let length = (self.buf.len() - body_start) as u32;
                self.buf[length_at..length_at + 4].copy_from_slice(&length.to_le_bytes());
            }

            Value::Struct(fields) => {
                self.align(8);
                for f in fields {
                    self.value(f);
                }
            }

            Value::DictEntry(k, val) => {
                self.align(8);
                self.value(k);
                self.value(val);
            }

            Value::Variant(inner) => {
                self.signature(&inner.signature());
                self.value(inner);
            }
        }
    }

    /// Write a sequence of values, as a message body.
    pub fn body(&mut self, values: &[Value]) {
        for v in values {
            self.value(v);
        }
    }
}

// ── Decoding ────────────────────────────────────────────────────────────────

/// Reads values, signature-directed, bounds-checking every access.
pub struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
    little_endian: bool,
}

impl<'a> Decoder<'a> {
    /// Decode a buffer whose position 0 is the alignment origin.
    pub fn new(buf: &'a [u8], little_endian: bool) -> Self {
        Self {
            buf,
            pos: 0,
            little_endian,
        }
    }

    /// Decode starting at `pos`, keeping the same alignment origin — needed for
    /// a header array, whose padding is measured from the start of the message.
    pub fn at(buf: &'a [u8], pos: usize, little_endian: bool) -> Self {
        Self {
            buf,
            pos,
            little_endian,
        }
    }

    pub fn position(&self) -> usize {
        self.pos
    }
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.buf.len() {
            return Err(CodecError::Truncated {
                wanted: n,
                available: self.remaining(),
            });
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn align(&mut self, n: usize) -> Result<()> {
        while !self.pos.is_multiple_of(n) {
            // Padding must exist; running off the end here means a bad length.
            if self.pos >= self.buf.len() {
                return Err(CodecError::Truncated {
                    wanted: 1,
                    available: 0,
                });
            }
            self.pos += 1;
        }
        Ok(())
    }

    pub fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        self.align(2)?;
        let b = self.take(2)?;
        let a = [b[0], b[1]];
        Ok(if self.little_endian {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        })
    }

    pub fn u32(&mut self) -> Result<u32> {
        self.align(4)?;
        let b = self.take(4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Ok(if self.little_endian {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }

    pub fn u64(&mut self) -> Result<u64> {
        self.align(8)?;
        let b = self.take(8)?;
        let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        Ok(if self.little_endian {
            u64::from_le_bytes(a)
        } else {
            u64::from_be_bytes(a)
        })
    }

    pub fn string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        let s = std::str::from_utf8(bytes)
            .map_err(|_| CodecError::BadUtf8)?
            .to_owned();
        self.take(1)?; // trailing NUL
        Ok(s)
    }

    pub fn signature(&mut self) -> Result<String> {
        let len = self.byte()? as usize;
        let bytes = self.take(len)?;
        let s = std::str::from_utf8(bytes)
            .map_err(|_| CodecError::BadUtf8)?
            .to_owned();
        self.take(1)?;
        Ok(s)
    }

    /// Read one value of the type named by `signature`.
    pub fn value(&mut self, signature: &str) -> Result<Value> {
        let code = *signature
            .as_bytes()
            .first()
            .ok_or_else(|| CodecError::BadSignature(signature.into()))?;
        Ok(match code {
            b'y' => Value::Byte(self.byte()?),
            b'b' => Value::Bool(self.u32()? != 0),
            b'n' => Value::Int16(self.u16()? as i16),
            b'q' => Value::Uint16(self.u16()?),
            b'i' => Value::Int32(self.u32()? as i32),
            b'u' => Value::Uint32(self.u32()?),
            b'h' => Value::UnixFd(self.u32()?),
            b'x' => Value::Int64(self.u64()? as i64),
            b't' => Value::Uint64(self.u64()?),
            b'd' => Value::Double(f64::from_bits(self.u64()?)),
            b's' => Value::Str(self.string()?),
            b'o' => Value::ObjectPath(self.string()?),
            b'g' => Value::Signature(self.signature()?),

            b'a' => {
                let element = signature[1..].to_string();
                if element.is_empty() {
                    return Err(CodecError::BadSignature(signature.into()));
                }
                let byte_len = self.u32()? as usize;
                let elem_align = element.as_bytes().first().map_or(1, |c| alignment(*c));
                self.align(elem_align)?;
                let end = self.pos + byte_len;
                if end > self.buf.len() {
                    return Err(CodecError::Truncated {
                        wanted: byte_len,
                        available: self.remaining(),
                    });
                }
                let mut items = Vec::new();
                while self.pos < end {
                    items.push(self.value(&element)?);
                }
                // A decode that overshot means the signature and bytes disagree.
                if self.pos != end {
                    return Err(CodecError::BadArrayLength(byte_len));
                }
                Value::Array { element, items }
            }

            b'(' => {
                self.align(8)?;
                let inner = signature
                    .get(1..signature.len().saturating_sub(1))
                    .ok_or_else(|| CodecError::BadSignature(signature.into()))?;
                let mut fields = Vec::new();
                for t in complete_types(inner) {
                    fields.push(self.value(&t)?);
                }
                Value::Struct(fields)
            }

            b'{' => {
                self.align(8)?;
                let inner = signature
                    .get(1..signature.len().saturating_sub(1))
                    .ok_or_else(|| CodecError::BadSignature(signature.into()))?;
                let parts = complete_types(inner);
                let (Some(kt), Some(vt)) = (parts.first(), parts.get(1)) else {
                    return Err(CodecError::BadSignature(signature.into()));
                };
                let k = self.value(kt)?;
                let v = self.value(vt)?;
                Value::DictEntry(Box::new(k), Box::new(v))
            }

            b'v' => {
                let inner_sig = self.signature()?;
                if type_length(inner_sig.as_bytes()) != Some(inner_sig.len()) {
                    return Err(CodecError::BadSignature(inner_sig));
                }
                Value::Variant(Box::new(self.value(&inner_sig)?))
            }

            other => return Err(CodecError::BadTypeCode(other as char)),
        })
    }

    /// Read a whole message body described by `signature`.
    pub fn body(&mut self, signature: &str) -> Result<Vec<Value>> {
        complete_types(signature)
            .iter()
            .map(|t| self.value(t))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode then decode, and require the value to survive unchanged.
    fn round_trip(v: Value) {
        let sig = v.signature();
        let mut enc = Encoder::new();
        enc.value(&v);
        let bytes = enc.finish();
        let mut dec = Decoder::new(&bytes, true);
        let got = dec.value(&sig).expect("decode failed");
        assert_eq!(got, v, "round trip changed the value (signature {sig})");
        assert_eq!(
            dec.remaining(),
            0,
            "decoder left {} bytes unread",
            dec.remaining()
        );
    }

    #[test]
    fn scalars_round_trip() {
        round_trip(Value::Byte(0xAB));
        round_trip(Value::Bool(true));
        round_trip(Value::Bool(false));
        round_trip(Value::Int16(-2));
        round_trip(Value::Uint16(65535));
        round_trip(Value::Int32(-70000));
        round_trip(Value::Uint32(4_000_000_000));
        round_trip(Value::Int64(-1));
        round_trip(Value::Uint64(u64::MAX));
        round_trip(Value::Double(-0.5));
    }

    #[test]
    fn strings_round_trip_including_empty_and_utf8() {
        round_trip(Value::Str(String::new()));
        round_trip(Value::Str("org.bluez".into()));
        round_trip(Value::Str("naïve ☕".into()));
        round_trip(Value::ObjectPath("/org/bluez/hci0".into()));
        round_trip(Value::Signature("a{sv}".into()));
    }

    #[test]
    fn containers_round_trip() {
        round_trip(Value::bytes(&[]));
        round_trip(Value::bytes(&[1, 2, 3]));
        round_trip(Value::string_array(["a".into(), "bb".into()]));
        round_trip(Value::Struct(vec![
            Value::Uint32(1),
            Value::Str("x".into()),
        ]));
        round_trip(Value::Variant(Box::new(Value::Str("v".into()))));
        round_trip(Value::dict([
            ("Powered".into(), Value::Bool(true)),
            ("Address".into(), Value::Str("AA:BB:CC:DD:EE:FF".into())),
        ]));
    }

    #[test]
    fn the_get_managed_objects_shape_round_trips() {
        // `a{oa{sa{sv}}}` is the reply this crate leans on hardest.
        let managed = Value::Array {
            element: "{oa{sa{sv}}}".into(),
            items: vec![Value::DictEntry(
                Box::new(Value::ObjectPath("/org/bluez/hci0/dev_AA_BB".into())),
                Box::new(Value::Array {
                    element: "{sa{sv}}".into(),
                    items: vec![Value::DictEntry(
                        Box::new(Value::Str("org.bluez.Device1".into())),
                        Box::new(Value::dict([
                            ("Name".into(), Value::Str("Pixel".into())),
                            ("RSSI".into(), Value::Int16(-42)),
                            (
                                "UUIDs".into(),
                                Value::string_array(
                                    ["0000180f-0000-1000-8000-00805f9b34fb".into()],
                                ),
                            ),
                        ])),
                    )],
                }),
            )],
        };
        round_trip(managed.clone());

        // …and navigating it gives the values back.
        let by_path = managed.as_map();
        let ifaces = by_path.get("/org/bluez/hci0/dev_AA_BB").unwrap().as_map();
        let props = ifaces.get("org.bluez.Device1").unwrap();
        assert_eq!(props.get("Name").unwrap().as_str(), Some("Pixel"));
        assert_eq!(props.get("RSSI").unwrap().as_i64(), Some(-42));
    }

    #[test]
    fn alignment_padding_is_inserted_between_values() {
        // byte then uint32: the u32 must start at offset 4, not 1.
        let mut enc = Encoder::new();
        enc.body(&[Value::Byte(1), Value::Uint32(2)]);
        let bytes = enc.finish();
        assert_eq!(bytes.len(), 8, "expected 1 byte + 3 pad + 4");
        assert_eq!(&bytes[1..4], &[0, 0, 0], "padding must be zero");

        let mut dec = Decoder::new(&bytes, true);
        assert_eq!(
            dec.body("yu").unwrap(),
            vec![Value::Byte(1), Value::Uint32(2)]
        );
    }

    #[test]
    fn struct_elements_are_eight_aligned() {
        // A byte then a struct: the struct starts at 8.
        let mut enc = Encoder::new();
        enc.body(&[Value::Byte(1), Value::Struct(vec![Value::Byte(2)])]);
        let bytes = enc.finish();
        assert_eq!(bytes.len(), 9);
        assert_eq!(bytes[8], 2);
    }

    #[test]
    fn empty_arrays_keep_their_element_type() {
        let empty = Value::Array {
            element: "{sv}".into(),
            items: vec![],
        };
        round_trip(empty.clone());
        assert_eq!(empty.signature(), "a{sv}");
    }

    #[test]
    fn big_endian_messages_decode() {
        // The bus may speak either byte order; only the decoder must cope.
        let bytes = [0x00, 0x00, 0x00, 0x2A];
        let mut dec = Decoder::new(&bytes, false);
        assert_eq!(dec.value("u").unwrap(), Value::Uint32(42));
    }

    // ── Hostile input ───────────────────────────────────────────────────────
    // Lengths come from the peer, so every one of these is reachable from the
    // network and must be an error rather than a panic.

    #[test]
    fn a_truncated_buffer_errors_rather_than_panicking() {
        let mut dec = Decoder::new(&[1, 2], true);
        assert!(matches!(dec.value("u"), Err(CodecError::Truncated { .. })));
    }

    #[test]
    fn an_oversized_string_length_errors() {
        // Claims 0xFFFFFFFF bytes of string in a 4-byte buffer.
        let bytes = [0xFF, 0xFF, 0xFF, 0xFF];
        let mut dec = Decoder::new(&bytes, true);
        assert!(matches!(dec.value("s"), Err(CodecError::Truncated { .. })));
    }

    #[test]
    fn an_oversized_array_length_errors() {
        let bytes = [0xFF, 0xFF, 0xFF, 0x7F];
        let mut dec = Decoder::new(&bytes, true);
        assert!(matches!(dec.value("ay"), Err(CodecError::Truncated { .. })));
    }

    #[test]
    fn a_bad_type_code_errors() {
        let mut dec = Decoder::new(&[0], true);
        assert!(matches!(dec.value("Z"), Err(CodecError::BadTypeCode('Z'))));
        assert!(matches!(
            Decoder::new(&[0], true).value(""),
            Err(CodecError::BadSignature(_))
        ));
    }

    #[test]
    fn a_variant_carrying_a_bad_signature_errors() {
        let mut enc = Encoder::new();
        enc.signature("a"); // "a" alone is not a complete type
        let bytes = enc.finish();
        let mut dec = Decoder::new(&bytes, true);
        assert!(matches!(dec.value("v"), Err(CodecError::BadSignature(_))));
    }

    #[test]
    fn invalid_utf8_in_a_string_errors() {
        let mut bytes = vec![2, 0, 0, 0, 0xFF, 0xFE, 0];
        bytes.truncate(7);
        let mut dec = Decoder::new(&bytes, true);
        assert!(matches!(dec.value("s"), Err(CodecError::BadUtf8)));
    }
}
