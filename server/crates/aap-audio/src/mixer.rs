//! [`MixerSink<F>`] — pull-based multi-source mixer, format-typed.
//!
//! The mixer holds a list of [`AudioStream<F>`] sources, pulls every one on
//! each tick, and sums them.  It implements [`AudioStream<F>`] itself so it
//! can be handed straight to an [`AudioService<F>`].  Every source is already
//! in the mixer's format `F`, so the inner loop is a plain i32 accumulate —
//! no byte reinterpretation, no per-source format guesswork.
//!
//! Three doors to register a source:
//!
//! | Door | Source kind | Conversion |
//! |---|---|---|
//! | [`add_stream`](MixerSink::add_stream)      | typed pull [`AudioStream<F>`] | none |
//! | [`add_source`](MixerSink::add_source)      | typed push ([`Sink<F>`])      | none |
//! | [`add_raw_source`](MixerSink::add_raw_source) | foreign push ([`RawSink<F>`]) | [`Normalizer<F>`] |
//!
//! [`AudioService<F>`]: crate::service::AudioService

use std::collections::VecDeque;

use tokio::sync::mpsc;
use tracing::warn;

use crate::format::AudioFormat;
use crate::resampler::Normalizer;
use crate::source::{audio_source, Pcm, RawPcm, Sink};
use crate::stream::AudioStream;

/// Maximum audio buffered per push source before the oldest samples drop.
const OVERFLOW_CAP_MS: u32 = 200;

/// Channel depth (in chunks) for push/raw sources.
const SOURCE_CHANNEL_CHUNKS: usize = 16;

// ── RawSink<F> ────────────────────────────────────────────────────────────────

/// Write-end of a *foreign* push source.
///
/// Producers whose format is only known at runtime (decoders, FFI callbacks)
/// hold this and push [`RawPcm`].  Conversion + validation happen on the
/// mixer side via a [`Normalizer<F>`]; contract violations are logged and
/// dropped, never propagated.  Cheaply cloneable; full buffer drops the chunk.
#[derive(Clone, Debug)]
pub struct RawSink<F: AudioFormat> {
    tx: mpsc::Sender<RawPcm>,
    _fmt: std::marker::PhantomData<F>,
}

impl<F: AudioFormat> RawSink<F> {
    /// Push one foreign chunk, dropping it if the buffer is full.
    pub fn push(&self, raw: RawPcm) {
        let _ = self.tx.try_send(raw);
    }
}

// ── PushStream<F> — typed push adapter ────────────────────────────────────────

/// [`AudioStream<F>`] adapter draining a typed [`Sink<F>`] into a ring buffer.
struct PushStream<F: AudioFormat> {
    rx: mpsc::Receiver<Pcm<F>>,
    buf: VecDeque<i16>,
    overflow_cap: usize,
}

impl<F: AudioFormat> AudioStream<F> for PushStream<F> {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<Pcm<F>> {
        while let Ok(chunk) = self.rx.try_recv() {
            for s in chunk.into_samples() {
                if self.buf.len() < self.overflow_cap {
                    self.buf.push_back(s);
                }
            }
        }
        drain_buffer::<F>(&mut self.buf, duration_ms)
    }
}

// ── RawPushStream<F> — foreign push adapter ───────────────────────────────────

/// [`AudioStream<F>`] adapter draining a [`RawSink<F>`] through a
/// [`Normalizer<F>`] into a ring buffer.  The normalizer is the runtime
/// boundary: bad foreign chunks are logged and dropped here.
struct RawPushStream<F: AudioFormat> {
    rx: mpsc::Receiver<RawPcm>,
    norm: Normalizer<F>,
    buf: VecDeque<i16>,
    overflow_cap: usize,
}

impl<F: AudioFormat> AudioStream<F> for RawPushStream<F> {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<Pcm<F>> {
        while let Ok(raw) = self.rx.try_recv() {
            match self.norm.accept(raw) {
                Ok(pcm) => {
                    for s in pcm.into_samples() {
                        if self.buf.len() < self.overflow_cap {
                            self.buf.push_back(s);
                        }
                    }
                }
                Err(e) => warn!(
                    error = %e,
                    channel = ?F::CHANNEL,
                    "audio: dropped foreign chunk failing the format contract"
                ),
            }
        }
        drain_buffer::<F>(&mut self.buf, duration_ms)
    }
}

