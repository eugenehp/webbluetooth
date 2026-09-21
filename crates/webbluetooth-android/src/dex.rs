//! A DEX writer, for classes that do not exist until we make them.
//!
//! Android's BLE APIs are callback-driven, and every callback type —
//! `BluetoothGattCallback`, `ScanCallback`, `BluetoothGattServerCallback`,
//! `AdvertiseCallback` — is an **abstract class**, not an interface. So
//! `java.lang.reflect.Proxy` cannot help: it only proxies interfaces. Something
//! has to subclass them, and on a device there is no compiler to do it.
//!
//! This is the same problem CoreBluetooth poses, and it gets the same answer:
//! build the class at run time. Where Apple has `objc_allocateClassPair`,
//! Android has a class *file format* — so the class is emitted as a DEX,
//! loaded with `InMemoryDexClassLoader`, and its methods bound to Rust
//! functions with `RegisterNatives`.
//!
//! # Why this is far less work than it sounds
//!
//! A method declared `native` has **no bytecode** — no `code_item` at all, just
//! a metadata entry. Since every override we need is native, this writer emits
//! constant tables and nothing else. The single exception is the constructor,
//! which must call `super()`, and that is four code units written by hand:
//!
//! ```text
//!     invoke-direct {v0}, Lsuper;-><init>()V
//!     return-void
//! ```
//!
//! So there is no compiler here, and no optimiser — a layout problem and a
//! handful of sorted tables.
//!
//! # The parts that must be exactly right
//!
//! * **Tables are sorted**, and the verifier rejects the file if they are not:
//!   strings by UTF-16 code point, types by descriptor, protos by return type
//!   then parameters, methods by `(class, name, proto)`.
//! * **`encoded_method` stores index *deltas***, not indices, so the order of
//!   the method list and the order of `method_ids` are coupled.
//! * **The header carries an Adler-32 and a SHA-1** over everything that
//!   follows them, written last.
//! * **The `map_list` must describe every section**, in offset order.

use std::collections::BTreeMap;

const HEADER_SIZE: usize = 0x70;
const ENDIAN_TAG: u32 = 0x1234_5678;
/// `dex\n035\0` — the oldest format every `InMemoryDexClassLoader` accepts.
const MAGIC: [u8; 8] = [0x64, 0x65, 0x78, 0x0A, 0x30, 0x33, 0x35, 0x00];

const ACC_PUBLIC: u32 = 0x1;
const ACC_NATIVE: u32 = 0x100;
const ACC_CONSTRUCTOR: u32 = 0x1_0000;

mod map_type {
    pub const HEADER: u16 = 0x0000;
    pub const STRING_ID: u16 = 0x0001;
    pub const TYPE_ID: u16 = 0x0002;
    pub const PROTO_ID: u16 = 0x0003;
    pub const METHOD_ID: u16 = 0x0005;
    pub const CLASS_DEF: u16 = 0x0006;
    pub const MAP_LIST: u16 = 0x1000;
    pub const TYPE_LIST: u16 = 0x1001;
    pub const CLASS_DATA: u16 = 0x2000;
    pub const CODE: u16 = 0x2001;
    pub const STRING_DATA: u16 = 0x2002;
}

/// One method to declare `native` on the generated class.
#[derive(Debug, Clone)]
pub struct NativeMethod {
    /// The Java name, e.g. `onConnectionStateChange`.
    pub name: String,
    /// A JNI type descriptor, e.g. `(Landroid/bluetooth/BluetoothGatt;II)V`.
    pub descriptor: String,
}

impl NativeMethod {
    pub fn new(name: &str, descriptor: &str) -> Self {
        Self {
            name: name.into(),
            descriptor: descriptor.into(),
        }
    }
}

/// Builds a DEX containing one class: a subclass with native overrides.
#[derive(Debug, Clone)]
pub struct DexBuilder {
    class: String,
    superclass: String,
    methods: Vec<NativeMethod>,
}

impl DexBuilder {
    /// `class` and `superclass` are internal names, e.g.
    /// `dev/webbluetooth/GattCallback` and
    /// `android/bluetooth/BluetoothGattCallback`.
    pub fn new(class: &str, superclass: &str) -> Self {
        Self {
            class: class.into(),
            superclass: superclass.into(),
            methods: Vec::new(),
        }
    }

    pub fn native_method(mut self, name: &str, descriptor: &str) -> Self {
        self.methods.push(NativeMethod::new(name, descriptor));
        self
    }

