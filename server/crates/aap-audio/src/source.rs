//! Typed PCM buffer ([`Pcm`]), foreign input ([`RawPcm`]), and the push
//! handle ([`Sink`]).
//!
//! [`Pcm<F>`] is the only buffer that flows through the mixer and service:
//! its format is proven by the type `F`, its samples are decoded i16 (never
//! raw bytes), and the interleave invariant is checked exactly once at
//! construction.  Foreign audio — decoders, FFI callbacks, test fixtures —
//! enters as [`RawPcm`], which has no [`AudioStream`] impl and can only reach
//! the pipe through the [`Normalizer`] boundary.
//!
//! [`AudioStream`]: crate::stream::AudioStream
//! [`Normalizer`]: crate::resampler::Normalizer

use std::marker::PhantomData;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::format::{AudioFormat, FormatError, FormatSpec};

// ── Pcm<F> ────────────────────────────────────────────────────────────────────

/// A chunk of interleaved S16 PCM whose format is proven by the type `F`.
///
/// Samples are decoded `i16`, interleaved across channels (L, R, L, R… for
/// stereo).  The length is always a whole multiple of `F::CHANNEL_COUNT`;
/// this invariant is established by [`from_interleaved`](Self::from_interleaved)
/// and preserved by every transform.
///
/// The only ways to obtain one are [`from_interleaved`](Self::from_interleaved)
/// / [`silence`](Self::silence), or as the output of a typed
/// [`AudioStream<F>`](crate::stream::AudioStream).  There is no constructor
/// that takes a format-blind byte buffer.
#[derive(Debug, Clone)]
pub struct Pcm<F: AudioFormat> {
    timestamp_us: u64,
    samples: Vec<i16>,
    _fmt: PhantomData<F>,
}

impl<F: AudioFormat> Pcm<F> {
    /// Build a chunk from interleaved i16 samples already in `F`'s layout.
    ///
    /// Errors with [`FormatError::RaggedFrame`] if `samples.len()` is not a
    /// whole number of interleaved frames.  An empty buffer is accepted (0 is
    /// a valid frame count); emptiness is the [`Normalizer`]'s concern.
    ///
    /// [`Normalizer`]: crate::resampler::Normalizer
    pub fn from_interleaved(timestamp_us: u64, samples: Vec<i16>) -> Result<Self, FormatError> {
        let ch = F::CHANNEL_COUNT as usize;
        if ch == 0 || samples.len() % ch != 0 {
            return Err(FormatError::RaggedFrame {
                len: samples.len(),
                channel_count: F::CHANNEL_COUNT,
            });
        }
        Ok(Self {
            timestamp_us,
            samples,
            _fmt: PhantomData,
        })
    }

    /// A silent chunk of `duration_ms` ms in `F`'s format.
    ///
    /// Used to pad push-source underruns without leaving the typed pipe.
    pub fn silence(timestamp_us: u64, duration_ms: u32) -> Self {
        Self {
            timestamp_us,
            samples: vec![0i16; F::spec().frames_per_chunk(duration_ms)],
            _fmt: PhantomData,
        }
    }

    /// Presentation timestamp in microseconds.
    pub fn timestamp_us(&self) -> u64 {
        self.timestamp_us
    }

    /// Overwrite the presentation timestamp (the mixer re-stamps on mix).
    pub fn set_timestamp_us(&mut self, timestamp_us: u64) {
        self.timestamp_us = timestamp_us;
    }

    /// Interleaved i16 samples in `F`'s channel layout.
    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    /// Mutable interleaved samples (the mixer accumulates in place).
    pub fn samples_mut(&mut self) -> &mut [i16] {
        &mut self.samples
    }

    /// Consume the chunk, yielding its sample buffer.
    pub fn into_samples(self) -> Vec<i16> {
        self.samples
    }

    /// Number of interleaved frames (`samples.len() / F::CHANNEL_COUNT`).
    pub fn frames(&self) -> usize {
        self.samples.len() / F::CHANNEL_COUNT as usize
    }

    /// Serialize to S16LE wire bytes.  Used only at the wire edge
    /// (the AV-channel media frame).
    pub fn to_le_bytes(&self) -> Bytes {
        let mut buf = Vec::with_capacity(self.samples.len() * 2);
        for s in &self.samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        Bytes::from(buf)
    }
}

// ── RawPcm ────────────────────────────────────────────────────────────────────

/// Foreign PCM whose format is known only at runtime.
///
/// Produced by decoders (WAV, …) and FFI callbacks (Flutter).  It carries its
/// *claimed* [`FormatSpec`] as data and deliberately has **no**
/// [`AudioStream`](crate::stream::AudioStream) impl: the only way it reaches
/// the mixer/service is through [`Normalizer::accept`], which validates the
/// claim and converts it into a typed [`Pcm`].
///
/// [`Normalizer::accept`]: crate::resampler::Normalizer::accept
#[derive(Debug, Clone)]
pub struct RawPcm {
    /// The format the producer claims `samples` are in.
    pub spec: FormatSpec,
    /// Presentation timestamp in microseconds.
    pub timestamp_us: u64,
    /// Interleaved i16 samples, claimed to match `spec`.
    pub samples: Vec<i16>,
}

// ── Sink<F> ───────────────────────────────────────────────────────────────────

/// Write-end of a typed push source.
///
/// Producers already emitting `F`-format audio hold this and call
/// [`push`](Self::push) per chunk.  Cheaply cloneable; pushes are
/// fire-and-forget — if the buffer is full the chunk is dropped to preserve
/// real-time behaviour on the producer side.
#[derive(Clone, Debug)]
pub struct Sink<F: AudioFormat> {
    tx: mpsc::Sender<Pcm<F>>,
}

impl<F: AudioFormat> Sink<F> {
    /// Push one typed chunk, dropping it if the buffer is full.
    pub fn push(&self, chunk: Pcm<F>) {
        let _ = self.tx.try_send(chunk);
    }
}

/// Create a linked ([`Sink<F>`], [`Receiver`](mpsc::Receiver)) pair.
///
/// `capacity` is the number of chunks buffered before pushes are dropped.
/// 8–16 chunks (≈ 80–160 ms at 10 ms chunks) gives headroom without latency.
pub fn audio_source<F: AudioFormat>(capacity: usize) -> (Sink<F>, mpsc::Receiver<Pcm<F>>) {
    let (tx, rx) = mpsc::channel(capacity);
    (Sink { tx }, rx)
}
