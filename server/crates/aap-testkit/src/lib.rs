//! Test producers for the smartcar audio/video pipeline.
//!
//! Provides drop-in frame sources that replace real renderers (Flutter) during
//! bringup and CI validation:
//!
//! - [`TestVideoProducer`] — encodes a colour-cycling H.264 test pattern and
//!   pushes timestamped NAL units into a [`VideoFrameSender`].
//! - [`TestAudioProducer`] — generates a sine-wave tone or streams samples
//!   from a WAV/raw-PCM file into an [`AudioSourceHandle`].
//!
//! Both producers run their loops on a **blocking thread** (CPU-bound work,
//! no async needed) and are started with
//! `tokio::task::spawn_blocking(|| producer.run(...))`.

pub mod audio;
pub mod video;

pub use audio::{
    LoopingWavStream, TestAudioProducer, ASSET_KICK_IN, ASSET_SNARE_UNDER, ASSET_SYNTH_01,
};
pub use video::TestVideoProducer;
