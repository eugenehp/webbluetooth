//! The D-Bus type system.
//!
//! Every value on the bus is one of these, and every message body is described
//! by a *signature* — a string of type codes like `a{sv}` (array of string →
//! variant) or `oa{sa{sv}}` (the reply to `GetManagedObjects`). Marshalling is
//! signature-directed: the bytes carry no type tags, so the decoder must be
//! told what it is reading.

use std::collections::HashMap;
use std::fmt;

/// A D-Bus value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    Int16(i16),
    Uint16(u16),
    Int32(i32),
    Uint32(u32),
    Int64(i64),
    Uint64(u64),
    Double(f64),
    /// `s`
    Str(String),
    /// `o` — syntactically a string, semantically a path.
    ObjectPath(String),
    /// `g`
    Signature(String),
    /// `h` — an index into the message's file-descriptor array.
    UnixFd(u32),
    /// `a<elem>`. The element signature is carried so an empty array still
    /// marshals with the right type.
    Array {
        element: String,
        items: Vec<Value>,
    },
    /// `(...)`
    Struct(Vec<Value>),
    /// `v` — a value that carries its own signature.
    Variant(Box<Value>),
    /// `{kv}`, only ever valid as an array element.
    DictEntry(Box<Value>, Box<Value>),
}

impl Value {
    /// The signature of this value.
    pub fn signature(&self) -> String {
        match self {
            Self::Byte(_) => "y".into(),
            Self::Bool(_) => "b".into(),
            Self::Int16(_) => "n".into(),
            Self::Uint16(_) => "q".into(),
            Self::Int32(_) => "i".into(),
            Self::Uint32(_) => "u".into(),
            Self::Int64(_) => "x".into(),
            Self::Uint64(_) => "t".into(),
            Self::Double(_) => "d".into(),
            Self::Str(_) => "s".into(),
            Self::ObjectPath(_) => "o".into(),
            Self::Signature(_) => "g".into(),
            Self::UnixFd(_) => "h".into(),
            Self::Array { element, .. } => format!("a{element}"),
            Self::Struct(fields) => {
                let inner: String = fields.iter().map(Self::signature).collect();
                format!("({inner})")
            }
            Self::Variant(_) => "v".into(),
            Self::DictEntry(k, v) => format!("{{{}{}}}", k.signature(), v.signature()),
        }
    }

    /// Build an `a{sv}` from pairs — the shape of nearly every BlueZ argument.
    pub fn dict(entries: impl IntoIterator<Item = (String, Value)>) -> Self {
        Self::Array {
            element: "{sv}".into(),
            items: entries
                .into_iter()
                .map(|(k, v)| {
                    Value::DictEntry(
                        Box::new(Value::Str(k)),
                        Box::new(Value::Variant(Box::new(v))),
                    )
                })
                .collect(),
        }
    }

    /// Build an `as` from strings.
    pub fn string_array(items: impl IntoIterator<Item = String>) -> Self {
        Self::Array {
            element: "s".into(),
            items: items.into_iter().map(Value::Str).collect(),
        }
    }

    /// Build an `ay` from bytes.
    pub fn bytes(data: &[u8]) -> Self {
        Self::Array {
            element: "y".into(),
            items: data.iter().copied().map(Value::Byte).collect(),
        }
    }

    // ── Accessors ───────────────────────────────────────────────────────────
    // A value that came off the bus may be wrapped in a variant, sometimes
    // several deep, so every accessor unwraps first. Reading `Value::Str` from
    // a property without that is the single most common D-Bus client bug.

