//! Android Auto H.264 video projection service (`aap-video`).
//!
//! This crate provides [`VideoService`], which implements the
//! [`aap_contracts::Service`] trait for the Android Auto video channel
//! (`ChannelId::Video`).
//!
//! # Quick start
//!
//! ```rust,ignore
//! use aap_video::{VideoService, VideoConfig};
//!
//! let service = VideoService::new(VideoConfig::default());
//! registry.register(service);
//! ```

#![warn(missing_docs)]

pub mod channel;
mod config;
mod mode;
mod service;
mod sink;

pub use channel::{
    video_frame_channel, video_start_gate, VideoFrameReceiver, VideoFrameSender, VideoStartRx,
    VideoStartTx,
};
pub use config::VideoConfig;
pub use mode::{mode_from_setup_response, VideoMode, DEFAULT_VIDEO_MODE, VIDEO_MODES};
pub use service::VideoService;
pub use sink::{FrameSink, NullSink};
