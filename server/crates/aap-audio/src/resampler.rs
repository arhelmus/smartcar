//! Rate/channel conversion and the typed-pipe boundary.
//!
//! Two doors — and only these two — turn audio into a typed [`Pcm<F>`]:
//!
//! | Door | Input | Use |
//! |---|---|---|
//! | [`Normalizer<Out>`]   | [`RawPcm`] (runtime format) | foreign producers: decoders, FFI |
//! | [`ResampleStream<In, Out>`] | [`AudioStream<In>`] (typed) | rate-convert between known formats |
//!
//! [`Normalizer::accept`] is the single place every check the rest of the
//! pipeline elides lives: empty-buffer, ragged-frame, an *exhaustive* channel
//! adaptation, and sample-rate conversion.  It returns [`FormatError`] rather
//! than panicking so a misbehaving source is logged and dropped without
//! taking down the pipe.  The linear-interpolation kernel ([`RateConverter`])
//! is private and only ever driven by these two structs — never constructed
//! with loose integers by a caller.

use std::marker::PhantomData;

use crate::format::{AudioFormat, FormatError};
use crate::source::{Pcm, RawPcm};
use crate::stream::AudioStream;

// ── Channel adaptation ────────────────────────────────────────────────────────

/// Adapt interleaved i16 `samples` from `in_ch` to `out_ch` channels.
///
/// Exhaustive: only identity, mono→stereo (duplicate) and stereo→mono
/// (average) are defined.  Any other layout pair is a hard
/// [`FormatError::ChannelCount`] — never a silent reinterpretation.
fn adapt_channels(samples: Vec<i16>, in_ch: u32, out_ch: u32) -> Result<Vec<i16>, FormatError> {
    match (in_ch, out_ch) {
        (i, o) if i == o => Ok(samples),
        (1, 2) => Ok(samples.iter().flat_map(|&s| [s, s]).collect()),
        (2, 1) => Ok(samples
            .chunks_exact(2)
            .map(|c| ((c[0] as i32 + c[1] as i32) / 2) as i16)
            .collect()),
        _ => Err(FormatError::ChannelCount {
            got: in_ch,
            want: out_ch,
        }),
    }
}

// ── RateConverter kernel ──────────────────────────────────────────────────────

/// Stateful linear-interpolation sample-rate converter for one channel layout.
///
/// Carries the last input frame and the fractional read position across
/// [`process`](Self::process) calls so consecutive blocks join seamlessly.
/// Private: only [`Normalizer`] and [`ResampleStream`] build one, always from
/// [`AudioFormat`] constants, never from caller-supplied integers.
///
/// The kernel is linear interpolation: cheap, dependency-free, good enough
/// for looping test material.  It is **not** band-limited — swap the kernel
/// if production audio quality is needed.
struct RateConverter {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    /// Input frames advanced per output frame (`in_rate / out_rate`).
    ratio: f64,
    /// Last input frame seen (`channels` samples); silence until primed.
    last: Vec<i16>,
    /// Next output sample's source position. `0.0` == `last`, `1.0` == `input[0]`.
    pos: f64,
}

