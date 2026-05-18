//! [`MixerSink`] — pull-based multi-source audio mixer.
//!
//! # Design
//!
//! The mixer holds a list of [`AudioStream`] sources and pulls from every one
//! on each [`next_chunk`](AudioStream::next_chunk) call, summing the results.
//! The mixer itself implements [`AudioStream`] so it can be passed directly to
//! [`AudioService`](super::service::AudioService).
//!
//! Two kinds of source are supported:
//!
//! | Source kind | How to register | Driven by |
//! |---|---|---|
//! | **Pull** — generates on demand | [`add_stream`](MixerSink::add_stream) | mixer tick |
//! | **Push** — fed by an external callback | [`add_source`](MixerSink::add_source) | caller + internal buffer |
//!
//! Push sources (e.g. Flutter's audio callback) write into an
//! [`AudioSourceHandle`].  The handle feeds a small ring buffer inside a
//! [`PushStream`] which the mixer drains on every tick.

use std::collections::VecDeque;

use bytes::Bytes;
use tokio::sync::mpsc;

use super::config::AudioStreamConfig;
use super::convert::{ConvertingHandle, InputFormat};
use super::source::{audio_source, AudioSourceHandle, PcmChunk};
use super::stream::AudioStream;

/// Maximum audio buffered per push source before the oldest samples are dropped.
const OVERFLOW_CAP_MS: u32 = 200;

// ── PushStream ────────────────────────────────────────────────────────────────

/// [`AudioStream`] adapter for push-based sources.
///
/// Holds a small ring buffer that an [`AudioSourceHandle`] writes into.
/// [`next_chunk`](AudioStream::next_chunk) drains the channel and pops
/// the requested number of samples, padding with silence on underrun.
struct PushStream {
    rx: mpsc::Receiver<PcmChunk>,
    buf: VecDeque<i16>,
    overflow_cap: usize,
    config: AudioStreamConfig,
}

impl PushStream {
    fn new(rx: mpsc::Receiver<PcmChunk>, overflow_cap: usize, config: AudioStreamConfig) -> Self {
        Self {
            rx,
            buf: VecDeque::new(),
            overflow_cap,
            config,
        }
    }
}

impl AudioStream for PushStream {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<PcmChunk> {
        // Drain all pending chunks from the push side into the ring buffer.
        while let Ok(chunk) = self.rx.try_recv() {
            for pair in chunk.samples.chunks_exact(2) {
                if self.buf.len() < self.overflow_cap {
                    self.buf.push_back(i16::from_le_bytes([pair[0], pair[1]]));
                }
            }
        }

        if self.buf.is_empty() {
            return None;
        }

        let n = self.config.frames_per_chunk(duration_ms);
        let samples: Vec<u8> = (0..n)
            .flat_map(|_| self.buf.pop_front().unwrap_or(0).to_le_bytes())
            .collect();

        Some(PcmChunk {
            timestamp_us: 0,
            samples: Bytes::from(samples),
        })
    }
}

// ── MixerSink ─────────────────────────────────────────────────────────────────

/// Pull-based multi-source audio mixer implementing [`AudioStream`].
///
/// ```ignore
/// let mut mixer = MixerSink::new(AudioStreamConfig::media_audio());
///
/// // Pull source: generated on demand, no thread.
/// mixer.add_stream(Box::new(LoopingWavStream::from_embedded_wav(BYTES, cfg)?));
///
/// // Push source: Flutter / external callback writes here.
/// let handle = mixer.add_source();   // give AudioSourceHandle to Flutter
///
/// let svc = AudioService::new(ChannelId::MediaAudio, cfg, Box::new(mixer));
/// ```
pub struct MixerSink {
    sources: Vec<Box<dyn AudioStream>>,
    config: AudioStreamConfig,
    timestamp_us: u64,
}

impl MixerSink {
    /// Create an empty mixer for the given output format.
    pub fn new(config: AudioStreamConfig) -> Self {
        Self {
            sources: Vec::new(),
            config,
            timestamp_us: 0,
        }
    }

    /// The output format this mixer was configured with.
    pub fn config(&self) -> &AudioStreamConfig {
        &self.config
    }

    /// Register a pull-based source (e.g. a looping WAV or sine wave).
    ///
    /// The source is driven entirely by the mixer tick — no separate thread
    /// or channel is involved.
    pub fn add_stream(&mut self, stream: Box<dyn AudioStream>) {
        self.sources.push(stream);
    }

    /// Register a push-based source and return its write handle.
    ///
    /// The handle is given to an external producer (Flutter audio callback,
    /// test thread, …) which pushes [`PcmChunk`]s into it.  The mixer drains
    /// them on every tick via an internal ring buffer.
    pub fn add_source(&mut self) -> AudioSourceHandle {
        let overflow_cap = self.config.frames_per_chunk(OVERFLOW_CAP_MS);
        let (handle, rx) = audio_source(16);
        self.sources.push(Box::new(PushStream::new(
            rx,
            overflow_cap,
            self.config.clone(),
        )));
        handle
    }

    /// Like [`add_source`](Self::add_source) but wraps the handle in a
    /// [`ConvertingHandle`] that converts the source's channel layout to the
    /// mixer's output channel layout on every push.
    pub fn add_converting_source(&mut self, input: InputFormat) -> ConvertingHandle {
        let overflow_cap = self.config.frames_per_chunk(OVERFLOW_CAP_MS);
        let (handle, rx) = audio_source(16);
        self.sources.push(Box::new(PushStream::new(
            rx,
            overflow_cap,
            self.config.clone(),
        )));
        ConvertingHandle::new(handle, input, self.config.channel_count)
    }
}

