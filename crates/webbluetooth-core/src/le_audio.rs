//! LE Audio and ISO transport contracts.
//!
//! This module defines transport and codec boundaries only. LC3 encoding and
//! decoding remain backend-provided because implementations and licensing vary
//! by platform.

/// An LE Isochronous stream identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IsoStream {
    /// Connected Isochronous Stream.
    Cis {
        /// HCI ISO connection handle.
        handle: u16,
    },
    /// Broadcast Isochronous Stream.
    Bis {
        /// HCI ISO broadcast handle.
        handle: u16,
    },
}

impl IsoStream {
    /// Construct a CIS stream identifier.
    pub const fn cis(handle: u16) -> Self {
        Self::Cis { handle }
    }
    /// Construct a BIS stream identifier.
    pub const fn bis(handle: u16) -> Self {
        Self::Bis { handle }
    }
    /// Return the HCI ISO handle.
    pub const fn handle(self) -> u16 {
        match self {
            Self::Cis { handle } | Self::Bis { handle } => handle,
        }
    }
}

/// Metadata attached to one ISO SDU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoMetadata {
    /// CIS or BIS source stream.
    pub stream: IsoStream,
    /// SDU sequence number.
    pub sequence: u16,
    /// Presentation timestamp in microseconds, when available.
    pub timestamp_micros: Option<u64>,
}

/// LE Audio stream configuration shared by CIS and BIS transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoConfiguration {
    /// ISO stream identifier.
    pub stream: IsoStream,
    /// Audio sampling rate in Hz.
    pub sample_rate_hz: u32,
    /// Frame duration in microseconds.
    pub frame_duration_micros: u32,
    /// Number of PCM channels.
    pub channels: u8,
    /// Encoded LC3 octets per frame.
    pub octets_per_frame: u16,
}

impl IsoConfiguration {
    /// Validate a stream configuration against common LE Audio limits.
    pub fn validate(&self) -> Result<(), CodecError> {
        if !matches!(
            self.sample_rate_hz,
            8000 | 16000 | 24000 | 32000 | 44100 | 48000 | 96000
        ) || !matches!(self.frame_duration_micros, 7500 | 10000)
            || !(1..=8).contains(&self.channels)
            || self.octets_per_frame == 0
        {
            return Err(CodecError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// One encoded LC3 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lc3Frame(Vec<u8>);

impl Lc3Frame {
    /// Wrap an encoded LC3 frame.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    /// Borrow the encoded frame.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Consume the frame.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Backend boundary for LC3 encoding and decoding.
pub trait Lc3Codec: Send {
    /// Encode one PCM frame to LC3.
    fn encode(&mut self, pcm: &[i16]) -> Result<Lc3Frame, CodecError>;
    /// Decode one LC3 frame to PCM.
    fn decode(&mut self, frame: &Lc3Frame) -> Result<Vec<i16>, CodecError>;
}

/// LC3 codec operation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// The frame or PCM length is invalid for the negotiated configuration.
    InvalidFrame,
    /// The ISO/LC3 stream configuration is outside supported bounds.
    InvalidConfiguration,
    /// No codec implementation is available for the requested configuration.
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_streams_preserve_kind_and_handle() {
        assert_eq!(IsoStream::cis(0x123).handle(), 0x123);
        assert_eq!(IsoStream::bis(0x456).handle(), 0x456);
    }

    #[test]
    fn lc3_frames_round_trip_bytes() {
        let frame = Lc3Frame::new(vec![1, 2, 3]);
        assert_eq!(frame.as_bytes(), &[1, 2, 3]);
        assert_eq!(frame.into_bytes(), vec![1, 2, 3]);
    }

    #[test]
    fn validates_common_iso_configurations() {
        let configuration = IsoConfiguration {
            stream: IsoStream::cis(1),
            sample_rate_hz: 48000,
            frame_duration_micros: 10000,
            channels: 2,
            octets_per_frame: 120,
        };
        assert!(configuration.validate().is_ok());
        assert!(IsoConfiguration {
            sample_rate_hz: 11025,
            ..configuration
        }
        .validate()
        .is_err());
    }
}
