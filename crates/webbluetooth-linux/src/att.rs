//! The Attribute Protocol.
//!
//! ATT is what GATT is made of, and on Linux it is reachable directly: an
//! L2CAP socket bound to the fixed ATT channel (CID 4) is a GATT connection,
//! with the kernel establishing the LE link when the socket connects. No
//! daemon, no D-Bus, no privileges.
//!
//! The protocol is small. Every PDU is an opcode byte and a payload, all
//! little-endian, and every request gets exactly one response — the peer will
//! not accept a second request until the first is answered, which is why a
//! client has to serialise them.
//!
//! GATT's discovery procedures are built from four of them:
//!
//! | To find | Send | Over |
//! |---|---|---|
//! | primary services | `Read By Group Type` of `0x2800` | `0x0001..=0xFFFF` |
//! | characteristics | `Read By Type` of `0x2803` | the service's handle range |
//! | descriptors | `Find Information` | after the value handle |
//! | a value | `Read` / `Read Blob` | one handle |
//!
//! Subscribing is not a protocol operation at all: you write `0x0001` to the
//! characteristic's Client Characteristic Configuration descriptor, and
//! notifications then arrive unsolicited as `Handle Value Notification`.

/// ATT opcodes.
pub mod opcode {
    pub const ERROR_RESPONSE: u8 = 0x01;
    pub const EXCHANGE_MTU_REQUEST: u8 = 0x02;
    pub const EXCHANGE_MTU_RESPONSE: u8 = 0x03;
    pub const FIND_INFORMATION_REQUEST: u8 = 0x04;
    pub const FIND_INFORMATION_RESPONSE: u8 = 0x05;
    pub const READ_BY_TYPE_REQUEST: u8 = 0x08;
    pub const READ_BY_TYPE_RESPONSE: u8 = 0x09;
    pub const READ_REQUEST: u8 = 0x0A;
    pub const READ_RESPONSE: u8 = 0x0B;
    pub const READ_BLOB_REQUEST: u8 = 0x0C;
    pub const READ_BLOB_RESPONSE: u8 = 0x0D;
    pub const READ_BY_GROUP_TYPE_REQUEST: u8 = 0x10;
    pub const READ_BY_GROUP_TYPE_RESPONSE: u8 = 0x11;
    pub const WRITE_REQUEST: u8 = 0x12;
    pub const WRITE_RESPONSE: u8 = 0x13;
    pub const PREPARE_WRITE_REQUEST: u8 = 0x16;
    pub const PREPARE_WRITE_RESPONSE: u8 = 0x17;
    pub const EXECUTE_WRITE_REQUEST: u8 = 0x18;
    pub const EXECUTE_WRITE_RESPONSE: u8 = 0x19;
    pub const WRITE_COMMAND: u8 = 0x52;
    pub const HANDLE_VALUE_NOTIFICATION: u8 = 0x1B;
    pub const HANDLE_VALUE_INDICATION: u8 = 0x1D;
    pub const HANDLE_VALUE_CONFIRMATION: u8 = 0x1E;
}

/// GATT's own attribute types.
pub mod gatt_type {
    pub const PRIMARY_SERVICE: u16 = 0x2800;
    pub const SECONDARY_SERVICE: u16 = 0x2801;
    pub const INCLUDE: u16 = 0x2802;
    pub const CHARACTERISTIC: u16 = 0x2803;
    pub const CLIENT_CHARACTERISTIC_CONFIGURATION: u16 = 0x2902;
}

/// An ATT error code, as the peer reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttError(pub u8);

impl AttError {
    pub const INVALID_HANDLE: u8 = 0x01;
    pub const READ_NOT_PERMITTED: u8 = 0x02;
    pub const WRITE_NOT_PERMITTED: u8 = 0x03;
    pub const INSUFFICIENT_AUTHENTICATION: u8 = 0x05;
    pub const REQUEST_NOT_SUPPORTED: u8 = 0x06;
    pub const INVALID_OFFSET: u8 = 0x07;
    pub const INSUFFICIENT_AUTHORIZATION: u8 = 0x08;
    pub const ATTRIBUTE_NOT_FOUND: u8 = 0x0A;
    pub const ATTRIBUTE_NOT_LONG: u8 = 0x0B;
    pub const INVALID_ATTRIBUTE_VALUE_LENGTH: u8 = 0x0D;
    pub const UNLIKELY_ERROR: u8 = 0x0E;
    pub const INSUFFICIENT_ENCRYPTION: u8 = 0x0F;
    pub const INSUFFICIENT_RESOURCES: u8 = 0x11;

