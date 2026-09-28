//! A2DP codec capability and negotiation modeling.

use std::fmt;

/// A2DP codec families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    /// Subband Codec, mandatory for A2DP devices.
    Sbc,
    /// MPEG-2/4 AAC.
    Aac,
    /// Qualcomm aptX family.
    AptX,
    /// Sony LDAC.
    Ldac,
}

/// SBC channel mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbcChannelMode {
    /// One audio channel.
    Mono,
    /// Independent left/right channels.
    DualChannel,
    /// Stereo channels with separate bit allocation.
    Stereo,
    /// Joint stereo with shared bit allocation.
    JointStereo,
}

/// SBC allocation method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbcAllocation {
    /// Allocate bits using loudness.
    Loudness,
    /// Allocate bits using signal-to-noise ratio.
    SNR,
}

/// Validated SBC codec configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SbcConfiguration {
    /// Sampling frequency in Hz.
    pub sample_rate_hz: u32,
    /// Channel mode.
    pub channel_mode: SbcChannelMode,
    /// Number of blocks per SBC frame.
    pub blocks: u8,
    /// Number of subbands.
    pub subbands: u8,
    /// Allocation method.
    pub allocation: SbcAllocation,
    /// Minimum bitpool value.
    pub min_bitpool: u8,
    /// Maximum bitpool value.
    pub max_bitpool: u8,
}

impl SbcConfiguration {
    /// Decode the four-byte SBC media codec configuration.
    pub fn parse(bytes: &[u8]) -> Result<Self, NegotiationError> {
        if bytes.len() != 4 {
            return Err(NegotiationError::InvalidSbcConfiguration);
        }
        let sample_rate_hz = match bytes[0] >> 6 {
            0 => 16000,
            1 => 32000,
            2 => 44100,
            3 => 48000,
            _ => unreachable!(),
        };
        let channel_mode = match (bytes[0] >> 4) & 3 {
            0 => SbcChannelMode::Mono,
            1 => SbcChannelMode::DualChannel,
            2 => SbcChannelMode::Stereo,
            _ => SbcChannelMode::JointStereo,
        };
        let blocks = match bytes[1] >> 4 {
            0 => 4,
            1 => 8,
            2 => 12,
            _ => 16,
        };
        let subbands = if bytes[1] & 1 != 0 { 8 } else { 4 };
        let allocation = if bytes[1] & 2 != 0 {
            SbcAllocation::SNR
        } else {
            SbcAllocation::Loudness
        };
        if bytes[2] > bytes[3] || bytes[2] == 0 {
            return Err(NegotiationError::InvalidSbcConfiguration);
        }
        Ok(Self {
            sample_rate_hz,
            channel_mode,
            blocks,
            subbands,
            allocation,
            min_bitpool: bytes[2],
            max_bitpool: bytes[3],
        })
    }

    /// Encode this configuration to the four-byte SBC media codec form.
    pub fn encode(self) -> Result<[u8; 4], NegotiationError> {
        let rate = match self.sample_rate_hz {
            16000 => 0,
            32000 => 1,
            44100 => 2,
            48000 => 3,
            _ => return Err(NegotiationError::InvalidSbcConfiguration),
        };
        let mode = match self.channel_mode {
            SbcChannelMode::Mono => 0,
            SbcChannelMode::DualChannel => 1,
            SbcChannelMode::Stereo => 2,
            SbcChannelMode::JointStereo => 3,
        };
        let blocks = match self.blocks {
            4 => 0,
            8 => 1,
            12 => 2,
            16 => 3,
            _ => return Err(NegotiationError::InvalidSbcConfiguration),
        };
        if !matches!(self.subbands, 4 | 8)
            || self.min_bitpool == 0
            || self.min_bitpool > self.max_bitpool
        {
            return Err(NegotiationError::InvalidSbcConfiguration);
        }
        Ok([
            (rate << 6) | (mode << 4),
            (blocks << 4)
                | (u8::from(matches!(self.allocation, SbcAllocation::SNR)) << 1)
                | u8::from(self.subbands == 8),
            self.min_bitpool,
            self.max_bitpool,
        ])
    }
}

/// A structurally valid, complete SBC frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbcFrame(Vec<u8>);

/// An RTP/SBC media packet containing complete SBC frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbcMediaPacket {
    /// RTP payload type.
    pub payload_type: u8,
    /// RTP sequence number.
    pub sequence: u16,
    /// RTP timestamp.
    pub timestamp: u32,
    /// RTP marker bit.
    pub marker: bool,
    /// Complete SBC frames in this packet.
    pub frames: Vec<SbcFrame>,
}

/// Why an SBC media frame or packet could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbcMediaError {
    /// The media bytes ended before a required header or frame.
    Truncated,
    /// The SBC sync word or RTP header is invalid.
    InvalidHeader,
    /// The frame header does not match the negotiated configuration.
    ConfigurationMismatch,
    /// The frame length or packet frame count is invalid.
    InvalidLength,
    /// Fragmented SBC payloads are not supported by this complete-frame API.
    Fragmented,
}

/// AVDTP signaling packetization type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdtpPacketType {
    /// One complete signaling message.
    Single,
    /// First packet of a fragmented message.
    Start,
    /// Middle packet of a fragmented message.
    Continue,
    /// Final packet of a fragmented message.
    End,
}

/// AVDTP signaling message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdtpMessageType {
    /// A command from the initiator.
    Command,
    /// A general reject response.
    GeneralReject,
    /// An accepted response.
    ResponseAccept,
    /// A rejected response.
    ResponseReject,
}

/// A decoded AVDTP response result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvdtpResponseResult {
    /// An accepted response with signal-specific parameters.
    Accepted {
        /// Signal-specific response parameters.
        parameters: Vec<u8>,
    },
    /// A signal-specific rejection.
    Rejected {
        /// Protocol error code.
        error_code: u8,
        /// Signal-specific error information.
        error_info: Vec<u8>,
    },
    /// A general reject response.
    GeneralReject {
        /// Protocol error code.
        error_code: u8,
    },
}

/// A typed AVDTP response associated with a signal transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvdtpResponse {
    /// Four-bit transaction label.
    pub transaction: u8,
    /// Signal that produced the response.
    pub signal: AvdtpSignalIdentifier,
    /// Response result and payload.
    pub result: AvdtpResponseResult,
}

/// AVDTP media type advertised by a stream endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdtpMediaType {
    /// Audio media.
    Audio,
    /// Video media.
    Video,
    /// Multimedia media.
    Multimedia,
}

/// AVDTP stream endpoint direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdtpEndpointType {
    /// Receives a media stream.
    Sink,
    /// Sends a media stream.
    Source,
}

/// One endpoint descriptor from an AVDTP Discover response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvdtpStreamEndpointDescriptor {
    /// Stream endpoint identifier.
    pub seid: AvdtpStreamEndpointId,
    /// Whether the endpoint is already in use.
    pub in_use: bool,
    /// Endpoint media type.
    pub media_type: AvdtpMediaType,
    /// Endpoint direction.
    pub endpoint_type: AvdtpEndpointType,
}

/// Why an AVDTP Discover response could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdtpDiscoverError {
    /// The response length is not a whole number of descriptors.
    InvalidLength,
    /// More descriptors were supplied than the protocol can identify.
    TooManyEndpoints,
    /// An endpoint identifier is invalid or reserved.
    InvalidStreamEndpointId,
    /// Reserved descriptor bits were set.
    ReservedBitsSet,
    /// The media type is not supported by this model.
    UnsupportedMediaType,
    /// The response contains the same endpoint more than once.
    DuplicateStreamEndpointId,
    /// The response is for a different signal.
    WrongSignal,
    /// The response was not accepted.
    NotAccepted,
}

/// A typed AVDTP service category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvdtpServiceCategory {
    /// Media transport category.
    MediaTransport,
    /// SBC media codec capability.
    Sbc(AvdtpSbcCapability),
    /// An unknown or currently unmodeled category.
    Unknown {
        /// Unrecognized service-category identifier.
        category: u8,
        /// Raw category payload.
        data: Vec<u8>,
    },
}

