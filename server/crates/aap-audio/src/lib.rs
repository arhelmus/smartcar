//! Android Auto audio channel — types, mixer, and service.
//!
//! # Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | [`config`] | [`AudioStreamConfig`] and [`AudioType`] |
//! | [`source`] | [`PcmChunk`], [`AudioSourceHandle`], [`audio_source`] |
//! | [`stream`] | [`AudioStream`] trait and [`NullStream`] |
//! | [`mixer`] | [`MixerSink`] — ring-buffer multi-producer mixer |
//! | [`resampler`] | [`ResampleStream`] — pull-side sample-rate adapter |
//! | [`service`] | [`AudioService`] — AA channel service |
//!
//! # Typical wiring
//!
//! ```ignore
//! // 1. Build a mixer for each audio channel.
//! let mut media_mixer = MixerSink::new(AudioStreamConfig::media_audio());
//!
//! // 2. Register one or more producers.
//! let handle = media_mixer.add_source();      // give to TestAudioProducer / Flutter
//!
//! // 3. Create the service backed by the mixer.
//! let svc = AudioService::new(
//!     ChannelId::MediaAudio,
//!     AudioStreamConfig::media_audio(),
//!     Box::new(media_mixer),
//! );
//!
//! // 4. Register with the connection.
//! registry.register(svc);
//! ```

pub mod config;
pub mod convert;
pub mod mixer;
pub mod resampler;
pub mod service;
pub mod source;
pub mod stream;

pub use config::{AudioStreamConfig, AudioType};
pub use convert::{AudioSink, ConvertingHandle, InputFormat};
pub use mixer::MixerSink;
pub use resampler::{ResampleStream, Resampler};
pub use service::AudioService;
pub use source::{audio_source, AudioSourceHandle, PcmChunk};
pub use stream::{AudioStream, NullStream};