    /// `ATTRIBUTE_NOT_FOUND` ends a discovery loop rather than failing it —
    /// it is how the peer says "no more".
    pub fn is_end_of_discovery(self) -> bool {
        self.0 == Self::ATTRIBUTE_NOT_FOUND
    }

    pub fn describe(self) -> &'static str {
        match self.0 {
            Self::INVALID_HANDLE => "invalid handle",
            Self::READ_NOT_PERMITTED => "read not permitted",
            Self::WRITE_NOT_PERMITTED => "write not permitted",
            Self::INSUFFICIENT_AUTHENTICATION => "insufficient authentication",
            Self::REQUEST_NOT_SUPPORTED => "request not supported",
            Self::INVALID_OFFSET => "invalid offset",
            Self::INSUFFICIENT_AUTHORIZATION => "insufficient authorization",
            Self::ATTRIBUTE_NOT_FOUND => "attribute not found",
            Self::ATTRIBUTE_NOT_LONG => "attribute not long",
            Self::INVALID_ATTRIBUTE_VALUE_LENGTH => "invalid attribute value length",
            Self::UNLIKELY_ERROR => "unlikely error",
            Self::INSUFFICIENT_ENCRYPTION => "insufficient encryption",
            Self::INSUFFICIENT_RESOURCES => "insufficient resources",
            _ => "unknown ATT error",
        }
    }
}

impl std::fmt::Display for AttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (0x{:02x})", self.describe(), self.0)
    }
}

/// A 16- or 128-bit attribute type, as it appears on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uuid {
    Short(u16),
    Long([u8; 16]),
}

impl Uuid {
    /// Decode from wire bytes: 2 for a short UUID, 16 for a long one.
    ///
    /// Both go out least-significant byte first, so a 128-bit UUID on the wire
    /// is the reverse of how it is written.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes.len() {
            2 => Some(Self::Short(u16::from_le_bytes([bytes[0], bytes[1]]))),
            16 => {
                let mut be = [0u8; 16];
                for (i, b) in bytes.iter().enumerate() {
                    be[15 - i] = *b;
                }
                Some(Self::Long(be))
            }
            _ => None,
        }
    }

    /// Encode to wire bytes, least-significant first.
    pub fn to_wire(&self) -> Vec<u8> {
        match self {
            Self::Short(v) => v.to_le_bytes().to_vec(),
            Self::Long(be) => be.iter().rev().copied().collect(),
        }
    }

    /// The canonical 128-bit lowercase string, expanding the Bluetooth base
    /// UUID for short forms.
    pub fn to_canonical(&self) -> String {
        let bytes = match self {
            Self::Short(v) => {
                let mut b = [
                    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5F,
                    0x9B, 0x34, 0xFB,
                ];
                b[2] = (*v >> 8) as u8;
                b[3] = *v as u8;
                b
            }
            Self::Long(b) => *b,
        };
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }
}

/// A discovered primary service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub start_handle: u16,
    pub end_handle: u16,
    pub uuid: Uuid,
}

/// A discovered characteristic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Characteristic {
    /// The handle of the characteristic *declaration*.
    pub handle: u16,
    /// The handle its value lives at — what reads and writes address.
    pub value_handle: u16,
    pub properties: u8,
    pub uuid: Uuid,
}

/// A discovered descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub handle: u16,
    pub uuid: Uuid,
}

// ── Encoding requests ───────────────────────────────────────────────────────

pub fn exchange_mtu_request(mtu: u16) -> Vec<u8> {
    let mut pdu = vec![opcode::EXCHANGE_MTU_REQUEST];
    pdu.extend_from_slice(&mtu.to_le_bytes());
    pdu
}

