//! AVRCP command and event modeling.

use std::fmt;

/// AVRCP pass-through commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Start playback.
    Play,
    /// Pause playback.
    Pause,
    /// Stop playback.
    Stop,
    /// Skip to the next track.
    Forward,
    /// Return to the previous track.
    Backward,
    /// Increase volume.
    VolumeUp,
    /// Decrease volume.
    VolumeDown,
}

/// An AVRCP pass-through command frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassThrough {
    /// AVCTP transaction label, 0..=15.
    pub transaction: u8,
    /// Command operation.
    pub command: Command,
    /// Whether this is a key press (`true`) or release (`false`).
    pub pressed: bool,
}

impl PassThrough {
    /// Encode an AVCTP pass-through frame for the target unit.
    pub fn encode(self) -> Result<Vec<u8>, Error> {
        if self.transaction > 0x0f {
            return Err(Error::InvalidValue);
        }
        let mut bytes = vec![
            (self.transaction << 4) | if self.pressed { 0 } else { 0x80 },
            0x48,
            self.command.operation_id(),
        ];
        bytes.push(0);
        Ok(bytes)
    }

    /// Decode a pass-through frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 3 || bytes[1] != 0x48 {
            return Err(Error::InvalidValue);
        }
        let command = match bytes[2] {
            0x44 => Command::Play,
            0x46 => Command::Pause,
            0x45 => Command::Stop,
            0x4b => Command::Forward,
            0x4c => Command::Backward,
            0x41 => Command::VolumeUp,
            0x42 => Command::VolumeDown,
            _ => return Err(Error::InvalidValue),
        };
        Ok(Self {
            transaction: bytes[0] >> 4,
            command,
            pressed: bytes[0] & 0x80 == 0,
        })
    }
}

impl Command {
    /// AVRCP pass-through operation ID.
    pub const fn operation_id(self) -> u8 {
        match self {
            Self::Play => 0x44,
            Self::Pause => 0x46,
            Self::Stop => 0x45,
            Self::Forward => 0x4b,
            Self::Backward => 0x4c,
            Self::VolumeUp => 0x41,
            Self::VolumeDown => 0x42,
        }
    }
}

/// Playback state reported by a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    /// No track is playing.
    Stopped,
    /// A track is playing.
    Playing,
    /// Playback is paused.
    Paused,
    /// The target is seeking.
    FwdSeek,
    /// The target is reverse-seeking.
    RevSeek,
    /// The target has no usable state.
    Error,
}

/// A minimal AVRCP media metadata snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Metadata {
    /// Track title.
    pub title: Option<String>,
    /// Performing artist.
    pub artist: Option<String>,
    /// Album title.
    pub album: Option<String>,
    /// Track number, when reported.
    pub track: Option<u32>,
    /// Total track count, when reported.
    pub total_tracks: Option<u32>,
}

/// An AVRCP event received from a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Playback status changed.
    PlaybackStatus(PlaybackStatus),
    /// Track metadata changed.
    Metadata(Metadata),
    /// Absolute volume changed, in the AVRCP 0..=127 range.
    Volume(u8),
}

impl Event {
    /// Decode a registered AVRCP notification event payload.
    pub fn decode(event_id: u8, payload: &[u8]) -> Result<Self, Error> {
        if payload.len() != 1 {
            return Err(Error::InvalidValue);
        }
        match event_id {
            0x01 => playback_status(payload[0]).map(Self::PlaybackStatus),
            0x0d => absolute_volume(payload[0]).map(Self::Volume),
            _ => Err(Error::InvalidValue),
        }
    }
}

/// Why an AVRCP value could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A value was outside the protocol's range.
    InvalidValue,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid AVRCP value")
    }
}

impl std::error::Error for Error {}

/// Decode an AVRCP playback-status byte.
pub fn playback_status(value: u8) -> Result<PlaybackStatus, Error> {
    match value {
        0x00 => Ok(PlaybackStatus::Stopped),
        0x01 => Ok(PlaybackStatus::Playing),
        0x02 => Ok(PlaybackStatus::Paused),
        0x03 => Ok(PlaybackStatus::FwdSeek),
        0x04 => Ok(PlaybackStatus::RevSeek),
        0xff => Ok(PlaybackStatus::Error),
        _ => Err(Error::InvalidValue),
    }
}

/// Validate and normalize AVRCP absolute volume.
pub fn absolute_volume(value: u8) -> Result<u8, Error> {
    (value <= 0x7f).then_some(value).ok_or(Error::InvalidValue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_have_spec_operation_ids() {
        assert_eq!(Command::Play.operation_id(), 0x44);
        assert_eq!(Command::Pause.operation_id(), 0x46);
    }

    #[test]
    fn playback_and_volume_values_are_checked() {
        assert_eq!(playback_status(0x01).unwrap(), PlaybackStatus::Playing);
        assert!(playback_status(0x55).is_err());
        assert_eq!(absolute_volume(0x7f).unwrap(), 0x7f);
        assert!(absolute_volume(0x80).is_err());
    }

    #[test]
    fn pass_through_frames_round_trip() {
        let command = PassThrough {
            transaction: 3,
            command: Command::Play,
            pressed: true,
        };
        assert_eq!(
            PassThrough::decode(&command.encode().unwrap()).unwrap(),
            command
        );
    }

    #[test]
    fn notification_events_decode_registered_payloads() {
        assert_eq!(
            Event::decode(0x01, &[0x01]).unwrap(),
            Event::PlaybackStatus(PlaybackStatus::Playing)
        );
        assert_eq!(Event::decode(0x0d, &[0x7f]).unwrap(), Event::Volume(0x7f));
    }

    #[test]
    fn notification_events_reject_invalid_payloads() {
        assert!(Event::decode(0x01, &[]).is_err());
        assert!(Event::decode(0x01, &[0x55]).is_err());
        assert!(Event::decode(0x0d, &[0x80]).is_err());
        assert!(Event::decode(0x03, &[0]).is_err());
        assert!(Event::decode(0x01, &[0, 1]).is_err());
    }
}
