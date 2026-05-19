//! Android Auto H.264 video projection service (`aap-video`).
//!
//! This crate provides [`VideoService`], which implements the
//! [`aap_contracts::Service`] trait for the Android Auto video channel
//! (`ChannelId::Video`).
//!
//! # Quick start
//!
//! ```rust,ignore
//! use aap_video::{advertise, VideoService, SOFTWARE_CAPS};
//!
//! let advertised = advertise(&SOFTWARE_CAPS);
//! registry.register(VideoService::new(advertised));
//! ```

#![warn(missing_docs)]

pub mod channel;
mod mode;
mod service;
mod sink;

pub use channel::{
    video_frame_channel, video_start_gate, VideoFrameReceiver, VideoFrameSender, VideoStartRx,
    VideoStartTx,
};
pub use mode::{advertise, resolve, RenderCaps, VideoCfg, CATALOG, FALLBACK, SOFTWARE_CAPS};
pub use service::VideoService;
pub use sink::{FrameSink, NullSink};
