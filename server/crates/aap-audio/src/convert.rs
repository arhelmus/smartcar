//! Format-converting push handle and the [`AudioSink`] trait.
//!
//! Producers call [`AudioSink::push_i16`] with i16 samples in their native
//! channel layout.  [`ConvertingHandle`] converts the channel count before
//! forwarding to the mixer; [`AudioSourceHandle`] passes samples through as-is.

use bytes::Bytes;

use crate::source::{AudioSourceHandle, PcmChunk};

// ── AudioSink trait ───────────────────────────────────────────────────────────

/// Trait implemented by push handles that accept i16 PCM samples.
///
/// Producers call [`push_i16`](AudioSink::push_i16) and do not need to know
/// whether the downstream handle will convert the channel layout.
pub trait AudioSink: Send {
    /// Push one interleaved i16 chunk with the given presentation timestamp.
    ///
    /// `samples` contains `frames_per_chunk` values in the producer's channel
    /// layout.  The sink converts to the mixer's channel layout if needed.
    fn push_i16(&self, timestamp_us: u64, samples: &[i16]);
}

impl AudioSink for AudioSourceHandle {
    fn push_i16(&self, timestamp_us: u64, samples: &[i16]) {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.blocking_push(PcmChunk {
            timestamp_us,
            samples: Bytes::from(bytes),
        });
    }
}

// ── InputFormat ───────────────────────────────────────────────────────────────

/// Describes the channel layout of audio a producer will push.
///
/// Passed to [`MixerSink::add_converting_source`] so the returned
/// [`ConvertingHandle`] knows how to adapt the producer's layout to the
/// mixer's output layout.
pub struct InputFormat {
    /// Number of interleaved channels in the producer's output (1 = mono, 2 = stereo).
    pub channel_count: u32,
}

// ── ConvertingHandle ──────────────────────────────────────────────────────────

/// A push handle that converts channel count before forwarding to the mixer.
///
/// Obtained from [`MixerSink::add_converting_source`].
pub struct ConvertingHandle {
    inner: AudioSourceHandle,
    input_channels: u32,
    output_channels: u32,
}

impl ConvertingHandle {
    pub(crate) fn new(inner: AudioSourceHandle, input: InputFormat, output_channels: u32) -> Self {
        Self {
            inner,
            input_channels: input.channel_count,
            output_channels,
        }
    }
}

impl AudioSink for ConvertingHandle {
    fn push_i16(&self, timestamp_us: u64, samples: &[i16]) {
        let bytes: Vec<u8> = match (self.input_channels, self.output_channels) {
            (i, o) if i == o => samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
            (1, 2) => {
                // mono → stereo: duplicate each sample on both channels
                samples
                    .iter()
                    .flat_map(|&s| s.to_le_bytes().into_iter().chain(s.to_le_bytes()))
                    .collect()
            }
            (2, 1) => {
                // stereo → mono: average L+R pairs
                samples
                    .chunks_exact(2)
                    .flat_map(|c| {
                        let avg = ((c[0] as i32 + c[1] as i32) / 2) as i16;
                        avg.to_le_bytes()
                    })
                    .collect()
            }
            _ => samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
        };
        self.inner.blocking_push(PcmChunk {
            timestamp_us,
            samples: Bytes::from(bytes),
        });
    }
}