/// Pop one `duration_ms` chunk from `buf`, padding underrun with silence.
/// Returns `None` only when the buffer is completely empty (no source active).
fn drain_buffer<F: AudioFormat>(buf: &mut VecDeque<i16>, duration_ms: u32) -> Option<Pcm<F>> {
    if buf.is_empty() {
        return None;
    }
    let n = F::spec().frames_per_chunk(duration_ms);
    let samples: Vec<i16> = (0..n).map(|_| buf.pop_front().unwrap_or(0)).collect();
    Pcm::from_interleaved(0, samples).ok()
}

// ── MixerSink<F> ──────────────────────────────────────────────────────────────

/// Pull-based multi-source mixer for format `F`.
pub struct MixerSink<F: AudioFormat> {
    sources: Vec<Box<dyn AudioStream<F>>>,
    timestamp_us: u64,
}

impl<F: AudioFormat> Default for MixerSink<F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F: AudioFormat> MixerSink<F> {
    /// Create an empty mixer.  The output format is `F` — no config argument.
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            timestamp_us: 0,
        }
    }

    fn overflow_cap() -> usize {
        F::spec().frames_per_chunk(OVERFLOW_CAP_MS)
    }

    /// Register a typed pull source (looping WAV, sine, …).
    pub fn add_stream(&mut self, stream: Box<dyn AudioStream<F>>) {
        self.sources.push(stream);
    }

    /// Register a typed push source; the producer already emits `F`.
    pub fn add_source(&mut self) -> Sink<F> {
        let (sink, rx) = audio_source::<F>(SOURCE_CHANNEL_CHUNKS);
        self.sources.push(Box::new(PushStream {
            rx,
            buf: VecDeque::new(),
            overflow_cap: Self::overflow_cap(),
        }));
        sink
    }

    /// Register a *foreign* push source.  Pushed [`RawPcm`] is validated and
    /// converted to `F` by an internal [`Normalizer<F>`]; chunks violating
    /// the format contract are logged and dropped.
    pub fn add_raw_source(&mut self) -> RawSink<F> {
        let (tx, rx) = mpsc::channel(SOURCE_CHANNEL_CHUNKS);
        self.sources.push(Box::new(RawPushStream {
            rx,
            norm: Normalizer::<F>::new(),
            buf: VecDeque::new(),
            overflow_cap: Self::overflow_cap(),
        }));
        RawSink {
            tx,
            _fmt: std::marker::PhantomData,
        }
    }
}

impl<F: AudioFormat> AudioStream<F> for MixerSink<F> {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<Pcm<F>> {
        let n = F::spec().frames_per_chunk(duration_ms);
        let mut mixed = vec![0i32; n];
        let mut any_active = false;

        for source in &mut self.sources {
            if let Some(chunk) = source.next_chunk(duration_ms) {
                any_active = true;
                for (slot, &s) in mixed.iter_mut().zip(chunk.samples()) {
                    *slot += s as i32;
                }
            }
        }

        if !any_active {
            return None;
        }

        let ts = self.timestamp_us;
        self.timestamp_us += duration_ms as u64 * 1_000;

