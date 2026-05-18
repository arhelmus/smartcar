//! PCM test-audio sources.
//!
//! # Pull source (preferred)
//!
//! [`LoopingWavStream`] implements [`AudioStream`] directly — it generates
//! samples synchronously when the mixer calls `next_chunk()`.  No thread,
//! no channel, no timing drift.  Add it to a [`MixerSink`] with
//! [`add_stream`](aap_audio::MixerSink::add_stream) or pass it straight to
//! [`AudioService`](aap_audio::AudioService).
//!
//! # Push source (for external callbacks)
//!
//! [`TestAudioProducer`] runs on a blocking thread and pushes chunks through
//! an [`AudioSink`].  Use this when the audio rate is dictated by an external
//! source (Flutter engine callback, microphone capture, …).
//!
//! # Embedded assets
//!
//! | Constant | File | Format |
//! |---|---|---|
//! | [`ASSET_KICK_IN`]     | `01_KickIn.wav`    | 24-bit mono 44 100 Hz |
//! | [`ASSET_SYNTH_01`]    | `16_Synth01.wav`   | 24-bit stereo 44 100 Hz |
//! | [`ASSET_SNARE_UNDER`] | `05_SnareUnder.wav`| 24-bit mono 44 100 Hz |

use std::{
    f32::consts::TAU,
    io::Cursor,
    path::PathBuf,
    time::{Duration, Instant},
};

use bytes::Bytes;
use tracing::{info, warn};

use aap_audio::{AudioSink, AudioStream, AudioStreamConfig, PcmChunk};

// ── Embedded assets ───────────────────────────────────────────────────────────

/// Embedded kick-drum sample: 24-bit PCM, mono, 44 100 Hz.
pub static ASSET_KICK_IN: &[u8] = include_bytes!("../assets/01_KickIn.wav");

/// Embedded synth loop: 24-bit PCM, stereo, 44 100 Hz.
pub static ASSET_SYNTH_01: &[u8] = include_bytes!("../assets/16_Synth01.wav");

/// Embedded snare sample: 24-bit PCM, mono, 44 100 Hz.
pub static ASSET_SNARE_UNDER: &[u8] = include_bytes!("../assets/05_SnareUnder.wav");

// ── LoopingWavStream ──────────────────────────────────────────────────────────

/// Pull-based looping WAV source implementing [`AudioStream`].
///
/// Decodes the WAV file once at construction (handling any integer bit depth
/// and channel count), then serves chunks on demand in `next_chunk()`.
/// No thread, no channel, no timing drift — driven entirely by the caller.
pub struct LoopingWavStream {
    /// Pre-decoded, channel-converted samples in the output format.
    samples: Vec<i16>,
    cursor: usize,
    config: AudioStreamConfig,
    timestamp_us: u64,
}

impl LoopingWavStream {
    /// Load an embedded WAV (from `include_bytes!`) into a looping stream.
    ///
    /// The WAV's sample rate must match `config.sample_rate`.
    /// Bit depth and channel count are converted automatically.
    pub fn from_embedded_wav(
        bytes: &'static [u8],
        config: AudioStreamConfig,
    ) -> anyhow::Result<Self> {
        let (raw, wav_channels) = decode_wav(Cursor::new(bytes), &config)?;
        let samples = convert_channels(raw, wav_channels, config.channel_count);
        Ok(Self {
            samples,
            cursor: 0,
            config,
            timestamp_us: 0,
        })
    }

    /// Load an external WAV file into a looping stream.
    pub fn from_wav(
        path: impl AsRef<std::path::Path>,
        config: AudioStreamConfig,
    ) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path.as_ref())?;
        let (raw, wav_channels) = decode_wav(std::io::BufReader::new(file), &config)?;
        let samples = convert_channels(raw, wav_channels, config.channel_count);
        Ok(Self {
            samples,
            cursor: 0,
            config,
            timestamp_us: 0,
        })
    }
}

impl AudioStream for LoopingWavStream {
    fn next_chunk(&mut self, duration_ms: u32) -> Option<PcmChunk> {
        let n = self.config.frames_per_chunk(duration_ms);
        let mut chunk = Vec::with_capacity(n * 2);
        for _ in 0..n {
            if self.cursor >= self.samples.len() {
                self.cursor = 0;
            }
            chunk.extend_from_slice(&self.samples[self.cursor].to_le_bytes());
            self.cursor += 1;
        }
        let ts = self.timestamp_us;
        self.timestamp_us += duration_ms as u64 * 1_000;
        Some(PcmChunk {
            timestamp_us: ts,
            samples: Bytes::from(chunk),
        })
    }
}

// ── WAV decoding helpers ──────────────────────────────────────────────────────

/// Decode a WAV to a flat interleaved i16 buffer, returning the WAV's channel count.
///
/// Validates sample rate against `config`. Bit depth is converted; channel
/// count is returned so the caller can convert if needed.
fn decode_wav<R: std::io::Read + std::io::Seek>(
    reader: R,
    config: &AudioStreamConfig,
) -> anyhow::Result<(Vec<i16>, u32)> {
    let mut wav = hound::WavReader::new(reader)?;
    let spec = wav.spec();

    anyhow::ensure!(
        spec.sample_rate == config.sample_rate,
        "WAV sample rate {} does not match config {} Hz",
        spec.sample_rate,
        config.sample_rate,
    );

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

    Ok((samples, spec.channels as u32))
}

