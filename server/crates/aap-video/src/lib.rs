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

mod config;
mod service;
mod sink;

pub use config::VideoConfig;
pub use service::VideoService;
pub use sink::{FrameSink, NullSink};