/// SBC capability bitfields from an AVDTP Media Codec category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvdtpSbcCapability {
    /// Supported sampling-frequency bitmask.
    pub sampling_frequency_bits: u8,
    /// Supported channel-mode bitmask.
    pub channel_mode_bits: u8,
    /// Supported block-length bitmask.
    pub block_length_bits: u8,
    /// Supported allocation-method bitmask.
    pub allocation_method_bits: u8,
    /// Supported subband bitmask.
    pub subbands_bits: u8,
    /// Minimum supported bitpool.
    pub min_bitpool: u8,
    /// Maximum supported bitpool.
    pub max_bitpool: u8,
}

/// Why an AVDTP capability response could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdtpCapabilitiesError {
    /// The response belongs to another signal.
    WrongSignal,
    /// The response was rejected.
    NotAccepted,
    /// No service categories were supplied.
    EmptyResponse,
    /// Too many service categories were supplied.
    TooManyCategories,
    /// A category record has an invalid length.
    InvalidCategoryLength,
    /// A known category has invalid data.
    InvalidCategoryData,
}

impl AvdtpStreamEndpointDescriptor {
    /// Parse endpoint descriptors from a Discover response payload.
    pub fn parse_discover_response(parameters: &[u8]) -> Result<Vec<Self>, AvdtpDiscoverError> {
        if !parameters.len().is_multiple_of(2) {
            return Err(AvdtpDiscoverError::InvalidLength);
        }
        if parameters.len() / 2 > 63 {
            return Err(AvdtpDiscoverError::TooManyEndpoints);
        }
        let mut seen = 0u64;
        let mut endpoints = Vec::with_capacity(parameters.len() / 2);
        for descriptor in parameters.chunks_exact(2) {
            if descriptor[0] & 3 != 0 || descriptor[1] & 7 != 0 {
                return Err(AvdtpDiscoverError::ReservedBitsSet);
            }
            let seid_value = (descriptor[0] >> 2) & 0x1f;
            let seid = AvdtpStreamEndpointId::new(seid_value)
                .ok_or(AvdtpDiscoverError::InvalidStreamEndpointId)?;
            let bit = 1u64 << seid.get();
            if seen & bit != 0 {
                return Err(AvdtpDiscoverError::DuplicateStreamEndpointId);
            }
            seen |= bit;
            let media_type = match descriptor[1] >> 4 {
                0 => AvdtpMediaType::Audio,
                1 => AvdtpMediaType::Video,
                2 => AvdtpMediaType::Multimedia,
                _ => return Err(AvdtpDiscoverError::UnsupportedMediaType),
            };
            let endpoint_type = if descriptor[1] & 8 == 0 {
                AvdtpEndpointType::Sink
            } else {
                AvdtpEndpointType::Source
            };
            endpoints.push(Self {
                seid,
                in_use: descriptor[0] & 0x80 != 0,
                media_type,
                endpoint_type,
            });
        }
        Ok(endpoints)
    }
}

/// AVDTP signaling identifiers defined by the profile specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdtpSignalIdentifier {
    /// Discover stream endpoints.
    Discover,
    /// Read endpoint capabilities.
    GetCapabilities,
    /// Configure an endpoint.
    SetConfiguration,
    /// Read an endpoint configuration.
    GetConfiguration,
    /// Change a configuration.
    Reconfigure,
    /// Read all endpoint capabilities.
    GetAllCapabilities,
    /// Start streaming.
    Start,
    /// Suspend streaming.
    Suspend,
    /// Close a stream.
    Close,
    /// Abort a stream.
    Abort,
    /// Exchange security-control data.
    SecurityControl,
    /// Read endpoint state.
    GetStreamEndpointState,
    /// Read endpoint information.
    GetStreamEndpointInfo,
    /// Report transport delay.
    DelayReport,
    /// An identifier not known by this version of the library.
    Unknown(u8),
}

/// A six-bit AVDTP ACP Stream Endpoint Identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvdtpStreamEndpointId(u8);

impl AvdtpStreamEndpointId {
    /// Construct a valid stream endpoint identifier.
    pub const fn new(value: u8) -> Option<Self> {
        if value == 0 || value > 0x3f {
            None
        } else {
            Some(Self(value))
        }
    }

    /// Return the unshifted endpoint identifier.
    pub const fn get(self) -> u8 {
        self.0
    }

    const fn encode(self) -> u8 {
        self.0 << 2
    }

    const fn decode(value: u8) -> Option<Self> {
        if value & 3 != 0 {
            None
        } else {
            Self::new(value >> 2)
        }
    }
}

impl AvdtpSignalIdentifier {
    /// Return the six-bit wire value.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Discover => 0x01,
            Self::GetCapabilities => 0x02,
            Self::SetConfiguration => 0x03,
            Self::GetConfiguration => 0x04,
            Self::Reconfigure => 0x05,
            Self::GetAllCapabilities => 0x06,
            Self::Start => 0x07,
            Self::Suspend => 0x08,
            Self::Close => 0x09,
            Self::Abort => 0x0a,
            Self::SecurityControl => 0x0b,
            Self::GetStreamEndpointState => 0x0c,
            Self::GetStreamEndpointInfo => 0x0d,
            Self::DelayReport => 0x0e,
            Self::Unknown(value) => value & 0x3f,
        }
    }

    /// Convert a six-bit wire value to a typed identifier.
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0x01 => Self::Discover,
            0x02 => Self::GetCapabilities,
            0x03 => Self::SetConfiguration,
            0x04 => Self::GetConfiguration,
            0x05 => Self::Reconfigure,
            0x06 => Self::GetAllCapabilities,
            0x07 => Self::Start,
            0x08 => Self::Suspend,
            0x09 => Self::Close,
            0x0a => Self::Abort,
            0x0b => Self::SecurityControl,
            0x0c => Self::GetStreamEndpointState,
            0x0d => Self::GetStreamEndpointInfo,
            0x0e => Self::DelayReport,
            value => Self::Unknown(value & 0x3f),
        }
    }
}

/// A complete AVDTP signaling packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvdtpSignalingPacket {
    /// Four-bit transaction label.
    pub transaction: u8,
    /// Packetization type.
    pub packet_type: AvdtpPacketType,
    /// Command or response kind.
    pub message_type: AvdtpMessageType,
    /// Six-bit AVDTP signal identifier.
    pub signal_identifier: u8,
    /// Signal-specific parameters.
    pub parameters: Vec<u8>,
}

/// Why an AVDTP signaling packet could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvdtpError {
    /// The packet ended before its fixed header.
    Truncated,
    /// A transaction label or signal identifier is out of range.
    InvalidIdentifier,
    /// Reserved header bits were set.
    ReservedBitsSet,
    /// Fragmented signaling packets are not supported by this codec.
    Fragmented,
    /// A signal's parameter length or count is invalid.
    InvalidParameterLength,
    /// An ACP stream endpoint identifier is invalid.
    InvalidStreamEndpointId,
}

impl fmt::Display for AvdtpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "truncated AVDTP signaling packet",
            Self::InvalidIdentifier => "invalid AVDTP signaling identifier",
            Self::ReservedBitsSet => "reserved AVDTP header bits are set",
            Self::Fragmented => "fragmented AVDTP signaling packet is unsupported",
            Self::InvalidParameterLength => "invalid AVDTP signal parameter length",
            Self::InvalidStreamEndpointId => "invalid AVDTP stream endpoint identifier",
        })
    }
}

impl std::error::Error for AvdtpError {}

impl AvdtpSignalingPacket {
    /// Build a Discover command.
    pub fn discover(transaction: u8) -> Self {
        Self::command(transaction, AvdtpSignalIdentifier::Discover, Vec::new())
    }

    /// Build a Get Capabilities command for one stream endpoint.
    pub fn get_capabilities(transaction: u8, seid: AvdtpStreamEndpointId) -> Self {
        Self::command(
            transaction,
            AvdtpSignalIdentifier::GetCapabilities,
            vec![seid.encode()],
        )
    }