        let samples: Vec<i16> = mixed
            .iter()
            .map(|&s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
            .collect();

        let mut pcm = Pcm::from_interleaved(ts, samples).ok()?;
        pcm.set_timestamp_us(ts);
        Some(pcm)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{FormatSpec, SpeechFmt};
    use crate::source::RawPcm;
    use tokio::runtime::Runtime;

    type F = SpeechFmt; // 16 kHz mono → 160 samples / 10 ms

    fn pcm(samples: &[i16], ts: u64) -> Pcm<F> {
        Pcm::from_interleaved(ts, samples.to_vec()).unwrap()
    }

    #[test]
    fn empty_mixer_returns_none() {
        let mut mixer = MixerSink::<F>::new();
        assert!(mixer.next_chunk(10).is_none());
    }

    #[test]
    fn single_push_source_passthrough() {
        Runtime::new().unwrap().block_on(async {
            let mut mixer = MixerSink::<F>::new();
            let sink = mixer.add_source();
            let samples: Vec<i16> = (0..160).collect();
            sink.push(pcm(&samples, 0));

            let out = mixer.next_chunk(10).expect("chunk");
            assert_eq!(out.samples(), samples.as_slice());
            assert_eq!(out.timestamp_us(), 0);
        });
    }

    #[test]
    fn two_push_sources_summed_and_clamped() {
        Runtime::new().unwrap().block_on(async {
            let mut mixer = MixerSink::<F>::new();
            let h1 = mixer.add_source();
            let h2 = mixer.add_source();
            h1.push(pcm(&vec![i16::MAX; 160], 0));
            h2.push(pcm(&vec![i16::MAX; 160], 0));

            let out = mixer.next_chunk(10).unwrap();
            assert!(out.samples().iter().all(|&s| s == i16::MAX));
        });
    }

    #[test]
    fn silent_push_source_returns_none() {
        Runtime::new().unwrap().block_on(async {
            let mut mixer = MixerSink::<F>::new();
            let _h = mixer.add_source();
            assert!(mixer.next_chunk(10).is_none());
        });
    }

    #[test]
    fn push_source_underrun_pads_with_silence() {
        Runtime::new().unwrap().block_on(async {
            let mut mixer = MixerSink::<F>::new();
            let sink = mixer.add_source();
            sink.push(pcm(&[100i16; 80], 0));

            let out = mixer.next_chunk(10).unwrap();
            let s = out.samples();
            assert_eq!(s.len(), 160);
            assert!(s[..80].iter().all(|&v| v == 100));
            assert!(s[80..].iter().all(|&v| v == 0));
        });
    }

    #[test]
    fn mixer_restamps_per_chunk() {
        Runtime::new().unwrap().block_on(async {
            let mut mixer = MixerSink::<F>::new();
            let sink = mixer.add_source();
            sink.push(pcm(&vec![1i16; 160], 999)); // producer ts ignored
            sink.push(pcm(&vec![1i16; 160], 999));

            let c1 = mixer.next_chunk(10).unwrap();
            let c2 = mixer.next_chunk(10).unwrap();
            assert_eq!(c1.timestamp_us(), 0);
            assert_eq!(c2.timestamp_us(), 10_000);
        });
    }

    #[test]
    fn raw_source_normalized_and_mixed() {
        Runtime::new().unwrap().block_on(async {
            let mut mixer = MixerSink::<F>::new();
            let raw = mixer.add_raw_source();
            // Stereo 44.1k foreign input → adapted to mono 16k by Normalizer.
            let input: Vec<i16> = (0..441).flat_map(|_| [300i16, 500i16]).collect();
            raw.push(RawPcm {
                spec: FormatSpec {
                    sample_rate: 44_100,
                    bit_depth: 16,
                    channel_count: 2,
                    audio_type: crate::format::AudioType::Speech,
                },
                timestamp_us: 0,
                samples: input,
            });

            let out = mixer.next_chunk(10).unwrap();
            // (300+500)/2 = 400, downsampled 44.1k→16k.
            assert!(out.samples().iter().all(|&s| (s - 400).abs() <= 3));
        });
    }

    #[test]
    fn raw_source_bad_chunk_dropped_not_fatal() {
        Runtime::new().unwrap().block_on(async {
            let mut mixer = MixerSink::<F>::new();
            let raw = mixer.add_raw_source();
            // 3 samples claimed stereo → ragged → dropped, mixer stays silent.
            raw.push(RawPcm {
                spec: FormatSpec {
                    sample_rate: 16_000,
                    bit_depth: 16,
                    channel_count: 2,
                    audio_type: crate::format::AudioType::Speech,
                },
                timestamp_us: 0,
                samples: vec![1, 2, 3],
            });
            assert!(mixer.next_chunk(10).is_none());
        });
    }

    #[test]
    fn pull_stream_mixed_with_push_source() {
        struct ConstStream(i16);
        impl AudioStream<F> for ConstStream {
            fn next_chunk(&mut self, ms: u32) -> Option<Pcm<F>> {
                let n = F::spec().frames_per_chunk(ms);
                Pcm::from_interleaved(0, vec![self.0; n]).ok()
            }
        }
        Runtime::new().unwrap().block_on(async {
            let mut mixer = MixerSink::<F>::new();
            mixer.add_stream(Box::new(ConstStream(100)));
            let sink = mixer.add_source();
            sink.push(pcm(&vec![200i16; 160], 0));

            let out = mixer.next_chunk(10).unwrap();
            assert!(out.samples().iter().all(|&s| s == 300));
        });
    }
}
