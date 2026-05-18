//! [`AudioService`] — Android Auto AV-channel service for one audio stream.
//!
//! Handles the per-channel handshake (`SETUP_RESPONSE` → `START_INDICATION`)
//! and drives outbound PCM frame production via an [`AudioStream`] on every
//! [`tick`](aap_contracts::Service::tick) call from the connection loop.
//!
//! # Wire flow (phone-initiated)
//!
//! ```text
//! Phone → HU : SETUP_REQUEST       (0x8000)  codec=PCM  [sent by Connection]
//! HU → Phone : SETUP_RESPONSE      (0x8003)  status, max_unacked
//! Phone → HU : START_INDICATION    (0x8001)  session=1, config_index=0
//! Phone → HU : AV_MEDIA_WITH_TS    (0x0000)  [ts: u64 BE][S16LE samples…]  ← tick
//! HU → Phone : AV_MEDIA_ACK        (0x8004)  flow control (ignored for now)
//! ```

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use prost::Message as _;
use tracing::{debug, info, warn};

use aap_contracts::{ChannelId, Frame, FrameFlags, Service, ServiceDescriptor, ServiceError};
use aap_proto::data::{AudioConfig as ProtoAudioConfig, AvChannel, ChannelDescriptor};
use aap_proto::enums::{audio_type, av_stream_type};

use super::config::{AudioStreamConfig, AudioType};
use super::stream::AudioStream;

// ── AV message IDs (shared with video channel) ────────────────────────────────

/// Phone → HU: timestamped PCM payload.
const MSG_DATA_WITH_TS: u16 = 0x0000;
/// Phone → HU: start streaming (sent after receiving SETUP_RESPONSE).
const MSG_START: u16 = 0x8001;
/// HU → Phone: setup complete — contains status and max_unacked window.
const MSG_SETUP_RESPONSE: u16 = 0x8003;
/// HU → Phone: flow-control acknowledgement.
const MSG_ACK: u16 = 0x8004;

// ── AudioService ──────────────────────────────────────────────────────────────

/// Android Auto audio projection service for one stream type.
///
/// Instantiate one service per audio channel (`MediaAudio`, `SpeechAudio`,
/// `SystemAudio`), backed by a [`MixerSink`](super::mixer::MixerSink) or any
/// other [`AudioStream`] implementation.
///
/// Register the service with [`ServiceRegistry`](aap_core::ServiceRegistry)
/// before starting the connection; the connection loop will call [`tick`] on
/// the audio ticker and dispatch inbound HU messages to [`handle`].
///
/// [`tick`]: aap_contracts::Service::tick
/// [`handle`]: aap_contracts::Service::handle
pub struct AudioService {
    channel: ChannelId,
    config: AudioStreamConfig,
    stream: Box<dyn AudioStream>,
    /// Duration of each outbound PCM chunk in milliseconds.
    chunk_ms: u32,
    /// Set to `true` after `SETUP_RESPONSE` is received and `START` is sent.
    active: bool,
}

impl AudioService {
    /// Create a new `AudioService`.
    ///
    /// - `channel` must be one of `MediaAudio`, `SpeechAudio`, `SystemAudio`.
    /// - `config` describes the PCM format; it must match the audio type
    ///   implied by `channel`.
    /// - `stream` is the audio source — typically a [`MixerSink`].
    ///
    /// [`MixerSink`]: super::mixer::MixerSink
    pub fn new(
        channel: ChannelId,
        config: AudioStreamConfig,
        stream: Box<dyn AudioStream>,
    ) -> Self {
        Self {
            channel,
            config,
            stream,
            chunk_ms: 10,
            active: false,
        }
    }

    /// Build the `ChannelDescriptor` proto bytes for service discovery.
    fn build_descriptor_bytes(&self) -> Bytes {
        let proto_audio_type = match self.config.audio_type {
            AudioType::Media => audio_type::Enum::Media as i32,
            AudioType::Speech => audio_type::Enum::Speech as i32,
            AudioType::System => audio_type::Enum::System as i32,
        };

        let audio_cfg = ProtoAudioConfig {
            sample_rate: self.config.sample_rate,
            bit_depth: self.config.bit_depth,
            channel_count: self.config.channel_count,
        };

        let av_channel = AvChannel {
            stream_type: av_stream_type::Enum::Audio as i32,
            audio_type: Some(proto_audio_type),
            audio_configs: vec![audio_cfg],
            video_configs: vec![],
            available_while_in_call: Some(true),
        };

        let cd = ChannelDescriptor {
            channel_id: 0, // filled in by aap-core
            av_channel: Some(av_channel),
            sensor_channel: None,
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

    /// Wrap `body` in a data frame for this service's channel.
    fn build_frame(&self, message_id: u16, body: Bytes) -> Frame {
        let mut payload = BytesMut::with_capacity(2 + body.len());
        payload.put_u16(message_id);
        payload.put(body);
        Frame {
            channel: self.channel,
            flags: FrameFlags::FIRST | FrameFlags::LAST,
            payload: payload.freeze(),
        }
    }
}

#[async_trait]
impl Service for AudioService {
    fn channel(&self) -> ChannelId {
        self.channel
    }

    fn descriptor(&self) -> ServiceDescriptor {
        ServiceDescriptor {
            channel: self.channel,
            descriptor_bytes: self.build_descriptor_bytes(),
        }
    }

    async fn handle(
        &mut self,
        message_id: u16,
        payload: Bytes,
    ) -> Result<Vec<Frame>, ServiceError> {
        match message_id {
            MSG_SETUP_RESPONSE => {
                // HU accepted our SETUP; reply with START to begin streaming.
                info!(channel = ?self.channel, "audio: setup response received — sending start");
                self.active = true;
                // START proto: { session_id(varint,f1)=1, configuration_index(varint,f2)=0 }
                let body = Bytes::from_static(&[0x08, 0x01, 0x10, 0x00]);
                Ok(vec![self.build_frame(MSG_START, body)])
            }

            MSG_ACK => {
                // Flow-control acknowledgement — ignored until ACK-window tracking lands.
                debug!(channel = ?self.channel, payload_len = payload.len(), "audio: ack");
                Ok(vec![])
            }

            unknown => {
                warn!(channel = ?self.channel, message_id = unknown, "audio: unknown message id");
                Ok(vec![])
            }
        }
    }

    async fn tick(&mut self) -> Result<Vec<Frame>, ServiceError> {
        if !self.active {
            return Ok(vec![]);
        }
        let Some(chunk) = self.stream.next_chunk(self.chunk_ms) else {
            return Ok(vec![]);
        };
        // AV_MEDIA_WITH_TIMESTAMP body: [timestamp_us: u64 BE][S16LE samples]
        let mut body = BytesMut::with_capacity(8 + chunk.samples.len());
        body.put_u64(chunk.timestamp_us);
        body.put(chunk.samples);
        Ok(vec![self.build_frame(MSG_DATA_WITH_TS, body.freeze())])
    }
}