    /// Emit the DEX.
    pub fn build(&self) -> Vec<u8> {
        let class_descriptor = format!("L{};", self.class);
        let super_descriptor = format!("L{};", self.superclass);

        // ── Collect every string the file mentions ──────────────────────────
        let mut strings: Vec<String> = vec![
            class_descriptor.clone(),
            super_descriptor.clone(),
            "<init>".into(),
            "V".into(),
        ];
        let mut types: Vec<String> = vec![
            class_descriptor.clone(),
            super_descriptor.clone(),
            "V".into(),
        ];

        // The constructor, plus every native override.
        let mut protos: Vec<Proto> = vec![Proto::parse("()V")];
        for m in &self.methods {
            strings.push(m.name.clone());
            let proto = Proto::parse(&m.descriptor);
            types.push(proto.return_type.clone());
            types.extend(proto.parameters.iter().cloned());
            protos.push(proto);
        }
        for p in &protos {
            strings.push(p.shorty.clone());
            strings.push(p.return_type.clone());
            strings.extend(p.parameters.iter().cloned());
        }
        strings.extend(types.iter().cloned());

        strings.sort_by(|a, b| compare_mutf8(a, b));
        strings.dedup();
        let string_index: BTreeMap<&str, u32> = strings
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), i as u32))
            .collect();

        // Types are sorted by their descriptor's string index.
        types.sort_by(|a, b| compare_mutf8(a, b));
        types.dedup();
        let type_index: BTreeMap<&str, u32> = types
            .iter()
            .enumerate()
            .map(|(i, t)| (t.as_str(), i as u32))
            .collect();

        // Protos sort by return type, then by parameter list.
        protos.sort_by(|a, b| {
            let by_return =
                type_index[a.return_type.as_str()].cmp(&type_index[b.return_type.as_str()]);
            by_return.then_with(|| {
                let a_params: Vec<u32> = a
                    .parameters
                    .iter()
                    .map(|p| type_index[p.as_str()])
                    .collect();
                let b_params: Vec<u32> = b
                    .parameters
                    .iter()
                    .map(|p| type_index[p.as_str()])
                    .collect();
                a_params.cmp(&b_params)
            })
        });
        protos.dedup_by(|a, b| a.descriptor == b.descriptor);
        let proto_index: BTreeMap<&str, u32> = protos
            .iter()
            .enumerate()
            .map(|(i, p)| (p.descriptor.as_str(), i as u32))
            .collect();

        // ── Method ids ──────────────────────────────────────────────────────
        // `super.<init>()V` is referenced by the constructor's bytecode, so it
        // needs an id even though we do not define it.
        let mut method_ids: Vec<MethodId> = vec![
            MethodId {
                class: class_descriptor.clone(),
                name: "<init>".into(),
                proto: "()V".into(),
            },
            MethodId {
                class: super_descriptor.clone(),
                name: "<init>".into(),
                proto: "()V".into(),
            },
        ];
        for m in &self.methods {
            method_ids.push(MethodId {
                class: class_descriptor.clone(),
                name: m.name.clone(),
                proto: m.descriptor.clone(),
            });
        }
        method_ids.sort_by_key(|m| {
            (
                type_index[m.class.as_str()],
                string_index[m.name.as_str()],
                proto_index[m.proto.as_str()],
            )
        });
        let method_index: BTreeMap<(&str, &str, &str), u32> = method_ids
            .iter()
            .enumerate()
            .map(|(i, m)| {
                (
                    (m.class.as_str(), m.name.as_str(), m.proto.as_str()),
                    i as u32,
                )
            })
            .collect();

        // ── Lay the file out ────────────────────────────────────────────────
        let string_ids_off = HEADER_SIZE;
        let type_ids_off = string_ids_off + strings.len() * 4;
        let proto_ids_off = type_ids_off + types.len() * 4;
        let method_ids_off = proto_ids_off + protos.len() * 12;
        let class_defs_off = method_ids_off + method_ids.len() * 8;
        let data_off = class_defs_off + 32;

        let mut data = Section::new(data_off);

        // Parameter type lists, one per proto that takes arguments.
        let mut type_list_offsets: BTreeMap<String, usize> = BTreeMap::new();
        for p in &protos {
            if p.parameters.is_empty() {
                continue;
            }
            data.align(4);
            type_list_offsets.insert(p.descriptor.clone(), data.position());
            data.u32(p.parameters.len() as u32);
            for param in &p.parameters {
                data.u16(type_index[param.as_str()] as u16);
            }
        }

        // String data.
        let mut string_data_offsets: Vec<usize> = Vec::with_capacity(strings.len());
        for s in &strings {
            string_data_offsets.push(data.position());
            // The length is in UTF-16 code units, the bytes are MUTF-8.
            data.uleb128(s.chars().map(|c| c.len_utf16()).sum::<usize>() as u32);
            data.bytes(&mutf8(s));
            data.u8(0);
        }

        // The constructor's code: call super, return.
        data.align(4);
        let code_off = data.position();
        let super_init = method_index[&(super_descriptor.as_str(), "<init>", "()V")];
        data.u16(1); // registers_size — just `this`
        data.u16(1); // ins_size
        data.u16(1); // outs_size
        data.u16(0); // tries_size
        data.u32(0); // debug_info_off
        data.u32(4); // insns_size, in 16-bit units
                     // invoke-direct {v0}, super.<init>()V   (format 35c: A=1 arg, G=0)
        data.u16(0x1070);
        data.u16(super_init as u16);
        data.u16(0x0000); // registers C..F, all zero: v0
        data.u16(0x000E); // return-void

        // class_data_item: no fields, one direct method (the constructor),
        // and every native override as a virtual method.
        let class_data_off = data.position();
        data.uleb128(0); // static_fields_size
        data.uleb128(0); // instance_fields_size
        data.uleb128(1); // direct_methods_size
        data.uleb128(self.methods.len() as u32); // virtual_methods_size

        let ctor = method_index[&(class_descriptor.as_str(), "<init>", "()V")];
        data.uleb128(ctor); // first entry's index is absolute
        data.uleb128(ACC_PUBLIC | ACC_CONSTRUCTOR);
        data.uleb128(code_off as u32);

        // Virtual methods are stored as deltas, so they must be emitted in
        // method_ids order.
        let mut virtuals: Vec<u32> = self
            .methods
            .iter()
            .map(|m| {
                method_index[&(
                    class_descriptor.as_str(),
                    m.name.as_str(),
                    m.descriptor.as_str(),
                )]
            })
            .collect();
        virtuals.sort_unstable();
        let mut previous = 0u32;
        for (i, index) in virtuals.iter().enumerate() {
            data.uleb128(if i == 0 { *index } else { index - previous });
            data.uleb128(ACC_PUBLIC | ACC_NATIVE);
            data.uleb128(0); // native methods have no code
            previous = *index;
        }

        // ── map_list, last, describing everything ───────────────────────────
        data.align(4);
        let map_off = data.position();
        let mut map: Vec<(u16, u32, u32)> = vec![
            (map_type::HEADER, 1, 0),
            (
                map_type::STRING_ID,
                strings.len() as u32,
                string_ids_off as u32,
            ),
            (map_type::TYPE_ID, types.len() as u32, type_ids_off as u32),
            (
                map_type::PROTO_ID,
                protos.len() as u32,
                proto_ids_off as u32,
            ),
            (
                map_type::METHOD_ID,
                method_ids.len() as u32,
                method_ids_off as u32,
            ),
            (map_type::CLASS_DEF, 1, class_defs_off as u32),
        ];
        if !type_list_offsets.is_empty() {
            let first = *type_list_offsets.values().min().expect("non-empty");
            map.push((
                map_type::TYPE_LIST,
                type_list_offsets.len() as u32,
                first as u32,
            ));
        }
        map.push((
            map_type::STRING_DATA,
            strings.len() as u32,
            string_data_offsets[0] as u32,
        ));
        map.push((map_type::CODE, 1, code_off as u32));
        map.push((map_type::CLASS_DATA, 1, class_data_off as u32));
        map.push((map_type::MAP_LIST, 1, map_off as u32));
        map.sort_by_key(|(_, _, offset)| *offset);

        data.u32(map.len() as u32);
        for (kind, size, offset) in &map {
            data.u16(*kind);
            data.u16(0);
            data.u32(*size);
            data.u32(*offset);
        }

        let data_bytes = data.finish();
        let data_size = data_bytes.len();
        let file_size = data_off + data_size;

        // ── Assemble ────────────────────────────────────────────────────────
        let mut out = vec![0u8; file_size];
        out[..8].copy_from_slice(&MAGIC);

        let mut header = Cursor::new(&mut out, 32);
        header.u32(file_size as u32);
        header.u32(HEADER_SIZE as u32);
        header.u32(ENDIAN_TAG);
        header.u32(0); // link_size
        header.u32(0); // link_off
        header.u32(map_off as u32);
        header.u32(strings.len() as u32);
        header.u32(string_ids_off as u32);
        header.u32(types.len() as u32);
        header.u32(type_ids_off as u32);
        header.u32(protos.len() as u32);
        header.u32(proto_ids_off as u32);
        header.u32(0); // field_ids_size
        header.u32(0); // field_ids_off — zero when empty
        header.u32(method_ids.len() as u32);
        header.u32(method_ids_off as u32);
        header.u32(1); // class_defs_size
        header.u32(class_defs_off as u32);
        header.u32(data_size as u32);
        header.u32(data_off as u32);

        let mut strings_cursor = Cursor::new(&mut out, string_ids_off);
        for offset in &string_data_offsets {
            strings_cursor.u32(*offset as u32);
        }

        let mut types_cursor = Cursor::new(&mut out, type_ids_off);
        for t in &types {
            types_cursor.u32(string_index[t.as_str()]);
        }

        let mut protos_cursor = Cursor::new(&mut out, proto_ids_off);
        for p in &protos {
            protos_cursor.u32(string_index[p.shorty.as_str()]);
            protos_cursor.u32(type_index[p.return_type.as_str()]);
            protos_cursor.u32(type_list_offsets.get(&p.descriptor).copied().unwrap_or(0) as u32);
        }

        let mut methods_cursor = Cursor::new(&mut out, method_ids_off);
        for m in &method_ids {
            methods_cursor.u16(type_index[m.class.as_str()] as u16);
            methods_cursor.u16(proto_index[m.proto.as_str()] as u16);
            methods_cursor.u32(string_index[m.name.as_str()]);
        }

        let mut class_cursor = Cursor::new(&mut out, class_defs_off);
        class_cursor.u32(type_index[class_descriptor.as_str()]);
        class_cursor.u32(ACC_PUBLIC);
        class_cursor.u32(type_index[super_descriptor.as_str()]);
        class_cursor.u32(0); // interfaces_off
        class_cursor.u32(0xFFFF_FFFF); // source_file_idx — NO_INDEX
        class_cursor.u32(0); // annotations_off
        class_cursor.u32(class_data_off as u32);
        class_cursor.u32(0); // static_values_off

        out[data_off..].copy_from_slice(&data_bytes);

        // The signature covers everything after it; the checksum covers
        // everything after *it*, signature included. Order matters.
        let signature = sha1(&out[32..]);
        out[12..32].copy_from_slice(&signature);
        let checksum = adler32(&out[12..]);
        out[8..12].copy_from_slice(&checksum.to_le_bytes());

        out
    }
}

