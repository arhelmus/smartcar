//! Ordered frame channel + start gate for the video production pipeline.
//!
//! H.264 is delta-coded: every NAL must reach the head unit **in order and
//! without loss**, and the stream must **begin on a keyframe (IDR)** or the
//! decoder never initialises.  Two primitives enforce that:
//!
//! - [`video_frame_channel`] — a bounded **ordered** mpsc.  The producer
//!   (Flutter embedder or testkit) sends every encoded frame; [`Connection`]
//!   drains them in sequence on each ~30 fps tick.  Bounded so a stalled
//!   consumer applies back-pressure (frames delayed, never dropped) instead
//!   of growing memory without limit.
//! - [`video_start_gate`] — a one-shot focus gate.  The producer blocks on
//!   [`VideoStartRx::wait`] until [`Connection`] fires [`VideoStartTx::signal`]
//!   on `VIDEO_FOCUS_NOTIFICATION`.  This guarantees the producer's *first*
//!   encoded frame (a fresh-encoder IDR with SPS/PPS) is the first frame the
//!   head unit sees — nothing is encoded into a void before focus is granted.
//!
//! # Frame payload format
//!
//! Each `Bytes` value is the body of an `AV_MEDIA_WITH_TIMESTAMP_INDICATION`
//! (msg id `0x0000`) frame:
//!
//! ```text
//! [timestamp_us : u64 BE][H.264 Annex-B NAL unit(s)]
//! ```
//!
//! `Connection` prepends the 2-byte message id before writing to the wire.

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};

use crate::mode::VideoCfg;

/// How many encoded frames the channel buffers before the producer blocks.
/// 16 frames ≈ 0.5 s at 30 fps — enough to ride out scheduling jitter
/// without unbounded growth.
const CHANNEL_CAPACITY: usize = 16;

/// Write-end of the ordered video frame channel — held by the frame producer.
pub type VideoFrameSender = mpsc::Sender<Bytes>;

/// Read-end of the ordered video frame channel — held by [`Connection`].
pub type VideoFrameReceiver = mpsc::Receiver<Bytes>;

/// Create a linked ([`VideoFrameSender`], [`VideoFrameReceiver`]) pair.
///
/// Hand the sender to the producer and the receiver to [`Connection::new`].
pub fn video_frame_channel() -> (VideoFrameSender, VideoFrameReceiver) {
    mpsc::channel(CHANNEL_CAPACITY)
}

/// Connection-side trigger that releases the video producer once the head
/// unit has granted video focus, carrying the negotiated [`VideoCfg`].
pub struct VideoStartTx(oneshot::Sender<VideoCfg>);

impl VideoStartTx {
    /// Release the producer with the negotiated resolution so it sizes the
    /// encoder (and renderer) to the head unit's screen and starts from a
    /// fresh IDR.  A no-op if the producer is already gone (receiver dropped).
    pub fn signal(self, cfg: VideoCfg) {
        let _ = self.0.send(cfg);
    }
}

/// Producer-side gate — blocks the encode loop until focus is granted.
pub struct VideoStartRx(oneshot::Receiver<VideoCfg>);

impl VideoStartRx {
    /// Block the current (blocking) thread until [`VideoStartTx::signal`] is
    /// called, returning the negotiated [`VideoCfg`].  `None` if the trigger
    /// was dropped without signalling (connection torn down before focus) —
    /// the producer should then stop.
    pub fn wait(self) -> Option<VideoCfg> {
        self.0.blocking_recv().ok()
    }
}

/// Create a linked ([`VideoStartTx`], [`VideoStartRx`]) focus gate.
///
/// `Connection` holds the [`VideoStartTx`] and fires it when
/// `VIDEO_FOCUS_NOTIFICATION` arrives; the producer holds the
/// [`VideoStartRx`] and waits on it before its first encode.
pub fn video_start_gate() -> (VideoStartTx, VideoStartRx) {
    let (tx, rx) = oneshot::channel();
    (VideoStartTx(tx), VideoStartRx(rx))
}
