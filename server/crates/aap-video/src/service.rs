//! `VideoService` — Android Auto H.264 video channel implementation.
//!
//! # Video channel message flow
//!
//! After the channel is opened the phone (us) initiates the AV setup:
//!
//! ```text
//! Phone → HU : MEDIA_MESSAGE_SETUP         (0x8000)  codec=H264_BP
//! HU    → Phone : MEDIA_MESSAGE_CONFIG     (0x8003)  status=ready, max_unacked
//! HU    → Phone : MEDIA_MESSAGE_VIDEO_FOCUS_NOTIFICATION (0x8008) focus=projected
//! Phone → HU : MEDIA_MESSAGE_START         (0x8001)  session_id, config_index
//! Phone → HU : MEDIA_MESSAGE_DATA          (0x0000)  H.264 NAL units (with timestamp)
//! HU    → Phone : MEDIA_MESSAGE_ACK        (0x8004)  flow control
//! ```
//!
//! `VideoService` handles the **incoming** messages (HU → Phone): CONFIG, ACK,
//! and VIDEO_FOCUS_NOTIFICATION.  The SETUP and START are sent by
//! `Connection::post_channel_init` and the dispatch loop respectively.

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use tracing::{debug, info, warn};

use aap_contracts::{ChannelId, Frame, FrameFlags, Service, ServiceDescriptor, ServiceError};

use crate::sink::{FrameSink, NullSink};
use crate::VideoConfig;

// ── Media message IDs for the video channel (MediaMessageId.proto) ────────────

/// HU → Phone: setup complete, contains codec config.
const MSG_MEDIA_CONFIG: u16 = 0x8003;
/// Phone → HU: start streaming (sent in response to CONFIG).
const MSG_MEDIA_START: u16 = 0x8001;
/// HU → Phone: flow-control acknowledgement for a video frame.
const MSG_MEDIA_ACK: u16 = 0x8004;
/// HU → Phone: video focus notification (e.g., focus=projected after setup).
const MSG_VIDEO_FOCUS_NOTIFICATION: u16 = 0x8008;

// ── VideoService ─────────────────────────────────────────────────────────────

/// Android Auto H.264 video projection service.
///
/// Handles the AV-channel setup/start/stop handshake and forwards raw H.264
/// NAL units to the configured [`FrameSink`].  By default a [`NullSink`] is
/// used (NALs are logged and discarded).  Pass a custom sink via
/// [`VideoService::with_sink`] to plug in a real renderer.
pub struct VideoService {
    config: VideoConfig,
    /// Sink for outbound NAL units — used when video sending is implemented.
    #[allow(dead_code)]
    sink: Box<dyn FrameSink>,
}

impl VideoService {
    /// Create a new `VideoService` backed by the no-op [`NullSink`].
    pub fn new(config: VideoConfig) -> Self {
        Self {
            config,
            sink: Box::new(NullSink),
        }
    }

    /// Create a `VideoService` that forwards NAL units to `sink`.
    pub fn with_sink(config: VideoConfig, sink: Box<dyn FrameSink>) -> Self {
        Self { config, sink }
    }

    /// Build the encoded `AVChannel` descriptor bytes for service discovery.
    ///
    /// Returns a prost-encoded `ChannelDescriptor` (without `channel_id`, which
    /// `aap-core` fills in) advertising a single 720p/30fps video configuration.
    fn build_descriptor_bytes(&self) -> Bytes {
        use aap_proto::data::{AvChannel, ChannelDescriptor, VideoConfig as ProtoVideoConfig};
        use aap_proto::enums::{av_stream_type, video_fps, video_resolution};
        use prost::Message;

        // Map our fps to the proto enum value.
        let proto_fps = if self.config.fps >= 60 {
            video_fps::Enum::_60 as i32
        } else {
            video_fps::Enum::_30 as i32
        };

        // Map our resolution to the proto enum value.
        let proto_resolution = match (self.config.width, self.config.height) {
            (w, _) if w >= 1920 => video_resolution::Enum::_1080p as i32,
            (w, _) if w >= 1280 => video_resolution::Enum::_720p as i32,
            _ => video_resolution::Enum::_480p as i32,
        };

        let video_cfg = ProtoVideoConfig {
            video_resolution: proto_resolution,
            video_fps: proto_fps,
            margin_width: self.config.margin,
            margin_height: self.config.margin,
            dpi: 140,
            additional_depth: None,
        };

        let av_channel = AvChannel {
            stream_type: av_stream_type::Enum::Video as i32,
            audio_type: None,
            audio_configs: vec![],
            video_configs: vec![video_cfg],
            available_while_in_call: Some(false),
        };

        // Encode a ChannelDescriptor with channel_id=0 (aap-core overwrites it).
        let cd = ChannelDescriptor {
            channel_id: 0,
            sensor_channel: None,
            av_channel: Some(av_channel),
            input_channel: None,
            av_input_channel: None,
            bluetooth_channel: None,
            navigation_channel: None,
            media_info_channel: None,
            vendor_extension_channel: None,
        };

        let mut buf = Vec::with_capacity(cd.encoded_len());
        cd.encode(&mut buf).expect("encoding into Vec never fails");
        Bytes::from(buf)
    }

