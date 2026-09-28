//! Bluetooth HID report-descriptor parsing.

use std::fmt;

/// HIDP transaction type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionType {
    /// Input report data.
    Input,
    /// Output report data.
    Output,
    /// Feature report data.
    Feature,
    /// Get-report request.
    GetReport,
    /// Set-report request.
    SetReport,
    /// Handshake/status response.
    Handshake,
    /// Control request.
    Control,
}

/// A HIDP packet without transport framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidPacket {
    /// Transaction kind.
    pub transaction: TransactionType,
    /// Transaction parameter nibble.
    pub parameter: u8,
    /// Packet payload excluding the HIDP header byte.
    pub payload: Vec<u8>,
}

impl HidPacket {
    /// Parse one HIDP packet.
    pub fn parse(bytes: &[u8]) -> Result<Self, HidError> {
        let header = *bytes.first().ok_or(HidError::Truncated)?;
        let transaction = match header >> 4 {
            0x0a => TransactionType::Input,
            0x0b => TransactionType::Output,
            0x0d => TransactionType::Feature,
            0x04 => TransactionType::GetReport,
            0x05 => TransactionType::SetReport,
            0x00 => TransactionType::Handshake,
            0x01 => TransactionType::Control,
            _ => return Err(HidError::InvalidPacket),
        };
        Ok(Self {
            transaction,
            parameter: header & 0x0f,
            payload: bytes[1..].to_vec(),
        })
    }

    /// Encode the HIDP header and payload.
    pub fn encode(&self) -> Result<Vec<u8>, HidError> {
        let kind = match self.transaction {
            TransactionType::Input => 0x0a,
            TransactionType::Output => 0x0b,
            TransactionType::Feature => 0x0d,
            TransactionType::GetReport => 0x04,
            TransactionType::SetReport => 0x05,
            TransactionType::Handshake => 0x00,
            TransactionType::Control => 0x01,
        };
        if self.parameter > 0x0f {
            return Err(HidError::InvalidPacket);
        }
        let mut encoded = vec![(kind << 4) | self.parameter];
        encoded.extend_from_slice(&self.payload);
        Ok(encoded)
    }
}

/// A parsed HID report field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportField {
    /// Report ID, or zero when the descriptor has no Report ID item.
    pub report_id: u8,
    /// Usage page assigned to the field.
    pub usage_page: u16,
    /// Usage value, when the descriptor specifies one.
    pub usage: Option<u16>,
    /// Number of fields in this item.
    pub count: u32,
    /// Bits per field.
    pub size: u32,
    /// Whether the field is an input field.
    pub input: bool,
    /// Raw HID main-item flags.
    pub flags: u8,
}

/// A parsed HID report descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReportDescriptor {
    fields: Vec<ReportField>,
}

/// One decoded scalar value from an input report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputValue {
    /// Field metadata that produced this value.
    pub field: ReportField,
    /// Index within the field's report-count values.
    pub index: u32,
    /// Unsigned bit value, before logical-minimum/sign interpretation.
    pub value: u32,
}

