//! `VideoService` — Android Auto H.264 video channel implementation.
//!
//! # AV-channel message IDs
//!
//! Values are taken from `AVChannelMessageIdsEnum.proto` in `third_party/AAProto`:
//!
//! ```text
//! AV_MEDIA_WITH_TIMESTAMP_INDICATION = 0x0000
//! AV_MEDIA_INDICATION                = 0x0001
//! SETUP_REQUEST                      = 0x8000
//! START_INDICATION                   = 0x8001
//! STOP_INDICATION                    = 0x8002
//! SETUP_RESPONSE                     = 0x8003
//! AV_MEDIA_ACK_INDICATION            = 0x8004
//! AV_INPUT_OPEN_REQUEST              = 0x8005
//! AV_INPUT_OPEN_RESPONSE             = 0x8006
//! VIDEO_FOCUS_REQUEST                = 0x8007
//! VIDEO_FOCUS_INDICATION             = 0x8008
//! ```

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use tracing::{debug, info, warn};

use aap_contracts::{ChannelId, Frame, FrameFlags, Service, ServiceDescriptor, ServiceError};

use crate::VideoConfig;

// ── AV-channel message IDs (from AVChannelMessageIdsEnum.proto) ──────────────

/// Raw H.264 NAL data with a leading timestamp field.
const MSG_AV_MEDIA_WITH_TIMESTAMP: u16 = 0x0000;
/// Raw H.264 NAL data without a timestamp.
const MSG_AV_MEDIA: u16 = 0x0001;
/// Head unit → phone: request AV-channel setup.
const MSG_SETUP_REQUEST: u16 = 0x8000;
/// Head unit → phone: start streaming.
const MSG_START_INDICATION: u16 = 0x8001;
/// Head unit → phone: stop streaming.
const MSG_STOP_INDICATION: u16 = 0x8002;
/// Phone → head unit: setup acknowledgement.
const MSG_SETUP_RESPONSE: u16 = 0x8003;
/// Head unit → phone: flow-control acknowledgement.
const MSG_AV_MEDIA_ACK: u16 = 0x8004;

// ── AVChannelSetupStatus enum values (from AVChannelSetupStatusEnum.proto) ───

/// `AVChannelSetupStatus::OK = 2`
const SETUP_STATUS_OK: i32 = 2;

// ── VideoService ─────────────────────────────────────────────────────────────

/// Android Auto H.264 video projection service.
///
/// Handles the AV-channel setup/start/stop handshake and accepts raw H.264 NAL
/// units from the connected phone. In P0 the NAL data is only logged; a real
/// renderer will be wired in later.
pub struct VideoService {
    config: VideoConfig,
}

impl VideoService {
    /// Create a new `VideoService` with the given configuration.
    pub fn new(config: VideoConfig) -> Self {
        Self { config }
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

    /// Encode an `AVChannelSetupResponse` protobuf body by hand.
    ///
    /// The AV channel setup/start messages are not included in `aap-proto`'s
    /// generated types (they were not compiled in W1), so we encode the minimal
    /// protobuf fields directly.
    ///
    /// `AVChannelSetupResponse` fields:
    ///   field 1 (varint): `media_status` — `AVChannelSetupStatus::OK = 2`
    ///   field 2 (varint): `max_unacked`  — 1
    ///   field 3 (varint, repeated): `configs` — [0]
    fn encode_setup_response() -> Bytes {
        let mut buf = BytesMut::new();
        // field 1: media_status = OK (2) — tag = (1 << 3) | 0 = 0x08
        buf.put_u8(0x08);
        buf.put_u8(SETUP_STATUS_OK as u8);
        // field 2: max_unacked = 1 — tag = (2 << 3) | 0 = 0x10
        buf.put_u8(0x10);
        buf.put_u8(1u8);
        // field 3: configs[0] = 0 — tag = (3 << 3) | 0 = 0x18
        buf.put_u8(0x18);
        buf.put_u8(0u8);
        buf.freeze()
    }

    /// Build a video-channel data frame with the given `message_id` and `body`.
    ///
    /// Video channel frames are data frames (no `CONTROL` flag) with the
    /// two-byte message_id prepended to the protobuf body.
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
            MSG_SETUP_REQUEST => {
                // The head unit sends AVChannelSetupRequest { config_index: u32 }.
                // We acknowledge with SETUP_RESPONSE carrying status=OK.
                debug!(payload_len = payload.len(), "video: received SetupRequest");
                info!("video: channel setup — sending SetupResponse OK");
                let body = Self::encode_setup_response();
                Ok(vec![Self::build_frame(MSG_SETUP_RESPONSE, body)])
            }

            MSG_START_INDICATION => {
                // AVChannelStartIndication { session, config }
                // No reply required; just log and transition to streaming.
                info!("video: received StartIndication — streaming active");
                Ok(vec![])
            }

            MSG_STOP_INDICATION => {
                // AVChannelStopIndication (empty body)
                info!("video: received StopIndication — streaming stopped");
                Ok(vec![])
            }

            MSG_AV_MEDIA_ACK => {
                // Flow-control acknowledgement from the head unit.
                debug!("video: received MediaAck");
                Ok(vec![])
            }

            MSG_AV_MEDIA | MSG_AV_MEDIA_WITH_TIMESTAMP => {
                // Raw H.264 NAL unit bytes (with or without leading timestamp).
                // P0: log the byte count only; a renderer will consume these later.
                debug!(
                    bytes = payload.len(),
                    with_timestamp = (message_id == MSG_AV_MEDIA_WITH_TIMESTAMP),
                    "video: received H.264 NAL unit"
                );
                Ok(vec![])
            }

            unknown => {
                warn!(message_id = unknown, "video: unsupported message id");
                Err(ServiceError::UnsupportedMessage(unknown))
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use aap_contracts::{ChannelId, ServiceError};

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
    async fn handle_unknown_message_returns_error() {
        let mut service = make_service();
        let result = service.handle(0xDEAD, Bytes::new()).await;
        assert!(matches!(
            result,
            Err(ServiceError::UnsupportedMessage(0xDEAD))
        ));
    }

    #[tokio::test]
    async fn handle_setup_request_returns_response() {
        let mut service = make_service();
        // Simulate an AVChannelSetupRequest body (config_index=0, encoded as
        // protobuf varint field 1 = 0: tag 0x08, value 0x00).
        let setup_req_body = Bytes::from_static(&[0x08, 0x00]);
        let frames = service
            .handle(MSG_SETUP_REQUEST, setup_req_body)
            .await
            .unwrap();

        assert_eq!(frames.len(), 1, "expected exactly one response frame");
        let frame = &frames[0];
        assert_eq!(frame.channel, ChannelId::Video);

        // The first two bytes of the payload must be MSG_SETUP_RESPONSE.
        assert!(frame.payload.len() >= 2);
        let resp_id = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
        assert_eq!(resp_id, MSG_SETUP_RESPONSE);

        // The body must start with field 1 = SETUP_STATUS_OK (2).
        assert!(
            frame.payload.len() >= 4,
            "response body must carry at least media_status"
        );
        // tag 0x08 = field 1, varint; value = 2 (OK)
        assert_eq!(frame.payload[2], 0x08);
        assert_eq!(frame.payload[3], SETUP_STATUS_OK as u8);
    }
}
