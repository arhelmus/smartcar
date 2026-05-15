//! H.264 NAL-unit decoder: compressed bytes → RGBA8888 frames.
//!
//! Wraps OpenH264 (downloaded from Cisco's CDN by the `openh264` crate's
//! build script).  The decoder is stateful: SPS/PPS NALs must be seen before
//! the first IDR slice; the `openh264` crate handles that automatically.

use anyhow::Context as _;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;

/// A single decoded video frame in RGBA8888 format.
pub struct DecodedFrame {
    /// RGBA8888 pixel data, row-major, `width × height × 4` bytes.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Stateful H.264 decoder.
pub struct H264Decoder {
    inner: Decoder,
}

impl H264Decoder {
    pub fn new() -> anyhow::Result<Self> {
        let inner = Decoder::new().context("failed to initialise OpenH264 decoder")?;
        Ok(Self { inner })
    }

    /// Decode one NAL unit.
    ///
    /// Returns `Some(frame)` when decoding produces a complete picture.
    /// Returns `None` for SPS, PPS, and other non-picture NALs.
    pub fn decode_nal(&mut self, nal: &[u8]) -> anyhow::Result<Option<DecodedFrame>> {
        let Some(yuv) = self.inner.decode(nal)? else {
            return Ok(None);
        };

        let (width, height) = yuv.dimensions();
        let mut rgba = vec![0u8; width * height * 4];
        yuv.write_rgba8(&mut rgba);

        Ok(Some(DecodedFrame {
            rgba,
            width: width as u32,
            height: height as u32,
        }))
    }
}
