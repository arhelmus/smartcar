//! Flutter → Android Auto video producer.
//!
//! Reads the latest composited frame from the shared [`PixelStore`] (written
//! by the engine's `surface_present_callback`), converts it to I420, encodes
//! H.264 with openh264, and pushes timestamped NAL units into a
//! [`VideoFrameSender`] — the same contract the testkit video producer
//! fulfils, so [`Connection`] is unaware of the source.
//!
//! Like the testkit producer it runs on a **blocking thread** and waits on the
//! video focus gate before its first encode, so the first frame on the wire is
//! a fresh-encoder IDR.

use std::time::{Duration, Instant};

use bytes::{BufMut, BytesMut};
use openh264::{
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod},
    formats::YUVBuffer,
    OpenH264API,
};
use tracing::{info, warn};

use aap_video::{VideoFrameSender, VideoStartRx};

use crate::texture::SharedPixelStore;

/// Encoded surface size.  Matches the testkit producer (the known-good
/// resolution against openauto); the engine is told to render at this size
/// via `send_window_metrics`.
pub const WIDTH: usize = 800;
pub const HEIGHT: usize = 480;

/// Flutter software buffer is the platform-native 32-bit format.  On
/// little-endian hosts Skia's N32 is byte order **B, G, R, A**.  Flip this if
/// red/blue look swapped when verifying against openauto.
const PIXEL_IS_BGRA: bool = true;

/// Blocking Flutter video producer.
pub struct FlutterVideoProducer {
    encoder: Encoder,
    frame_count: u64,
    fps: u32,
}

impl FlutterVideoProducer {
    /// Initialise the openh264 encoder (same profile as the testkit producer).
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
            "flutter video producer ready"
        );
        Ok(Self {
            encoder,
            frame_count: 0,
            fps,
        })
    }

    /// Blocking encode loop.  Waits on the focus gate, then encodes the latest
    /// Flutter frame at `fps` until the receiver is dropped.
    ///
    /// Call inside `tokio::task::spawn_blocking`.
    pub fn run(mut self, store: SharedPixelStore, tx: VideoFrameSender, start: VideoStartRx) {
        if !start.wait() {
            info!("flutter video producer: focus gate dropped before signal, stopping");
            return;
        }
        info!("flutter video producer: focus granted — starting encode");

        let interval = Duration::from_secs_f64(1.0 / self.fps as f64);
        let mut deadline = Instant::now() + interval;

        loop {
            if let Some(nal) = self.encode_latest(&store) {
                if !nal.is_empty() {
                    let timestamp_us = (self.frame_count - 1) * 1_000_000 / self.fps as u64;
                    let mut buf = BytesMut::with_capacity(8 + nal.len());
                    buf.put_u64(timestamp_us);
                    buf.put_slice(&nal);
                    if tx.blocking_send(buf.freeze()).is_err() {
                        info!("flutter video producer: receiver dropped, stopping");
                        return;
                    }
                }
            }

            let now = Instant::now();
            if deadline > now {
                std::thread::sleep(deadline - now);
            } else {
                warn!(
                    overrun_us = (now - deadline).as_micros(),
                    "flutter video producer: encode overran frame deadline"
                );
            }
            deadline += interval;
        }
    }

    /// Snapshot the latest frame and encode it.  Returns `None` (skip this
    /// tick) until Flutter has produced a frame at the expected size.
    fn encode_latest(&mut self, store: &SharedPixelStore) -> Option<Vec<u8>> {
        let (rgba, w, h) = {
            let s = store.read();
            if s.width as usize != WIDTH || s.height as usize != HEIGHT {
                return None;
            }
            (s.rgba.clone(), s.width as usize, s.height as usize)
        };
        let yuv = rgba_to_i420(&rgba, w, h);
        let bitstream = self.encoder.encode(&yuv).expect("openh264 encode");
        self.frame_count += 1;
        Some(bitstream.to_vec())
    }
}

/// Convert a packed 32-bit frame to planar I420 (BT.601 limited range).
///
/// `buf` is `w × h × 4` bytes.  Chroma is 2×2-averaged.
fn rgba_to_i420(buf: &[u8], w: usize, h: usize) -> YUVBuffer {
    let (ri, gi, bi) = if PIXEL_IS_BGRA { (2, 1, 0) } else { (0, 1, 2) };

    let n_luma = w * h;
    let n_chroma = n_luma / 4;
    let mut out = vec![0u8; n_luma + 2 * n_chroma];
    let (y_plane, uv) = out.split_at_mut(n_luma);
    let (u_plane, v_plane) = uv.split_at_mut(n_chroma);

    for y in 0..h {
        for x in 0..w {
            let p = (y * w + x) * 4;
            let r = buf[p + ri] as i32;
            let g = buf[p + gi] as i32;
            let b = buf[p + bi] as i32;
            y_plane[y * w + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
        }
    }

    let cw = w / 2;
    for cy in 0..h / 2 {
        for cx in 0..cw {
            let mut rs = 0i32;
            let mut gs = 0i32;
            let mut bs = 0i32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = ((cy * 2 + dy) * w + (cx * 2 + dx)) * 4;
                    rs += buf[p + ri] as i32;
                    gs += buf[p + gi] as i32;
                    bs += buf[p + bi] as i32;
                }
            }
            let (r, g, b) = (rs / 4, gs / 4, bs / 4);
            u_plane[cy * cw + cx] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            v_plane[cy * cw + cx] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }

    YUVBuffer::from_vec(out, w, h)
}
