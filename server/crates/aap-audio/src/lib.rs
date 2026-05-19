//! Android Auto audio channel — format-typed pipeline.
//!
//! The audio format is carried in the type system: every buffer is a
//! [`Pcm<F>`] whose layout is proven by `F: AudioFormat`.  A rate, channel
//! or stream mismatch is a compile error.  The *only* runtime boundary is
//! [`Normalizer`], where foreign audio ([`RawPcm`]) is validated and
//! converted into the typed pipe.
//!
//! # Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | [`format`] | [`AudioFormat`], markers, [`FormatSpec`], [`FormatError`] |
//! | [`source`] | [`Pcm<F>`], [`RawPcm`], [`Sink<F>`] |
//! | [`stream`] | [`AudioStream<F>`] trait, [`NullStream`] |
//! | [`resampler`] | [`Normalizer`] (boundary), [`ResampleStream`] |
//! | [`mixer`] | [`MixerSink<F>`] — multi-source mixer + [`RawSink<F>`] |
//! | [`service`] | [`AudioService<F>`] — AA channel service |
//! | [`config`] | [`AudioStreamConfig`] back-compat alias |
//!
//! # Typical wiring
//!
//! ```ignore
//! let mut mixer = MixerSink::<MediaFmt>::new();
//! let raw_sink = mixer.add_raw_source();          // foreign producer door
//! let svc = AudioService::new(Box::new(mixer));   // channel = MediaFmt::CHANNEL
//! registry.register(svc);
//! ```

pub mod config;
pub mod format;
pub mod mixer;
pub mod resampler;
pub mod service;
pub mod source;
pub mod stream;

pub use config::AudioStreamConfig;
pub use format::{AudioFormat, AudioType, FormatError, FormatSpec, MediaFmt, SpeechFmt, SystemFmt};
pub use mixer::{MixerSink, RawSink};
pub use resampler::{Normalizer, ResampleStream};
pub use service::AudioService;
pub use source::{audio_source, Pcm, RawPcm, Sink};
pub use stream::{AudioStream, NullStream};
