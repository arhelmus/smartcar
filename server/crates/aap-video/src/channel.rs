//! Watch-channel types for the video frame production pipeline.
//!
//! The render thread (Flutter embedder or testkit) holds the
//! [`VideoFrameSender`] and pushes encoded frames via
//! [`VideoFrameSender::send`].  [`Connection`] holds the
//! [`VideoFrameReceiver`] and reads the latest frame on each 30 fps tick.
//!
//! Using a `watch` channel gives "latest frame wins" semantics: if the render
//! thread runs faster than the send rate, stale intermediate frames are
//! automatically discarded and only the most recent one is forwarded.
//!
//! # Frame payload format
//!
//! The `Bytes` value carried by the channel is the body of an
//! `AV_MEDIA_WITH_TIMESTAMP_INDICATION` (msg id `0x0000`) frame:
//!
//! ```text
//! [timestamp_us : u64 BE][H.264 Annex-B NAL unit(s)]
//! ```
//!
//! `Connection` prepends the 2-byte message id before writing to the wire.

use bytes::Bytes;
use tokio::sync::watch;

/// The value type carried through the video frame channel.
///
/// `None` until the first frame is produced.
pub type VideoFramePayload = Option<Bytes>;

/// Write-end of the video frame channel — held by the frame producer.
pub type VideoFrameSender = watch::Sender<VideoFramePayload>;

/// Read-end of the video frame channel — held by [`Connection`].
pub type VideoFrameReceiver = watch::Receiver<VideoFramePayload>;

/// Create a linked ([`VideoFrameSender`], [`VideoFrameReceiver`]) pair.
///
/// Call this once in `main` before constructing the producer and the
/// connection; hand the sender to the producer and the receiver to
/// [`Connection::new`].
pub fn video_frame_channel() -> (VideoFrameSender, VideoFrameReceiver) {
    watch::channel(None)
}