    /// Build a video-channel data frame with the given `message_id` and `body`.
    fn build_frame(message_id: u16, body: Bytes) -> Frame {
        let mut payload = BytesMut::with_capacity(2 + body.len());
        payload.put_u16(message_id);
        payload.put(body);
        Frame {
            channel: ChannelId::Video,
            flags: FrameFlags::FIRST | FrameFlags::LAST,
            payload: payload.freeze(),
        }
    }
}

#[async_trait]
impl Service for VideoService {
    fn channel(&self) -> ChannelId {
        ChannelId::Video
    }

    fn descriptor(&self) -> ServiceDescriptor {
        ServiceDescriptor {
            channel: ChannelId::Video,
            descriptor_bytes: self.build_descriptor_bytes(),
        }
    }

    async fn handle(
        &mut self,
        message_id: u16,
        payload: Bytes,
    ) -> Result<Vec<Frame>, ServiceError> {
        match message_id {
            MSG_MEDIA_CONFIG => {
                // Head unit sends MEDIA_MESSAGE_CONFIG after we send MEDIA_MESSAGE_SETUP.
                // Respond with MEDIA_MESSAGE_START to begin the video session.
                info!(
                    payload_len = payload.len(),
                    "video: received Config — sending Start"
                );
                // Start proto: { session_id(varint,f1)=1, configuration_index(varint,f2)=0 }
                let body = Bytes::from_static(&[0x08, 0x01, 0x10, 0x00]);
                Ok(vec![Self::build_frame(MSG_MEDIA_START, body)])
            }

            MSG_VIDEO_FOCUS_NOTIFICATION => {
                // Head unit grants video focus (e.g., VIDEO_FOCUS_PROJECTED) after setup.
                // No reply required.
                info!("video: received VideoFocusNotification — projection active");
                Ok(vec![])
            }

            MSG_MEDIA_ACK => {
                // Flow-control acknowledgement from the head unit for a sent video frame.
                debug!("video: received MediaAck");
                Ok(vec![])
            }

            unknown => {
                warn!(
                    message_id = unknown,
                    "video: unsupported message id; dropping"
                );
                Ok(vec![])
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use aap_contracts::ChannelId;

    use super::*;
    use crate::VideoConfig;

    fn make_service() -> VideoService {
        VideoService::new(VideoConfig::default())
    }

    #[test]
    fn descriptor_has_correct_channel() {
        let service = make_service();
        assert_eq!(service.channel(), ChannelId::Video);
    }

    #[tokio::test]
    async fn handle_config_sends_start() {
        let mut service = make_service();
        // Simulate MEDIA_MESSAGE_CONFIG from head unit.
        // Config proto: { status=READY(2), max_unacked=1, config_indices=[0] }
        let config_body = Bytes::from_static(&[0x08, 0x02, 0x10, 0x01, 0x18, 0x00]);
        let frames = service.handle(MSG_MEDIA_CONFIG, config_body).await.unwrap();

        assert_eq!(
            frames.len(),
            1,
            "config must elicit exactly one Start frame"
        );
        let frame = &frames[0];
        assert_eq!(frame.channel, ChannelId::Video);

        // First two bytes must be MSG_MEDIA_START (0x8001).
        assert!(frame.payload.len() >= 2);
        let msg_id = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
        assert_eq!(msg_id, MSG_MEDIA_START);
    }

    #[tokio::test]
    async fn handle_video_focus_notification_is_no_op() {
        let mut service = make_service();
        let frames = service
            .handle(MSG_VIDEO_FOCUS_NOTIFICATION, Bytes::new())
            .await
            .unwrap();
        assert!(frames.is_empty());
    }

    #[tokio::test]
    async fn handle_ack_is_no_op() {
        let mut service = make_service();
        let frames = service.handle(MSG_MEDIA_ACK, Bytes::new()).await.unwrap();
        assert!(frames.is_empty());
    }

    #[tokio::test]
    async fn handle_unknown_message_drops_gracefully() {
        let mut service = make_service();
        let result = service.handle(0xDEAD, Bytes::new()).await;
        // Should succeed (no error) but return no frames.
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
