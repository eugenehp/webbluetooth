//! Turning a characteristic's bytes into something readable, and back.
//!
//! An explorer never knows what a value means. All it can honestly do is show
//! the same bytes several ways and let the operator recognise one of them,
//! which is why the detail pane offers hex, ASCII, decimal and a short list of
//! numeric readings rather than picking an interpretation.

/// Space-separated uppercase hex — `01 A2 FF`.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// Printable ASCII, with everything else as `.`.
///
/// Deliberately not `String::from_utf8_lossy`: a replacement character is
/// several columns wide and breaks the alignment with the hex beside it, and a
/// control byte is not a decoding failure here — it is just not printable.
pub fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            }
        })
        .collect()
}

/// Space-separated decimal bytes — `1 162 255`.
pub fn decimal(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A classic hex dump: offset, sixteen bytes, the ASCII gutter.
///
/// For anything longer than a line or two, where `hex` alone stops being
/// countable by eye.
pub fn hexdump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let hexpart = chunk
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!(
            "{:04X}  {:<47}  {}\n",
            row * 16,
            hexpart,
            ascii(chunk)
        ));
    }
    out
}

/// Read a hex string, ignoring whitespace, `:`, `-` and a leading `0x`.
///
/// Lenient about separators because the bytes being pasted in came from
/// somewhere else — a datasheet, a packet capture, this program's own hex
/// field — and each of those writes them differently.
pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = text
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':' && *c != '-' && *c != '_')
        .collect();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(format!(
            "{} hex digits is an odd number — a byte needs two",
            cleaned.len()
        ));
    }
    if let Some(bad) = cleaned.chars().find(|c| !c.is_ascii_hexdigit()) {
        return Err(format!("{bad:?} is not a hex digit"));
    }
    Ok(cleaned
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let s = std::str::from_utf8(pair).expect("ascii");
            u8::from_str_radix(s, 16).expect("checked above")
        })
        .collect())
}

/// Every numeric reading the length permits, little-endian first.
///
/// GATT is little-endian throughout, so that is what leads; big-endian is
/// offered too because plenty of vendor characteristics ignore the convention.
/// Returns `(label, rendering)` pairs, and nothing at all for a length no
/// integer fits.
pub fn interpretations(bytes: &[u8]) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    match bytes.len() {
        1 => {
            out.push(("u8", bytes[0].to_string()));
            out.push(("i8", (bytes[0] as i8).to_string()));
        }
        2 => {
            let le = u16::from_le_bytes([bytes[0], bytes[1]]);
            out.push(("u16 LE", le.to_string()));
            out.push(("i16 LE", (le as i16).to_string()));
            out.push((
                "u16 BE",
                u16::from_be_bytes([bytes[0], bytes[1]]).to_string(),
            ));
        }
        4 => {
            let raw = [bytes[0], bytes[1], bytes[2], bytes[3]];
            let le = u32::from_le_bytes(raw);
            out.push(("u32 LE", le.to_string()));
            out.push(("i32 LE", (le as i32).to_string()));
            out.push(("f32 LE", format!("{}", f32::from_le_bytes(raw))));
            out.push(("u32 BE", u32::from_be_bytes(raw).to_string()));
        }
        8 => {
            let mut raw = [0u8; 8];
            raw.copy_from_slice(bytes);
            out.push(("u64 LE", u64::from_le_bytes(raw).to_string()));
            out.push(("f64 LE", format!("{}", f64::from_le_bytes(raw))));
        }
        _ => {}
    }
    // Worth showing whenever it is the whole value and not a coincidence:
    // device names, firmware revisions and model strings are all plain UTF-8.
    if !bytes.is_empty() {
        if let Ok(text) = std::str::from_utf8(bytes) {
            if text.chars().all(|c| !c.is_control()) {
                out.push(("UTF-8", text.to_owned()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_through_the_parser() {
        let bytes = vec![0x00, 0x01, 0xA2, 0xFF, 0x7F];
        assert_eq!(hex(&bytes), "00 01 A2 FF 7F");
        assert_eq!(parse_hex(&hex(&bytes)).unwrap(), bytes);
    }

    #[test]
    fn the_parser_takes_however_it_was_written() {
        for text in ["0xA2FF", "a2 ff", "A2:FF", "a2-FF", "  A2FF  ", "A2_FF"] {
            assert_eq!(parse_hex(text).unwrap(), vec![0xA2, 0xFF], "{text:?}");
        }
        assert_eq!(parse_hex("").unwrap(), Vec::<u8>::new());
        assert_eq!(parse_hex("   ").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn a_bad_hex_string_says_what_is_wrong_with_it() {
        assert!(parse_hex("A2F").unwrap_err().contains("odd number"));
        assert!(parse_hex("A2FG").unwrap_err().contains("not a hex digit"));
    }

    #[test]
    fn unprintable_bytes_stay_one_column_wide() {
        // One byte in, one column out — the hex beside it has to line up.
        let bytes: Vec<u8> = (0u8..=255).collect();
        assert_eq!(ascii(&bytes).chars().count(), 256);
        assert_eq!(ascii(b"hi\0\x1b\xff"), "hi...");
    }

    #[test]
    fn readings_are_offered_only_where_they_fit() {
        assert!(interpretations(&[]).is_empty());
        assert!(interpretations(&[1, 2, 3])
            .iter()
            .all(|(l, _)| *l == "UTF-8"));

        let two = interpretations(&[0x2C, 0x01]);
        assert_eq!(two.iter().find(|(l, _)| *l == "u16 LE").unwrap().1, "300");
        assert_eq!(two.iter().find(|(l, _)| *l == "u16 BE").unwrap().1, "11265");

        let neg = interpretations(&[0xFF]);
        assert_eq!(neg.iter().find(|(l, _)| *l == "u8").unwrap().1, "255");
        assert_eq!(neg.iter().find(|(l, _)| *l == "i8").unwrap().1, "-1");
    }

    #[test]
    fn utf8_is_offered_only_when_the_whole_value_is_text() {
        let has_text = |b: &[u8]| interpretations(b).iter().any(|(l, _)| *l == "UTF-8");
        assert!(has_text(b"Nordic_Blinky"));
        assert!(!has_text(&[0xFF, 0xFE, 0xFD]));
        assert!(!has_text(b"two\nlines"), "a control byte is not a reading");
    }

    #[test]
    fn a_dump_is_sixteen_bytes_to_a_line() {
        let dump = hexdump(&(0u8..20).collect::<Vec<_>>());
        let lines: Vec<&str> = dump.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("0000  00 01 02"));
        assert!(lines[1].starts_with("0010  10 11 12 13 "));
    }
}