pub fn read_by_group_type_request(start: u16, end: u16, group_type: u16) -> Vec<u8> {
    let mut pdu = vec![opcode::READ_BY_GROUP_TYPE_REQUEST];
    pdu.extend_from_slice(&start.to_le_bytes());
    pdu.extend_from_slice(&end.to_le_bytes());
    pdu.extend_from_slice(&group_type.to_le_bytes());
    pdu
}

pub fn read_by_type_request(start: u16, end: u16, attribute_type: u16) -> Vec<u8> {
    let mut pdu = vec![opcode::READ_BY_TYPE_REQUEST];
    pdu.extend_from_slice(&start.to_le_bytes());
    pdu.extend_from_slice(&end.to_le_bytes());
    pdu.extend_from_slice(&attribute_type.to_le_bytes());
    pdu
}

pub fn find_information_request(start: u16, end: u16) -> Vec<u8> {
    let mut pdu = vec![opcode::FIND_INFORMATION_REQUEST];
    pdu.extend_from_slice(&start.to_le_bytes());
    pdu.extend_from_slice(&end.to_le_bytes());
    pdu
}

pub fn read_request(handle: u16) -> Vec<u8> {
    let mut pdu = vec![opcode::READ_REQUEST];
    pdu.extend_from_slice(&handle.to_le_bytes());
    pdu
}

pub fn read_blob_request(handle: u16, offset: u16) -> Vec<u8> {
    let mut pdu = vec![opcode::READ_BLOB_REQUEST];
    pdu.extend_from_slice(&handle.to_le_bytes());
    pdu.extend_from_slice(&offset.to_le_bytes());
    pdu
}

pub fn write_request(handle: u16, value: &[u8]) -> Vec<u8> {
    let mut pdu = vec![opcode::WRITE_REQUEST];
    pdu.extend_from_slice(&handle.to_le_bytes());
    pdu.extend_from_slice(value);
    pdu
}

/// A write with no response. The peer never acknowledges it, so there is
/// nothing to wait for and nothing to learn if it is dropped.
/// `Prepare Write Request` — one fragment of a value too long for one write.
///
/// The peer queues the fragment and echoes it back; nothing is applied until
/// [`execute_write_request`]. `offset` is the position within the attribute,
/// so the fragments together spell the whole value.
pub fn prepare_write_request(handle: u16, offset: u16, value: &[u8]) -> Vec<u8> {
    let mut pdu = Vec::with_capacity(5 + value.len());
    pdu.push(opcode::PREPARE_WRITE_REQUEST);
    pdu.extend_from_slice(&handle.to_le_bytes());
    pdu.extend_from_slice(&offset.to_le_bytes());
    pdu.extend_from_slice(value);
    pdu
}

/// `Execute Write Request` — apply the queued fragments, or discard them.
pub fn execute_write_request(apply: bool) -> Vec<u8> {
    vec![opcode::EXECUTE_WRITE_REQUEST, u8::from(apply)]
}

/// `Exchange MTU Response` — our answer when the *peer* opens the exchange.
pub fn exchange_mtu_response(mtu: u16) -> Vec<u8> {
    let mut pdu = Vec::with_capacity(3);
    pdu.push(opcode::EXCHANGE_MTU_RESPONSE);
    pdu.extend_from_slice(&mtu.to_le_bytes());
    pdu
}

/// `Error Response` — how a server declines a request it will not serve.
pub fn error_response(request: u8, handle: u16, error: AttError) -> Vec<u8> {
    let mut pdu = Vec::with_capacity(5);
    pdu.push(opcode::ERROR_RESPONSE);
    pdu.push(request);
    pdu.extend_from_slice(&handle.to_le_bytes());
    pdu.push(error.0);
    pdu
}

/// Is this opcode a request the *peer* is making of us?
///
/// ATT numbers requests even and responses odd, but not uniformly, and both
/// travel the same connection: a client that treats every inbound PDU as the
/// reply to its own outstanding request will consume the peer's questions as
/// answers. Commands (bit 6) and notifications are excluded — they need no
/// reply and are handled elsewhere.
pub fn is_peer_request(opcode: u8) -> bool {
    matches!(
        opcode,
        opcode::EXCHANGE_MTU_REQUEST
            | opcode::FIND_INFORMATION_REQUEST
            | opcode::READ_BY_TYPE_REQUEST
            | opcode::READ_REQUEST
            | opcode::READ_BLOB_REQUEST
            | opcode::READ_BY_GROUP_TYPE_REQUEST
            | opcode::WRITE_REQUEST
            | opcode::PREPARE_WRITE_REQUEST
            | opcode::EXECUTE_WRITE_REQUEST
    )
}

