//! Control-channel message type ids.
//!
//! Values mirror `aasdk`'s `ControlMessageIdsEnum`. The agent implementing W1
//! (`aap-proto`) is responsible for keeping `aap-proto`'s generated types in
//! sync with these ids; treat this enum as the canonical source within Rust.
//!
//! For non-control channels, message ids are channel-specific and not enumerated
//! here — those crates define their own constants.

/// Control-channel message identifiers (u16 BE, occupies the first two bytes
/// of a frame payload when `FrameFlags::CONTROL` is set).
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    /// Phone → head unit: propose protocol version.
    VersionRequest = 0x0001,
    /// Head unit → phone: accepted protocol version.
    VersionResponse = 0x0002,
    /// TLS handshake data exchange.
    SslHandshake = 0x0003,
    /// TLS handshake complete; both sides switch to encrypted mode.
    AuthComplete = 0x0004,
    /// Phone → head unit: enumerate available services.
    ServiceDiscoveryRequest = 0x0005,
    /// Head unit → phone: list of supported services.
    ServiceDiscoveryResponse = 0x0006,
    /// Phone → head unit: open a specific channel.
    ChannelOpenRequest = 0x0007,
    /// Head unit → phone: channel open result.
    ChannelOpenResponse = 0x0008,
    /// Keep-alive ping from either side.
    PingRequest = 0x000B,
    /// Keep-alive pong in response to a ping.
    PingResponse = 0x000C,
    /// Request navigation UI focus.
    NavigationFocusRequest = 0x000D,
    /// Navigation focus grant/deny.
    NavigationFocusResponse = 0x000E,
    /// Initiate graceful session shutdown.
    ShutdownRequest = 0x000F,
    /// Acknowledge shutdown request.
    ShutdownResponse = 0x0010,
    /// Request to start a voice session.
    VoiceSessionRequest = 0x0011,
    /// Request audio focus for a channel.
    AudioFocusRequest = 0x0012,
    /// Audio focus grant/deny response.
    AudioFocusResponse = 0x0013,
}

impl MessageType {
    /// Raw on-the-wire u16.
    pub const fn as_u16(self) -> u16 {
        self as u16
    }
}

impl TryFrom<u16> for MessageType {
    type Error = u16;
    fn try_from(v: u16) -> Result<Self, u16> {
        Ok(match v {
            0x0001 => Self::VersionRequest,
            0x0002 => Self::VersionResponse,
            0x0003 => Self::SslHandshake,
            0x0004 => Self::AuthComplete,
            0x0005 => Self::ServiceDiscoveryRequest,
            0x0006 => Self::ServiceDiscoveryResponse,
            0x0007 => Self::ChannelOpenRequest,
            0x0008 => Self::ChannelOpenResponse,
            0x000B => Self::PingRequest,
            0x000C => Self::PingResponse,
            0x000D => Self::NavigationFocusRequest,
            0x000E => Self::NavigationFocusResponse,
            0x000F => Self::ShutdownRequest,
            0x0010 => Self::ShutdownResponse,
            0x0011 => Self::VoiceSessionRequest,
            0x0012 => Self::AudioFocusRequest,
            0x0013 => Self::AudioFocusResponse,
            other => return Err(other),
        })
    }
}