struct MethodId {
    class: String,
    name: String,
    proto: String,
}

/// A parsed method descriptor.
#[derive(Debug, Clone)]
struct Proto {
    descriptor: String,
    shorty: String,
    return_type: String,
    parameters: Vec<String>,
}

impl Proto {
    /// Split `(Landroid/bluetooth/BluetoothGatt;II)V` into its parts.
    fn parse(descriptor: &str) -> Self {
        let bytes = descriptor.as_bytes();
        assert_eq!(
            bytes.first(),
            Some(&b'('),
            "not a method descriptor: {descriptor}"
        );
        let close = descriptor
            .find(')')
            .expect("unterminated method descriptor");

        let mut parameters = Vec::new();
        let mut i = 1;
        while i < close {
            let len = type_length(&bytes[i..close]).expect("malformed parameter type");
            parameters.push(descriptor[i..i + len].to_string());
            i += len;
        }
        let return_type = descriptor[close + 1..].to_string();

        // The shorty collapses every reference type to `L`.
        let mut shorty = String::new();
        shorty.push(shorty_char(&return_type));
        for p in &parameters {
            shorty.push(shorty_char(p));
        }

        Self {
            descriptor: descriptor.to_string(),
            shorty,
            return_type,
            parameters,
        }
    }
}

/// How many bytes the first type descriptor occupies.
fn type_length(bytes: &[u8]) -> Option<usize> {
    match bytes.first()? {
        b'[' => Some(1 + type_length(&bytes[1..])?),
        b'L' => bytes.iter().position(|b| *b == b';').map(|i| i + 1),
        b'V' | b'Z' | b'B' | b'S' | b'C' | b'I' | b'J' | b'F' | b'D' => Some(1),
        _ => None,
    }
}

