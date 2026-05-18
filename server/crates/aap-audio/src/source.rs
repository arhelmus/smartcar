//! Audio producer handle — the write-end of a PCM source.

use bytes::Bytes;
use tokio::sync::mpsc;

/// One chunk of raw S16LE PCM audio with a presentation timestamp.
///
/// Samples are interleaved across channels (left, right, left, right… for
/// stereo). The length of `samples` must be a multiple of
/// `channel_count × 2` bytes.
#[derive(Debug, Clone)]
pub struct PcmChunk {
    /// Presentation timestamp in microseconds (monotonically increasing).
    pub timestamp_us: u64,
    /// Raw S16LE samples, interleaved across channels.
    pub samples: Bytes,
}

/// Write-end of a PCM source channel.
///
/// Producers (testkit, Flutter audio callback) hold this handle and call
/// [`push`](Self::push) or [`blocking_push`](Self::blocking_push) for each
/// chunk they generate.
///
/// The handle is cheaply cloneable — multiple producers can share one source.
/// Pushes are fire-and-forget: if the internal channel buffer is full the
/// chunk is silently dropped, preserving real-time behaviour on the producer.
#[derive(Clone, Debug)]
pub struct AudioSourceHandle {
    tx: mpsc::Sender<PcmChunk>,
}

impl AudioSourceHandle {
    /// Push one chunk from an async context.
    ///
    /// Drops the chunk silently if the buffer is full.
    pub fn push(&self, chunk: PcmChunk) {
        let _ = self.tx.try_send(chunk);
    }

    /// Push one chunk from a blocking (non-async) thread.
    ///
    /// Drops the chunk silently if the buffer is full.
    pub fn blocking_push(&self, chunk: PcmChunk) {
        let _ = self.tx.try_send(chunk);
    }
}

/// Create a linked ([`AudioSourceHandle`], [`Receiver`](mpsc::Receiver)) pair.
///
/// `capacity` is the number of [`PcmChunk`]s the channel can buffer before
/// new pushes are dropped.  A value of 8–16 chunks (≈ 80–160 ms at 10 ms
/// chunk size) gives comfortable headroom without accumulating latency.
///
/// The `Receiver` is consumed by the mixer ([`MixerSink`]) or any other
/// downstream consumer.  The [`AudioSourceHandle`] is given to the producer.
pub fn audio_source(capacity: usize) -> (AudioSourceHandle, mpsc::Receiver<PcmChunk>) {
    let (tx, rx) = mpsc::channel(capacity);
    (AudioSourceHandle { tx }, rx)
}