impl ReportDescriptor {
    /// Parse a HID report descriptor.
    pub fn parse(bytes: &[u8]) -> Result<Self, HidError> {
        let mut offset = 0;
        let mut usage_page = 0u16;
        let mut usage = None;
        let mut report_id = 0;
        let mut size = 0;
        let mut count = 0;
        let mut global_stack = Vec::new();
        let mut collection_depth = 0usize;
        let mut fields = Vec::new();
        while offset < bytes.len() {
            let prefix = bytes[offset];
            offset += 1;
            if prefix == 0xfe {
                if offset + 2 > bytes.len() {
                    return Err(HidError::Truncated);
                }
                let length = bytes[offset] as usize;
                offset += 2;
                if offset + length > bytes.len() {
                    return Err(HidError::Truncated);
                }
                offset += length;
                continue;
            }
            let length = match prefix & 3 {
                0 => 0,
                1 => 1,
                2 => 2,
                _ => 4,
            };
            let item_type = (prefix >> 2) & 3;
            let tag = prefix >> 4;
            if offset + length > bytes.len() {
                return Err(HidError::Truncated);
            }
            let value = unsigned(&bytes[offset..offset + length]);
            offset += length;
            match (item_type, tag) {
                (1, 0) => usage_page = value as u16,
                (1, 7) => {
                    if length != 1 || value > 32 {
                        return Err(HidError::InvalidDescriptor);
                    }
                    size = value as u32;
                }
                (1, 9) => {
                    if value == 0 {
                        return Err(HidError::InvalidDescriptor);
                    }
                    count = value as u32;
                }
                (1, 8) => {
                    if length != 1 || value == 0 {
                        return Err(HidError::InvalidDescriptor);
                    }
                    report_id = value as u8;
                }
                (1, 10) => global_stack.push((usage_page, size, count, report_id)),
                (1, 11) => {
                    let Some((saved_usage_page, saved_size, saved_count, saved_report_id)) =
                        global_stack.pop()
                    else {
                        return Err(HidError::InvalidDescriptor);
                    };
                    usage_page = saved_usage_page;
                    size = saved_size;
                    count = saved_count;
                    report_id = saved_report_id;
                }
                (2, 0) => {
                    if length == 4 {
                        usage_page = (value >> 16) as u16;
                    }
                    usage = Some(value as u16);
                }
                (0, 8) | (0, 9) => {
                    if length != 1 {
                        return Err(HidError::InvalidDescriptor);
                    }
                    fields.push(ReportField {
                        report_id,
                        usage_page,
                        usage,
                        count,
                        size,
                        input: item_type == 0 && tag == 8,
                        flags: value as u8,
                    });
                    usage = None;
                }
                (0, 10) => {
                    collection_depth += 1;
                    usage = None;
                }
                (0, 12) => {
                    if collection_depth == 0 {
                        return Err(HidError::InvalidDescriptor);
                    }
                    collection_depth -= 1;
                    usage = None;
                }
                _ => {}
            }
        }
        if !global_stack.is_empty() || collection_depth != 0 {
            return Err(HidError::InvalidDescriptor);
        }
        Ok(Self { fields })
    }

    /// All input and output fields in descriptor order.
    pub fn fields(&self) -> &[ReportField] {
        &self.fields
    }

    /// Input fields only.
    pub fn input_fields(&self) -> impl Iterator<Item = &ReportField> {
        self.fields.iter().filter(|field| field.input)
    }

    /// Decode scalar input values from one HID input report.
    pub fn decode_input(&self, report: &[u8]) -> Result<Vec<InputValue>, HidError> {
        let mut bit_offset = 0usize;
        let report_id = self
            .fields
            .iter()
            .find_map(|field| (field.report_id != 0).then_some(field.report_id));
        let selected_report_id = if report_id.is_some() {
            let selected = report.first().copied().ok_or(HidError::InvalidReport)?;
            if !self.fields.iter().any(|field| field.report_id == selected) {
                return Err(HidError::InvalidReport);
            }
            selected
        } else {
            0
        };
        let data = if selected_report_id != 0 {
            &report[1..]
        } else {
            report
        };
        let mut values = Vec::new();
        for field in &self.fields {
            if field.report_id != selected_report_id {
                continue;
            }
            for index in 0..field.count {
                let bits = field.size as usize;
                if bits == 0 || bits > 32 || bit_offset + bits > data.len() * 8 {
                    return Err(HidError::InvalidReport);
                }
                if !field.input {
                    bit_offset += bits;
                    continue;
                }
                let mut value = 0u32;
                for bit in 0..bits {
                    let source = bit_offset + bit;
                    if data[source / 8] & (1 << (source % 8)) != 0 {
                        value |= 1 << bit;
                    }
                }
                values.push(InputValue {
                    field: *field,
                    index,
                    value,
                });
                bit_offset += bits;
            }
        }
        Ok(values)
    }

    /// Encode scalar values into an HID output report.
    pub fn encode_output(&self, values: &[u32]) -> Result<Vec<u8>, HidError> {
        let fields: Vec<_> = self.fields.iter().filter(|field| !field.input).collect();
        let expected: usize = fields.iter().map(|field| field.count as usize).sum();
        if values.len() != expected {
            return Err(HidError::InvalidReport);
        }
        let report_id = fields.first().map(|field| field.report_id).unwrap_or(0);
        if fields.iter().any(|field| field.report_id != report_id) {
            return Err(HidError::InvalidReport);
        }
        let bits: usize = fields
            .iter()
            .map(|field| field.count as usize * field.size as usize)
            .sum();
        if bits == 0
            || fields
                .iter()
                .any(|field| field.size == 0 || field.size > 32)
        {
            return Err(HidError::InvalidReport);
        }
        let mut output = vec![0u8; bits.div_ceil(8) + usize::from(report_id != 0)];
        if report_id != 0 {
            output[0] = report_id;
        }
        let mut bit_offset = usize::from(report_id != 0) * 8;
        let mut value_index = 0;
        for field in fields {
            for _ in 0..field.count {
                let value = values[value_index];
                value_index += 1;
                if field.size < 32 && value >= (1u32 << field.size) {
                    return Err(HidError::InvalidReport);
                }
                for bit in 0..field.size as usize {
                    if value & (1u32 << bit) != 0 {
                        let target = bit_offset + bit;
                        output[target / 8] |= 1 << (target % 8);
                    }
                }
                bit_offset += field.size as usize;
            }
        }
        Ok(output)
    }
}

