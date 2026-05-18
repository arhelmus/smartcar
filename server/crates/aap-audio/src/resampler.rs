//! [`ResampleStream`] — pull-side sample-rate adapter.
//!
//! Sits between a source and the mixer, pulling input-rate audio from an
//! inner [`AudioStream`] and emitting output-rate audio.  It implements
//! [`AudioStream`] itself, so it chains transparently:
//!
//! ```ignore
//! mixer.add_stream(Box::new(ResampleStream::new(
//!     Box::new(LoopingWavStream::from_embedded_wav(WAV_44K, in_cfg)?),
//!     in_cfg,                 // 44 100 Hz
//!     mixer.config().sample_rate,  // 48 000 Hz
//! )));
//! ```
//!
//! Rate conversion only — channel-count adaptation stays with the source
//! (e.g. [`LoopingWavStream`](../../aap_testkit/index.html)).  The kernel is
//! linear interpolation: cheap, dependency-free, and good enough for looping
//! test material.  It is **not** a band-limited resampler — swap the kernel
//! if production audio quality is needed.

use bytes::Bytes;

use super::config::AudioStreamConfig;
use super::source::PcmChunk;
use super::stream::AudioStream;

// ── Resampler kernel ──────────────────────────────────────────────────────────

/// Stateful linear-interpolation sample-rate converter for one channel layout.
///
/// Carries the last input frame and the fractional read position across
/// [`process`](Self::process) calls so consecutive blocks join seamlessly.
/// Not cheaply cloneable — holds per-stream filter state.
pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    /// Input frames advanced per output frame (`in_rate / out_rate`).
    ratio: f64,
    /// Last input frame seen so far (`channels` samples); silence until primed.
    last: Vec<i16>,
    /// Next output sample's source position. `0.0` == `last`, `1.0` == `input[0]`.
    pos: f64,
}

impl Resampler {
    /// Create a converter from `in_rate` to `out_rate` for `channels`-channel
    /// interleaved i16 audio.
    pub fn new(in_rate: u32, out_rate: u32, channels: u32) -> Self {
        let channels = channels as usize;
        Self {
            in_rate,
            out_rate,
            channels,
            ratio: in_rate as f64 / out_rate as f64,
            last: vec![0i16; channels],
            // Start at the first real input frame so the output doesn't open
            // with a synthetic silence sample blended from the zeroed `last`.
            pos: 1.0,
        }
    }

    /// Convert one block of interleaved i16 input to interleaved i16 output.
    ///
    /// Output length is approximately `input.len() * out_rate / in_rate`;
    /// the exact count varies per call because the fractional read position
    /// is carried over.
    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        if self.in_rate == self.out_rate {
            return input.to_vec();
        }
        let ch = self.channels;
        if ch == 0 || input.len() < ch {
            return Vec::new();
        }

        let n = input.len() / ch; // whole input frames

        // Virtual frame array V: V[0] = self.last, V[1..=n] = input frames.
        let sample_at = |idx: usize, c: usize| -> f64 {
            if idx == 0 {
                self.last[c] as f64
            } else {
                input[(idx - 1) * ch + c] as f64
            }
        };

        let mut out: Vec<i16> = Vec::with_capacity((n as f64 / self.ratio) as usize * ch + ch);
        let mut p = self.pos;
        // Interpolating at p needs V[floor(p)] and V[floor(p)+1]; the highest
        // valid right index is n, so produce while p < n.
        while p < n as f64 {
            let i0 = p.floor() as usize;
            let alpha = p - i0 as f64;
            for c in 0..ch {
                let a = sample_at(i0, c);
                let b = sample_at(i0 + 1, c);
                let v = a + (b - a) * alpha;
                out.push(v.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
            }
            p += self.ratio;
        }

        // Re-anchor for the next call: the last input frame becomes the new
        // `last` (position n in V), so shift the carried position by n.
        for c in 0..ch {
            self.last[c] = input[(n - 1) * ch + c];
        }
        self.pos = p - n as f64;

        out
    }
}

