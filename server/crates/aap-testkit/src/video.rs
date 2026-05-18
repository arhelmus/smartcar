//! H.264 test-pattern producer.
//!
//! Generates solid-colour YUV frames cycling Red → Green → Blue every 90
//! frames (~3 s at 30 fps), encodes them with openh264, and pushes
//! timestamped NAL units into a [`VideoFrameSender`].

use std::time::{Duration, Instant};

use bytes::{BufMut, BytesMut};
use openh264::{
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod},
    formats::YUVBuffer,
    OpenH264API,
};
use tracing::{info, warn};

use aap_video::{VideoFrameSender, VideoStartRx};

const WIDTH: usize = 800;
const HEIGHT: usize = 480;

/// Blocking H.264 test-pattern producer.
///
/// Create with [`TestVideoProducer::new`], then hand off to a blocking thread:
///
/// ```ignore
/// let (tx, rx) = video_frame_channel();
/// let (start_tx, start_rx) = video_start_gate();
/// let producer = TestVideoProducer::new(30)?;
/// tokio::task::spawn_blocking(move || producer.run(tx, start_rx));
/// // Give rx + start_tx to Connection::new(…, rx, start_tx)
/// ```
pub struct TestVideoProducer {
    encoder: Encoder,
    frame_count: u64,
    fps: u32,
}

impl TestVideoProducer {
    /// Initialise the openh264 encoder.  Returns an error if the native
    /// library fails to load.
    pub fn new(fps: u32) -> anyhow::Result<Self> {
        let config = EncoderConfig::new()
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .bitrate(BitRate::from_bps(2_000_000))
            .skip_frames(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(fps));
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)?;
        info!(
            width = WIDTH,
            height = HEIGHT,
            fps,
            "test video producer ready"
        );
        Ok(Self {
            encoder,
            frame_count: 0,
            fps,
        })
    }

    /// Blocking encode loop.  Waits on the focus gate, then encodes from a
    /// fresh IDR and streams every NAL in order until the receiver is dropped.
    ///
    /// Call this inside `tokio::task::spawn_blocking` or `std::thread::spawn`.
    pub fn run(mut self, tx: VideoFrameSender, start: VideoStartRx) {
        // Block until the head unit grants video focus.  Encoding only now
        // guarantees the first frame the head unit sees is a keyframe.
        if !start.wait() {
            info!("test video producer: focus gate dropped before signal, stopping");
            return;
        }
        info!("test video producer: focus granted — starting encode");

        let interval = Duration::from_secs_f64(1.0 / self.fps as f64);
        let mut deadline = Instant::now() + interval;

        loop {
            let nal = self.next_frame();

            if !nal.is_empty() {
                let timestamp_us = (self.frame_count - 1) * 1_000_000 / self.fps as u64;
                let mut buf = BytesMut::with_capacity(8 + nal.len());
                buf.put_u64(timestamp_us);
                buf.put_slice(&nal);
                // Bounded ordered channel: blocks if the consumer is behind
                // (back-pressure) and errs only once the receiver is gone.
                if tx.blocking_send(buf.freeze()).is_err() {
                    info!("test video producer: receiver dropped, stopping");
                    return;
                }
            }

            // Sleep until the next frame deadline, absorbing any overrun.
            let now = Instant::now();
            if deadline > now {
                std::thread::sleep(deadline - now);
            } else {
                warn!(
                    overrun_us = (now - deadline).as_micros(),
                    "test video producer: encode overran frame deadline"
                );
            }
            deadline += interval;
        }
    }

    fn next_frame(&mut self) -> Vec<u8> {
        let yuv = self.make_yuv();
        let bitstream = self.encoder.encode(&yuv).expect("openh264 encode");
        self.frame_count += 1;
        bitstream.to_vec()
    }

    fn make_yuv(&self) -> YUVBuffer {
        // Cycle Red → Green → Blue every 90 frames (~3 s at 30 fps).
        let (y, u, v): (u8, u8, u8) = match (self.frame_count / 90) % 3 {
            0 => (76, 84, 255),  // Red
            1 => (150, 44, 21),  // Green
            _ => (29, 255, 107), // Blue
        };
        let n_luma = WIDTH * HEIGHT;
        let n_chroma = n_luma / 4;
        let mut buf = vec![y; n_luma];
        buf.extend(vec![u; n_chroma]);
        buf.extend(vec![v; n_chroma]);
        YUVBuffer::from_vec(buf, WIDTH, HEIGHT)
    }
}