pub fn write_command(handle: u16, value: &[u8]) -> Vec<u8> {
    let mut pdu = vec![opcode::WRITE_COMMAND];
    pdu.extend_from_slice(&handle.to_le_bytes());
    pdu.extend_from_slice(value);
    pdu
}

pub fn handle_value_confirmation() -> Vec<u8> {
    vec![opcode::HANDLE_VALUE_CONFIRMATION]
}

// ── Decoding responses ──────────────────────────────────────────────────────

/// One `Include` declaration: a service pointing at another service.
///
/// `uuid` is absent when the included service has a 128-bit UUID, which does
/// not fit the declaration — the UUID then has to be read from the included
/// service's own declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct Include {
    /// Where the declaration itself lives.
    pub handle: u16,
    /// The included service's first handle.
    pub start_handle: u16,
    /// Its last handle.
    pub end_handle: u16,
    pub uuid: Option<Uuid>,
}

/// What came back.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// The peer refused. `opcode` is the request it refused.
    Error {
        opcode: u8,
        handle: u16,
        error: AttError,
    },
    Mtu(u16),
    Services(Vec<Service>),
    Characteristics(Vec<Characteristic>),
    Includes(Vec<Include>),
    Descriptors(Vec<Descriptor>),
    Value(Vec<u8>),
    WriteComplete,
    /// Unsolicited: a subscription delivering a value.
    Notification {
        handle: u16,
        value: Vec<u8>,
    },
    /// Unsolicited, and must be confirmed.
    Indication {
        handle: u16,
        value: Vec<u8>,
    },
    /// One fragment of a long write was queued by the peer.
    PrepareWriteComplete,
    /// The peer is making a request of us, which is not a reply to anything.
    PeerRequest {
        opcode: u8,
        body: Vec<u8>,
    },
    /// An opcode this client does not implement.
    Unknown(u8),
}

