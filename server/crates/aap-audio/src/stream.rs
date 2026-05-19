//! [`AudioStream<F>`] — pull interface for a format-typed PCM source.

use crate::format::AudioFormat;
use crate::source::Pcm;

/// Pull interface implemented by any source an [`AudioService`] can drive.
///
/// The type parameter `F` pins the format: a stream can only feed a mixer or
/// service of the *same* `F`, so a rate/layout mismatch is a compile error.
///
/// On each tick the consumer calls [`next_chunk`](AudioStream::next_chunk).
/// Returning `None` means every upstream source is silent this tick; the
/// caller should send nothing (not silence frames) so the head unit's jitter
/// buffer absorbs the gap.
///
/// [`AudioService`]: crate::service::AudioService
pub trait AudioStream<F: AudioFormat>: Send {
    /// Produce the next chunk of `duration_ms` ms of audio, or `None` when
    /// every upstream source is silent this tick.
    fn next_chunk(&mut self, duration_ms: u32) -> Option<Pcm<F>>;
}

/// No-op stream — always silent.  Used when output is intentionally disabled
/// without removing the service from the registry.
pub struct NullStream;

impl<F: AudioFormat> AudioStream<F> for NullStream {
    fn next_chunk(&mut self, _duration_ms: u32) -> Option<Pcm<F>> {
        None
    }
}
