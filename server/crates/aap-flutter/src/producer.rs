//! Flutter → Android Auto video producer.
//!
//! Reads the latest composited frame from the shared [`PixelStore`] (written
//! by the engine's `surface_present_callback`), converts it to I420, encodes
//! H.264 with openh264, and pushes timestamped NAL units into a
//! [`VideoFrameSender`] — the same contract the testkit video producer
//! fulfils, so [`Connection`] is unaware of the source.
//!
//! It runs on a **blocking thread** and waits on the video focus gate, which
//! yields the [`VideoCfg`] the head unit negotiated.  Only then are the
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

use aap_video::{VideoCfg, VideoFrameSender, VideoStartRx};

use crate::engine::FlutterEngineHandle;
use crate::texture::SharedPixelStore;

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
    /// [`VideoCfg`], then encode the latest Flutter frame until the receiver
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
        let Some(cfg) = start.wait() else {
            info!("flutter video producer: focus gate dropped before signal, stopping");
            return;
        };

        // Flutter renders the inner (post-margin) area at the negotiated
        // density; the encoded frame is the full width×height with the inner
        // image letterboxed into it.
        let (iw, ih) = cfg.inner();
        if let Err(e) = engine.send_window_metrics(iw, ih, cfg.pixel_ratio()) {
            tracing::warn!(error = %e, "flutter video producer: window metrics failed");
            return;
        }

        // Run at the negotiated rate. If the host can't sustain it the
        // deadline loop below drops the backlog rather than falling behind
        // forever; smarter throttling is a later concern.
        let fps = cfg.fps_hz();
        let mut encoder = match build_encoder(fps) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "flutter video producer: encoder init failed");
                return;
            }
        };
        info!(
            width = cfg.width,
            height = cfg.height,
            inner_w = iw,
            inner_h = ih,
            fps,
            dpi = cfg.dpi,
            "flutter video producer: focus granted — starting encode"
        );

        let interval = Duration::from_secs_f64(1.0 / fps as f64);
        let mut deadline = Instant::now() + interval;
        let mut frame_count: u64 = 0;

        loop {
            if let Some(nal) = encode_latest(&mut encoder, &store, &cfg) {
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

/// Snapshot the latest Flutter frame, letterbox it into the full encoded
/// frame, and encode.  `None` (skip this tick) until Flutter has produced a
/// frame at the negotiated inner (post-margin) size.
fn encode_latest(
    encoder: &mut Encoder,
    store: &SharedPixelStore,
    cfg: &VideoCfg,
) -> Option<Vec<u8>> {
    let (iw, ih) = cfg.inner();
    let inner = {
        let s = store.read();
        if s.width != iw || s.height != ih {
            return None;
        }
        s.rgba.clone()
    };

    let (fw, fh) = (cfg.width as usize, cfg.height as usize);
    let canvas = if iw == cfg.width && ih == cfg.height {
        // No margins — the inner buffer already is the full frame.
        inner
    } else {
        // Letterbox: blit the inner image into a black full-size canvas at
        // the margin offset.  Black is all-zero RGB (alpha is ignored by the
        // I420 conversion); BT.601 maps it to Y=16, U=V=128.
        let (ox, oy) = cfg.offset();
        let (ox, oy, iw, ih) = (ox as usize, oy as usize, iw as usize, ih as usize);
        let mut canvas = vec![0u8; fw * fh * 4];
        for row in 0..ih {
            let src = row * iw * 4;
            let dst = ((oy + row) * fw + ox) * 4;
            canvas[dst..dst + iw * 4].copy_from_slice(&inner[src..src + iw * 4]);
        }
        canvas
    };

    let yuv = rgba_to_i420(&canvas, fw, fh);
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
