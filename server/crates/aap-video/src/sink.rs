use bytes::Bytes;

/// Sink for decoded H.264 NAL units received on the Android Auto video channel.
///
/// [`VideoService`](crate::VideoService) calls [`push_nal`](FrameSink::push_nal)
/// for every `AV_MEDIA` or `AV_MEDIA_WITH_TIMESTAMP` frame it receives from
/// the connected phone.
pub trait FrameSink: Send {
    /// Deliver one H.264 NAL unit to the sink.
    ///
    /// `timestamp_us` is `Some` when the frame arrived with a leading 8-byte
    /// presentation timestamp (µs, big-endian), `None` for bare `AV_MEDIA`
    /// frames.  `data` contains the raw NAL unit bytes.
    fn push_nal(&mut self, timestamp_us: Option<u64>, data: Bytes);
}

/// No-op sink — discards every NAL unit with a debug-level log.
///
/// This is the default used by [`VideoService::new`](crate::VideoService::new).
pub struct NullSink;

impl FrameSink for NullSink {
    fn push_nal(&mut self, timestamp_us: Option<u64>, data: Bytes) {
        tracing::debug!(
            bytes = data.len(),
            timestamp_us = ?timestamp_us,
            "video: null sink dropped NAL"
        );
    }
}
