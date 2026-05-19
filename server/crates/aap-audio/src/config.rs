//! Backwards-compatible runtime-config shims.
//!
//! The source of truth for audio formats is now the type-level
//! [`AudioFormat`](crate::format::AudioFormat) trait and its marker types.
//! [`AudioStreamConfig`] is retained as an alias of the erased
//! [`FormatSpec`](crate::format::FormatSpec) for callers that still want a
//! runtime value, with the original per-stream constructors.

use crate::format::{AudioFormat, FormatSpec, MediaFmt, SpeechFmt, SystemFmt};

/// Erased, runtime-valued audio format. Alias of [`FormatSpec`].
pub type AudioStreamConfig = FormatSpec;

impl FormatSpec {
    /// 48 kHz stereo — the primary media audio channel.
    pub fn media_audio() -> Self {
        MediaFmt::spec()
    }

    /// 16 kHz mono — navigation guidance / TTS.
    pub fn speech_audio() -> Self {
        SpeechFmt::spec()
    }

    /// 16 kHz mono — system sounds and notifications.
    pub fn system_audio() -> Self {
        SystemFmt::spec()
    }
}
