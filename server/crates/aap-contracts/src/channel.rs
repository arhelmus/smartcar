//! Android Auto channel identifiers.
//!
//! Values mirror `aasdk`'s `ChannelId` enum
//! (see `server/third_party/aasdk/.../ChannelId.proto` once the submodule is
//! initialized). Verify exact integer values after submodule init; the values
//! below match the canonical aasdk source at time of writing.

use std::fmt;

/// Channel identifier as transmitted on the wire (1 byte).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelId {
    /// Control channel — version, SSL, service discovery, channel mgmt.
    Control = 0,
    /// HID / touchpad input events from the head unit.
    Input = 1,
    /// Sensor data (GPS, gyro, accelerometer, etc.).
    Sensor = 2,
    /// H.264 video stream from the projection source.
    Video = 3,
    /// Primary media audio (music, navigation TTS).
    MediaAudio = 4,
    /// Speech / voice-assistant audio.
    SpeechAudio = 5,
    /// System audio (ringtones, notifications).
    SystemAudio = 6,
    /// Audio/video input from the head unit (reverse camera, etc.).
    AvInput = 7,
    /// Bluetooth channel for phone calls / pairing.
    Bluetooth = 8,
    /// Sentinel — used when no channel is selected.
    None = 255,
}

impl ChannelId {
    /// Returns the raw byte value as sent on the wire.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for ChannelId {
    type Error = u8;
    fn try_from(v: u8) -> Result<Self, u8> {
        Ok(match v {
            0 => Self::Control,
            1 => Self::Input,
            2 => Self::Sensor,
            3 => Self::Video,
            4 => Self::MediaAudio,
            5 => Self::SpeechAudio,
            6 => Self::SystemAudio,
            7 => Self::AvInput,
            8 => Self::Bluetooth,
            255 => Self::None,
            other => return Err(other),
        })
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        for v in [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 255] {
            let id = ChannelId::try_from(v).unwrap();
            assert_eq!(id.as_u8(), v);
        }
        assert!(ChannelId::try_from(99).is_err());
    }
}
