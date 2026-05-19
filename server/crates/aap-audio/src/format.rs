//! Audio format as a type — the single source of truth for PCM layout.
//!
//! Every buffer in the pipe is tagged with a format type implementing
//! [`AudioFormat`].  The format's sample rate, channel count, AA stream type
//! and wire channel are *associated constants*, so a mismatch (wrong rate,
//! wrong channel layout, Speech config on the Media channel) is a compile
//! error rather than a runtime corruption.
//!
//! [`FormatSpec`] is the erased, runtime-valued view of a format — used only
//! where the format must cross a dynamic boundary (the protobuf
//! `AudioConfig` descriptor and foreign input via [`RawPcm`]).
//!
//! [`RawPcm`]: crate::source::RawPcm

use aap_contracts::ChannelId;

// ── AudioType ─────────────────────────────────────────────────────────────────

/// Which Android Auto audio stream a channel carries.
///
/// Each variant maps to a distinct `AVChannel` in the service-discovery
/// descriptor, differentiated by the `audio_type` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioType {
    /// Music and media playback.
    Media,
    /// Navigation TTS / guidance prompts.
    Speech,
    /// UI sounds and system notifications.
    System,
}

// ── AudioFormat ───────────────────────────────────────────────────────────────

/// A PCM audio format expressed in the type system.
///
/// Implemented by zero-sized marker types ([`MediaFmt`], [`SpeechFmt`],
/// [`SystemFmt`]).  The trait is intentionally open: tests or future codecs
/// may define their own formats.  Every constant is fixed at compile time so
/// the rest of the pipeline can be generic over `F: AudioFormat` and never
/// re-check the layout.
///
/// Android Auto PCM is always signed 16-bit little-endian; [`BIT_DEPTH`]
/// defaults to 16 and should not be overridden.
///
/// [`BIT_DEPTH`]: AudioFormat::BIT_DEPTH
pub trait AudioFormat: 'static + Send + Sync {
    /// Sample rate in Hz.
    const SAMPLE_RATE: u32;
    /// Interleaved channel count (1 = mono, 2 = stereo).
    const CHANNEL_COUNT: u32;
    /// Bits per sample — always 16 for Android Auto PCM.
    const BIT_DEPTH: u32 = 16;
    /// Which AA stream this format describes.
    const AUDIO_TYPE: AudioType;
    /// The wire channel this format is streamed on.
    const CHANNEL: ChannelId;

    /// Erase the compile-time constants into a runtime [`FormatSpec`].
    fn spec() -> FormatSpec {
        FormatSpec {
            sample_rate: Self::SAMPLE_RATE,
            bit_depth: Self::BIT_DEPTH,
            channel_count: Self::CHANNEL_COUNT,
            audio_type: Self::AUDIO_TYPE,
        }
    }
}

/// Media audio — 48 kHz stereo, `ChannelId::MediaAudio`.
#[derive(Debug, Clone, Copy)]
pub struct MediaFmt;
impl AudioFormat for MediaFmt {
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNEL_COUNT: u32 = 2;
    const AUDIO_TYPE: AudioType = AudioType::Media;
    const CHANNEL: ChannelId = ChannelId::MediaAudio;
}

/// Speech / navigation TTS — 16 kHz mono, `ChannelId::SpeechAudio`.
#[derive(Debug, Clone, Copy)]
pub struct SpeechFmt;
impl AudioFormat for SpeechFmt {
    const SAMPLE_RATE: u32 = 16_000;
    const CHANNEL_COUNT: u32 = 1;
    const AUDIO_TYPE: AudioType = AudioType::Speech;
    const CHANNEL: ChannelId = ChannelId::SpeechAudio;
}

/// System sounds / notifications — 16 kHz mono, `ChannelId::SystemAudio`.
///
/// openauto hardcodes its SystemAudio output at 16 kHz mono; this format
/// matches so samples reach the device at the correct pitch and speed.
#[derive(Debug, Clone, Copy)]
pub struct SystemFmt;
impl AudioFormat for SystemFmt {
    const SAMPLE_RATE: u32 = 16_000;
    const CHANNEL_COUNT: u32 = 1;
    const AUDIO_TYPE: AudioType = AudioType::System;
    const CHANNEL: ChannelId = ChannelId::SystemAudio;
}

// ── FormatSpec ────────────────────────────────────────────────────────────────

/// Runtime-valued view of an [`AudioFormat`].
///
/// Obtained from [`AudioFormat::spec`].  Used only where the format must be a
/// value rather than a type: building the `AVChannel.audio_configs` protobuf
/// descriptor, and describing the *claimed* format of foreign input
/// ([`RawPcm`](crate::source::RawPcm)) at the [`Normalizer`] boundary.
///
/// [`Normalizer`]: crate::resampler::Normalizer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FormatSpec {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Bits per sample — always 16 for Android Auto PCM.
    pub bit_depth: u32,
    /// Interleaved channel count (1 = mono, 2 = stereo).
    pub channel_count: u32,
    /// Which AA stream this config describes.
    pub audio_type: AudioType,
}

impl FormatSpec {
    /// Number of per-channel samples in a chunk of `duration_ms` ms.
    pub fn samples_per_chunk(&self, duration_ms: u32) -> usize {
        (self.sample_rate * duration_ms / 1000) as usize
    }

    /// Total number of interleaved i16 values in a chunk of `duration_ms` ms
    /// (`samples_per_chunk × channel_count`).
    pub fn frames_per_chunk(&self, duration_ms: u32) -> usize {
        self.samples_per_chunk(duration_ms) * self.channel_count as usize
    }
}

// ── FormatError ───────────────────────────────────────────────────────────────

/// A format-contract violation detected at the [`Normalizer`] boundary.
///
/// Returned (never panicked) so a misbehaving foreign producer is logged and
/// dropped without taking down the audio pipe.
///
/// [`Normalizer`]: crate::resampler::Normalizer
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    /// Sample count is not a whole number of interleaved frames.
    #[error("ragged frame: {len} samples not divisible by {channel_count} channels")]
    RaggedFrame {
        /// Total interleaved sample count received.
        len: usize,
        /// Channel count the buffer was claimed to be in.
        channel_count: u32,
    },
    /// The input channel layout cannot be adapted to the target
    /// (only mono↔stereo and identity are supported).
    #[error("unconvertible channel count: got {got}, want {want}")]
    ChannelCount {
        /// Channel count of the input.
        got: u32,
        /// Channel count the target format requires.
        want: u32,
    },
    /// The buffer carried no samples (or rate conversion produced none).
    #[error("empty audio buffer")]
    Empty,
}