    /// Build a Get All Capabilities command for one stream endpoint.
    pub fn get_all_capabilities(transaction: u8, seid: AvdtpStreamEndpointId) -> Self {
        Self::command(
            transaction,
            AvdtpSignalIdentifier::GetAllCapabilities,
            vec![seid.encode()],
        )
    }

    /// Build a Get Configuration command for one stream endpoint.
    pub fn get_configuration(transaction: u8, seid: AvdtpStreamEndpointId) -> Self {
        Self::command(
            transaction,
            AvdtpSignalIdentifier::GetConfiguration,
            vec![seid.encode()],
        )
    }

    /// Build a Set Configuration command for an SBC stream.
    pub fn set_sbc_configuration(
        transaction: u8,
        acp_seid: AvdtpStreamEndpointId,
        int_seid: AvdtpStreamEndpointId,
        configuration: SbcConfiguration,
    ) -> Result<Self, AvdtpError> {
        let codec = configuration
            .encode()
            .map_err(|_| AvdtpError::InvalidParameterLength)?;
        let mut parameters = vec![
            acp_seid.encode(),
            int_seid.encode(),
            0x01,
            0x00,
            0x07,
            0x06,
            0x00,
            0x00,
        ];
        parameters.extend_from_slice(&codec);
        Ok(Self::command(
            transaction,
            AvdtpSignalIdentifier::SetConfiguration,
            parameters,
        ))
    }

    /// Build an SBC Reconfigure command for an existing stream endpoint.
    pub fn reconfigure_sbc(
        transaction: u8,
        acp_seid: AvdtpStreamEndpointId,
        configuration: SbcConfiguration,
    ) -> Result<Self, AvdtpError> {
        let codec = configuration
            .encode()
            .map_err(|_| AvdtpError::InvalidParameterLength)?;
        let mut parameters = vec![acp_seid.encode(), 0x07, 0x06, 0x00, 0x00];
        parameters.extend_from_slice(&codec);
        Ok(Self::command(
            transaction,
            AvdtpSignalIdentifier::Reconfigure,
            parameters,
        ))
    }

    /// Build a Suspend command for one stream endpoint.
    pub fn suspend(transaction: u8, seid: AvdtpStreamEndpointId) -> Self {
        Self::command(
            transaction,
            AvdtpSignalIdentifier::Suspend,
            vec![1, seid.encode()],
        )
    }

    /// Build a Close command for one stream endpoint.
    pub fn close(transaction: u8, seid: AvdtpStreamEndpointId) -> Self {
        Self::command(
            transaction,
            AvdtpSignalIdentifier::Close,
            vec![seid.encode()],
        )
    }

    /// Build an Abort command for one stream endpoint.
    pub fn abort(transaction: u8, seid: AvdtpStreamEndpointId) -> Self {
        Self::command(
            transaction,
            AvdtpSignalIdentifier::Abort,
            vec![seid.encode()],
        )
    }

    /// Build a Start command for one or more stream endpoints.
    pub fn start(transaction: u8, seids: &[AvdtpStreamEndpointId]) -> Result<Self, AvdtpError> {
        if seids.is_empty() || seids.len() > u8::MAX as usize {
            return Err(AvdtpError::InvalidParameterLength);
        }
        let mut parameters = Vec::with_capacity(seids.len() + 1);
        parameters.push(seids.len() as u8);
        parameters.extend(seids.iter().map(|seid| seid.encode()));
        Ok(Self::command(
            transaction,
            AvdtpSignalIdentifier::Start,
            parameters,
        ))
    }

    /// Construct a single-packet AVDTP command.
    pub fn command(transaction: u8, signal: AvdtpSignalIdentifier, parameters: Vec<u8>) -> Self {
        Self {
            transaction,
            packet_type: AvdtpPacketType::Single,
            message_type: AvdtpMessageType::Command,
            signal_identifier: signal.as_u8(),
            parameters,
        }
    }

    /// Construct an accepted response.
    pub fn accept(transaction: u8, signal: AvdtpSignalIdentifier, parameters: Vec<u8>) -> Self {
        Self {
            transaction,
            packet_type: AvdtpPacketType::Single,
            message_type: AvdtpMessageType::ResponseAccept,
            signal_identifier: signal.as_u8(),
            parameters,
        }
    }

    /// Construct a signal-specific rejection response.
    pub fn reject(
        transaction: u8,
        signal: AvdtpSignalIdentifier,
        error_code: u8,
        mut error_info: Vec<u8>,
    ) -> Self {
        let mut parameters = vec![error_code];
        parameters.append(&mut error_info);
        Self {
            transaction,
            packet_type: AvdtpPacketType::Single,
            message_type: AvdtpMessageType::ResponseReject,
            signal_identifier: signal.as_u8(),
            parameters,
        }
    }

    /// Construct a general rejection response.
    pub fn general_reject(transaction: u8, signal: AvdtpSignalIdentifier, error_code: u8) -> Self {
        Self {
            transaction,
            packet_type: AvdtpPacketType::Single,
            message_type: AvdtpMessageType::GeneralReject,
            signal_identifier: signal.as_u8(),
            parameters: vec![error_code],
        }
    }

    /// Return the typed signal identifier, preserving unknown values.
    pub const fn signal(&self) -> AvdtpSignalIdentifier {
        AvdtpSignalIdentifier::from_u8(self.signal_identifier)
    }

    /// Encode one complete AVDTP signaling packet.
    pub fn encode(&self) -> Result<Vec<u8>, AvdtpError> {
        if self.transaction > 0x0f || self.signal_identifier > 0x3f {
            return Err(AvdtpError::InvalidIdentifier);
        }
        let packet_type = match self.packet_type {
            AvdtpPacketType::Single => 0,
            AvdtpPacketType::Start => 1,
            AvdtpPacketType::Continue => 2,
            AvdtpPacketType::End => 3,
        };
        if self.packet_type != AvdtpPacketType::Single {
            return Err(AvdtpError::Fragmented);
        }
        self.validate_command_parameters()?;
        self.validate_response_parameters()?;
        let message_type = match self.message_type {
            AvdtpMessageType::Command => 0,
            AvdtpMessageType::GeneralReject => 1,
            AvdtpMessageType::ResponseAccept => 2,
            AvdtpMessageType::ResponseReject => 3,
        };
        let mut bytes = vec![
            (self.transaction << 4) | (packet_type << 2) | message_type,
            self.signal_identifier,
        ];
        bytes.extend_from_slice(&self.parameters);
        Ok(bytes)
    }