fn shorty_char(descriptor: &str) -> char {
    match descriptor.as_bytes().first() {
        Some(b'L') | Some(b'[') => 'L',
        Some(c) => *c as char,
        None => 'V',
    }
}

/// Compare two strings the way the DEX spec orders them: by UTF-16 code point.
fn compare_mutf8(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars().cmp(b.chars())
}

/// Modified UTF-8: like UTF-8, except NUL is two bytes and astral characters
/// are written as a surrogate pair. Class and method names are ASCII in
/// practice, so this only has to be correct, not fast.
fn mutf8(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let code = c as u32;
        match code {
            0 => out.extend_from_slice(&[0xC0, 0x80]),
            0x01..=0x7F => out.push(code as u8),
            0x80..=0x7FF => {
                out.push(0xC0 | (code >> 6) as u8);
                out.push(0x80 | (code & 0x3F) as u8);
            }
            0x800..=0xFFFF => {
                out.push(0xE0 | (code >> 12) as u8);
                out.push(0x80 | ((code >> 6) & 0x3F) as u8);
                out.push(0x80 | (code & 0x3F) as u8);
            }
            _ => {
                // Astral: encode the UTF-16 surrogate pair, three bytes each.
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let unit = *unit as u32;
                    out.push(0xE0 | (unit >> 12) as u8);
                    out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                    out.push(0x80 | (unit & 0x3F) as u8);
                }
            }
        }
    }
    out
}

