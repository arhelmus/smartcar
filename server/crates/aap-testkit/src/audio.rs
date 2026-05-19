//! PCM test-audio source.
//!
//! [`LoopingWavStream<F>`] is a pull-based [`AudioStream<F>`]: it decodes an
//! embedded WAV once at its *native* rate and channel count, runs the whole
//! buffer through the [`Normalizer<F>`] boundary a single time to obtain
//! samples in `F`'s format, then loops them.  No thread, no channel, no
//! timing drift — the mixer tick drives sample generation synchronously.  All
//! rate/channel conversion lives in the [`Normalizer`]; the testkit performs
//! none itself.
//!
//! # Embedded assets (all 44 100 Hz, 24-bit)
//!
//! | Constant | File | Layout |
//! |---|---|---|
//! | [`ASSET_KICK_IN`]     | `01_KickIn.wav`    | mono |
//! | [`ASSET_SYNTH_01`]    | `16_Synth01.wav`   | stereo |
//! | [`ASSET_SNARE_UNDER`] | `05_SnareUnder.wav`| mono |

use std::{io::Cursor, marker::PhantomData};

use anyhow::Context as _;
use tracing::info;

use aap_audio::{AudioFormat, AudioStream, FormatSpec, Normalizer, Pcm, RawPcm};

// ── Embedded assets ───────────────────────────────────────────────────────────

/// Embedded kick-drum sample: 24-bit PCM, mono, 44 100 Hz.
pub static ASSET_KICK_IN: &[u8] = include_bytes!("../assets/01_KickIn.wav");

/// Embedded synth loop: 24-bit PCM, stereo, 44 100 Hz.
pub static ASSET_SYNTH_01: &[u8] = include_bytes!("../assets/16_Synth01.wav");

/// Embedded snare sample: 24-bit PCM, mono, 44 100 Hz.
pub static ASSET_SNARE_UNDER: &[u8] = include_bytes!("../assets/05_SnareUnder.wav");

// ── LoopingWavStream<F> ───────────────────────────────────────────────────────

/// Pull-based looping WAV source producing format `F`.
///
/// The WAV is decoded and normalized into `F` once at construction; thereafter
/// `next_chunk` just slices the cached loop.
pub struct LoopingWavStream<F: AudioFormat> {
    /// Pre-decoded, normalized samples in `F`'s format (the full loop).
    samples: Vec<i16>,
    cursor: usize,
    timestamp_us: u64,
    _fmt: PhantomData<F>,
}

impl<F: AudioFormat> LoopingWavStream<F> {
    /// Load an embedded WAV (`include_bytes!`) into a looping `F` stream.
    pub fn from_embedded_wav(bytes: &'static [u8]) -> anyhow::Result<Self> {
        let (samples, channels, rate) = decode_wav(Cursor::new(bytes))?;
        Self::normalize(samples, channels, rate)
    }

    /// Load an external WAV file into a looping `F` stream.
    pub fn from_wav(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path.as_ref())?;
        let (samples, channels, rate) = decode_wav(std::io::BufReader::new(file))?;
        Self::normalize(samples, channels, rate)
    }

    /// Run the decoded buffer through the [`Normalizer<F>`] boundary once.
    fn normalize(samples: Vec<i16>, channels: u32, sample_rate: u32) -> anyhow::Result<Self> {
        let raw = RawPcm {
            spec: FormatSpec {
                sample_rate,
                bit_depth: 16,
                channel_count: channels,
                audio_type: F::AUDIO_TYPE,
            },
            timestamp_us: 0,
            samples,
        };
        let pcm = Normalizer::<F>::new()
            .accept(raw)
            .context("normalizing test WAV into the target audio format")?;
        Ok(Self {
            samples: pcm.into_samples(),
            cursor: 0,
            timestamp_us: 0,
            _fmt: PhantomData,
        })
    }
}

impl<F: AudioFormat> AudioStream<F> for LoopingWavStream<F> {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<Pcm<F>> {
        if self.samples.is_empty() {
            return None;
        }
        let n = F::spec().frames_per_chunk(duration_ms);
        let mut chunk = Vec::with_capacity(n);
        for _ in 0..n {
            if self.cursor >= self.samples.len() {
                self.cursor = 0;
            }
            chunk.push(self.samples[self.cursor]);
            self.cursor += 1;
        }
        let ts = self.timestamp_us;
        self.timestamp_us += duration_ms as u64 * 1_000;
        Pcm::from_interleaved(ts, chunk).ok()
    }
}

// ── WAV decoding ──────────────────────────────────────────────────────────────

/// Decode a WAV to interleaved i16 at its native rate/layout.
///
/// Returns `(samples, channel_count, sample_rate)`.  Bit depth is reduced to
/// 16; rate and channel count are passed through untouched — the
/// [`Normalizer`] does any conversion.
fn decode_wav<R: std::io::Read + std::io::Seek>(reader: R) -> anyhow::Result<(Vec<i16>, u32, u32)> {
    let mut wav = hound::WavReader::new(reader)?;
    let spec = wav.spec();

    let samples: Vec<i16> = if spec.bits_per_sample == 16 {
        wav.samples::<i16>().collect::<Result<_, _>>()?
    } else {
        let shift = spec.bits_per_sample - 16;
        wav.samples::<i32>()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|s| (s >> shift) as i16)
            .collect()
    };

    info!(
        sample_rate = spec.sample_rate,
        channels = spec.channels,
        bits = spec.bits_per_sample,
        total_samples = samples.len(),
        "decoded WAV"
    );

    Ok((samples, spec.channels as u32, spec.sample_rate))
}
