//! The negotiable video-config menu.
//!
//! Android Auto video setup is *menu-by-index*: the source advertises an
//! ordered list of fully-specified [`VideoCfg`]s in the `AVChannel`
//! descriptor; the head unit replies with the index it accepts in its
//! `AVChannelSetupResponse`.  It cannot request arbitrary values — only pick a
//! row.  So the advertised list **is** the contract: order is append-only, and
//! the same list instance that was advertised must be used to resolve the
//! head unit's index.
//!
//! [`CATALOG`] is everything we could ever offer; [`advertise`] filters it by
//! the active renderer's [`RenderCaps`] so we never advertise a config we'd
//! then silently degrade.

use prost::Message;

/// One fully-specified, advertisable video configuration.
///
/// Mirrors the proto `data.VideoConfig` (the fields actually on the wire) plus
/// the derived pixel geometry.  `width`/`height` are a convention applied to
/// the resolution enum — they are *not* themselves on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoCfg {
    /// `enums::video_resolution::Enum` value (`_480p=1`, `_720p=2`, `_1080p=3`).
    pub resolution: i32,
    /// `enums::video_fps::Enum` value (`_30=1`, `_60=2`).
    pub fps: i32,
    /// Encoded frame width in pixels (convention for `resolution`).
    pub width: u32,
    /// Encoded frame height in pixels (convention for `resolution`).
    pub height: u32,
    /// Total horizontal letterbox, in pixels (split evenly left/right).
    pub margin_width: u32,
    /// Total vertical letterbox, in pixels (split evenly top/bottom).
    pub margin_height: u32,
    /// Display density; drives the renderer's logical pixel ratio.
    pub dpi: u32,
    /// Extra colour depth.  Always `None` — the pipeline is 8-bit.
    pub additional_depth: Option<u32>,
}

impl VideoCfg {
    /// Frame rate in Hz (30 or 60).
    pub fn fps_hz(&self) -> u32 {
        if self.fps == 2 {
            60
        } else {
            30
        }
    }

    /// Logical→physical pixel ratio for the renderer, from `dpi`
    /// (160 dpi = Android mdpi baseline = ratio 1.0).
    pub fn pixel_ratio(&self) -> f64 {
        self.dpi as f64 / 160.0
    }

    /// Renderable area inside the letterbox margins: the encoded frame is
    /// `width × height`, but content is composited into this inner rect.
    pub fn inner(&self) -> (u32, u32) {
        (
            self.width.saturating_sub(self.margin_width),
            self.height.saturating_sub(self.margin_height),
        )
    }

    /// Top-left offset of the inner rect within the encoded frame.
    pub fn offset(&self) -> (u32, u32) {
        (self.margin_width / 2, self.margin_height / 2)
    }
}

const fn cfg(resolution: i32, width: u32, height: u32, fps: i32) -> VideoCfg {
    VideoCfg {
        resolution,
        fps,
        width,
        height,
        margin_width: 0,
        margin_height: 0,
        dpi: 160,
        additional_depth: None,
    }
}

/// Every config we could ever advertise, in the canonical index order.
///
/// **Append-only.**  The head unit references rows by index into the
/// *advertised* (filtered) list; reordering or inserting silently
/// reinterprets a head unit's selection.
pub const CATALOG: &[VideoCfg] = &[
    cfg(1, 800, 480, 1),
    cfg(2, 1280, 720, 1),
    cfg(3, 1920, 1080, 1),
    cfg(1, 800, 480, 2),
    cfg(2, 1280, 720, 2),
    cfg(3, 1920, 1080, 2),
];

/// Used only if the head unit's `AVChannelSetupResponse` can't be parsed.
pub const FALLBACK: VideoCfg = CATALOG[0];

/// What the active render path can actually deliver.  The advertised menu is
/// filtered to this so negotiation stays honest.
#[derive(Clone, Copy, Debug)]
pub struct RenderCaps {
    /// Highest sustainable frame rate (Hz).
    pub max_fps: u32,
    /// Highest `video_resolution` enum value the path can drive.
    pub max_resolution: i32,
}

/// Software path: all resolutions, 30 fps only (Flutter's software
/// rasteriser + CPU H.264 can't sustain 60).  The board GPU path will pass
/// wider caps.
pub const SOFTWARE_CAPS: RenderCaps = RenderCaps {
    max_fps: 30,
    max_resolution: 3,
};

/// Filter [`CATALOG`] to the configs `caps` can deliver, preserving order.
pub fn advertise(caps: &RenderCaps) -> Vec<VideoCfg> {
    CATALOG
        .iter()
        .copied()
        .filter(|c| c.fps_hz() <= caps.max_fps && c.resolution <= caps.max_resolution)
        .collect()
}

/// Minimal view of `AVChannelSetupResponse` (`gb.xxy.trial.proto.messages`).
///
/// `optional`/lenient so a partial or unexpected body still decodes.
#[derive(Message)]
struct SetupResponse {
    #[prost(uint32, optional, tag = "1")]
    media_status: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    max_unacked: Option<u32>,
    #[prost(uint32, repeated, tag = "3")]
    configs: ::prost::alloc::vec::Vec<u32>,
}

/// Resolve the head unit's selected config against the **exact list that was
/// advertised** (indices are positions in that list).  `None` if the body
/// doesn't decode or the index is out of range.
pub fn resolve(advertised: &[VideoCfg], body: &[u8]) -> Option<VideoCfg> {
    let resp = SetupResponse::decode(body).ok()?;
    let idx = *resp.configs.first()? as usize;
    advertised.get(idx).copied()
}
