//! Flutter → Android Auto video producer.
//!
//! Reads the latest composited frame from the shared [`PixelStore`] (written
//! by the engine's `surface_present_callback`), converts it to I420, encodes
//! H.264 with openh264, and pushes timestamped NAL units into a
//! [`VideoFrameSender`] — the same contract the testkit video producer
//! fulfils, so [`Connection`] is unaware of the source.
//!
//! It runs on a **blocking thread** and waits on the video focus gate, which
//! yields the [`VideoMode`] the head unit negotiated.  Only then are the
//! engine's window metrics and the encoder sized — so the render surface and
//! the H.264 stream both match the head unit's screen — and the first encoded
//! frame is a fresh-encoder IDR.

use std::time::{Duration, Instant};

use bytes::{BufMut, BytesMut};
use openh264::{
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod},
    formats::YUVBuffer,
    OpenH264API,
};
use tracing::{debug, info};

use aap_video::{VideoFrameSender, VideoMode, VideoStartRx};

use crate::engine::FlutterEngineHandle;
use crate::texture::SharedPixelStore;

/// Frame-rate ceiling for the software path.
///
/// Flutter's software rasteriser plus a CPU openh264 encode cannot sustain
/// 30 fps at projection resolutions on a typical host (the encode overran the
/// 33 ms budget nearly every frame).  Capping the cadence keeps the pipeline
/// real-time — fewer, on-time frames beat a perpetually backlogged 30 fps.
const SOFTWARE_FPS_CAP: u32 = 20;

/// Flutter software buffer is the platform-native 32-bit format.  On
/// little-endian hosts Skia's N32 is byte order **B, G, R, A**.  Flip this if
/// red/blue look swapped when verifying against openauto.
const PIXEL_IS_BGRA: bool = true;

/// Blocking Flutter video producer.  Construct, then hand to a blocking thread.
pub struct FlutterVideoProducer;

impl FlutterVideoProducer {
    pub fn new() -> Self {
        Self
    }

    /// Block on the focus gate, size the engine + encoder to the negotiated
    /// [`VideoMode`], then encode the latest Flutter frame until the receiver
    /// is dropped.  Call inside `tokio::task::spawn_blocking`.
    ///
    /// `engine` is owned here so the engine outlives the encode loop and is
    /// shut down cleanly when this returns.
    pub fn run(
        self,
        store: SharedPixelStore,
        tx: VideoFrameSender,
        start: VideoStartRx,
        engine: FlutterEngineHandle,
    ) {
        let Some(mode) = start.wait() else {
            info!("flutter video producer: focus gate dropped before signal, stopping");
            return;
        };

        // Size the render surface to the head unit's screen.
        if let Err(e) = engine.send_window_metrics(mode.width, mode.height, 1.0) {
            tracing::warn!(error = %e, "flutter video producer: window metrics failed");
            return;
        }

        let fps = mode.fps.clamp(1, SOFTWARE_FPS_CAP);
        let mut encoder = match build_encoder(fps) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "flutter video producer: encoder init failed");
                return;
            }
        };
        info!(
            width = mode.width,
            height = mode.height,
            fps,
            "flutter video producer: focus granted — starting encode"
        );

        let interval = Duration::from_secs_f64(1.0 / fps as f64);
        let mut deadline = Instant::now() + interval;
        let mut frame_count: u64 = 0;

        loop {
            if let Some(nal) = encode_latest(&mut encoder, &store, &mode) {
                if !nal.is_empty() {
                    let timestamp_us = frame_count * 1_000_000 / fps as u64;
                    frame_count += 1;
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
            if now < deadline {
                std::thread::sleep(deadline - now);
                deadline += interval;
            } else {
                // Behind: reset the cadence and drop the backlog instead of
                // accumulating lateness (which would never recover).
                debug!(
                    behind_us = (now - deadline).as_micros(),
                    "flutter video producer: behind deadline, dropping backlog"
                );
                deadline = now + interval;
            }
        }
    }
}

impl Default for FlutterVideoProducer {
    fn default() -> Self {
        Self::new()
    }
}

fn build_encoder(fps: u32) -> anyhow::Result<Encoder> {
    let config = EncoderConfig::new()
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .bitrate(BitRate::from_bps(2_000_000))
        .skip_frames(true)
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps));
    Ok(Encoder::with_api_config(
        OpenH264API::from_source(),
        config,
    )?)
}

/// Snapshot the latest frame and encode it.  `None` (skip this tick) until
/// Flutter has produced a frame at the negotiated size.
fn encode_latest(
    encoder: &mut Encoder,
    store: &SharedPixelStore,
    mode: &VideoMode,
) -> Option<Vec<u8>> {
    let (rgba, w, h) = {
        let s = store.read();
        if s.width != mode.width || s.height != mode.height {
            return None;
        }
        (s.rgba.clone(), s.width as usize, s.height as usize)
    };
    let yuv = rgba_to_i420(&rgba, w, h);
    let bitstream = encoder.encode(&yuv).expect("openh264 encode");
    Some(bitstream.to_vec())
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