    fn validate_command_parameters(&self) -> Result<(), AvdtpError> {
        if self.message_type != AvdtpMessageType::Command {
            return Ok(());
        }
        match AvdtpSignalIdentifier::from_u8(self.signal_identifier) {
            AvdtpSignalIdentifier::Discover if !self.parameters.is_empty() => {
                Err(AvdtpError::InvalidParameterLength)
            }
            AvdtpSignalIdentifier::GetCapabilities => validate_single_seid(&self.parameters),
            AvdtpSignalIdentifier::GetAllCapabilities
            | AvdtpSignalIdentifier::GetConfiguration
            | AvdtpSignalIdentifier::Close
            | AvdtpSignalIdentifier::Abort => validate_single_seid(&self.parameters),
            AvdtpSignalIdentifier::SetConfiguration => {
                if self.parameters.len() != 12 {
                    return Err(AvdtpError::InvalidParameterLength);
                }
                AvdtpStreamEndpointId::decode(self.parameters[0])
                    .ok_or(AvdtpError::InvalidStreamEndpointId)?;
                AvdtpStreamEndpointId::decode(self.parameters[1])
                    .ok_or(AvdtpError::InvalidStreamEndpointId)?;
                if self.parameters[2..6] != [0x01, 0x00, 0x07, 0x06]
                    || self.parameters[6] != 0
                    || self.parameters[7] != 0
                {
                    return Err(AvdtpError::InvalidParameterLength);
                }
                SbcConfiguration::parse(&self.parameters[8..])
                    .map(|_| ())
                    .map_err(|_| AvdtpError::InvalidParameterLength)
            }
            AvdtpSignalIdentifier::Reconfigure => {
                if self.parameters.len() != 9 {
                    return Err(AvdtpError::InvalidParameterLength);
                }
                AvdtpStreamEndpointId::decode(self.parameters[0])
                    .ok_or(AvdtpError::InvalidStreamEndpointId)?;
                if self.parameters[1..5] != [0x07, 0x06, 0x00, 0x00] {
                    return Err(AvdtpError::InvalidParameterLength);
                }
                SbcConfiguration::parse(&self.parameters[5..])
                    .map(|_| ())
                    .map_err(|_| AvdtpError::InvalidParameterLength)
            }
            AvdtpSignalIdentifier::Suspend => {
                if self.parameters.len() != 2 || self.parameters[0] != 1 {
                    return Err(AvdtpError::InvalidParameterLength);
                }
                AvdtpStreamEndpointId::decode(self.parameters[1])
                    .ok_or(AvdtpError::InvalidStreamEndpointId)
                    .map(|_| ())
            }
            AvdtpSignalIdentifier::Start => {
                let Some((&count, values)) = self.parameters.split_first() else {
                    return Err(AvdtpError::InvalidParameterLength);
                };
                if count == 0 || values.len() != count as usize {
                    return Err(AvdtpError::InvalidParameterLength);
                }
                for value in values {
                    AvdtpStreamEndpointId::decode(*value)
                        .ok_or(AvdtpError::InvalidStreamEndpointId)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn validate_response_parameters(&self) -> Result<(), AvdtpError> {
        match self.message_type {
            AvdtpMessageType::GeneralReject if self.parameters.len() != 1 => {
                Err(AvdtpError::InvalidParameterLength)
            }
            AvdtpMessageType::ResponseReject if self.parameters.is_empty() => {
                Err(AvdtpError::InvalidParameterLength)
            }
            AvdtpMessageType::ResponseAccept => {
                match AvdtpSignalIdentifier::from_u8(self.signal_identifier) {
                    AvdtpSignalIdentifier::Discover if !self.parameters.len().is_multiple_of(2) => {
                        Err(AvdtpError::InvalidParameterLength)
                    }
                    AvdtpSignalIdentifier::SetConfiguration
                    | AvdtpSignalIdentifier::Reconfigure
                    | AvdtpSignalIdentifier::Start
                    | AvdtpSignalIdentifier::Suspend
                    | AvdtpSignalIdentifier::Close
                    | AvdtpSignalIdentifier::Abort
                        if !self.parameters.is_empty() =>
                    {
                        Err(AvdtpError::InvalidParameterLength)
                    }
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }

    /// Decode one complete AVDTP signaling packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, AvdtpError> {
        if bytes.len() < 2 {
            return Err(AvdtpError::Truncated);
        }
        let header = bytes[0];
        let packet_type = match (header >> 2) & 3 {
            0 => AvdtpPacketType::Single,
            1 => AvdtpPacketType::Start,
            2 => AvdtpPacketType::Continue,
            _ => AvdtpPacketType::End,
        };
        if packet_type != AvdtpPacketType::Single {
            return Err(AvdtpError::Fragmented);
        }
        let signal_identifier = bytes[1];
        if signal_identifier & 0xc0 != 0 {
            return Err(AvdtpError::ReservedBitsSet);
        }
        let message_type = match header & 3 {
            0 => AvdtpMessageType::Command,
            1 => AvdtpMessageType::GeneralReject,
            2 => AvdtpMessageType::ResponseAccept,
            _ => AvdtpMessageType::ResponseReject,
        };
        let packet = Self {
            transaction: header >> 4,
            packet_type,
            message_type,
            signal_identifier,
            parameters: bytes[2..].to_vec(),
        };
        packet.validate_command_parameters()?;
        packet.validate_response_parameters()?;
        Ok(packet)
    }

    /// Interpret this packet as a typed response.
    pub fn response(&self) -> Result<AvdtpResponse, AvdtpError> {
        let result = match self.message_type {
            AvdtpMessageType::ResponseAccept => AvdtpResponseResult::Accepted {
                parameters: self.parameters.clone(),
            },
            AvdtpMessageType::ResponseReject => {
                let (&error_code, error_info) = self
                    .parameters
                    .split_first()
                    .ok_or(AvdtpError::InvalidParameterLength)?;
                AvdtpResponseResult::Rejected {
                    error_code,
                    error_info: error_info.to_vec(),
                }
            }
            AvdtpMessageType::GeneralReject => {
                if self.parameters.len() != 1 {
                    return Err(AvdtpError::InvalidParameterLength);
                }
                AvdtpResponseResult::GeneralReject {
                    error_code: self.parameters[0],
                }
            }
            AvdtpMessageType::Command => {
                return Err(AvdtpError::InvalidParameterLength);
            }
        };
        Ok(AvdtpResponse {
            transaction: self.transaction,
            signal: self.signal(),
            result,
        })
    }
}

impl AvdtpResponse {
    /// Parse endpoint descriptors from an accepted Discover response.
    pub fn discover_endpoints(
        &self,
    ) -> Result<Vec<AvdtpStreamEndpointDescriptor>, AvdtpDiscoverError> {
        if self.signal != AvdtpSignalIdentifier::Discover {
            return Err(AvdtpDiscoverError::WrongSignal);
        }
        let AvdtpResponseResult::Accepted { parameters } = &self.result else {
            return Err(AvdtpDiscoverError::NotAccepted);
        };
        AvdtpStreamEndpointDescriptor::parse_discover_response(parameters)
    }

    /// Parse service categories from an accepted capabilities response.
    pub fn capabilities(&self) -> Result<Vec<AvdtpServiceCategory>, AvdtpCapabilitiesError> {
        if !matches!(
            self.signal,
            AvdtpSignalIdentifier::GetCapabilities | AvdtpSignalIdentifier::GetAllCapabilities
        ) {
            return Err(AvdtpCapabilitiesError::WrongSignal);
        }
        let AvdtpResponseResult::Accepted { parameters } = &self.result else {
            return Err(AvdtpCapabilitiesError::NotAccepted);
        };
        if parameters.is_empty() {
            return Err(AvdtpCapabilitiesError::EmptyResponse);
        }
        let mut offset = 0;
        let mut categories = Vec::new();
        while offset < parameters.len() {
            if categories.len() == 32 || parameters.len() - offset < 2 {
                return Err(if categories.len() == 32 {
                    AvdtpCapabilitiesError::TooManyCategories
                } else {
                    AvdtpCapabilitiesError::InvalidCategoryLength
                });
            }
            let category = parameters[offset];
            let length = parameters[offset + 1] as usize;
            offset += 2;
            let end = offset
                .checked_add(length)
                .ok_or(AvdtpCapabilitiesError::InvalidCategoryLength)?;
            if end > parameters.len() {
                return Err(AvdtpCapabilitiesError::InvalidCategoryLength);
            }
            let data = &parameters[offset..end];
            let parsed = match category {
                0x01 if data.is_empty() => AvdtpServiceCategory::MediaTransport,
                0x07 if data.len() == 6 => {
                    if data[0] != 0 || data[1] != 0 || data[4] == 0 || data[4] > data[5] {
                        return Err(AvdtpCapabilitiesError::InvalidCategoryData);
                    }
                    AvdtpServiceCategory::Sbc(AvdtpSbcCapability {
                        sampling_frequency_bits: data[2] >> 4,
                        channel_mode_bits: data[2] & 0x0f,
                        block_length_bits: data[3] >> 4,
                        allocation_method_bits: data[3] & 3,
                        subbands_bits: (data[3] >> 2) & 3,
                        min_bitpool: data[4],
                        max_bitpool: data[5],
                    })
                }
                _ => AvdtpServiceCategory::Unknown {
                    category,
                    data: data.to_vec(),
                },
            };
            categories.push(parsed);
            offset = end;
        }
        Ok(categories)
    }
}

fn validate_single_seid(parameters: &[u8]) -> Result<(), AvdtpError> {
    if parameters.len() != 1 {
        return Err(AvdtpError::InvalidParameterLength);
    }
    AvdtpStreamEndpointId::decode(parameters[0])
        .ok_or(AvdtpError::InvalidStreamEndpointId)
        .map(|_| ())
}

impl fmt::Display for SbcMediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "truncated SBC media data",
            Self::InvalidHeader => "invalid SBC or RTP header",
            Self::ConfigurationMismatch => "SBC frame configuration mismatch",
            Self::InvalidLength => "invalid SBC media length",
            Self::Fragmented => "fragmented SBC payload is unsupported",
        })
    }
}

impl std::error::Error for SbcMediaError {}

impl SbcFrame {
    /// Validate one complete SBC frame against the negotiated configuration.
    pub fn parse(bytes: &[u8], configuration: &SbcConfiguration) -> Result<Self, SbcMediaError> {
        if bytes.len() < 4 {
            return Err(SbcMediaError::Truncated);
        }
        if bytes[0] != 0x9c {
            return Err(SbcMediaError::InvalidHeader);
        }
        let encoded = configuration
            .encode()
            .map_err(|_| SbcMediaError::ConfigurationMismatch)?;
        if bytes[1] != encoded[0]
            || bytes[2] < configuration.min_bitpool
            || bytes[2] > configuration.max_bitpool
        {
            return Err(SbcMediaError::ConfigurationMismatch);
        }
        let expected = sbc_frame_length(bytes[1], bytes[2]);
        if expected < 4 || bytes.len() != expected {
            return Err(if bytes.len() < expected {
                SbcMediaError::Truncated
            } else {
                SbcMediaError::InvalidLength
            });
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Borrow the encoded SBC frame.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consume the frame into its encoded bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl SbcMediaPacket {
    /// Encode a complete RTP/SBC media packet.
    pub fn encode(&self) -> Result<Vec<u8>, SbcMediaError> {
        if self.frames.is_empty() || self.frames.len() > 15 {
            return Err(SbcMediaError::InvalidLength);
        }
        let mut bytes = vec![0x80, self.payload_type & 0x7f | u8::from(self.marker) << 7];
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        bytes.extend_from_slice(&self.timestamp.to_be_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.push(self.frames.len() as u8);
        for frame in &self.frames {
            bytes.extend_from_slice(&frame.0);
        }
        Ok(bytes)
    }

    /// Decode a complete, non-fragmented RTP/SBC media packet.
    pub fn decode(bytes: &[u8], configuration: &SbcConfiguration) -> Result<Self, SbcMediaError> {
        if bytes.len() < 13 {
            return Err(SbcMediaError::Truncated);
        }
        if bytes[0] != 0x80 {
            return Err(SbcMediaError::InvalidHeader);
        }
        let payload_type = bytes[1] & 0x7f;
        let marker = bytes[1] & 0x80 != 0;
        let sequence = u16::from_be_bytes([bytes[2], bytes[3]]);
        let timestamp = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let count = bytes[12] & 0x0f;
        if bytes[12] & 0xf0 != 0 || count == 0 {
            return if bytes[12] & 0xc0 != 0 {
                Err(SbcMediaError::Fragmented)
            } else {
                Err(SbcMediaError::InvalidLength)
            };
        }
        let mut offset = 13;
        let mut frames = Vec::with_capacity(count as usize);
        for _ in 0..count {
            if offset >= bytes.len() {
                return Err(SbcMediaError::Truncated);
            }
            let length = sbc_frame_length_at(bytes, offset)?;
            let end = offset
                .checked_add(length)
                .ok_or(SbcMediaError::InvalidLength)?;
            if end > bytes.len() {
                return Err(SbcMediaError::Truncated);
            }
            frames.push(SbcFrame::parse(&bytes[offset..end], configuration)?);
            offset = end;
        }
        if offset != bytes.len() {
            return Err(SbcMediaError::InvalidLength);
        }
        Ok(Self {
            payload_type,
            sequence,
            timestamp,
            marker,
            frames,
        })
    }
}

fn sbc_frame_length(header: u8, bitpool: u8) -> usize {
    let blocks = [4, 8, 12, 16][(header >> 4 & 3) as usize];
    let channels = match header >> 2 & 3 {
        0 => 1,
        1 => 2,
        _ => 2,
    };
    let subbands = if header & 1 != 0 { 8 } else { 4 };
    let base = if channels == 1 || header >> 2 & 3 == 1 {
        4 * subbands * channels
    } else {
        4 * subbands
    };
    4 + (base + blocks * channels * bitpool as usize).div_ceil(8)
}

fn sbc_frame_length_at(bytes: &[u8], offset: usize) -> Result<usize, SbcMediaError> {
    if bytes.len() - offset < 4 {
        return Err(SbcMediaError::Truncated);
    }
    if bytes[offset] != 0x9c {
        return Err(SbcMediaError::InvalidHeader);
    }
    Ok(sbc_frame_length(bytes[offset + 1], bytes[offset + 2]))
}

/// A codec capability advertised by an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    /// Codec family.
    pub codec: Codec,
    /// Codec-specific configuration bytes, excluding the AVDTP header.
    pub configuration: Vec<u8>,
}

/// A negotiated codec configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    /// Selected codec family.
    pub codec: Codec,
    /// Selected codec configuration bytes.
    pub configuration: Vec<u8>,
}

/// Why codec negotiation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegotiationError {
    /// No codec is supported by both endpoints.
    NoCommonCodec,
    /// A codec-specific capability intersection was empty.
    IncompatibleConfiguration,
    /// SBC media codec bytes are malformed or unsupported.
    InvalidSbcConfiguration,
}

impl fmt::Display for NegotiationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoCommonCodec => "no common A2DP codec",
            Self::IncompatibleConfiguration => "A2DP codec configurations are incompatible",
            Self::InvalidSbcConfiguration => "invalid SBC configuration",
        })
    }
}