    /// Peel off any `Variant` wrappers.
    pub fn peel(&self) -> &Value {
        match self {
            Self::Variant(inner) => inner.peel(),
            other => other,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.peel() {
            Self::Str(s) | Self::ObjectPath(s) | Self::Signature(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.peel() {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self.peel() {
            Self::Byte(v) => Some(*v as u64),
            Self::Uint16(v) => Some(*v as u64),
            Self::Uint32(v) => Some(*v as u64),
            Self::Uint64(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self.peel() {
            Self::Byte(v) => Some(*v as i64),
            Self::Int16(v) => Some(*v as i64),
            Self::Uint16(v) => Some(*v as i64),
            Self::Int32(v) => Some(*v as i64),
            Self::Uint32(v) => Some(*v as i64),
            Self::Int64(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self.peel() {
            Self::Array { items, .. } => Some(items),
            Self::Struct(fields) => Some(fields),
            _ => None,
        }
    }

    /// Collect an `ay` into bytes.
    pub fn as_bytes(&self) -> Option<Vec<u8>> {
        let items = self.as_array()?;
        items
            .iter()
            .map(|v| match v.peel() {
                Self::Byte(b) => Some(*b),
                _ => None,
            })
            .collect()
    }

    /// Collect an `as` / `ao` into strings.
    pub fn as_strings(&self) -> Vec<String> {
        self.as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Interpret an `a{sv}` (or any dict) as a map.
    pub fn as_map(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        if let Some(items) = self.as_array() {
            for item in items {
                if let Self::DictEntry(k, v) = item.peel() {
                    if let Some(key) = k.as_str() {
                        out.insert(key.to_owned(), (**v).clone());
                    }
                }
            }
        }
        out
    }

    /// Look a key up in an `a{sv}`.
    pub fn get(&self, key: &str) -> Option<Value> {
        if let Some(items) = self.as_array() {
            for item in items {
                if let Self::DictEntry(k, v) = item.peel() {
                    if k.as_str() == Some(key) {
                        return Some((**v).clone());
                    }
                }
            }
        }
        None
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Str(s) | Self::ObjectPath(s) | Self::Signature(s) => write!(f, "{s}"),
            Self::Variant(v) => write!(f, "{v}"),
            other => write!(f, "{other:?}"),
        }
    }
}

// ── Signatures ──────────────────────────────────────────────────────────────

/// The natural alignment of a type code, in bytes.
///
/// Padding between values is what makes D-Bus marshalling fiddly: every value
/// starts at a multiple of its alignment, counted from the **start of the
/// message**, not from the start of the body.
pub fn alignment(code: u8) -> usize {
    match code {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b's' | b'o' | b'a' | b'h' => 4,
        b'x' | b't' | b'd' | b'(' | b')' | b'{' | b'}' | b'r' | b'e' => 8,
        _ => 1,
    }
}

/// Split a signature into its complete top-level types.
///
/// `"sa{sv}u"` → `["s", "a{sv}", "u"]`. Needed because a struct or array body
/// is described by a concatenation, not a list.
pub fn complete_types(signature: &str) -> Vec<String> {
    let bytes = signature.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let len = match type_length(&bytes[i..]) {
            Some(n) => n,
            None => break,
        };
        out.push(signature[i..i + len].to_string());
        i += len;
    }
    out
}

/// How many bytes of `signature` the first complete type occupies.
pub fn type_length(signature: &[u8]) -> Option<usize> {
    let first = *signature.first()?;
    match first {
        // `a` is followed by exactly one complete type.
        b'a' => Some(1 + type_length(&signature[1..])?),
        // Balanced brackets, which may nest.
        b'(' | b'{' => {
            let (open, close) = if first == b'(' {
                (b'(', b')')
            } else {
                (b'{', b'}')
            };
            let mut depth = 0usize;
            for (i, &c) in signature.iter().enumerate() {
                if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
            }
            None
        }
        _ => Some(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_round_trip_through_values() {
        assert_eq!(Value::Str("x".into()).signature(), "s");
        assert_eq!(Value::bytes(&[1, 2]).signature(), "ay");
        assert_eq!(Value::string_array(["a".into()]).signature(), "as");
        assert_eq!(
            Value::dict([("k".into(), Value::Uint32(1))]).signature(),
            "a{sv}"
        );
        assert_eq!(
            Value::Struct(vec![Value::Str("a".into()), Value::Uint32(2)]).signature(),
            "(su)"
        );
    }

    #[test]
    fn complete_types_splits_concatenations() {
        assert_eq!(complete_types("sa{sv}u"), vec!["s", "a{sv}", "u"]);
        assert_eq!(complete_types("ay"), vec!["ay"]);
        assert_eq!(complete_types(""), Vec::<String>::new());
        // The GetManagedObjects reply type — nested arrays of dicts.
        assert_eq!(complete_types("a{oa{sa{sv}}}"), vec!["a{oa{sa{sv}}}"]);
        assert_eq!(complete_types("(is)(bb)"), vec!["(is)", "(bb)"]);
    }

    #[test]
    fn nested_brackets_are_balanced_not_counted() {
        assert_eq!(type_length(b"a{sa{sv}}"), Some(9));
        assert_eq!(type_length(b"((ii)(ss))x"), Some(10));
        // Unbalanced input must not panic or over-read.
        assert_eq!(type_length(b"(is"), None);
        assert_eq!(type_length(b""), None);
    }

    #[test]
    fn alignments_match_the_specification() {
        for (code, want) in [
            (b'y', 1),
            (b'b', 4),
            (b'n', 2),
            (b'q', 2),
            (b'i', 4),
            (b'u', 4),
            (b'x', 8),
            (b't', 8),
            (b'd', 8),
            (b's', 4),
            (b'o', 4),
            (b'g', 1),
            (b'a', 4),
            (b'(', 8),
            (b'{', 8),
            (b'v', 1),
            (b'h', 4),
        ] {
            assert_eq!(alignment(code), want, "alignment of {}", code as char);
        }
    }

    #[test]
    fn accessors_peel_variants() {
        // BlueZ returns every property wrapped in a variant.
        let wrapped = Value::Variant(Box::new(Value::Str("Pixel".into())));
        assert_eq!(wrapped.as_str(), Some("Pixel"));

        let nested = Value::Variant(Box::new(Value::Variant(Box::new(Value::Bool(true)))));
        assert_eq!(nested.as_bool(), Some(true));

        let map = Value::dict([("Name".into(), Value::Str("Pixel".into()))]);
        assert_eq!(map.get("Name").unwrap().as_str(), Some("Pixel"));
        assert_eq!(map.get("Missing"), None);
    }

    #[test]
    fn byte_arrays_collect() {
        let v = Value::bytes(&[0xDE, 0xAD]);
        assert_eq!(v.as_bytes(), Some(vec![0xDE, 0xAD]));
        // A non-byte array is not bytes.
        assert_eq!(Value::string_array(["a".into()]).as_bytes(), None);
    }
}
