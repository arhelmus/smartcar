//! `AudioStream` — pull interface for a mixed PCM source.

use super::source::PcmChunk;

/// Pull interface implemented by any audio source an [`AudioService`] can drive.
///
/// On each tick the service calls [`next_chunk`](AudioStream::next_chunk) to
/// obtain the next slice of audio.  Returning `None` means all sources are
/// currently silent; the caller should send nothing (not silence frames) so
/// the head unit's own jitter buffer handles the gap.
pub trait AudioStream: Send {
    /// Produce the next chunk of `duration_ms` milliseconds of audio.
    ///
    /// Returns `None` when every upstream source is silent this tick.
    fn next_chunk(&mut self, duration_ms: u32) -> Option<PcmChunk>;
}

/// No-op stream — always silent.  Used when audio output is intentionally
/// disabled without removing the service from the registry.
pub struct NullStream;

impl AudioStream for NullStream {
    fn next_chunk(&mut self, _duration_ms: u32) -> Option<PcmChunk> {
        None
    }
}
