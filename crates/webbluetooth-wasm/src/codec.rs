//! The wire format between Rust and the shim.
//!
//! WebAssembly can pass integers and floats, and nothing else. Every string,
//! UUID and byte array crosses as a `(pointer, length)` into linear memory,
//! which means both sides need to agree on how a message is laid out inside
//! that buffer.
//!
//! The format is deliberately the dullest thing that works: little-endian
//! `u32` for every number, and a length-prefix before every variable-length
//! field. There is no self-description, no tags and no versioning, because
//! both ends ship together — a mismatch is a build error in this repository,
//! not a runtime negotiation.
//!
//! ```text
//! u8      a byte
//! u16     two bytes, little-endian
//! u32     four bytes, little-endian
//! bool    one byte, 0 or 1
//! bytes   u32 length, then that many bytes
//! str     bytes, valid UTF-8
//! option  bool present, then the value if present
//! list<T> u32 count, then that many T
//! ```

/// Builds a message.
#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }

    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }

    pub fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }

    pub fn option_str(&mut self, v: Option<&str>) -> &mut Self {
        match v {
            Some(s) => {
                self.bool(true);
                self.str(s)
            }
            None => self.bool(false),
        }
    }

    pub fn list<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) -> &mut Self {
        self.u32(items.len() as u32);
        for item in items {
            each(self, item);
        }
        self
    }

    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

/// Reads a message.
///
/// Every read is fallible and returns `None` past the end rather than
/// panicking: the bytes come from JavaScript, so a malformed message is
/// something to reject, not something to trust.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }

    pub fn bool(&mut self) -> Option<bool> {
        self.u8().map(|b| b != 0)
    }

    pub fn bytes(&mut self) -> Option<Vec<u8>> {
        let len = self.u32()? as usize;
        self.take(len).map(<[u8]>::to_vec)
    }

    pub fn str(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        let raw = self.take(len)?;
        // Lossy rather than failing: a name with a broken code unit in it is
        // still a usable name, and the alternative is dropping the device.
        Some(String::from_utf8_lossy(raw).into_owned())
    }

    pub fn option_str(&mut self) -> Option<Option<String>> {
        match self.bool()? {
            true => self.str().map(Some),
            false => Some(None),
        }
    }

    pub fn list<T>(&mut self, mut each: impl FnMut(&mut Self) -> Option<T>) -> Option<Vec<T>> {
        let count = self.u32()? as usize;
        // A count is not a promise: a corrupt message could claim four
        // billion, so the reader grows as it goes rather than reserving.
        let mut out = Vec::new();
        for _ in 0..count {
            out.push(each(self)?);
        }
        Some(out)
    }

    /// Whether everything was consumed — a message with bytes left over did
    /// not mean what this reader thought it meant.
    pub fn is_empty(&self) -> bool {
        self.at == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_survives_a_round_trip() {
        let mut w = Writer::new();
        w.u8(0xAB)
            .u16(0xBEEF)
            .u32(0xDEAD_BEEF)
            .bool(true)
            .bool(false)
            .bytes(&[1, 2, 3])
            .str("héllo")
            .option_str(Some("here"))
            .option_str(None);
        let buffer = w.finish();

        let mut r = Reader::new(&buffer);
        assert_eq!(r.u8(), Some(0xAB));
        assert_eq!(r.u16(), Some(0xBEEF));
        assert_eq!(r.u32(), Some(0xDEAD_BEEF));
        assert_eq!(r.bool(), Some(true));
        assert_eq!(r.bool(), Some(false));
        assert_eq!(r.bytes(), Some(vec![1, 2, 3]));
        assert_eq!(r.str().as_deref(), Some("héllo"));
        assert_eq!(r.option_str(), Some(Some("here".into())));
        assert_eq!(r.option_str(), Some(None));
        assert!(r.is_empty(), "the whole message should have been consumed");
    }

    #[test]
    fn lists_round_trip() {
        let mut w = Writer::new();
        w.list(&["a", "bb", "ccc"], |w, s| {
            w.str(s);
        });
        let buffer = w.finish();

        let mut r = Reader::new(&buffer);
        assert_eq!(
            r.list(|r| r.str()),
            Some(vec!["a".to_owned(), "bb".to_owned(), "ccc".to_owned()])
        );
        assert!(r.is_empty());
    }

    /// The bytes come from JavaScript, so a truncated message has to be
    /// rejected rather than read past the end of the buffer.
    #[test]
    fn reading_past_the_end_is_none_rather_than_a_panic() {
        let mut r = Reader::new(&[]);
        assert_eq!(r.u8(), None);
        assert_eq!(r.u32(), None);
        assert_eq!(r.bytes(), None);
        assert_eq!(r.str(), None);

        // A length prefix that runs off the end.
        let mut r = Reader::new(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(r.bytes(), None);

        // Truncated mid-value.
        let mut r = Reader::new(&[1, 2]);
        assert_eq!(r.u32(), None);
    }

    /// A count field is attacker-controlled, so it must not be used to
    /// reserve: `u32::MAX` items would ask for 4 GiB before reading anything.
    #[test]
    fn an_enormous_count_fails_instead_of_allocating() {
        let mut w = Writer::new();
        w.u32(u32::MAX);
        let buffer = w.finish();
        let mut r = Reader::new(&buffer);
        assert_eq!(r.list(|r| r.u8()), None);
    }

    #[test]
    fn a_short_read_leaves_the_message_unconsumed() {
        let mut w = Writer::new();
        w.u32(7).u32(9);
        let buffer = w.finish();
        let mut r = Reader::new(&buffer);
        assert_eq!(r.u32(), Some(7));
        assert!(!r.is_empty(), "one field is still pending");
    }
}
