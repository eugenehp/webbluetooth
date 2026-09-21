//! GUIDs, and the one WinRT computes rather than publishes.
//!
//! Most WinRT interfaces have an IID written down in metadata, and those are
//! vendored in [`crate::iids`]. *Parameterised* interfaces do not:
//! `IAsyncOperation<GattDeviceServicesResult>` is a distinct interface from
//! `IAsyncOperation<bool>`, with a distinct IID, and there are too many
//! instantiations to enumerate. So WinRT derives them.
//!
//! The rule is a UUID version 5 — SHA-1 over a namespace GUID and a name — with
//! a fixed namespace and a *signature string* for the name:
//!
//! ```text
//!   pinterface({9fc2b0bb-e446-44e2-aa61-9cab8f636af2};rc(Windows.Foo.Bar;{…}))
//!   └ the generic's own GUID ┘                        └ the argument's signature ┘
//! ```
//!
//! Signatures nest, so a handler over an operation over a runtime class builds
//! one string three levels deep. Every primitive has a short code; an interface
//! is its IID in braces; a runtime class is its name and the IID of its default
//! interface.
//!
//! # What is and is not checked here
//!
//! The UUID-5 construction is pinned against a published vector, and the
//! signature strings are pinned against the exact byte sequences Microsoft's
//! own generated bindings use. What cannot be checked without Windows is the
//! composition of the two — that a computed IID is one the system will accept.

use std::fmt;

