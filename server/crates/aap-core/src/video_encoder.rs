//! H.264 test-pattern encoder used during bringup to confirm the video pipeline.
//!
//! Generates solid-color YUV frames that cycle Red → Green → Blue every 90
//! frames (~3 seconds at 30 fps) and encodes them with openh264.

use openh264::{
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod},
    formats::YUVBuffer,
    OpenH264API,
};

pub const VIDEO_WIDTH: usize = 800;
pub const VIDEO_HEIGHT: usize = 480;

pub struct TestFrameEncoder {
    encoder: Encoder,
    pub frame_count: u64,
}

impl TestFrameEncoder {
    pub fn new() -> Result<Self, openh264::Error> {
        let config = EncoderConfig::new()
            .max_frame_rate(FrameRate::from_hz(30.0))
            .bitrate(BitRate::from_bps(2_000_000))
            .skip_frames(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(30));
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)?;
        Ok(Self {
            encoder,
            frame_count: 0,
        })
    }

    /// Encode the next test-pattern frame and return the Annex-B NAL bytes.
    pub fn next_frame(&mut self) -> Vec<u8> {
        let yuv = self.make_yuv();
        let bitstream = self.encoder.encode(&yuv).expect("openh264 encode");
        self.frame_count += 1;
        bitstream.to_vec()
    }

    fn make_yuv(&self) -> YUVBuffer {
        // Color cycle: Red, Green, Blue — switch every 90 frames (~3 s at 30 fps).
        let (y, u, v): (u8, u8, u8) = match (self.frame_count / 90) % 3 {
            0 => (76, 84, 255),   // Red
            1 => (150, 44, 21),   // Green
            _ => (29, 255, 107),  // Blue
        };
        let n_luma = VIDEO_WIDTH * VIDEO_HEIGHT;
        let n_chroma = n_luma / 4;
        let mut buf = vec![y; n_luma];
        buf.extend(vec![u; n_chroma]);
        buf.extend(vec![v; n_chroma]);
        YUVBuffer::from_vec(buf, VIDEO_WIDTH, VIDEO_HEIGHT)
    }
}