impl std::error::Error for NegotiationError {}

/// Intersect two validated SBC configurations.
pub fn negotiate_sbc_parameters(
    local: &SbcConfiguration,
    remote: &SbcConfiguration,
) -> Result<SbcConfiguration, NegotiationError> {
    if local.sample_rate_hz != remote.sample_rate_hz
        || local.channel_mode != remote.channel_mode
        || local.blocks != remote.blocks
        || local.subbands != remote.subbands
        || local.allocation != remote.allocation
    {
        return Err(NegotiationError::IncompatibleConfiguration);
    }
    let min_bitpool = local.min_bitpool.max(remote.min_bitpool);
    let max_bitpool = local.max_bitpool.min(remote.max_bitpool);
    if min_bitpool > max_bitpool {
        return Err(NegotiationError::IncompatibleConfiguration);
    }
    Ok(SbcConfiguration {
        sample_rate_hz: local.sample_rate_hz,
        channel_mode: local.channel_mode,
        blocks: local.blocks,
        subbands: local.subbands,
        allocation: local.allocation,
        min_bitpool,
        max_bitpool,
    })
}

/// Negotiate one concrete SBC configuration from two AVDTP capability sets.
pub fn negotiate_sbc_capabilities(
    local: &AvdtpSbcCapability,
    remote: &AvdtpSbcCapability,
) -> Result<SbcConfiguration, NegotiationError> {
    validate_sbc_capability(local)?;
    validate_sbc_capability(remote)?;
    let rates = local.sampling_frequency_bits & remote.sampling_frequency_bits;
    let modes = local.channel_mode_bits & remote.channel_mode_bits;
    let blocks = local.block_length_bits & remote.block_length_bits;
    let subbands = local.subbands_bits & remote.subbands_bits;
    let allocation = local.allocation_method_bits & remote.allocation_method_bits;
    let sample_rate_hz = select_masked(
        rates,
        &[(0x01, 48000), (0x02, 44100), (0x04, 32000), (0x08, 16000)],
    )
    .ok_or(NegotiationError::IncompatibleConfiguration)?;
    let channel_mode = match select_masked(
        modes,
        &[
            (0x01, SbcChannelMode::JointStereo),
            (0x02, SbcChannelMode::Stereo),
            (0x04, SbcChannelMode::DualChannel),
            (0x08, SbcChannelMode::Mono),
        ],
    ) {
        Some(value) => value,
        None => return Err(NegotiationError::IncompatibleConfiguration),
    };
    let blocks = select_masked(blocks, &[(0x01, 16), (0x02, 12), (0x04, 8), (0x08, 4)])
        .ok_or(NegotiationError::IncompatibleConfiguration)?;
    let subbands = match select_masked(subbands, &[(0x01, 8), (0x02, 4)]) {
        Some(value) => value,
        None => return Err(NegotiationError::IncompatibleConfiguration),
    };
    let allocation = match select_masked(
        allocation,
        &[(0x01, SbcAllocation::Loudness), (0x02, SbcAllocation::SNR)],
    ) {
        Some(value) => value,
        None => return Err(NegotiationError::IncompatibleConfiguration),
    };
    let min_bitpool = local.min_bitpool.max(remote.min_bitpool);
    let max_bitpool = local.max_bitpool.min(remote.max_bitpool);
    if min_bitpool > max_bitpool {
        return Err(NegotiationError::IncompatibleConfiguration);
    }
    Ok(SbcConfiguration {
        sample_rate_hz,
        channel_mode,
        blocks,
        subbands,
        allocation,
        min_bitpool,
        max_bitpool,
    })
}