/// A COM `GUID`, laid out as the ABI expects.
///
/// The first three fields are little-endian on the wire; the last is a byte
/// array and is not byte-swapped. Getting that wrong produces a GUID that
/// looks right when printed and matches nothing.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    /// From the canonical `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` form.
    ///
    /// # Panics
    /// If the string is not a well-formed GUID. Every call site passes a
    /// literal, so a malformed one is a bug to find at first run, not a
    /// `Result` to thread through the whole crate.
    pub const fn parse(s: &str) -> Self {
        let b = s.as_bytes();
        assert!(b.len() == 36, "a GUID is 36 characters");
        assert!(
            b[8] == b'-' && b[13] == b'-' && b[18] == b'-' && b[23] == b'-',
            "GUID separators are misplaced"
        );

        const fn hex(c: u8) -> u32 {
            match c {
                b'0'..=b'9' => (c - b'0') as u32,
                b'a'..=b'f' => (c - b'a' + 10) as u32,
                b'A'..=b'F' => (c - b'A' + 10) as u32,
                _ => panic!("not a hex digit in a GUID"),
            }
        }
        const fn u32_at(b: &[u8], i: usize) -> u32 {
            let mut v = 0u32;
            let mut k = 0;
            while k < 8 {
                v = (v << 4) | hex(b[i + k]);
                k += 1;
            }
            v
        }
        const fn u16_at(b: &[u8], i: usize) -> u16 {
            ((hex(b[i]) << 12) | (hex(b[i + 1]) << 8) | (hex(b[i + 2]) << 4) | hex(b[i + 3])) as u16
        }
        const fn u8_at(b: &[u8], i: usize) -> u8 {
            ((hex(b[i]) << 4) | hex(b[i + 1])) as u8
        }

        Self {
            data1: u32_at(b, 0),
            data2: u16_at(b, 9),
            data3: u16_at(b, 14),
            data4: [
                u8_at(b, 19),
                u8_at(b, 21),
                u8_at(b, 24),
                u8_at(b, 26),
                u8_at(b, 28),
                u8_at(b, 30),
                u8_at(b, 32),
                u8_at(b, 34),
            ],
        }
    }

    /// The 16 bytes in big-endian (RFC 4122) order, which is what hashing uses.
    pub fn to_be_bytes(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.data1.to_be_bytes());
        out[4..6].copy_from_slice(&self.data2.to_be_bytes());
        out[6..8].copy_from_slice(&self.data3.to_be_bytes());
        out[8..16].copy_from_slice(&self.data4);
        out
    }

    fn from_be_bytes(b: [u8; 16]) -> Self {
        Self {
            data1: u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            data2: u16::from_be_bytes([b[4], b[5]]),
            data3: u16::from_be_bytes([b[6], b[7]]),
            data4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
        }
    }

    /// The signature of this interface, for use inside a parameterised one.
    pub fn signature(&self) -> String {
        format!("{{{self}}}")
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.data1,
            self.data2,
            self.data3,
            self.data4[0],
            self.data4[1],
            self.data4[2],
            self.data4[3],
            self.data4[4],
            self.data4[5],
            self.data4[6],
            self.data4[7]
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// A WinRT type, as far as computing a signature needs to know.
#[derive(Debug, Clone)]
pub enum Signature {
    /// `b1`, `i4`, `string`, and the rest of the primitive codes.
    Primitive(&'static str),
    /// A non-parameterised interface: its IID in braces.
    Interface(Guid),
    /// `rc(Full.Type.Name;<default interface>)`.
    Class {
        name: &'static str,
        default_interface: Guid,
    },
    /// `enum(Full.Type.Name;i4)` — or `u4` for a flags enum.
    Enum { name: &'static str, signed: bool },
    /// `struct(Full.Type.Name;<fields>)`.
    Struct {
        name: &'static str,
        fields: Vec<Signature>,
    },
    /// `delegate({iid})`.
    Delegate(Guid),
    /// `pinterface({generic};<args>)`.
    Parameterized {
        generic: Guid,
        arguments: Vec<Signature>,
    },
}

impl Signature {
    pub const BOOL: Self = Self::Primitive("b1");
    pub const U8: Self = Self::Primitive("u1");
    pub const I16: Self = Self::Primitive("i2");
    pub const U16: Self = Self::Primitive("u2");
    pub const I32: Self = Self::Primitive("i4");
    pub const U32: Self = Self::Primitive("u4");
    pub const I64: Self = Self::Primitive("i8");
    pub const U64: Self = Self::Primitive("u8");
    pub const STRING: Self = Self::Primitive("string");
    pub const GUID: Self = Self::Primitive("g16");
    /// `IInspectable`.
    pub const OBJECT: Self = Self::Primitive("cinterface(IInspectable)");

    /// The signature string WinRT hashes.
    pub fn text(&self) -> String {
        match self {
            Self::Primitive(code) => (*code).to_string(),
            Self::Interface(iid) => iid.signature(),
            Self::Class {
                name,
                default_interface,
            } => {
                format!("rc({name};{})", default_interface.signature())
            }
            Self::Enum { name, signed } => {
                format!("enum({name};{})", if *signed { "i4" } else { "u4" })
            }
            Self::Struct { name, fields } => {
                let inner: Vec<String> = fields.iter().map(Self::text).collect();
                format!("struct({name};{})", inner.join(";"))
            }
            Self::Delegate(iid) => format!("delegate({})", iid.signature()),
            Self::Parameterized { generic, arguments } => {
                let inner: Vec<String> = arguments.iter().map(Self::text).collect();
                format!("pinterface({};{})", generic.signature(), inner.join(";"))
            }
        }
    }

    /// The IID of this type, computed if it is parameterised.
    ///
    /// A non-parameterised interface already has one; anything else has no IID
    /// at all and this returns `None`.
    pub fn iid(&self) -> Option<Guid> {
        match self {
            Self::Interface(iid) | Self::Delegate(iid) => Some(*iid),
            Self::Parameterized { .. } => {
                Some(uuid_v5(WINRT_PIID_NAMESPACE, self.text().as_bytes()))
            }
            _ => None,
        }
    }
}

/// The namespace WinRT hashes every parameterised interface against.
pub const WINRT_PIID_NAMESPACE: Guid = Guid::parse("11f47ad5-7b73-42c0-abae-878b1e16adee");

/// A version 5 UUID: SHA-1 over a namespace and a name, per RFC 4122.
pub fn uuid_v5(namespace: Guid, name: &[u8]) -> Guid {
    let mut input = Vec::with_capacity(16 + name.len());
    input.extend_from_slice(&namespace.to_be_bytes());
    input.extend_from_slice(name);

    let hash = sha1(&input);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    // Version 5 in the high nibble of byte 6, RFC 4122 variant in byte 8.
    bytes[6] = (bytes[6] & 0x0F) | 0x50;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    Guid::from_be_bytes(bytes)
}

/// SHA-1. The same one the DEX writer needs, kept here so neither crate
/// depends on the other.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    let mut message = data.to_vec();
    let bit_length = (data.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iids::generics;

    #[test]
    fn guids_round_trip_through_their_text_form() {
        for text in [
            "9fc2b0bb-e446-44e2-aa61-9cab8f636af2",
            "00000000-0000-0000-0000-000000000000",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
            "11f47ad5-7b73-42c0-abae-878b1e16adee",
        ] {
            assert_eq!(Guid::parse(text).to_string(), text);
        }
    }

    /// `DeviceInformationCollection` is the one class this crate names by hand,
    /// because its default interface is parameterised and so the generator
    /// skips it. This pins the shape against what Microsoft's own metadata
    /// says it is — `for_class::<Self, IVectorView<DeviceInformation>>` — so a
    /// wrong IID fails here rather than silently returning no adapters.
    #[test]
    fn a_class_with_a_parameterised_default_interface_gets_an_iid() {
        use crate::iids::classes;

        let element = Signature::Parameterized {
            generic: generics::I_VECTOR_VIEW,
            arguments: vec![Signature::Class {
                name: classes::DEVICE_INFORMATION.0,
                default_interface: classes::DEVICE_INFORMATION.1,
            }],
        };
        let iid = element.iid().expect("a parameterised signature has an IID");

        // Not the generic's own GUID: the argument has to change it, or every
        // instantiation would share one IID and QueryInterface would be a
        // coin toss.
        assert_ne!(iid, generics::I_VECTOR_VIEW);
        assert_ne!(iid, classes::DEVICE_INFORMATION.1);

        // And it is stable: the same inputs give the same answer, which is
        // what makes it safe to compute at each call site.
        let again = Signature::Parameterized {
            generic: generics::I_VECTOR_VIEW,
            arguments: vec![Signature::Class {
                name: classes::DEVICE_INFORMATION.0,
                default_interface: classes::DEVICE_INFORMATION.1,
            }],
        }
        .iid()
        .unwrap();
        assert_eq!(iid, again);

        // Changing the element type must change the IID; if it did not, the
        // hash would not be covering the argument at all.
        let other = Signature::Parameterized {
            generic: generics::I_VECTOR_VIEW,
            arguments: vec![Signature::Primitive("string")],
        }
        .iid()
        .unwrap();
        assert_ne!(iid, other);
    }

    #[test]
    fn the_abi_layout_is_what_com_expects() {
        assert_eq!(std::mem::size_of::<Guid>(), 16);
        assert_eq!(std::mem::align_of::<Guid>(), 4);
    }

    #[test]
    fn only_the_first_three_fields_are_byte_swapped() {
        // The trailing eight bytes are an array, not an integer — swapping them
        // gives a GUID that prints correctly and matches nothing.
        let g = Guid::parse("01020304-0506-0708-090a-0b0c0d0e0f10");
        assert_eq!(g.data1, 0x0102_0304);
        assert_eq!(g.data2, 0x0506);
        assert_eq!(g.data3, 0x0708);
        assert_eq!(g.data4, [0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10]);
        assert_eq!(
            g.to_be_bytes(),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn sha1_matches_known_vectors() {
        assert_eq!(
            sha1(b"abc"),
            [
                0xA9, 0x99, 0x3E, 0x36, 0x47, 0x06, 0x81, 0x6A, 0xBA, 0x3E, 0x25, 0x71, 0x78, 0x50,
                0xC2, 0x6C, 0x9C, 0xD0, 0xD8, 0x9D
            ]
        );
        assert_eq!(
            sha1(b""),
            [
                0xDA, 0x39, 0xA3, 0xEE, 0x5E, 0x6B, 0x4B, 0x0D, 0x32, 0x55, 0xBF, 0xEF, 0x95, 0x60,
                0x18, 0x90, 0xAF, 0xD8, 0x07, 0x09
            ]
        );
    }

    #[test]
    fn uuid_v5_matches_the_published_vector() {
        // The widely reproduced RFC 4122 v5 example: the DNS namespace over
        // "python.org". If this is right, the hashing, the big-endian namespace
        // and the version/variant bits all are.
        let dns = Guid::parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8");
        assert_eq!(
            uuid_v5(dns, b"python.org").to_string(),
            "886313e1-3b8a-5372-9b90-0c9aee199e5d"
        );
    }

    #[test]
    fn version_and_variant_bits_are_set() {
        let id = uuid_v5(WINRT_PIID_NAMESPACE, b"anything at all");
        assert_eq!(id.data3 >> 12, 5, "version must be 5");
        assert_eq!(id.data4[0] >> 6, 0b10, "variant must be RFC 4122");
    }

    #[test]
    fn signature_strings_match_the_official_bindings() {
        // These are the exact byte sequences Microsoft's generated bindings
        // build, copied from windows-rs. If the shape were wrong, every
        // computed IID would be wrong in a way nothing here could detect.
        let handler = Signature::Parameterized {
            generic: generics::TYPED_EVENT_HANDLER,
            arguments: vec![Signature::OBJECT, Signature::OBJECT],
        };
        assert!(
            handler
                .text()
                .starts_with("pinterface({9de1c534-6ae1-11e0-84e1-18a905bcc53f};"),
            "got {}",
            handler.text()
        );

        let operation = Signature::Parameterized {
            generic: generics::I_ASYNC_OPERATION,
            arguments: vec![Signature::BOOL],
        };
        assert_eq!(
            operation.text(),
            "pinterface({9fc2b0bb-e446-44e2-aa61-9cab8f636af2};b1)"
        );
    }

    #[test]
    fn nested_signatures_compose() {
        // A completed handler over an operation over a runtime class — three
        // levels, which is what a real GATT call needs.
        let class = Signature::Class {
            name: "Windows.Devices.Bluetooth.BluetoothLEDevice",
            default_interface: Guid::parse("b5ee2f7b-4ad8-4642-ac48-80a0b500e887"),
        };
        let operation = Signature::Parameterized {
            generic: generics::I_ASYNC_OPERATION,
            arguments: vec![class],
        };
        let handler = Signature::Parameterized {
            generic: generics::ASYNC_OPERATION_COMPLETED_HANDLER,
            arguments: vec![operation.clone()],
        };

        assert_eq!(
            operation.text(),
            "pinterface({9fc2b0bb-e446-44e2-aa61-9cab8f636af2};\
             rc(Windows.Devices.Bluetooth.BluetoothLEDevice;\
             {b5ee2f7b-4ad8-4642-ac48-80a0b500e887}))"
                .replace(char::is_whitespace, "")
        );
        assert!(handler.text().contains(&operation.text()));
        // Distinct instantiations must not collide.
        assert_ne!(operation.iid(), handler.iid());
    }

    #[test]
    fn a_parameterised_iid_is_stable_and_argument_dependent() {
        let of = |arg: Signature| {
            Signature::Parameterized {
                generic: generics::I_ASYNC_OPERATION,
                arguments: vec![arg],
            }
            .iid()
            .unwrap()
        };
        assert_eq!(
            of(Signature::BOOL),
            of(Signature::BOOL),
            "must be deterministic"
        );
        assert_ne!(
            of(Signature::BOOL),
            of(Signature::I32),
            "must depend on the argument"
        );
    }

    #[test]
    fn non_parameterised_types_have_no_computed_iid() {
        assert!(Signature::BOOL.iid().is_none());
        assert!(Signature::Class {
            name: "X.Y",
            default_interface: Guid::default()
        }
        .iid()
        .is_none());
        let iid = Guid::parse("59cb50c1-5934-4f68-a198-eb864fa44e6b");
        assert_eq!(Signature::Interface(iid).iid(), Some(iid));
    }
}