// ── ResampleStream ────────────────────────────────────────────────────────────

/// Pull-based [`AudioStream`] decorator that resamples an inner stream.
///
/// The inner stream must produce audio at `in_cfg`'s rate and channel count;
/// output is `out_rate` at the same channel count.  Manages its own
/// presentation timestamp (the mixer overrides it on mix anyway).
pub struct ResampleStream {
    inner: Box<dyn AudioStream>,
    resampler: Resampler,
    timestamp_us: u64,
}

impl ResampleStream {
    /// Wrap `inner` (producing `in_cfg`-rate audio) and resample to `out_rate`.
    pub fn new(inner: Box<dyn AudioStream>, in_cfg: AudioStreamConfig, out_rate: u32) -> Self {
        let resampler = Resampler::new(in_cfg.sample_rate, out_rate, in_cfg.channel_count);
        Self {
            inner,
            resampler,
            timestamp_us: 0,
        }
    }
}

impl AudioStream for ResampleStream {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<PcmChunk> {
        let chunk = self.inner.next_chunk(duration_ms)?;
        let input: Vec<i16> = chunk
            .samples
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();

        let out = self.resampler.process(&input);
        if out.is_empty() {
            return None;
        }

        let bytes: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
        let ts = self.timestamp_us;
        self.timestamp_us += duration_ms as u64 * 1_000;
        Some(PcmChunk {
            timestamp_us: ts,
            samples: Bytes::from(bytes),
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_rate_is_passthrough() {
        let mut r = Resampler::new(48_000, 48_000, 1);
        let input: Vec<i16> = (0..160).collect();
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn upsample_lengthens_proportionally() {
        let mut r = Resampler::new(44_100, 48_000, 1);
        // 441 frames @ 44.1k ≈ 480 frames @ 48k.
        let input: Vec<i16> = vec![1000; 441];
        let out = r.process(&input);
        assert!(
            (out.len() as i32 - 480).abs() <= 2,
            "expected ~480 samples, got {}",
            out.len()
        );
    }

    #[test]
    fn downsample_shortens_proportionally() {
        let mut r = Resampler::new(44_100, 16_000, 1);
        // 441 frames @ 44.1k ≈ 160 frames @ 16k.
        let input: Vec<i16> = vec![500; 441];
        let out = r.process(&input);
        assert!(
            (out.len() as i32 - 160).abs() <= 2,
            "expected ~160 samples, got {}",
            out.len()
        );
    }

    #[test]
    fn constant_signal_is_preserved() {
        let mut r = Resampler::new(44_100, 48_000, 1);
        for _ in 0..4 {
            let out = r.process(&vec![1234i16; 441]);
            assert!(out.iter().all(|&s| (s - 1234).abs() <= 1), "constant drift");
        }
    }

    #[test]
    fn stereo_channels_stay_independent() {
        let mut r = Resampler::new(44_100, 48_000, 2);
        // Interleaved L=100, R=-100.
        let input: Vec<i16> = (0..441).flat_map(|_| [100i16, -100i16]).collect();
        let out = r.process(&input);
        for pair in out.chunks_exact(2) {
            assert!((pair[0] - 100).abs() <= 1, "L drifted: {}", pair[0]);
            assert!((pair[1] + 100).abs() <= 1, "R drifted: {}", pair[1]);
        }
    }

    #[test]
    fn streaming_continuity_across_blocks() {
        // A ramp split across two blocks should stay monotonic through the seam.
        let mut r = Resampler::new(44_100, 48_000, 1);
        let b1: Vec<i16> = (0..441).collect();
        let b2: Vec<i16> = (441..882).collect();
        let mut out = r.process(&b1);
        out.extend(r.process(&b2));
        for w in out.windows(2) {
            assert!(
                w[1] >= w[0],
                "ramp not monotonic at seam: {} -> {}",
                w[0],
                w[1]
            );
        }
    }
}