// ── Little helpers for writing ──────────────────────────────────────────────

/// An append-only buffer that knows its absolute offset in the file.
struct Section {
    base: usize,
    buf: Vec<u8>,
}

impl Section {
    fn new(base: usize) -> Self {
        Self {
            base,
            buf: Vec::new(),
        }
    }
    fn position(&self) -> usize {
        self.base + self.buf.len()
    }
    fn align(&mut self, n: usize) {
        while !self.position().is_multiple_of(n) {
            self.buf.push(0);
        }
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    /// Unsigned LEB128, as every size and index in a `class_data_item` is.
    fn uleb128(&mut self, mut v: u32) {
        loop {
            let mut byte = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            self.buf.push(byte);
            if v == 0 {
                break;
            }
        }
    }
    fn finish(self) -> Vec<u8> {
        self.buf
    }
}

/// Writes into an already-sized buffer at a fixed offset.
struct Cursor<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a mut [u8], at: usize) -> Self {
        Self { buf, at }
    }
    fn u16(&mut self, v: u16) {
        self.buf[self.at..self.at + 2].copy_from_slice(&v.to_le_bytes());
        self.at += 2;
    }
    fn u32(&mut self, v: u32) {
        self.buf[self.at..self.at + 4].copy_from_slice(&v.to_le_bytes());
        self.at += 4;
    }
}