/// Convert a flat interleaved i16 buffer from `in_ch` to `out_ch` channels.
fn convert_channels(samples: Vec<i16>, in_ch: u32, out_ch: u32) -> Vec<i16> {
    match (in_ch, out_ch) {
        (i, o) if i == o => samples,
        (1, 2) => samples.iter().flat_map(|&s| [s, s]).collect(),
        (2, 1) => samples
            .chunks_exact(2)
            .map(|c| ((c[0] as i32 + c[1] as i32) / 2) as i16)
            .collect(),
        _ => samples,
    }
}

// ── TestAudioProducer (push model) ────────────────────────────────────────────

/// Blocking push-based audio producer for external-rate sources.
///
/// Runs on a blocking thread and pushes chunks via an [`AudioSink`] at
/// real-time pace.  Use [`LoopingWavStream`] instead for test audio — this
/// type is intended for sources whose rate is dictated externally (e.g. a
/// Flutter engine audio callback).
pub struct TestAudioProducer {
    source: Source,
    config: AudioStreamConfig,
    chunk_ms: u32,
}

enum Source {
    SineWave { frequency_hz: f32, amplitude: f32 },
    EmbeddedWav(&'static [u8]),
    WavFile(PathBuf),
}

impl TestAudioProducer {
    /// Pure sine-wave generator.
    pub fn sine_wave(frequency_hz: f32, amplitude: f32, config: AudioStreamConfig) -> Self {
        Self {
            source: Source::SineWave {
                frequency_hz,
                amplitude,
            },
            config,
            chunk_ms: 10,
        }
    }

    /// WAV file baked into the binary via `include_bytes!`.
    pub fn from_embedded_wav(bytes: &'static [u8], config: AudioStreamConfig) -> Self {
        Self {
            source: Source::EmbeddedWav(bytes),
            config,
            chunk_ms: 10,
        }
    }

    /// External WAV file loaded at runtime.
    pub fn from_wav(path: PathBuf, config: AudioStreamConfig) -> Self {
        Self {
            source: Source::WavFile(path),
            config,
            chunk_ms: 10,
        }
    }

    pub fn with_chunk_ms(mut self, chunk_ms: u32) -> Self {
        self.chunk_ms = chunk_ms;
        self
    }

    /// Blocking production loop. Call inside `tokio::task::spawn_blocking`.
    pub fn run(self, sink: impl AudioSink) -> anyhow::Result<()> {
        match self.source {
            Source::SineWave {
                frequency_hz,
                amplitude,
            } => run_sine(sink, &self.config, self.chunk_ms, frequency_hz, amplitude),
            Source::EmbeddedWav(bytes) => {
                let (raw, wav_ch) = decode_wav(Cursor::new(bytes), &self.config)?;
                let samples = convert_channels(raw, wav_ch, self.config.channel_count);
                run_samples_loop(sink, &self.config, self.chunk_ms, samples)
            }
            Source::WavFile(path) => {
                let file = std::fs::File::open(&path)?;
                let (raw, wav_ch) = decode_wav(std::io::BufReader::new(file), &self.config)?;
                let samples = convert_channels(raw, wav_ch, self.config.channel_count);
                run_samples_loop(sink, &self.config, self.chunk_ms, samples)
            }
        }
    }
}

fn run_sine(
    sink: impl AudioSink,
    config: &AudioStreamConfig,
    chunk_ms: u32,
    frequency_hz: f32,
    amplitude: f32,
) -> anyhow::Result<()> {
    info!(
        frequency_hz,
        amplitude,
        sample_rate = config.sample_rate,
        "sine-wave producer started"
    );

    let interval = Duration::from_millis(chunk_ms as u64);
    let frames = config.frames_per_chunk(chunk_ms);
    let phase_step = TAU * frequency_hz / config.sample_rate as f32;
    let channels = config.channel_count as usize;
    let samples_per_frame = frames / channels;

    let mut phase: f32 = 0.0;
    let mut timestamp_us: u64 = 0;
    let chunk_duration_us = chunk_ms as u64 * 1_000;
    let mut deadline = Instant::now() + interval;

    loop {
        let mut chunk = Vec::with_capacity(frames);
        for _ in 0..samples_per_frame {
            let s = (amplitude * phase.sin() * i16::MAX as f32) as i16;
            for _ in 0..channels {
                chunk.push(s);
            }
            phase += phase_step;
            if phase >= TAU {
                phase -= TAU;
            }
        }
        sink.push_i16(timestamp_us, &chunk);
        timestamp_us += chunk_duration_us;

        let now = Instant::now();
        if deadline > now {
            std::thread::sleep(deadline - now);
        } else {
            warn!(
                overrun_us = (now - deadline).as_micros(),
                "sine producer overran deadline"
            );
        }
        deadline += interval;
    }
}

fn run_samples_loop(
    sink: impl AudioSink,
    config: &AudioStreamConfig,
    chunk_ms: u32,
    samples: Vec<i16>,
) -> anyhow::Result<()> {
    let interval = Duration::from_millis(chunk_ms as u64);
    let frames = config.frames_per_chunk(chunk_ms);
    let chunk_duration_us = chunk_ms as u64 * 1_000;

    let mut cursor: usize = 0;
    let mut timestamp_us: u64 = 0;
    let mut deadline = Instant::now() + interval;

    loop {
        let mut chunk = Vec::with_capacity(frames);
        for _ in 0..frames {
            if cursor >= samples.len() {
                cursor = 0;
            }
            chunk.push(samples[cursor]);
            cursor += 1;
        }
        sink.push_i16(timestamp_us, &chunk);
        timestamp_us += chunk_duration_us;

        let now = Instant::now();
        if deadline > now {
            std::thread::sleep(deadline - now);
        } else {
            warn!(
                overrun_us = (now - deadline).as_micros(),
                "WAV producer overran deadline"
            );
        }
        deadline += interval;
    }
}
