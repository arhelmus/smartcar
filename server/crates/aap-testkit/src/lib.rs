//! Test producers for the smartcar audio/video pipeline.
//!
//! Provides drop-in frame sources that replace real renderers (Flutter) during
//! bringup and CI validation:
//!
//! - [`TestVideoProducer`] — encodes a colour-cycling H.264 test pattern and
//!   pushes timestamped NAL units into a [`VideoFrameSender`].
//! - [`LoopingWavStream`] — decodes an embedded WAV and serves it, normalized
//!   into the target audio format, as a pull-based `AudioStream`.
//!
//! The video producer runs its loop on a **blocking thread** (CPU-bound work,
//! no async needed), started with
//! `tokio::task::spawn_blocking(|| producer.run(...))`.

pub mod audio;
pub mod video;

pub use audio::{LoopingWavStream, ASSET_KICK_IN, ASSET_SNARE_UNDER, ASSET_SYNTH_01};
pub use video::TestVideoProducer;
