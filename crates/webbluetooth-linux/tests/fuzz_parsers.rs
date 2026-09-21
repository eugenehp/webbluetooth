//! Feed the wire parsers garbage and require that they do not panic.
//!
//! Every parser here decodes bytes that arrive from somewhere untrusted — the
//! air, or the system bus — and each was written by hand. A malformed packet
//! must produce `None`, an empty list, or a partial result; it must never index
//! out of bounds, subtract past zero, or allocate on a length it read from the
//! input without checking.
//!
//! This is a fuzzer rather than a set of cases because the interesting inputs
//! are the ones nobody thought of. It is deterministic — a fixed seed, an
//! xorshift generator, no dependencies — so a failure is reproducible and CI
//! does not go red at random. Widening `ROUNDS` explores more.

use webbluetooth_linux::dbus::Message;
use webbluetooth_linux::{att, hci};

/// How many inputs each target tries.
///
/// Miri makes every operation orders of magnitude slower, so under it the
/// corpus is small — the point there is not coverage but that Miri is watching
/// for undefined behaviour a normal run would miss. `WEBBLUETOOTH_FUZZ_ROUNDS`
/// overrides either way, for a longer soak.
fn rounds() -> usize {
    if let Ok(n) = std::env::var("WEBBLUETOOTH_FUZZ_ROUNDS") {
        if let Ok(n) = n.parse() {
            return n;
        }
    }
    if cfg!(miri) {
        500
    } else {
        200_000
    }
}

/// xorshift64*, so the corpus is reproducible without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn byte(&mut self) -> u8 {
        (self.next() >> 24) as u8
    }

    /// A buffer of up to `max` bytes.
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let len = (self.next() as usize) % (max + 1);
        (0..len).map(|_| self.byte()).collect()
    }
}

/// Mostly-random bytes, but with the first byte drawn from the real opcodes
/// often enough to get past the initial match and into the length handling,
/// which is where the bugs live.
fn plausible_pdu(rng: &mut Rng) -> Vec<u8> {
    let mut pdu = rng.bytes(64);
    if !pdu.is_empty() && rng.next().is_multiple_of(2) {
        const OPCODES: [u8; 14] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x11, 0x13, 0x1B,
        ];
        pdu[0] = OPCODES[(rng.next() as usize) % OPCODES.len()];
    }
    pdu
}

#[test]
fn att_pdus_survive_arbitrary_bytes() {
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    for _ in 0..rounds() {
        let pdu = plausible_pdu(&mut rng);
        // The contract is "no panic", so the result is deliberately unused.
        let _ = att::parse(&pdu);
    }
}

#[test]
fn advertising_reports_survive_arbitrary_bytes() {
    let mut rng = Rng(0x5EED_1234_ABCD_0002);
    for _ in 0..rounds() {
        let body = rng.bytes(96);
        let _ = hci::parse_advertising_report(&body);
        let _ = hci::parse_extended_advertising_report(&body);
    }
}

/// A report whose declared count and lengths disagree with the buffer is the
/// shape most likely to walk off the end, so it gets its own generator: a
/// believable header followed by a truncated body.
#[test]
fn reports_with_lying_lengths_are_survivable() {
    let mut rng = Rng(0x5EED_1234_ABCD_0003);
    for _ in 0..rounds() {
        let mut body = vec![(rng.next() % 6) as u8]; // claimed report count
        body.extend(rng.bytes(40));
        if body.len() > 9 {
            // Overwrite the legacy data-length field with something oversized.
            body[9] = rng.byte() | 0x80;
        }
        let _ = hci::parse_advertising_report(&body);
        let _ = hci::parse_extended_advertising_report(&body);
    }
}

#[test]
fn uuids_survive_arbitrary_lengths() {
    let mut rng = Rng(0x5EED_1234_ABCD_0004);
    for _ in 0..rounds() {
        let bytes = rng.bytes(20);
        let _ = att::Uuid::parse(&bytes);
    }
}

#[test]
fn advertising_data_records_survive_arbitrary_bytes() {
    let mut rng = Rng(0x5EED_1234_ABCD_0005);
    for _ in 0..rounds() {
        // AD is a chain of length-prefixed records, so a length that overruns
        // the buffer is the case to hammer.
        let mut data = Vec::new();
        while data.len() < 40 {
            data.push(rng.byte());
            data.extend(rng.bytes(6));
        }
        let mut advertisement = hci::Advertisement::default();
        hci::parse_advertising_data(&data, &mut advertisement);
    }
}

/// A D-Bus message is length-prefixed, signature-directed and alignment-padded,
/// which gives a malformed one three separate ways to send the decoder past the
/// end of the buffer. It is also the parser whose input comes from another
/// process rather than the air.
#[test]
fn dbus_messages_survive_arbitrary_bytes() {
    let mut rng = Rng(0x5EED_1234_ABCD_0006);
    for _ in 0..rounds() {
        let mut buf = rng.bytes(128);
        // A plausible fixed header often enough to reach the body decoder:
        // endianness, type, flags, protocol version.
        if buf.len() >= 16 && rng.next().is_multiple_of(2) {
            buf[0] = if rng.next().is_multiple_of(2) {
                b'l'
            } else {
                b'B'
            };
            buf[1] = (rng.next() % 5) as u8;
            buf[3] = 1;
        }
        let _ = Message::parse(&buf);
    }
}

/// The same, but with a body length that disagrees with what follows —
/// the field a decoder is most tempted to trust.
#[test]
fn dbus_messages_with_lying_lengths_are_survivable() {
    let mut rng = Rng(0x5EED_1234_ABCD_0007);
    for _ in 0..rounds() {
        let mut buf = vec![0u8; 16];
        buf[0] = b'l';
        buf[1] = 1 + (rng.next() % 4) as u8;
        buf[3] = 1;
        // Body length and header-array length, both oversized on purpose.
        buf[4..8].copy_from_slice(&(rng.next() as u32).to_le_bytes());
        buf[12..16].copy_from_slice(&(rng.next() as u32).to_le_bytes());
        buf.extend(rng.bytes(48));
        let _ = Message::parse(&buf);
    }
}