/// Decode one PDU.
pub fn parse(pdu: &[u8]) -> Option<Response> {
    let opcode = *pdu.first()?;
    let body = &pdu[1..];
    Some(match opcode {
        opcode::ERROR_RESPONSE => {
            if body.len() < 4 {
                return None;
            }
            Response::Error {
                opcode: body[0],
                handle: u16::from_le_bytes([body[1], body[2]]),
                error: AttError(body[3]),
            }
        }

        opcode::EXCHANGE_MTU_RESPONSE => {
            if body.len() < 2 {
                return None;
            }
            Response::Mtu(u16::from_le_bytes([body[0], body[1]]))
        }

        // The peer asking *us* something. Kept separate from every reply so it
        // can never be mistaken for one.
        op if is_peer_request(op) => Response::PeerRequest {
            opcode: op,
            body: body.to_vec(),
        },

        // A prepared fragment is echoed back; the echo is only an
        // acknowledgement, so the contents are not needed.
        opcode::PREPARE_WRITE_RESPONSE => Response::PrepareWriteComplete,
        opcode::EXECUTE_WRITE_RESPONSE => Response::WriteComplete,

        // Each entry is `length` bytes: handle, end-group handle, then the UUID.
        opcode::READ_BY_GROUP_TYPE_RESPONSE => {
            let length = *body.first()? as usize;
            if length < 6 {
                return None;
            }
            let mut services = Vec::new();
            for entry in body[1..].chunks(length) {
                if entry.len() < length {
                    break;
                }
                let Some(uuid) = Uuid::parse(&entry[4..length]) else {
                    break;
                };
                services.push(Service {
                    start_handle: u16::from_le_bytes([entry[0], entry[1]]),
                    end_handle: u16::from_le_bytes([entry[2], entry[3]]),
                    uuid,
                });
            }
            Response::Services(services)
        }

        // Each entry: declaration handle, then the declaration value —
        // properties byte, value handle, UUID.
        // `Read By Type Response` does not say which type it answered, and the
        // two declarations this client asks for arrive on the same opcode. GATT
        // gives them non-overlapping entry lengths, which is the only
        // discriminator there is:
        //
        //   characteristic: handle + properties + value handle + UUID = 7 or 21
        //   include:        handle + start + end [+ 16-bit UUID]      = 6 or 8
        //
        // A 128-bit included service has no UUID in its declaration, which is
        // why the 6-byte form exists; the caller reads it from the service
        // itself.
        opcode::READ_BY_TYPE_RESPONSE if matches!(*body.first()?, 6 | 8) => {
            let length = *body.first()? as usize;
            let mut includes = Vec::new();
            for entry in body[1..].chunks(length) {
                if entry.len() < length {
                    break;
                }
                includes.push(Include {
                    handle: u16::from_le_bytes([entry[0], entry[1]]),
                    start_handle: u16::from_le_bytes([entry[2], entry[3]]),
                    end_handle: u16::from_le_bytes([entry[4], entry[5]]),
                    uuid: if length == 8 {
                        Uuid::parse(&entry[6..8])
                    } else {
                        None
                    },
                });
            }
            Response::Includes(includes)
        }

        opcode::READ_BY_TYPE_RESPONSE => {
            let length = *body.first()? as usize;
            if length < 7 {
                return None;
            }
            let mut characteristics = Vec::new();
            for entry in body[1..].chunks(length) {
                if entry.len() < length {
                    break;
                }
                let Some(uuid) = Uuid::parse(&entry[5..length]) else {
                    break;
                };
                characteristics.push(Characteristic {
                    handle: u16::from_le_bytes([entry[0], entry[1]]),
                    properties: entry[2],
                    value_handle: u16::from_le_bytes([entry[3], entry[4]]),
                    uuid,
                });
            }
            Response::Characteristics(characteristics)
        }

        // Format 1 is 16-bit UUIDs, format 2 is 128-bit.
        opcode::FIND_INFORMATION_RESPONSE => {
            let format = *body.first()?;
            let uuid_len = match format {
                0x01 => 2,
                0x02 => 16,
                _ => return None,
            };
            let entry_len = 2 + uuid_len;
            let mut descriptors = Vec::new();
            for entry in body[1..].chunks(entry_len) {
                if entry.len() < entry_len {
                    break;
                }
                let Some(uuid) = Uuid::parse(&entry[2..entry_len]) else {
                    break;
                };
                descriptors.push(Descriptor {
                    handle: u16::from_le_bytes([entry[0], entry[1]]),
                    uuid,
                });
            }
            Response::Descriptors(descriptors)
        }

        opcode::READ_RESPONSE | opcode::READ_BLOB_RESPONSE => Response::Value(body.to_vec()),
        opcode::WRITE_RESPONSE => Response::WriteComplete,

        opcode::HANDLE_VALUE_NOTIFICATION => {
            if body.len() < 2 {
                return None;
            }
            Response::Notification {
                handle: u16::from_le_bytes([body[0], body[1]]),
                value: body[2..].to_vec(),
            }
        }

        opcode::HANDLE_VALUE_INDICATION => {
            if body.len() < 2 {
                return None;
            }
            Response::Indication {
                handle: u16::from_le_bytes([body[0], body[1]]),
                value: body[2..].to_vec(),
            }
        }

        other => Response::Unknown(other),
    })
}