impl AudioStream for MixerSink {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<PcmChunk> {
        let n = self.config.frames_per_chunk(duration_ms);
        let mut mixed = vec![0i32; n];
        let mut any_active = false;

        for source in &mut self.sources {
            if let Some(chunk) = source.next_chunk(duration_ms) {
                any_active = true;
                for (slot, pair) in mixed.iter_mut().zip(chunk.samples.chunks_exact(2)) {
                    *slot += i16::from_le_bytes([pair[0], pair[1]]) as i32;
                }
            }
        }

        if !any_active {
            return None;
        }

        let ts = self.timestamp_us;
        self.timestamp_us += duration_ms as u64 * 1_000;

        let samples: Vec<u8> = mixed
            .iter()
            .flat_map(|&s| (s.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes())
            .collect();

        Some(PcmChunk {
            timestamp_us: ts,
            samples: Bytes::from(samples),
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AudioStreamConfig;
    use crate::source::PcmChunk;
    use bytes::Bytes;
    use tokio::runtime::Runtime;

    fn mono_16k() -> AudioStreamConfig {
        AudioStreamConfig::speech_audio()
    }

    fn make_chunk(samples: &[i16], ts: u64) -> PcmChunk {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        PcmChunk {
            timestamp_us: ts,
            samples: Bytes::from(bytes),
        }
    }

    fn decode_chunk(chunk: &PcmChunk) -> Vec<i16> {
        chunk
            .samples
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    }

    #[test]
    fn empty_mixer_returns_none() {
        let mut mixer = MixerSink::new(mono_16k());
        assert!(mixer.next_chunk(10).is_none());
    }

    #[test]
    fn single_push_source_passthrough() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let mut mixer = MixerSink::new(mono_16k());
            let handle = mixer.add_source();

            let samples: Vec<i16> = (0..160).map(|i| i as i16).collect();
            handle.push(make_chunk(&samples, 0));

            let chunk = mixer.next_chunk(10).expect("should produce a chunk");
            assert_eq!(decode_chunk(&chunk), samples);
            assert_eq!(chunk.timestamp_us, 0);
        });
    }

    #[test]
    fn two_push_sources_are_summed_and_clamped() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let mut mixer = MixerSink::new(mono_16k());
            let h1 = mixer.add_source();
            let h2 = mixer.add_source();

            h1.push(make_chunk(&vec![i16::MAX; 160], 0));
            h2.push(make_chunk(&vec![i16::MAX; 160], 0));

            let chunk = mixer.next_chunk(10).unwrap();
            assert!(decode_chunk(&chunk).iter().all(|&s| s == i16::MAX));
        });
    }

    #[test]
    fn silent_push_source_returns_none() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let mut mixer = MixerSink::new(mono_16k());
            let _h = mixer.add_source(); // registered but never pushed to
            assert!(mixer.next_chunk(10).is_none());
        });
    }

    #[test]
    fn push_source_underrun_pads_with_silence() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let mut mixer = MixerSink::new(mono_16k());
            let handle = mixer.add_source();

            handle.push(make_chunk(&vec![100i16; 80], 0));

            let chunk = mixer.next_chunk(10).unwrap();
            let decoded = decode_chunk(&chunk);
            assert_eq!(decoded.len(), 160);
            assert!(decoded[..80].iter().all(|&s| s == 100));
            assert!(decoded[80..].iter().all(|&s| s == 0));
        });
    }

    #[test]
    fn timestamp_advances_per_chunk() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let mut mixer = MixerSink::new(mono_16k());
            let handle = mixer.add_source();

            handle.push(make_chunk(&vec![1i16; 160], 0));
            handle.push(make_chunk(&vec![1i16; 160], 10_000));

            let c1 = mixer.next_chunk(10).unwrap();
            let c2 = mixer.next_chunk(10).unwrap();
            assert_eq!(c1.timestamp_us, 0);
            assert_eq!(c2.timestamp_us, 10_000);
        });
    }

    #[test]
    fn pull_stream_mixed_with_push_source() {
        struct ConstStream(i16, AudioStreamConfig);
        impl AudioStream for ConstStream {
            fn next_chunk(&mut self, duration_ms: u32) -> Option<PcmChunk> {
                let n = self.1.frames_per_chunk(duration_ms);
                let bytes: Vec<u8> = std::iter::repeat(self.0)
                    .take(n)
                    .flat_map(|s| s.to_le_bytes())
                    .collect();
                Some(PcmChunk {
                    timestamp_us: 0,
                    samples: Bytes::from(bytes),
                })
            }
        }

        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let cfg = mono_16k();
            let mut mixer = MixerSink::new(cfg.clone());

            // Pull source: always outputs 100
            mixer.add_stream(Box::new(ConstStream(100, cfg.clone())));

            // Push source: outputs 200
            let handle = mixer.add_source();
            handle.push(make_chunk(&vec![200i16; 160], 0));

            let chunk = mixer.next_chunk(10).unwrap();
            // 100 + 200 = 300, within i16 range
            assert!(decode_chunk(&chunk).iter().all(|&s| s == 300));
        });
    }
}