/// Adler-32, as the DEX header's checksum.
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// SHA-1, as the DEX header's signature.
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
    while !(message.len() % 64).is_multiple_of(64) && message.len() % 64 != 56 {
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

    fn sample() -> Vec<u8> {
        DexBuilder::new(
            "dev/webbluetooth/GattCallback",
            "android/bluetooth/BluetoothGattCallback",
        )
        .native_method(
            "onConnectionStateChange",
            "(Landroid/bluetooth/BluetoothGatt;II)V",
        )
        .native_method(
            "onServicesDiscovered",
            "(Landroid/bluetooth/BluetoothGatt;I)V",
        )
        .native_method(
            "onCharacteristicChanged",
            "(Landroid/bluetooth/BluetoothGatt;Landroid/bluetooth/BluetoothGattCharacteristic;[B)V",
        )
        .build()
    }

    fn read_u32(dex: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([dex[at], dex[at + 1], dex[at + 2], dex[at + 3]])
    }

    #[test]
    fn the_header_is_well_formed() {
        let dex = sample();
        assert_eq!(&dex[..8], &MAGIC, "wrong magic");
        assert_eq!(
            read_u32(&dex, 32),
            dex.len() as u32,
            "file_size disagrees with the file"
        );
        assert_eq!(read_u32(&dex, 36), HEADER_SIZE as u32);
        assert_eq!(read_u32(&dex, 40), ENDIAN_TAG);
        assert_eq!(read_u32(&dex, 96), 1, "expected exactly one class_def");
    }

    #[test]
    fn the_checksum_and_signature_cover_the_right_ranges() {
        let dex = sample();
        // SHA-1 covers everything after the signature field.
        assert_eq!(&dex[12..32], &sha1(&dex[32..]), "signature mismatch");
        // Adler-32 covers everything after the checksum field — signature too.
        assert_eq!(read_u32(&dex, 8), adler32(&dex[12..]), "checksum mismatch");
    }

    #[test]
    fn a_changed_class_changes_the_signature() {
        let a = sample();
        let b = DexBuilder::new(
            "dev/webbluetooth/Other",
            "android/bluetooth/BluetoothGattCallback",
        )
        .native_method(
            "onServicesDiscovered",
            "(Landroid/bluetooth/BluetoothGatt;I)V",
        )
        .build();
        assert_ne!(&a[12..32], &b[12..32]);
    }

    #[test]
    fn sha1_matches_known_vectors() {
        // Hand-checking a DEX signature is impractical, so pin the primitive.
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
    fn adler32_matches_known_vectors() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"abc"), 0x024D_0127);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn method_descriptors_parse() {
        let p = Proto::parse("(Landroid/bluetooth/BluetoothGatt;II)V");
        assert_eq!(p.return_type, "V");
        assert_eq!(
            p.parameters,
            ["Landroid/bluetooth/BluetoothGatt;", "I", "I"]
        );
        assert_eq!(p.shorty, "VLII", "the shorty collapses references to L");

        let arrays = Proto::parse("([BLjava/lang/String;)[I");
        assert_eq!(arrays.parameters, ["[B", "Ljava/lang/String;"]);
        assert_eq!(arrays.return_type, "[I");
        assert_eq!(arrays.shorty, "LLL");

        let empty = Proto::parse("()V");
        assert!(empty.parameters.is_empty());
        assert_eq!(empty.shorty, "V");
    }

    #[test]
    fn uleb128_matches_the_spec() {
        let encode = |v: u32| {
            let mut s = Section::new(0);
            s.uleb128(v);
            s.finish()
        };
        assert_eq!(encode(0), vec![0x00]);
        assert_eq!(encode(1), vec![0x01]);
        assert_eq!(encode(127), vec![0x7F]);
        assert_eq!(encode(128), vec![0x80, 0x01]);
        assert_eq!(encode(16256), vec![0x80, 0x7F]);
    }

    #[test]
    fn strings_are_sorted_by_code_point() {
        // The verifier rejects an unsorted string table outright.
        let dex = sample();
        let count = read_u32(&dex, 56) as usize;
        let ids_off = read_u32(&dex, 60) as usize;

        let mut previous: Option<String> = None;
        for i in 0..count {
            let data_off = read_u32(&dex, ids_off + i * 4) as usize;
            // Skip the uleb128 length, then read to the NUL.
            let mut at = data_off;
            while dex[at] & 0x80 != 0 {
                at += 1;
            }
            at += 1;
            let end = dex[at..].iter().position(|b| *b == 0).unwrap() + at;
            let s = String::from_utf8_lossy(&dex[at..end]).into_owned();
            if let Some(prev) = &previous {
                assert!(
                    prev.chars().lt(s.chars()),
                    "string table out of order: {prev:?} then {s:?}"
                );
            }
            previous = Some(s);
        }
    }

    #[test]
    fn mutf8_encodes_nul_as_two_bytes() {
        // The one place MUTF-8 differs from UTF-8 for text we might emit.
        assert_eq!(mutf8("a\0b"), vec![b'a', 0xC0, 0x80, b'b']);
        assert_eq!(mutf8("onScanResult"), b"onScanResult".to_vec());
    }

    #[test]
    fn the_map_list_is_in_offset_order_and_ends_the_file() {
        let dex = sample();
        let map_off = read_u32(&dex, 52) as usize;
        let count = read_u32(&dex, map_off) as usize;
        assert!(count >= 8, "expected every section in the map, got {count}");

        let mut last = 0;
        for i in 0..count {
            let entry = map_off + 4 + i * 12;
            let offset = read_u32(&dex, entry + 8);
            assert!(offset >= last, "map_list is not in offset order");
            last = offset;
        }
        // The map is the last thing written.
        assert_eq!(last as usize, map_off);
    }

    #[test]
    fn a_class_with_no_overrides_still_builds() {
        // Degenerate but legal: the constructor alone.
        let dex = DexBuilder::new("dev/webbluetooth/Empty", "java/lang/Object").build();
        assert_eq!(&dex[..8], &MAGIC);
        assert_eq!(read_u32(&dex, 32), dex.len() as u32);
        assert_eq!(&dex[12..32], &sha1(&dex[32..]));
    }
}