fn validate_sbc_capability(capability: &AvdtpSbcCapability) -> Result<(), NegotiationError> {
    if capability.sampling_frequency_bits == 0
        || capability.sampling_frequency_bits > 0x0f
        || capability.channel_mode_bits == 0
        || capability.channel_mode_bits > 0x0f
        || capability.block_length_bits == 0
        || capability.block_length_bits > 0x0f
        || capability.subbands_bits == 0
        || capability.subbands_bits > 0x03
        || capability.allocation_method_bits == 0
        || capability.allocation_method_bits > 0x03
        || capability.min_bitpool == 0
        || capability.min_bitpool > capability.max_bitpool
    {
        Err(NegotiationError::InvalidSbcConfiguration)
    } else {
        Ok(())
    }
}

fn select_masked<T: Copy>(mask: u8, choices: &[(u8, T)]) -> Option<T> {
    choices
        .iter()
        .find_map(|(bit, value)| (mask & bit != 0).then_some(*value))
}

/// Select the first local codec that the remote endpoint also advertises.
pub fn negotiate(
    local: &[Capability],
    remote: &[Capability],
) -> Result<Configuration, NegotiationError> {
    let mut shared_codec = false;
    for candidate in local {
        if let Some(peer) = remote.iter().find(|peer| peer.codec == candidate.codec) {
            shared_codec = true;
            if candidate.codec == Codec::Sbc {
                let local_sbc = SbcConfiguration::parse(&candidate.configuration)?;
                let remote_sbc = SbcConfiguration::parse(&peer.configuration)?;
                match negotiate_sbc_parameters(&local_sbc, &remote_sbc) {
                    Ok(configuration) => {
                        return Ok(Configuration {
                            codec: Codec::Sbc,
                            configuration: configuration.encode()?.to_vec(),
                        });
                    }
                    Err(NegotiationError::IncompatibleConfiguration) => continue,
                    Err(error) => return Err(error),
                }
            }
            if candidate.configuration == peer.configuration {
                return Ok(Configuration {
                    codec: candidate.codec,
                    configuration: candidate.configuration.clone(),
                });
            }
        }
    }
    if shared_codec {
        Err(NegotiationError::IncompatibleConfiguration)
    } else {
        Err(NegotiationError::NoCommonCodec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(codec: Codec, bytes: &[u8]) -> Capability {
        Capability {
            codec,
            configuration: bytes.to_vec(),
        }
    }

    #[test]
    fn negotiates_the_first_exact_common_codec() {
        let sbc = [0xc0, 0xf2, 2, 53];
        let local = [cap(Codec::Aac, &[1]), cap(Codec::Sbc, &sbc)];
        let remote = [cap(Codec::Sbc, &sbc)];
        let selected = negotiate(&local, &remote).unwrap();
        assert_eq!(selected.codec, Codec::Sbc);
    }

    #[test]
    fn rejects_missing_or_incompatible_codecs() {
        assert_eq!(
            negotiate(&[cap(Codec::Aac, &[1])], &[]),
            Err(NegotiationError::NoCommonCodec)
        );
        assert_eq!(
            negotiate(&[cap(Codec::Aac, &[1])], &[cap(Codec::Aac, &[2])]),
            Err(NegotiationError::IncompatibleConfiguration)
        );
    }

    #[test]
    fn sbc_configuration_round_trips() {
        let configuration = SbcConfiguration {
            sample_rate_hz: 48000,
            channel_mode: SbcChannelMode::JointStereo,
            blocks: 16,
            subbands: 8,
            allocation: SbcAllocation::Loudness,
            min_bitpool: 2,
            max_bitpool: 53,
        };
        let encoded = configuration.encode().unwrap();
        assert_eq!(SbcConfiguration::parse(&encoded).unwrap(), configuration);
    }

    #[test]
    fn negotiates_sbc_bitpool_intersection() {
        let local = SbcConfiguration {
            sample_rate_hz: 48000,
            channel_mode: SbcChannelMode::JointStereo,
            blocks: 16,
            subbands: 8,
            allocation: SbcAllocation::Loudness,
            min_bitpool: 20,
            max_bitpool: 50,
        };
        let remote = SbcConfiguration {
            min_bitpool: 30,
            max_bitpool: 60,
            ..local
        };
        let selected = negotiate_sbc_parameters(&local, &remote).unwrap();
        assert_eq!(selected.min_bitpool, 30);
        assert_eq!(selected.max_bitpool, 50);
    }

    #[test]
    fn rejects_sbc_disjoint_parameters() {
        let local = SbcConfiguration {
            sample_rate_hz: 44100,
            channel_mode: SbcChannelMode::Stereo,
            blocks: 16,
            subbands: 8,
            allocation: SbcAllocation::Loudness,
            min_bitpool: 20,
            max_bitpool: 50,
        };
        let remote = SbcConfiguration {
            sample_rate_hz: 48000,
            ..local
        };
        assert_eq!(
            negotiate_sbc_parameters(&local, &remote),
            Err(NegotiationError::IncompatibleConfiguration)
        );
    }

    #[test]
    fn falls_back_to_a_later_compatible_codec() {
        let incompatible_sbc = SbcConfiguration {
            sample_rate_hz: 44100,
            channel_mode: SbcChannelMode::JointStereo,
            blocks: 16,
            subbands: 8,
            allocation: SbcAllocation::Loudness,
            min_bitpool: 20,
            max_bitpool: 30,
        };
        let compatible_sbc = SbcConfiguration {
            sample_rate_hz: 48000,
            ..incompatible_sbc
        };
        let selected = negotiate(
            &[
                cap(Codec::Sbc, &incompatible_sbc.encode().unwrap()),
                cap(Codec::Aac, &[1, 2]),
            ],
            &[
                cap(Codec::Sbc, &compatible_sbc.encode().unwrap()),
                cap(Codec::Aac, &[1, 2]),
            ],
        )
        .unwrap();
        assert_eq!(selected.codec, Codec::Aac);
    }

    fn test_sbc_configuration() -> SbcConfiguration {
        SbcConfiguration {
            sample_rate_hz: 48000,
            channel_mode: SbcChannelMode::JointStereo,
            blocks: 16,
            subbands: 8,
            allocation: SbcAllocation::Loudness,
            min_bitpool: 2,
            max_bitpool: 53,
        }
    }

    fn test_sbc_frame(configuration: &SbcConfiguration) -> Vec<u8> {
        let header = configuration.encode().unwrap();
        let length = sbc_frame_length(header[0], configuration.min_bitpool);
        let mut frame = vec![0; length];
        frame[0] = 0x9c;
        frame[1] = header[0];
        frame[2] = configuration.min_bitpool;
        frame[3] = 0;
        frame
    }

    #[test]
    fn validates_sbc_frame_and_media_packet_round_trip() {
        let configuration = test_sbc_configuration();
        let frame = SbcFrame::parse(&test_sbc_frame(&configuration), &configuration).unwrap();
        let packet = SbcMediaPacket {
            payload_type: 96,
            sequence: 7,
            timestamp: 160,
            marker: true,
            frames: vec![frame.clone(), frame],
        };
        let encoded = packet.encode().unwrap();
        assert_eq!(
            SbcMediaPacket::decode(&encoded, &configuration).unwrap(),
            packet
        );
    }

    #[test]
    fn rejects_invalid_sbc_media_frames() {
        let configuration = test_sbc_configuration();
        let mut frame = test_sbc_frame(&configuration);
        frame[0] = 0;
        assert_eq!(
            SbcFrame::parse(&frame, &configuration),
            Err(SbcMediaError::InvalidHeader)
        );
        let mut packet = vec![0x80, 96, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0xc0];
        assert_eq!(
            SbcMediaPacket::decode(&packet, &configuration),
            Err(SbcMediaError::Fragmented)
        );
        packet[12] = 0;
        assert_eq!(
            SbcMediaPacket::decode(&packet, &configuration),
            Err(SbcMediaError::InvalidLength)
        );
    }

    #[test]
    fn round_trips_single_avdtp_signaling_packet() {
        let packet = AvdtpSignalingPacket {
            transaction: 7,
            packet_type: AvdtpPacketType::Single,
            message_type: AvdtpMessageType::ResponseAccept,
            signal_identifier: 0x06,
            parameters: vec![1, 2, 3],
        };
        assert_eq!(
            AvdtpSignalingPacket::decode(&packet.encode().unwrap()).unwrap(),
            packet
        );
    }

    #[test]
    fn rejects_invalid_avdtp_headers() {
        let packet = AvdtpSignalingPacket {
            transaction: 0,
            packet_type: AvdtpPacketType::Single,
            message_type: AvdtpMessageType::Command,
            signal_identifier: 1,
            parameters: Vec::new(),
        };
        assert_eq!(
            AvdtpSignalingPacket::decode(&[0x04, 1]),
            Err(AvdtpError::Fragmented)
        );
        assert_eq!(
            AvdtpSignalingPacket::decode(&[0, 0xc1]),
            Err(AvdtpError::ReservedBitsSet)
        );
        assert_eq!(
            AvdtpSignalingPacket::decode(&[0]),
            Err(AvdtpError::Truncated)
        );
        assert_eq!(
            AvdtpSignalingPacket {
                signal_identifier: 0x40,
                ..packet
            }
            .encode(),
            Err(AvdtpError::InvalidIdentifier)
        );
    }

    #[test]
    fn maps_typed_avdtp_signal_identifiers() {
        let known = [
            (AvdtpSignalIdentifier::Discover, 0x01),
            (AvdtpSignalIdentifier::GetCapabilities, 0x02),
            (AvdtpSignalIdentifier::SetConfiguration, 0x03),
            (AvdtpSignalIdentifier::GetConfiguration, 0x04),
            (AvdtpSignalIdentifier::Reconfigure, 0x05),
            (AvdtpSignalIdentifier::GetAllCapabilities, 0x06),
            (AvdtpSignalIdentifier::Start, 0x07),
            (AvdtpSignalIdentifier::Suspend, 0x08),
            (AvdtpSignalIdentifier::Close, 0x09),
            (AvdtpSignalIdentifier::Abort, 0x0a),
            (AvdtpSignalIdentifier::SecurityControl, 0x0b),
            (AvdtpSignalIdentifier::GetStreamEndpointState, 0x0c),
            (AvdtpSignalIdentifier::GetStreamEndpointInfo, 0x0d),
            (AvdtpSignalIdentifier::DelayReport, 0x0e),
        ];
        for (signal, value) in known {
            assert_eq!(signal.as_u8(), value);
            assert_eq!(AvdtpSignalIdentifier::from_u8(value), signal);
        }
        assert_eq!(
            AvdtpSignalIdentifier::from_u8(0x3f),
            AvdtpSignalIdentifier::Unknown(0x3f)
        );
    }

    #[test]
    fn constructs_typed_avdtp_command() {
        let packet =
            AvdtpSignalingPacket::command(2, AvdtpSignalIdentifier::GetAllCapabilities, vec![12]);
        assert_eq!(packet.signal(), AvdtpSignalIdentifier::GetAllCapabilities);
        assert_eq!(packet.encode().unwrap(), vec![0x20, 0x06, 12]);
    }

    #[test]
    fn constructs_common_avdtp_commands() {
        let seid = AvdtpStreamEndpointId::new(3).unwrap();
        assert_eq!(
            AvdtpSignalingPacket::discover(1).encode().unwrap(),
            vec![0x10, 1]
        );
        assert_eq!(
            AvdtpSignalingPacket::get_capabilities(1, seid)
                .encode()
                .unwrap(),
            vec![0x10, 2, 12]
        );
        assert_eq!(
            AvdtpSignalingPacket::start(1, &[seid])
                .unwrap()
                .encode()
                .unwrap(),
            vec![0x10, 7, 1, 12]
        );
    }

    #[test]
    fn rejects_invalid_avdtp_endpoint_parameters() {
        assert!(AvdtpStreamEndpointId::new(0).is_none());
        assert!(AvdtpStreamEndpointId::new(64).is_none());
        assert_eq!(
            AvdtpSignalingPacket::start(1, &[]),
            Err(AvdtpError::InvalidParameterLength)
        );
        assert_eq!(
            AvdtpSignalingPacket {
                signal_identifier: AvdtpSignalIdentifier::Discover.as_u8(),
                parameters: vec![1],
                ..AvdtpSignalingPacket::discover(1)
            }
            .encode(),
            Err(AvdtpError::InvalidParameterLength)
        );
        assert_eq!(
            AvdtpSignalingPacket::decode(&[0x10, 0x01, 1]),
            Err(AvdtpError::InvalidParameterLength)
        );
        assert_eq!(
            AvdtpSignalingPacket::decode(&[0x10, 0x02, 0]),
            Err(AvdtpError::InvalidStreamEndpointId)
        );
    }

    #[test]
    fn round_trips_typed_avdtp_responses() {
        let accepted =
            AvdtpSignalingPacket::accept(1, AvdtpSignalIdentifier::GetAllCapabilities, vec![1, 2]);
        assert_eq!(
            accepted.response().unwrap().result,
            AvdtpResponseResult::Accepted {
                parameters: vec![1, 2]
            }
        );
        let rejected =
            AvdtpSignalingPacket::reject(1, AvdtpSignalIdentifier::SetConfiguration, 2, vec![3]);
        assert_eq!(
            AvdtpSignalingPacket::decode(&rejected.encode().unwrap())
                .unwrap()
                .response()
                .unwrap()
                .result,
            AvdtpResponseResult::Rejected {
                error_code: 2,
                error_info: vec![3]
            }
        );
        let general = AvdtpSignalingPacket::general_reject(1, AvdtpSignalIdentifier::Start, 1);
        assert_eq!(
            general.response().unwrap().result,
            AvdtpResponseResult::GeneralReject { error_code: 1 }
        );
    }

    #[test]
    fn rejects_malformed_avdtp_responses() {
        assert_eq!(
            AvdtpSignalingPacket::decode(&[0x11, 0x07]),
            Err(AvdtpError::InvalidParameterLength)
        );
        assert_eq!(
            AvdtpSignalingPacket::decode(&[0x12, 0x03, 1]),
            Err(AvdtpError::InvalidParameterLength)
        );
        assert_eq!(
            AvdtpSignalingPacket::decode(&[0x12, 0x01, 1]),
            Err(AvdtpError::InvalidParameterLength)
        );
    }

    #[test]
    fn parses_discover_endpoint_descriptors() {
        let response = AvdtpSignalingPacket::accept(
            1,
            AvdtpSignalIdentifier::Discover,
            vec![0x8c, 0x00, 0x10, 0x18],
        )
        .response()
        .unwrap();
        let endpoints = response.discover_endpoints().unwrap();
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].seid.get(), 3);
        assert!(endpoints[0].in_use);
        assert_eq!(endpoints[0].media_type, AvdtpMediaType::Audio);
        assert_eq!(endpoints[0].endpoint_type, AvdtpEndpointType::Sink);
        assert_eq!(endpoints[1].seid.get(), 4);
        assert_eq!(endpoints[1].media_type, AvdtpMediaType::Video);
        assert_eq!(endpoints[1].endpoint_type, AvdtpEndpointType::Source);
    }

    #[test]
    fn validates_discover_endpoint_descriptors() {
        assert!(AvdtpStreamEndpointDescriptor::parse_discover_response(&[])
            .unwrap()
            .is_empty());
        assert_eq!(
            AvdtpStreamEndpointDescriptor::parse_discover_response(&[1]),
            Err(AvdtpDiscoverError::InvalidLength)
        );
        assert_eq!(
            AvdtpStreamEndpointDescriptor::parse_discover_response(&[0x00, 0x08]),
            Err(AvdtpDiscoverError::InvalidStreamEndpointId)
        );
        assert_eq!(
            AvdtpStreamEndpointDescriptor::parse_discover_response(&[0x0c, 0x09]),
            Err(AvdtpDiscoverError::ReservedBitsSet)
        );
        assert_eq!(
            AvdtpStreamEndpointDescriptor::parse_discover_response(&[0x0c, 0x38]),
            Err(AvdtpDiscoverError::UnsupportedMediaType)
        );
        assert_eq!(
            AvdtpStreamEndpointDescriptor::parse_discover_response(&[0x0c, 0x08, 0x0c, 0x08]),
            Err(AvdtpDiscoverError::DuplicateStreamEndpointId)
        );
    }

    #[test]
    fn discover_endpoint_view_rejects_wrong_response_kind() {
        let command = AvdtpSignalingPacket::discover(1);
        assert_eq!(
            AvdtpResponse {
                transaction: 1,
                signal: command.signal(),
                result: AvdtpResponseResult::Rejected {
                    error_code: 1,
                    error_info: Vec::new(),
                },
            }
            .discover_endpoints(),
            Err(AvdtpDiscoverError::NotAccepted)
        );
        assert_eq!(
            AvdtpResponse {
                transaction: 1,
                signal: AvdtpSignalIdentifier::Start,
                result: AvdtpResponseResult::Accepted {
                    parameters: Vec::new()
                },
            }
            .discover_endpoints(),
            Err(AvdtpDiscoverError::WrongSignal)
        );
    }

    #[test]
    fn parses_sbc_capability_categories() {
        let response = AvdtpSignalingPacket::accept(
            1,
            AvdtpSignalIdentifier::GetCapabilities,
            vec![
                0x01, 0x00, 0x07, 0x06, 0x00, 0x00, 0xff, 0xff, 2, 53, 0x99, 1, 0xaa,
            ],
        )
        .response()
        .unwrap();
        let categories = response.capabilities().unwrap();
        assert!(matches!(
            categories[0],
            AvdtpServiceCategory::MediaTransport
        ));
        assert_eq!(
            categories[1],
            AvdtpServiceCategory::Sbc(AvdtpSbcCapability {
                sampling_frequency_bits: 15,
                channel_mode_bits: 15,
                block_length_bits: 15,
                allocation_method_bits: 3,
                subbands_bits: 3,
                min_bitpool: 2,
                max_bitpool: 53,
            })
        );
        assert_eq!(
            categories[2],
            AvdtpServiceCategory::Unknown {
                category: 0x99,
                data: vec![0xaa]
            }
        );
    }

    #[test]
    fn rejects_malformed_capability_categories() {
        let response = |parameters| {
            AvdtpSignalingPacket::accept(1, AvdtpSignalIdentifier::GetCapabilities, parameters)
                .response()
                .unwrap()
        };
        assert_eq!(
            response(vec![]).capabilities(),
            Err(AvdtpCapabilitiesError::EmptyResponse)
        );
        assert_eq!(
            response(vec![7, 6, 0]).capabilities(),
            Err(AvdtpCapabilitiesError::InvalidCategoryLength)
        );
        assert_eq!(
            response(vec![7, 6, 1, 0, 0, 0, 2, 53]).capabilities(),
            Err(AvdtpCapabilitiesError::InvalidCategoryData)
        );
        assert_eq!(
            AvdtpSignalingPacket::accept(1, AvdtpSignalIdentifier::Start, vec![])
                .response()
                .unwrap()
                .capabilities(),
            Err(AvdtpCapabilitiesError::WrongSignal)
        );
    }

    #[test]
    fn negotiates_sbc_capability_masks_deterministically() {
        let local = AvdtpSbcCapability {
            sampling_frequency_bits: 0x0f,
            channel_mode_bits: 0x0f,
            block_length_bits: 0x0f,
            allocation_method_bits: 0x03,
            subbands_bits: 0x03,
            min_bitpool: 2,
            max_bitpool: 53,
        };
        let remote = AvdtpSbcCapability {
            sampling_frequency_bits: 0x03,
            channel_mode_bits: 0x03,
            block_length_bits: 0x03,
            allocation_method_bits: 0x03,
            subbands_bits: 0x03,
            min_bitpool: 20,
            max_bitpool: 40,
        };
        assert_eq!(
            negotiate_sbc_capabilities(&local, &remote).unwrap(),
            SbcConfiguration {
                sample_rate_hz: 48000,
                channel_mode: SbcChannelMode::JointStereo,
                blocks: 16,
                subbands: 8,
                allocation: SbcAllocation::Loudness,
                min_bitpool: 20,
                max_bitpool: 40,
            }
        );
    }

    #[test]
    fn rejects_invalid_sbc_capabilities() {
        let valid = AvdtpSbcCapability {
            sampling_frequency_bits: 1,
            channel_mode_bits: 1,
            block_length_bits: 1,
            allocation_method_bits: 1,
            subbands_bits: 1,
            min_bitpool: 2,
            max_bitpool: 53,
        };
        assert_eq!(
            negotiate_sbc_capabilities(
                &AvdtpSbcCapability {
                    channel_mode_bits: 0,
                    ..valid
                },
                &valid,
            ),
            Err(NegotiationError::InvalidSbcConfiguration)
        );
        assert_eq!(
            negotiate_sbc_capabilities(
                &valid,
                &AvdtpSbcCapability {
                    sampling_frequency_bits: 2,
                    ..valid
                },
            ),
            Err(NegotiationError::IncompatibleConfiguration)
        );
    }

    #[test]
    fn constructs_single_endpoint_avdtp_commands() {
        let seid = AvdtpStreamEndpointId::new(3).unwrap();
        assert_eq!(
            AvdtpSignalingPacket::get_all_capabilities(1, seid).parameters,
            vec![12]
        );
        assert_eq!(
            AvdtpSignalingPacket::get_configuration(1, seid).parameters,
            vec![12]
        );
        assert_eq!(
            AvdtpSignalingPacket::suspend(1, seid).parameters,
            vec![1, 12]
        );
        assert_eq!(AvdtpSignalingPacket::close(1, seid).parameters, vec![12]);
        assert_eq!(AvdtpSignalingPacket::abort(1, seid).parameters, vec![12]);
    }

    #[test]
    fn constructs_sbc_set_configuration() {
        let seid = AvdtpStreamEndpointId::new(3).unwrap();
        let configuration = test_sbc_configuration();
        let packet = AvdtpSignalingPacket::set_sbc_configuration(
            1,
            seid,
            AvdtpStreamEndpointId::new(4).unwrap(),
            configuration,
        )
        .unwrap();
        assert_eq!(packet.parameters.len(), 12);
        assert_eq!(packet.parameters[0..2], [12, 16]);
        assert_eq!(
            SbcConfiguration::parse(&packet.parameters[8..]).unwrap(),
            configuration
        );
        assert!(packet.encode().is_ok());
    }

    #[test]
    fn constructs_sbc_reconfigure() {
        let configuration = test_sbc_configuration();
        let packet = AvdtpSignalingPacket::reconfigure_sbc(
            1,
            AvdtpStreamEndpointId::new(3).unwrap(),
            configuration,
        )
        .unwrap();
        assert_eq!(packet.parameters.len(), 9);
        assert_eq!(packet.parameters[..5], [12, 7, 6, 0, 0]);
        assert_eq!(
            SbcConfiguration::parse(&packet.parameters[5..]).unwrap(),
            configuration
        );
        assert!(packet.encode().is_ok());
    }
}
