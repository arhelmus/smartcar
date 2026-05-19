//! Negotiated video resolution.
//!
//! The phone advertises an ordered list of [`VideoMode`]s in the `AVChannel`
//! descriptor.  The head unit replies with an `AVChannelSetupResponse` whose
//! `configs` field holds the index/indices it selected — i.e. the head unit
//! picks the resolution matching its screen.  [`mode_from_setup_response`]
//! decodes that and resolves it back to a concrete `width × height × fps`.

use prost::Message;

/// One advertised video configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMode {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame rate in frames per second.
    pub fps: u32,
    /// `enums::video_resolution::Enum` value (`_480p=1`, `_720p=2`, `_1080p=3`).
    pub resolution_enum: i32,
}

/// Ordered list advertised in the AV descriptor.
///
/// The head unit's `AVChannelSetupResponse.configs` values are indices into
/// **this** slice, so order is the contract — only ever append.  480p is
/// `800×480` to match openauto's default `Video.Resolution` (`VIDEO_800x480`).
pub const VIDEO_MODES: &[VideoMode] = &[
    VideoMode {
        width: 800,
        height: 480,
        fps: 30,
        resolution_enum: 1,
    },
    VideoMode {
        width: 1280,
        height: 720,
        fps: 30,
        resolution_enum: 2,
    },
    VideoMode {
        width: 1920,
        height: 1080,
        fps: 30,
        resolution_enum: 3,
    },
];

/// Fallback when no `AVChannelSetupResponse` was parsed (480p).
pub const DEFAULT_VIDEO_MODE: VideoMode = VIDEO_MODES[0];

/// Minimal view of `AVChannelSetupResponse` (`gb.xxy.trial.proto.messages`).
///
/// Declared `optional` rather than `required` so a partial / unexpected body
/// still decodes instead of erroring.
#[derive(Message)]
struct SetupResponse {
    #[prost(uint32, optional, tag = "1")]
    media_status: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    max_unacked: Option<u32>,
    #[prost(uint32, repeated, tag = "3")]
    configs: ::prost::alloc::vec::Vec<u32>,
}

/// Decode an `AVChannelSetupResponse` body (the bytes after the 2-byte message
/// id) and resolve the head unit's first selected config index to a
/// [`VideoMode`].  `None` if the body doesn't decode or the index is unknown.
pub fn mode_from_setup_response(body: &[u8]) -> Option<VideoMode> {
    let resp = SetupResponse::decode(body).ok()?;
    let idx = *resp.configs.first()? as usize;
    VIDEO_MODES.get(idx).copied()
}
