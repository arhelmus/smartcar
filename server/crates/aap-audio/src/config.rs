//! PCM stream configuration types.

/// Which Android Auto audio stream a channel carries.
///
/// Each variant maps to a distinct `AVChannel` in the service-discovery
/// descriptor, differentiated by the `audio_type` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioType {
    /// Music and media playback — 48 kHz stereo.
    Media,
    /// Navigation TTS / guidance prompts — 16 kHz mono.
    Speech,
    /// UI sounds and system notifications — 48 kHz stereo.
    System,
}

/// PCM format for one AA audio stream.
///
/// Values here mirror what gets advertised in the `AVChannel.audio_configs`
/// descriptor during service discovery and what the AV-channel handshake
/// (`SETUP_REQUEST` → `SETUP_RESPONSE`) negotiates with the head unit.
///
/// The per-stream rates/layouts produced by the constructors below are
/// defined to match openauto's hardcoded audio output formats — keep them
/// in sync with openauto rather than the source material.
#[derive(Debug, Clone)]
pub struct AudioStreamConfig {
    /// Sample rate in Hz (48 000 for media/system, 16 000 for speech).
    pub sample_rate: u32,
    /// Bits per sample — always 16 for Android Auto PCM.
    pub bit_depth: u32,
    /// Interleaved channel count (2 = stereo, 1 = mono).
    pub channel_count: u32,
    /// Which AA stream this config describes.
    pub audio_type: AudioType,
}

impl AudioStreamConfig {
    /// 48 kHz stereo — the primary media audio channel.
    pub fn media_audio() -> Self {
        Self {
            sample_rate: 48_000,
            bit_depth: 16,
            channel_count: 2,
            audio_type: AudioType::Media,
        }
    }

    /// 16 kHz mono — navigation guidance / TTS.
    pub fn speech_audio() -> Self {
        Self {
            sample_rate: 16_000,
            bit_depth: 16,
            channel_count: 1,
            audio_type: AudioType::Speech,
        }
    }

    /// 16 kHz mono — system sounds and notifications.
    ///
    /// openauto hardcodes its SystemAudio output at 16 kHz mono; use this
    /// rate so the samples reach the device at the correct pitch and speed.
    pub fn system_audio() -> Self {
        Self {
            sample_rate: 16_000,
            bit_depth: 16,
            channel_count: 1,
            audio_type: AudioType::System,
        }
    }

    /// Number of per-channel samples in a chunk of `duration_ms` milliseconds.
    pub fn samples_per_chunk(&self, duration_ms: u32) -> usize {
        (self.sample_rate * duration_ms / 1000) as usize
    }

    /// Total number of i16 values in a chunk of `duration_ms` milliseconds
    /// (`samples_per_chunk × channel_count`).
    pub fn frames_per_chunk(&self, duration_ms: u32) -> usize {
        self.samples_per_chunk(duration_ms) * self.channel_count as usize
    }
}