impl RateConverter {
    fn new(in_rate: u32, out_rate: u32, channels: u32) -> Self {
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
    fn process(&mut self, input: &[i16]) -> Vec<i16> {
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

// ── Normalizer<Out> — the runtime boundary ────────────────────────────────────

/// The single runtime boundary into the typed pipe.
///
/// Accepts [`RawPcm`] of *any* claimed format and yields [`Pcm<Out>`],
/// validating and converting in one place.  Resampler state is carried across
/// calls; if the input sample rate changes mid-stream the kernel is rebuilt.
pub struct Normalizer<Out: AudioFormat> {
    rate: Option<RateConverter>,
    /// Input rate the current `rate` kernel was built for.
    kernel_in_rate: u32,
    _out: PhantomData<Out>,
}

impl<Out: AudioFormat> Default for Normalizer<Out> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Out: AudioFormat> Normalizer<Out> {
    /// Create a boundary targeting `Out`.
    pub fn new() -> Self {
        Self {
            rate: None,
            kernel_in_rate: 0,
            _out: PhantomData,
        }
    }

    /// Validate `raw` against its claimed format, adapt channels, resample to
    /// `Out`'s rate, and return a typed chunk.
    ///
    /// Errors (never panics) on any contract violation so the caller at the
    /// boundary can log and drop the chunk.
    pub fn accept(&mut self, raw: RawPcm) -> Result<Pcm<Out>, FormatError> {
        if raw.samples.is_empty() {
            return Err(FormatError::Empty);
        }
        let in_ch = raw.spec.channel_count;
        if in_ch == 0 || raw.samples.len() % in_ch as usize != 0 {
            return Err(FormatError::RaggedFrame {
                len: raw.samples.len(),
                channel_count: in_ch,
            });
        }

        // Channels first, so the rate kernel runs at the target layout.
        let adapted = adapt_channels(raw.samples, in_ch, Out::CHANNEL_COUNT)?;

        let in_rate = raw.spec.sample_rate;
        let converted = if in_rate == Out::SAMPLE_RATE {
            adapted
        } else {
            if self.rate.is_none() || self.kernel_in_rate != in_rate {
                self.rate = Some(RateConverter::new(
                    in_rate,
                    Out::SAMPLE_RATE,
                    Out::CHANNEL_COUNT,
                ));
                self.kernel_in_rate = in_rate;
            }
            self.rate.as_mut().unwrap().process(&adapted)
        };

        if converted.is_empty() {
            return Err(FormatError::Empty);
        }
        Pcm::from_interleaved(raw.timestamp_us, converted)
    }
}

// ── ResampleStream<In, Out> — typed→typed ─────────────────────────────────────

/// Pull-based [`AudioStream`] decorator that rate-converts a typed inner
/// stream from `In` to `Out`.
///
/// Rate conversion only — `In` and `Out` must share a channel count
/// (channel adaptation is the [`Normalizer`]'s job).  This is checked with a
/// `debug_assert` in [`new`](Self::new); the rates come from the format
/// constants, so there are no loose integers to get wrong.
pub struct ResampleStream<In: AudioFormat, Out: AudioFormat> {
    inner: Box<dyn AudioStream<In>>,
    rate: RateConverter,
    timestamp_us: u64,
    _out: PhantomData<Out>,
}

impl<In: AudioFormat, Out: AudioFormat> ResampleStream<In, Out> {
    /// Wrap `inner` (producing `In`) and resample to `Out`'s rate.
    pub fn new(inner: Box<dyn AudioStream<In>>) -> Self {
        debug_assert_eq!(
            In::CHANNEL_COUNT,
            Out::CHANNEL_COUNT,
            "ResampleStream is rate-only; use Normalizer for channel adaptation"
        );
        Self {
            inner,
            rate: RateConverter::new(In::SAMPLE_RATE, Out::SAMPLE_RATE, In::CHANNEL_COUNT),
            timestamp_us: 0,
            _out: PhantomData,
        }
    }
}

impl<In: AudioFormat, Out: AudioFormat> AudioStream<Out> for ResampleStream<In, Out> {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<Pcm<Out>> {
        let chunk = self.inner.next_chunk(duration_ms)?;
        let out = self.rate.process(chunk.samples());
        if out.is_empty() {
            return None;
        }
        let ts = self.timestamp_us;
        self.timestamp_us += duration_ms as u64 * 1_000;
        Pcm::from_interleaved(ts, out).ok()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{FormatSpec, MediaFmt, SpeechFmt};

    // ── RateConverter kernel ──────────────────────────────────────────────────

    #[test]
    fn equal_rate_is_passthrough() {
        let mut r = RateConverter::new(48_000, 48_000, 1);
        let input: Vec<i16> = (0..160).collect();
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn upsample_lengthens_proportionally() {
        let mut r = RateConverter::new(44_100, 48_000, 1);
        let input: Vec<i16> = vec![1000; 441];
        let out = r.process(&input);
        assert!((out.len() as i32 - 480).abs() <= 2, "got {}", out.len());
    }

    #[test]
    fn downsample_shortens_proportionally() {
        let mut r = RateConverter::new(44_100, 16_000, 1);
        let input: Vec<i16> = vec![500; 441];
        let out = r.process(&input);
        assert!((out.len() as i32 - 160).abs() <= 2, "got {}", out.len());
    }

    #[test]
    fn constant_signal_is_preserved() {
        let mut r = RateConverter::new(44_100, 48_000, 1);
        for _ in 0..4 {
            let out = r.process(&vec![1234i16; 441]);
            assert!(out.iter().all(|&s| (s - 1234).abs() <= 1), "constant drift");
        }
    }

    #[test]
    fn stereo_channels_stay_independent() {
        let mut r = RateConverter::new(44_100, 48_000, 2);
        let input: Vec<i16> = (0..441).flat_map(|_| [100i16, -100i16]).collect();
        let out = r.process(&input);
        for pair in out.chunks_exact(2) {
            assert!((pair[0] - 100).abs() <= 1, "L drifted: {}", pair[0]);
            assert!((pair[1] + 100).abs() <= 1, "R drifted: {}", pair[1]);
        }
    }

    #[test]
    fn streaming_continuity_across_blocks() {
        let mut r = RateConverter::new(44_100, 48_000, 1);
        let b1: Vec<i16> = (0..441).collect();
        let b2: Vec<i16> = (441..882).collect();
        let mut out = r.process(&b1);
        out.extend(r.process(&b2));
        for w in out.windows(2) {
            assert!(w[1] >= w[0], "ramp not monotonic: {} -> {}", w[0], w[1]);
        }
    }

    // ── Channel adaptation ────────────────────────────────────────────────────

    #[test]
    fn mono_to_stereo_duplicates() {
        let out = adapt_channels(vec![1, 2, 3], 1, 2).unwrap();
        assert_eq!(out, vec![1, 1, 2, 2, 3, 3]);
    }

    #[test]
    fn stereo_to_mono_averages() {
        let out = adapt_channels(vec![10, 20, 30, 40], 2, 1).unwrap();
        assert_eq!(out, vec![15, 35]);
    }

    #[test]
    fn unconvertible_layout_errors() {
        let err = adapt_channels(vec![0; 12], 3, 2).unwrap_err();
        assert!(matches!(err, FormatError::ChannelCount { got: 3, want: 2 }));
    }

    // ── Normalizer boundary ───────────────────────────────────────────────────

    fn raw(spec_rate: u32, ch: u32, samples: Vec<i16>) -> RawPcm {
        RawPcm {
            spec: FormatSpec {
                sample_rate: spec_rate,
                bit_depth: 16,
                channel_count: ch,
                audio_type: crate::format::AudioType::Media,
            },
            timestamp_us: 0,
            samples,
        }
    }

    #[test]
    fn normalizer_rejects_empty() {
        let mut n = Normalizer::<MediaFmt>::new();
        assert!(matches!(
            n.accept(raw(48_000, 2, vec![])),
            Err(FormatError::Empty)
        ));
    }

    #[test]
    fn normalizer_rejects_ragged() {
        let mut n = Normalizer::<MediaFmt>::new();
        // 3 samples claimed as stereo → ragged.
        assert!(matches!(
            n.accept(raw(48_000, 2, vec![1, 2, 3])),
            Err(FormatError::RaggedFrame { .. })
        ));
    }

    #[test]
    fn normalizer_passthrough_same_format() {
        let mut n = Normalizer::<MediaFmt>::new();
        let pcm = n.accept(raw(48_000, 2, vec![5, 6, 7, 8])).unwrap();
        assert_eq!(pcm.samples(), &[5, 6, 7, 8]);
    }

    #[test]
    fn normalizer_adapts_channels_then_rate() {
        // Stereo 44.1k → SpeechFmt (mono 16k): averages then downsamples.
        let mut n = Normalizer::<SpeechFmt>::new();
        let input: Vec<i16> = (0..441).flat_map(|_| [200i16, 400i16]).collect();
        let out = n.accept(raw(44_100, 2, input)).unwrap();
        // ~160 mono frames near the (200+400)/2 = 300 average.
        assert!((out.frames() as i32 - 160).abs() <= 2);
        assert!(out.samples().iter().all(|&s| (s - 300).abs() <= 2));
    }

    #[test]
    fn resample_stream_typed_chain() {
        struct ConstIn(i16);
        impl AudioStream<MediaFmt> for ConstIn {
            fn next_chunk(&mut self, ms: u32) -> Option<Pcm<MediaFmt>> {
                let n = MediaFmt::spec().frames_per_chunk(ms);
                Pcm::from_interleaved(0, vec![self.0; n]).ok()
            }
        }
        // MediaFmt is 48k stereo; ResampleStream<MediaFmt, MediaFmt> is a
        // rate-identity passthrough that still type-checks.
        let mut s: ResampleStream<MediaFmt, MediaFmt> = ResampleStream::new(Box::new(ConstIn(777)));
        let c = s.next_chunk(10).unwrap();
        assert!(c.samples().iter().all(|&v| v == 777));
    }
}