/// Why a HID report descriptor could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidError {
    /// The descriptor ended in the middle of an item.
    Truncated,
    /// The HIDP transaction type or parameter is invalid.
    InvalidPacket,
    /// The input report does not match the parsed descriptor.
    InvalidReport,
    /// The descriptor contains an unmatched global-state Pop item.
    InvalidDescriptor,
}

impl fmt::Display for HidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "truncated HID report descriptor or packet",
            Self::InvalidPacket => "invalid HIDP packet",
            Self::InvalidReport => "invalid HID input report",
            Self::InvalidDescriptor => "invalid HID report descriptor",
        })
    }
}

impl std::error::Error for HidError {}

fn unsigned(bytes: &[u8]) -> u64 {
    bytes.iter().enumerate().fold(0, |value, (index, byte)| {
        value | ((*byte as u64) << (index * 8))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_boot_keyboard_fields() {
        let descriptor = ReportDescriptor::parse(&[
            0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00,
            0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01,
            0xc0,
        ])
        .unwrap();
        let field = descriptor.input_fields().next().unwrap();
        assert_eq!(field.usage_page, 0x07);
        assert_eq!(field.count, 8);
        assert_eq!(field.size, 1);
    }

    #[test]
    fn rejects_truncated_items() {
        assert_eq!(
            ReportDescriptor::parse(&[0x05]).unwrap_err(),
            HidError::Truncated
        );
    }

    #[test]
    fn round_trips_hidp_input_reports() {
        let packet = HidPacket {
            transaction: TransactionType::Input,
            parameter: 0,
            payload: vec![0x01, 0x02],
        };
        assert_eq!(HidPacket::parse(&packet.encode().unwrap()).unwrap(), packet);
        assert!(HidPacket::parse(&[0xff]).is_err());
    }

    #[test]
    fn decodes_bit_packed_input_fields() {
        let descriptor =
            ReportDescriptor::parse(&[0x05, 0x01, 0x75, 0x01, 0x95, 0x02, 0x81, 0x02]).unwrap();
        let values = descriptor.decode_input(&[0b0000_0001]).unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].value, 1);
        assert_eq!(values[1].value, 0);
    }

    #[test]
    fn encodes_output_report_values() {
        let descriptor = ReportDescriptor::parse(&[0x75, 0x01, 0x95, 0x02, 0x91, 0x02]).unwrap();
        assert_eq!(descriptor.encode_output(&[1, 0]).unwrap(), vec![1]);
        assert!(descriptor.encode_output(&[2, 0]).is_err());
    }

    #[test]
    fn decodes_input_after_non_input_fields_at_the_correct_offset() {
        let descriptor = ReportDescriptor::parse(&[
            0x75, 0x01, // Report Size: 1
            0x95, 0x01, // Report Count: 1
            0x91, 0x00, // Output: 1 bit
            0x81, 0x00, // Input: 1 bit
        ])
        .unwrap();
        let values = descriptor.decode_input(&[0b0000_0010]).unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].value, 1);
    }

    #[test]
    fn decodes_the_selected_report_id_only() {
        let descriptor = ReportDescriptor::parse(&[
            0x85, 0x01, // Report ID 1
            0x75, 0x08, // Report Size: 8
            0x95, 0x01, // Report Count: 1
            0x81, 0x00, // Input
            0x85, 0x02, // Report ID 2
            0x81, 0x00, // Input
        ])
        .unwrap();
        let values = descriptor.decode_input(&[2, 0xab]).unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].field.report_id, 2);
        assert_eq!(values[0].value, 0xab);
        assert!(descriptor.decode_input(&[3, 0]).is_err());
    }

    #[test]
    fn encodes_output_with_the_output_field_report_id() {
        let descriptor = ReportDescriptor::parse(&[
            0x85, 0x01, // Report ID 1
            0x75, 0x08, // Report Size: 8
            0x95, 0x01, // Report Count: 1
            0x81, 0x02, // Input
            0x85, 0x02, // Report ID 2
            0x91, 0x02, // Output
        ])
        .unwrap();
        assert_eq!(descriptor.encode_output(&[0xab]).unwrap(), vec![2, 0xab]);
    }

    #[test]
    fn rejects_output_values_spanning_multiple_report_ids() {
        let descriptor = ReportDescriptor::parse(&[
            0x75, 0x08, 0x95, 0x01, 0x85, 0x01, 0x91, 0x02, 0x85, 0x02, 0x91, 0x02,
        ])
        .unwrap();
        assert!(descriptor.encode_output(&[1, 2]).is_err());
    }

    #[test]
    fn clears_usage_after_each_main_item() {
        let descriptor = ReportDescriptor::parse(&[
            0x05, 0x01, // Usage Page: Generic Desktop
            0x09, 0x30, // Usage: X
            0x81, 0x02, // Input
            0x81, 0x02, // Input without a new Usage
        ])
        .unwrap();
        assert_eq!(descriptor.fields()[0].usage, Some(0x30));
        assert_eq!(descriptor.fields()[1].usage, None);
    }

    #[test]
    fn restores_global_state_after_push_and_pop() {
        let descriptor = ReportDescriptor::parse(&[
            0x05, 0x01, // Usage Page: Generic Desktop
            0x75, 0x08, // Report Size: 8
            0x95, 0x01, // Report Count: 1
            0xa4, // Push global state
            0x05, 0x02, // Nested Usage Page
            0x75, 0x01, // Nested Report Size: 1
            0x81, 0x02, // Nested Input
            0xb4, // Pop global state
            0x81, 0x02, // Input using restored state
        ])
        .unwrap();
        assert_eq!(descriptor.fields()[0].usage_page, 2);
        assert_eq!(descriptor.fields()[0].size, 1);
        assert_eq!(descriptor.fields()[1].usage_page, 1);
        assert_eq!(descriptor.fields()[1].size, 8);
    }

    #[test]
    fn rejects_unmatched_global_pop() {
        assert_eq!(
            ReportDescriptor::parse(&[0xb4]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn rejects_unterminated_global_push() {
        assert_eq!(
            ReportDescriptor::parse(&[0xa4]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn rejects_unbalanced_collections() {
        assert_eq!(
            ReportDescriptor::parse(&[0xa1, 0x01]),
            Err(HidError::InvalidDescriptor)
        );
        assert_eq!(
            ReportDescriptor::parse(&[0xc0]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn rejects_invalid_report_id_items() {
        assert_eq!(
            ReportDescriptor::parse(&[0x85, 0x00]),
            Err(HidError::InvalidDescriptor)
        );
        assert_eq!(
            ReportDescriptor::parse(&[0x86, 0x01, 0x00]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn rejects_zero_length_main_items() {
        assert_eq!(
            ReportDescriptor::parse(&[0x80]),
            Err(HidError::InvalidDescriptor)
        );
        assert_eq!(
            ReportDescriptor::parse(&[0x90]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn rejects_non_byte_report_size() {
        assert_eq!(
            ReportDescriptor::parse(&[0x76, 0x08, 0x00]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn rejects_multi_byte_main_items() {
        assert_eq!(
            ReportDescriptor::parse(&[0x82, 0x02, 0x00]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn rejects_report_size_above_32() {
        assert_eq!(
            ReportDescriptor::parse(&[0x75, 0x21, 0x81, 0x02]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn rejects_zero_report_count() {
        assert_eq!(
            ReportDescriptor::parse(&[0x75, 0x08, 0x95, 0x00, 0x81, 0x02]),
            Err(HidError::InvalidDescriptor)
        );
    }

    #[test]
    fn clears_usage_across_collection_boundaries() {
        let descriptor = ReportDescriptor::parse(&[
            0x05, 0x01, // Usage Page: Generic Desktop
            0x09, 0x04, // Usage: Joystick
            0xa1, 0x01, // Collection: Application
            0x75, 0x08, // Report Size: 8
            0x95, 0x01, // Report Count: 1
            0x81, 0x02, // Input without a new Usage
            0xc0, // End Collection
        ])
        .unwrap();
        assert_eq!(descriptor.fields()[0].usage, None);
    }

    #[test]
    fn parses_extended_usage_page_from_four_byte_usage() {
        let descriptor = ReportDescriptor::parse(&[
            0x05, 0x01, // Usage Page: Generic Desktop
            0x0b, 0x34, 0x12, 0x07, 0x01, // Usage 0x1234, page 0x0107
            0x75, 0x08, 0x95, 0x01, 0x81, 0x02,
        ])
        .unwrap();
        assert_eq!(descriptor.fields()[0].usage_page, 0x0107);
        assert_eq!(descriptor.fields()[0].usage, Some(0x1234));
    }
}
