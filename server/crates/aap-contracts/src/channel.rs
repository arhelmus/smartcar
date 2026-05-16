//! Android Auto channel identifiers.
//!
//! Values mirror `aasdk`'s `ChannelId` enum
//! (see `server/third_party/aasdk/.../ChannelId.proto` once the submodule is
//! initialized). Verify exact integer values after submodule init; the values
//! below match the canonical aasdk source at time of writing.

use std::fmt;

/// Channel identifier as transmitted on the wire (1 byte).
///
/// Values match `aasdk`'s `ChannelId` enum exactly.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelId {
    /// Control channel — version, SSL, service discovery, channel mgmt.
    Control = 0,
    /// Car sensors provided by the head unit to the phone (GPS, driving status, night mode).
    Sensor = 1,
    /// Generic media-sink base ID (rarely used directly; specific channels are 3–6).
    MediaSink = 2,
    /// H.264 video stream from the phone to the head unit.
    Video = 3,
    /// Primary media audio (music) from phone to head unit.
    MediaAudio = 4,
    /// Guidance / navigation TTS audio from phone to head unit.
    SpeechAudio = 5,
    /// System audio (notifications, ringtones) from phone to head unit.
    SystemAudio = 6,
    /// Telephony audio sink on the head unit.
    TelephonyAudio = 7,
    /// HID / touch / key input events from the head unit to the phone.
    InputSource = 8,
    /// Microphone / media source from the phone to the head unit.
    Microphone = 9,
    /// Bluetooth pairing and phone-call channel.
    Bluetooth = 10,
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
            1 => Self::Sensor,
            2 => Self::MediaSink,
            3 => Self::Video,
            4 => Self::MediaAudio,
            5 => Self::SpeechAudio,
            6 => Self::SystemAudio,
            7 => Self::TelephonyAudio,
            8 => Self::InputSource,
            9 => Self::Microphone,
            10 => Self::Bluetooth,
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
        for v in [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 255] {
            let id = ChannelId::try_from(v).unwrap();
            assert_eq!(id.as_u8(), v);
        }
        assert!(ChannelId::try_from(99).is_err());
    }
}