/// Whether a PDU arrived unprompted, and so must not be matched to a request.
pub fn is_unsolicited(opcode: u8) -> bool {
    matches!(
        opcode,
        opcode::HANDLE_VALUE_NOTIFICATION | opcode::HANDLE_VALUE_INDICATION
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_uuids_expand_to_the_bluetooth_base() {
        assert_eq!(
            Uuid::Short(0x180D).to_canonical(),
            "0000180d-0000-1000-8000-00805f9b34fb"
        );
        assert_eq!(
            Uuid::Short(0x2A37).to_canonical(),
            "00002a37-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn uuids_go_on_the_wire_least_significant_first() {
        assert_eq!(Uuid::Short(0x180D).to_wire(), vec![0x0D, 0x18]);
        assert_eq!(Uuid::parse(&[0x0D, 0x18]), Some(Uuid::Short(0x180D)));

        // A 128-bit UUID is the written order reversed.
        let nordic_uart = [
            0x6E, 0x40, 0x00, 0x01, 0xB5, 0xA3, 0xF3, 0x93, 0xE0, 0xA9, 0xE5, 0x0E, 0x24, 0xDC,
            0xCA, 0x9E,
        ];
        let long = Uuid::Long(nordic_uart);
        assert_eq!(long.to_canonical(), "6e400001-b5a3-f393-e0a9-e50e24dcca9e");
        let wire = long.to_wire();
        assert_eq!(wire[0], 0x9E, "wire order must be reversed");
        assert_eq!(Uuid::parse(&wire), Some(long));
    }

    #[test]
    fn a_bad_uuid_length_is_rejected() {
        assert_eq!(Uuid::parse(&[]), None);
        assert_eq!(Uuid::parse(&[1, 2, 3]), None);
    }

    #[test]
    fn requests_encode_little_endian() {
        assert_eq!(read_request(0x0025), vec![0x0A, 0x25, 0x00]);
        assert_eq!(write_command(0x0012, &[0xAB]), vec![0x52, 0x12, 0x00, 0xAB]);
    }

    /// `Read By Type Response` does not name the type it answered, and both
    /// declarations this client asks for share the opcode. The entry length is
    /// the only thing that distinguishes them, and GATT is careful to make the
    /// two sets disjoint — 6 or 8 for an include, 7 or 21 for a characteristic.
    #[test]
    fn an_include_declaration_is_not_read_as_a_characteristic() {
        // 8-byte entries: declaration handle, start, end, 16-bit UUID.
        let pdu = [
            0x09, 0x08, // response, entry length 8
            0x02, 0x00, // declaration at 0x0002
            0x10, 0x00, // included service starts at 0x0010
            0x1F, 0x00, // …and ends at 0x001F
            0x0F, 0x18, // UUID 0x180F
        ];
        assert_eq!(
            parse(&pdu),
            Some(Response::Includes(vec![Include {
                handle: 0x0002,
                start_handle: 0x0010,
                end_handle: 0x001F,
                uuid: Some(Uuid::Short(0x180F)),
            }]))
        );
    }

    /// A 128-bit included service does not fit its declaration, so the UUID is
    /// absent and the caller has to read it from the service itself. Returning
    /// a wrong UUID here instead of `None` would name the wrong service.
    #[test]
    fn a_six_byte_include_has_no_uuid_to_give() {
        let pdu = [
            0x09, 0x06, // response, entry length 6
            0x02, 0x00, 0x10, 0x00, 0x1F, 0x00,
        ];
        let Some(Response::Includes(includes)) = parse(&pdu) else {
            panic!("expected includes");
        };
        assert_eq!(includes.len(), 1);
        assert_eq!(includes[0].start_handle, 0x0010);
        assert!(
            includes[0].uuid.is_none(),
            "a 6-byte declaration carries no UUID"
        );
    }

    #[test]
    fn a_characteristic_declaration_still_parses_as_one() {
        // 7-byte entries keep their old meaning.
        let pdu = [
            0x09, 0x07, // response, entry length 7
            0x03, 0x00, // declaration at 0x0003
            0x12, // read | write
            0x04, 0x00, // value handle 0x0004
            0x19, 0x2A, // UUID 0x2A19
        ];
        let Some(Response::Characteristics(found)) = parse(&pdu) else {
            panic!("expected characteristics");
        };
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].value_handle, 0x0004);
        assert_eq!(found[0].uuid, Uuid::Short(0x2A19));
    }

    #[test]
    fn several_includes_come_back_in_one_response() {
        let pdu = [
            0x09, 0x08, //
            0x02, 0x00, 0x10, 0x00, 0x1F, 0x00, 0x0F, 0x18, //
            0x03, 0x00, 0x20, 0x00, 0x2F, 0x00, 0x0D, 0x18,
        ];
        let Some(Response::Includes(includes)) = parse(&pdu) else {
            panic!("expected includes");
        };
        assert_eq!(includes.len(), 2);
        assert_eq!(includes[1].uuid, Some(Uuid::Short(0x180D)));
        assert_eq!(includes[1].end_handle, 0x002F);
    }

    #[test]
    fn a_truncated_include_entry_is_dropped_not_guessed() {
        // Entry length claims 8, only 5 bytes follow.
        let pdu = [0x09, 0x08, 0x02, 0x00, 0x10, 0x00, 0x1F];
        assert_eq!(parse(&pdu), Some(Response::Includes(vec![])));
    }

    #[test]
    fn long_writes_encode_offset_and_commit() {
        assert_eq!(
            prepare_write_request(0x0012, 0x0014, &[0xAA, 0xBB]),
            vec![0x16, 0x12, 0x00, 0x14, 0x00, 0xAA, 0xBB]
        );
        assert_eq!(execute_write_request(true), vec![0x18, 0x01]);
        // Abandoning a queue is the same request with the flag cleared, which
        // is how a half-sent value is prevented from being committed.
        assert_eq!(execute_write_request(false), vec![0x18, 0x00]);
    }

    #[test]
    fn a_prepared_write_echo_is_only_an_acknowledgement() {
        // The peer echoes handle, offset and value; none of it is needed.
        let echoed = [0x17, 0x12, 0x00, 0x14, 0x00, 0xAA, 0xBB];
        assert_eq!(parse(&echoed), Some(Response::PrepareWriteComplete));
        assert_eq!(parse(&[0x19]), Some(Response::WriteComplete));
    }

    #[test]
    fn the_peers_own_requests_are_not_replies() {
        // BlueZ opens with this, and treating it as the answer to our own
        // Exchange MTU Request is how a client ends up with the wrong MTU and
        // a peer waiting forever for a response.
        let request = [0x02, 0x17, 0x02];
        assert_eq!(
            parse(&request),
            Some(Response::PeerRequest {
                opcode: 0x02,
                body: vec![0x17, 0x02],
            })
        );
        // The response form still parses as ours.
        assert_eq!(parse(&[0x03, 0x17, 0x02]), Some(Response::Mtu(0x0217)));

        for op in [0x04u8, 0x08, 0x0A, 0x0C, 0x10, 0x12, 0x16, 0x18] {
            assert!(is_peer_request(op), "{op:#04x} should be a peer request");
        }
        // Responses, commands and notifications are not requests of us.
        for op in [0x01u8, 0x03, 0x05, 0x0B, 0x11, 0x13, 0x1B, 0x1D, 0x52] {
            assert!(!is_peer_request(op), "{op:#04x} is not a peer request");
        }
    }

    #[test]
    fn declining_a_request_names_it() {
        let pdu = error_response(0x0A, 0x0021, AttError(AttError::REQUEST_NOT_SUPPORTED));
        assert_eq!(pdu, vec![0x01, 0x0A, 0x21, 0x00, 0x06]);
        assert_eq!(exchange_mtu_response(517), vec![0x03, 0x05, 0x02]);
        assert_eq!(
            read_by_group_type_request(0x0001, 0xFFFF, gatt_type::PRIMARY_SERVICE),
            vec![0x10, 0x01, 0x00, 0xFF, 0xFF, 0x00, 0x28]
        );
        assert_eq!(exchange_mtu_request(517), vec![0x02, 0x05, 0x02]);
    }

    #[test]
    fn an_error_response_decodes() {
        // The peer refused a read of handle 0x0025: read not permitted.
        let pdu = [0x01, 0x0A, 0x25, 0x00, 0x02];
        assert_eq!(
            parse(&pdu),
            Some(Response::Error {
                opcode: opcode::READ_REQUEST,
                handle: 0x0025,
                error: AttError(AttError::READ_NOT_PERMITTED),
            })
        );
    }

    #[test]
    fn attribute_not_found_ends_discovery_rather_than_failing_it() {
        // Every discovery loop runs until the peer says this.
        assert!(AttError(AttError::ATTRIBUTE_NOT_FOUND).is_end_of_discovery());
        assert!(!AttError(AttError::READ_NOT_PERMITTED).is_end_of_discovery());
    }

    #[test]
    fn a_service_discovery_response_decodes() {
        // Two services, 16-bit UUIDs: entry length 6.
        let pdu = [
            0x11, 0x06, //
            0x01, 0x00, 0x09, 0x00, 0x00, 0x18, // 0x0001..0x0009 Generic Access
            0x0A, 0x00, 0x0F, 0x00, 0x0D, 0x18, // 0x000A..0x000F Heart Rate
        ];
        let Some(Response::Services(services)) = parse(&pdu) else {
            panic!("not a service response");
        };
        assert_eq!(services.len(), 2);
        assert_eq!(services[1].start_handle, 0x000A);
        assert_eq!(services[1].end_handle, 0x000F);
        assert_eq!(
            services[1].uuid.to_canonical(),
            "0000180d-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn a_characteristic_discovery_response_decodes() {
        // Entry length 7: handle, properties, value handle, 16-bit UUID.
        let pdu = [
            0x09, 0x07, //
            0x0B, 0x00, 0x10, 0x0C, 0x00, 0x37, 0x2A, // notify, value at 0x000C
        ];
        let Some(Response::Characteristics(chars)) = parse(&pdu) else {
            panic!("not a characteristic response");
        };
        assert_eq!(chars.len(), 1);
        assert_eq!(chars[0].handle, 0x000B);
        assert_eq!(
            chars[0].value_handle, 0x000C,
            "reads address the value handle"
        );
        assert_eq!(chars[0].properties, 0x10);
        assert_eq!(
            chars[0].uuid.to_canonical(),
            "00002a37-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn descriptor_discovery_decodes_both_formats() {
        // Format 1: 16-bit UUIDs.
        let short = [0x05, 0x01, 0x0D, 0x00, 0x02, 0x29];
        let Some(Response::Descriptors(d)) = parse(&short) else {
            panic!()
        };
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].handle, 0x000D);
        assert_eq!(d[0].uuid, Uuid::Short(0x2902));

        // Format 2: 128-bit.
        let mut long = vec![0x05, 0x02, 0x0E, 0x00];
        long.extend_from_slice(&Uuid::Short(0x2902).to_wire());
        long.extend_from_slice(&[0; 14]);
        let Some(Response::Descriptors(d)) = parse(&long) else {
            panic!()
        };
        assert_eq!(d[0].handle, 0x000E);

        // An unknown format is rejected rather than misread.
        assert_eq!(parse(&[0x05, 0x03, 0x00, 0x00]), None);
    }

    #[test]
    fn notifications_are_recognised_as_unsolicited() {
        let pdu = [0x1B, 0x0C, 0x00, 0x00, 0x48];
        assert_eq!(
            parse(&pdu),
            Some(Response::Notification {
                handle: 0x000C,
                value: vec![0x00, 0x48]
            })
        );
        assert!(is_unsolicited(opcode::HANDLE_VALUE_NOTIFICATION));
        assert!(is_unsolicited(opcode::HANDLE_VALUE_INDICATION));
        // A read response is a reply and must be matched to its request.
        assert!(!is_unsolicited(opcode::READ_RESPONSE));
    }

    #[test]
    fn truncated_pdus_are_rejected_rather_than_misread() {
        // Every one of these is reachable from a hostile or broken peer.
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0x01, 0x0A]), None);
        assert_eq!(parse(&[0x03]), None);
        assert_eq!(parse(&[0x1B, 0x0C]), None);
        // An entry length smaller than the fixed fields would underflow.
        assert_eq!(parse(&[0x11, 0x02, 0x00, 0x00]), None);
        assert_eq!(parse(&[0x09, 0x03, 0x00, 0x00]), None);
    }

    #[test]
    fn a_trailing_partial_entry_is_dropped_not_decoded() {
        // Three and a half entries: the half must not become a bogus service.
        let pdu = [0x11, 0x06, 0x01, 0x00, 0x09, 0x00, 0x00, 0x18, 0x0A, 0x00];
        let Some(Response::Services(services)) = parse(&pdu) else {
            panic!()
        };
        assert_eq!(services.len(), 1);
    }
}
