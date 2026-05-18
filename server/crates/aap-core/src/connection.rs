//! Android Auto control-channel state machine.
//!
//! [`Connection`] drives the full AA setup sequence (version negotiation, TLS
//! upgrade, service discovery, channel open, post-channel init) and then
//! enters a steady-state dispatch loop that routes data frames to the
//! appropriate service.

use std::time::Duration;

use bytes::Bytes;
use prost::Message;
use tracing::{debug, info, warn};

use aap_contracts::{AapError, Result};
use aap_contracts::{ChannelId, FrameFlags, MessageType, Transport};
use aap_video::VideoFrameReceiver;

use crate::control::{
    build_data_frame, encode_control, encode_control_on, parse_message_type, proto_body,
};
use crate::registry::ServiceRegistry;

// ── Wire constants ────────────────────────────────────────────────────────────

/// Major version of the AA protocol we support.
const AA_VERSION_MAJOR: u16 = 1;
/// Minor version of the AA protocol we advertise.
const AA_VERSION_MINOR: u16 = 1;

// ── Sensor message IDs (SensorMessageId.proto) ────────────────────────────────
const SENSOR_MSG_REQUEST: u16 = 0x8001;
const SENSOR_MSG_RESPONSE: u16 = 0x8002;
const SENSOR_MSG_BATCH: u16 = 0x8003;

// ── Media message IDs (MediaMessageId.proto) ──────────────────────────────────
const MEDIA_MSG_SETUP: u16 = 0x8000;
const MEDIA_MSG_START: u16 = 0x8001;
const MEDIA_MSG_CONFIG: u16 = 0x8003;

// ── Sensor types (SensorType.proto) ───────────────────────────────────────────
const SENSOR_TYPE_NIGHT_MODE: u8 = 10;
const SENSOR_TYPE_DRIVING_STATUS: u8 = 13;

// ── Media codec types (MediaCodecType.proto) ──────────────────────────────────
const MEDIA_CODEC_AUDIO_PCM: u8 = 1;
const MEDIA_CODEC_VIDEO_H264_BP: u8 = 3;

// ── Video data ────────────────────────────────────────────────────────────────
/// Phone → HU: H.264 NAL units (data frame, not a control message).
const MEDIA_MSG_DATA: u16 = 0x0000;
/// Phone → HU: request video projection focus.
const MSG_VIDEO_FOCUS_REQUEST: u16 = 0x8007;
/// HU → Phone: video focus granted; phone should start streaming.
const MSG_VIDEO_FOCUS_NOTIFICATION: u16 = 0x8008;
/// How often the connection checks for a new video frame from the producer.
const VIDEO_FRAME_INTERVAL: Duration = Duration::from_millis(33); // ~30 fps
/// How often the audio tick fires to drain the mixer and send PCM frames.
const AUDIO_FRAME_INTERVAL: Duration = Duration::from_millis(10); // 10 ms chunks

// ── Connection ────────────────────────────────────────────────────────────────

/// Drives the Android Auto control-channel protocol over a [`Transport`].
///
/// Create one instance per accepted connection and call [`Connection::run`].
/// The instance is consumed when `run` returns.
pub struct Connection<T: Transport> {
    transport: T,
    registry: ServiceRegistry,
    /// Set to `true` after the head unit grants video focus; triggers frame forwarding.
    video_active: bool,
    /// Latest encoded video frame from the producer (watch channel read-end).
    frame_rx: VideoFrameReceiver,
}

impl<T: Transport> Connection<T> {
    /// Wrap a transport, service registry, and video frame receiver into a new connection.
    ///
    /// `frame_rx` is the read-end of a [`video_frame_channel`](aap_video::video_frame_channel).
    /// The matching sender is held by a frame producer (e.g. [`aap_testkit::TestVideoProducer`]
    /// or the Flutter embedder), which pushes encoded NAL units independently.
    pub fn new(transport: T, registry: ServiceRegistry, frame_rx: VideoFrameReceiver) -> Self {
        Self {
            transport,
            registry,
            video_active: false,
            frame_rx,
        }
    }

    /// Run the full protocol state machine to completion.
    ///
    /// Returns `Ok(())` after a clean shutdown exchange, or an [`AapError`] on
    /// any protocol or transport error.
    pub async fn run(mut self) -> Result<()> {
        self.handshake_version().await?;
        self.handshake_tls().await?;
        self.recv_auth_complete().await?;
        let channels = self.service_discovery().await?;
        self.open_channels(&channels).await?;
        self.post_channel_init(&channels).await?;
        self.dispatch_loop().await
    }

    // ── Version negotiation ───────────────────────────────────────────────────

    /// Receive the head unit's `VersionRequest` and reply with `VersionResponse`.
    ///
    /// The head unit (openauto) always initiates by sending VersionRequest.
    /// We echo back the same major/minor with status 0 (compatible).
    ///
    /// Wire format:
    /// - VersionRequest  payload: [msg_id(2), major(2), minor(2)]
    /// - VersionResponse payload: [msg_id(2), major(2), minor(2), status(2)]
    async fn handshake_version(&mut self) -> Result<()> {
        let frame = self.transport.recv_frame().await?;
        self.expect_control_msg(&frame.payload, MessageType::VersionRequest)?;

        // Parse the requested version (bytes 2-5 of the payload, after the 2-byte msg_id).
        let major = if frame.payload.len() >= 4 {
            u16::from_be_bytes([frame.payload[2], frame.payload[3]])
        } else {
            AA_VERSION_MAJOR
        };
        let minor = if frame.payload.len() >= 6 {
            u16::from_be_bytes([frame.payload[4], frame.payload[5]])
        } else {
            AA_VERSION_MINOR
        };
        debug!(major, minor, "received VersionRequest");

        // Send VersionResponse: echo the requested version with status=0 (compatible).
        let mut payload = Vec::with_capacity(8);
        payload.extend_from_slice(&MessageType::VersionResponse.as_u16().to_be_bytes());
        payload.extend_from_slice(&major.to_be_bytes());
        payload.extend_from_slice(&minor.to_be_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes()); // STATUS_SUCCESS
        let resp = aap_contracts::Frame::control_bulk(ChannelId::Control, Bytes::from(payload));
        self.transport.send_frame(resp).await?;

        info!("version negotiation complete");
        Ok(())
    }

    // ── TLS handshake ─────────────────────────────────────────────────────────

    /// Drive the TLS upgrade by delegating entirely to the transport.
    ///
    /// The transport reads the initial `SslHandshake` frame itself and owns
    /// the full AA TLS frame-exchange loop. After this returns, all subsequent
    /// [`Transport::recv_frame`] / [`Transport::send_frame`] calls are
    /// transparently encrypted.
    async fn handshake_tls(&mut self) -> Result<()> {
        self.transport
            .upgrade_tls()
            .await
            .map_err(AapError::Transport)?;
        info!("TLS handshake complete");
        Ok(())
    }

    // ── Auth complete ─────────────────────────────────────────────────────────

    /// Wait for the head unit's `AuthComplete` indication.  No reply is needed.
    async fn recv_auth_complete(&mut self) -> Result<()> {
        let frame = self.transport.recv_frame().await?;
        self.expect_control_msg(&frame.payload, MessageType::AuthComplete)?;
        info!("auth complete received");
        Ok(())
    }

    // ── Service discovery ─────────────────────────────────────────────────────

    /// Send `ServiceDiscoveryRequest` and receive the head unit's `ServiceDiscoveryResponse`.
    ///
    /// Returns the list of channel IDs advertised by the head unit so the
    /// caller can open each one.
    async fn service_discovery(&mut self) -> Result<Vec<u32>> {
        let req = aap_proto::ServiceDiscoveryRequest {
            device_name: "Smartcar".into(),
            device_brand: "Smartcar".into(),
        };
        let frame = encode_control(MessageType::ServiceDiscoveryRequest, &req);
        self.transport.send_frame(frame).await?;

        let frame = self.transport.recv_frame().await?;
        self.expect_control_msg(&frame.payload, MessageType::ServiceDiscoveryResponse)?;

        let body = proto_body(&frame.payload);
        let resp = aap_proto::ServiceDiscoveryResponse::decode(body)
            .map_err(|e| AapError::Protocol(format!("ServiceDiscoveryResponse decode: {e}")))?;

        let channel_ids: Vec<u32> = resp.channels.iter().map(|c| c.channel_id).collect();
        info!(
            head_unit_name = %resp.head_unit_name,
            channel_ids = ?channel_ids,
            "service discovery complete"
        );
        Ok(channel_ids)
    }

    // ── Channel open ──────────────────────────────────────────────────────────

    /// Send `ChannelOpenRequest` on each channel the head unit advertised and
    /// wait for `ChannelOpenResponse` per channel.
    ///
    /// Each request is sent on the **specific channel** (not the control
    /// channel) because the head unit's per-channel service handles it there.
    async fn open_channels(&mut self, channels: &[u32]) -> Result<()> {
        for &channel_id in channels {
            // The control channel (0) is always open; skip it.
            if channel_id == 0 {
                continue;
            }
            let ch = match ChannelId::try_from(channel_id as u8) {
                Ok(ch) => ch,
                Err(_) => {
                    warn!(
                        channel = channel_id,
                        "unknown channel ID from service discovery; skipping"
                    );
                    continue;
                }
            };

            let req = aap_proto::ChannelOpenRequest {
                priority: 1,
                channel_id: channel_id as i32,
            };
            let frame = encode_control_on(ch, MessageType::ChannelOpenRequest, &req);
            self.transport.send_frame(frame).await?;

            let resp_frame = self.transport.recv_frame().await?;
            self.expect_control_msg(&resp_frame.payload, MessageType::ChannelOpenResponse)?;
            info!(channel = ?ch, "channel opened");
        }
        Ok(())
    }

    // ── Post-channel initialisation ───────────────────────────────────────────

    /// Send the setup messages that must originate from the phone after all
    /// channels are open.
    ///
    /// Protocol summary:
    /// - Sensor channel (1): phone sends `SENSOR_MESSAGE_REQUEST` for each sensor
    ///   type it wants; head unit responds with `SENSOR_MESSAGE_RESPONSE` and
    ///   periodic `SENSOR_MESSAGE_BATCH` payloads.
    /// - Media channels (3–6): phone sends `MEDIA_MESSAGE_SETUP` with the codec
    ///   type; head unit responds with `MEDIA_MESSAGE_CONFIG`; phone then sends
    ///   `MEDIA_MESSAGE_START` (handled in `dispatch_loop` after receiving CONFIG).
    async fn post_channel_init(&mut self, channels: &[u32]) -> Result<()> {
        let has = |id: u32| channels.contains(&id);

        // ── Sensor channel ───────────────────────────────────────────────────
        if has(ChannelId::Sensor.as_u8() as u32) {
            info!("sensor: requesting driving-status and night-mode data");
            // SensorRequest proto: { type(varint,f1), min_update_period(varint,f2) }
            for sensor_type in [SENSOR_TYPE_DRIVING_STATUS, SENSOR_TYPE_NIGHT_MODE] {
                let body = Bytes::from(vec![0x08, sensor_type, 0x10, 0x00]);
                let frame = build_data_frame(ChannelId::Sensor, SENSOR_MSG_REQUEST, body);
                self.transport.send_frame(frame).await?;
            }
        }

        // ── Video channel ────────────────────────────────────────────────────
        if has(ChannelId::Video.as_u8() as u32) {
            info!("video: sending channel setup (H.264 BP)");
            // Setup proto: { type(varint,f1) = MEDIA_CODEC_VIDEO_H264_BP }
            let body = Bytes::from(vec![0x08, MEDIA_CODEC_VIDEO_H264_BP]);
            let frame = build_data_frame(ChannelId::Video, MEDIA_MSG_SETUP, body);
            self.transport.send_frame(frame).await?;
        }

        // ── Audio channels ───────────────────────────────────────────────────
        for (ch_id, channel) in [
            (ChannelId::MediaAudio.as_u8() as u32, ChannelId::MediaAudio),
            (
                ChannelId::SpeechAudio.as_u8() as u32,
                ChannelId::SpeechAudio,
            ),
            (
                ChannelId::SystemAudio.as_u8() as u32,
                ChannelId::SystemAudio,
            ),
        ] {
            if has(ch_id) {
                info!(?channel, "audio: sending channel setup (PCM)");
                // Setup proto: { type(varint,f1) = MEDIA_CODEC_AUDIO_PCM }
                let body = Bytes::from(vec![0x08, MEDIA_CODEC_AUDIO_PCM]);
                let frame = build_data_frame(channel, MEDIA_MSG_SETUP, body);
                self.transport.send_frame(frame).await?;
            }
        }

        info!("post-channel init complete — entering dispatch loop");
        Ok(())
    }

    // ── Steady-state dispatch loop ────────────────────────────────────────────

    /// Read frames forever and dispatch them to services or handle control messages.
    ///
    /// Runs two background tickers alongside the receive loop:
    /// - **video** (~30 fps): forwards the latest encoded frame from the producer.
    /// - **audio** (10 ms): calls `tick()` on every registered audio service so
    ///   each mixer drains its sources and emits the next PCM chunk.
    async fn dispatch_loop(&mut self) -> Result<()> {
        let mut video_ticker = tokio::time::interval(VIDEO_FRAME_INTERVAL);
        video_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut audio_ticker = tokio::time::interval(AUDIO_FRAME_INTERVAL);
        audio_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                frame_result = self.transport.recv_frame() => {
                    let frame = frame_result?;

                    if frame.flags.contains(FrameFlags::CONTROL) {
                        let Some(mt_result) = parse_message_type(&frame.payload) else {
                            warn!("control frame with empty payload; skipping");
                            continue;
                        };

                        match mt_result {
                            Ok(MessageType::ShutdownRequest) => {
                                info!("shutdown requested by head unit");
                                let resp = encode_control(
                                    MessageType::ShutdownResponse,
                                    &aap_proto::ShutdownResponse {},
                                );
                                self.transport.send_frame(resp).await?;
                                return Ok(());
                            }
                            Ok(mt) => {
                                if let Err(e) = self.handle_control_frame(mt, &frame.payload).await {
                                    warn!("error handling control frame {:?}: {e}", mt);
                                }
                            }
                            Err(unknown) => {
                                warn!(id = unknown, "unknown control message type; skipping");
                            }
                        }
                    } else if let Err(e) = self.dispatch_data_frame(frame).await {
                        warn!("error dispatching data frame: {e}");
                    }
                }

                _ = video_ticker.tick(), if self.video_active => {
                    if let Err(e) = self.send_video_frame().await {
                        warn!("error sending video frame: {e}");
                    }
                }

                _ = audio_ticker.tick() => {
                    if let Err(e) = self.tick_audio_services().await {
                        warn!("error on audio tick: {e}");
                    }
                }
            }
        }
    }

    /// Call `tick()` on every registered audio service and forward returned frames.
    async fn tick_audio_services(&mut self) -> Result<()> {
        for channel in [
            ChannelId::MediaAudio,
            ChannelId::SpeechAudio,
            ChannelId::SystemAudio,
        ] {
            if let Some(svc) = self.registry.get_mut(channel) {
                let frames = svc.tick().await.map_err(AapError::Service)?;
                for frame in frames {
                    self.transport.send_frame(frame).await?;
                }
            }
        }
        Ok(())
    }

    /// Forward the latest video frame from the producer to the head unit, if
    /// a new one has arrived since the last tick.
    async fn send_video_frame(&mut self) -> Result<()> {
        if !self.frame_rx.has_changed().unwrap_or(false) {
            return Ok(());
        }
        // borrow_and_update marks the value as seen so has_changed returns
        // false until the producer sends the next frame.
        let payload = self.frame_rx.borrow_and_update().clone();
        if let Some(bytes) = payload {
            // bytes = [timestamp_us : u64 BE][H.264 Annex-B NAL unit(s)]
            let frame = build_data_frame(ChannelId::Video, MEDIA_MSG_DATA, bytes);
            self.transport.send_frame(frame).await?;
        }
        Ok(())
    }

    // ── Control frame handler ─────────────────────────────────────────────────

    /// Handle one control-channel message (other than `ShutdownRequest`).
    async fn handle_control_frame(&mut self, mt: MessageType, payload: &Bytes) -> Result<()> {
        match mt {
            MessageType::PingRequest => {
                let body = proto_body(payload);
                let req = aap_proto::PingRequest::decode(body)
                    .map_err(|e| AapError::Protocol(format!("PingRequest decode: {e}")))?;
                debug!(timestamp = req.timestamp, "ping");
                let resp = encode_control(
                    MessageType::PingResponse,
                    &aap_proto::PingResponse {
                        timestamp: req.timestamp,
                    },
                );
                self.transport.send_frame(resp).await?;
            }

            MessageType::NavigationFocusRequest => {
                let body = proto_body(payload);
                let req = aap_proto::NavigationFocusRequest::decode(body).map_err(|e| {
                    AapError::Protocol(format!("NavigationFocusRequest decode: {e}"))
                })?;
                debug!(nav_type = req.r#type, "navigation focus request");
                let resp = encode_control(
                    MessageType::NavigationFocusResponse,
                    &aap_proto::NavigationFocusResponse { r#type: req.r#type },
                );
                self.transport.send_frame(resp).await?;
            }

            MessageType::AudioFocusRequest => {
                let body = proto_body(payload);
                let req = aap_proto::AudioFocusRequest::decode(body)
                    .map_err(|e| AapError::Protocol(format!("AudioFocusRequest decode: {e}")))?;
                debug!(
                    audio_focus_type = req.audio_focus_type,
                    "audio focus request"
                );
                // Reply with GAIN (1) by default.
                let resp = encode_control(
                    MessageType::AudioFocusResponse,
                    &aap_proto::AudioFocusResponse {
                        audio_focus_state: 1, // enums::audio_focus_state::Enum::Gain
                    },
                );
                self.transport.send_frame(resp).await?;
            }

            other => {
                warn!(?other, "unhandled control message; skipping");
            }
        }

        Ok(())
    }

    // ── Data frame dispatch ───────────────────────────────────────────────────

    /// Extract the `message_id` from a data frame and dispatch it to the
    /// matching service, or handle channel-specific built-in logic.
    async fn dispatch_data_frame(&mut self, frame: aap_contracts::Frame) -> Result<()> {
        let channel = frame.channel;

        // Data frames: [msg_id_hi, msg_id_lo, <proto body>]
        if frame.payload.len() < 2 {
            warn!(?channel, "data frame payload too short; skipping");
            return Ok(());
        }
        let message_id = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
        let payload = frame.payload.slice(2..);

        match self.registry.get_mut(channel) {
            Some(service) => {
                let outbound = service
                    .handle(message_id, payload)
                    .await
                    .map_err(AapError::Service)?;
                for out_frame in outbound {
                    self.transport.send_frame(out_frame).await?;
                }
            }
            None => {
                // Built-in handling for channels without a registered service.
                match channel {
                    ChannelId::Sensor => {
                        self.handle_sensor_frame(message_id).await?;
                    }
                    ChannelId::Video => {
                        self.handle_video_sink_frame(message_id).await?;
                    }
                    ChannelId::MediaAudio | ChannelId::SpeechAudio | ChannelId::SystemAudio => {
                        debug!(
                            ?channel,
                            message_id, "audio: no service registered; dropping"
                        );
                    }
                    _ => {
                        warn!(
                            ?channel,
                            message_id, "no service registered for channel; dropping frame"
                        );
                    }
                }
            }
        }

        // Activate video streaming once the head unit grants projection focus.
        if channel == ChannelId::Video
            && message_id == MSG_VIDEO_FOCUS_NOTIFICATION
            && !self.video_active
        {
            info!("video: focus granted — starting test-pattern stream");
            self.video_active = true;
        }

        Ok(())
    }

    /// Handle a data frame on the sensor channel (1).
    ///
    /// The head unit responds to our `SENSOR_MESSAGE_REQUEST` with a
    /// `SENSOR_MESSAGE_RESPONSE`, then streams `SENSOR_MESSAGE_BATCH` payloads
    /// containing driving status, night mode, and GPS data.
    async fn handle_sensor_frame(&self, message_id: u16) -> Result<()> {
        match message_id {
            SENSOR_MSG_RESPONSE => {
                info!("sensor: request accepted by head unit");
            }
            SENSOR_MSG_BATCH => {
                debug!("sensor: received sensor batch");
            }
            other => {
                warn!(other, "sensor: unknown message id; dropping");
            }
        }
        Ok(())
    }

    /// Handle a data frame on the video channel (3).
    ///
    /// After `MEDIA_MESSAGE_SETUP` → `MEDIA_MESSAGE_CONFIG`, we send
    /// `MEDIA_MESSAGE_START` to start the rendering pipeline on the head unit,
    /// then send `VIDEO_FOCUS_REQUEST` so the head unit will grant focus via
    /// `VIDEO_FOCUS_NOTIFICATION` and we can start streaming.
    async fn handle_video_sink_frame(&mut self, message_id: u16) -> Result<()> {
        match message_id {
            MEDIA_MSG_CONFIG => {
                info!("video: received config, sending start + focus request");
                let start_body = Bytes::from_static(&[0x08, 0x01, 0x10, 0x00]);
                let start_frame = build_data_frame(ChannelId::Video, MEDIA_MSG_START, start_body);
                self.transport.send_frame(start_frame).await?;
                // VideoFocusRequest { mode=1 (PROJECTION) }
                let focus_body = Bytes::from_static(&[0x08, 0x01]);
                let focus_frame =
                    build_data_frame(ChannelId::Video, MSG_VIDEO_FOCUS_REQUEST, focus_body);
                self.transport.send_frame(focus_frame).await?;
            }
            other => {
                debug!(other, "video sink: unhandled message id; dropping");
            }
        }
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Assert that the first two bytes of `payload` equal `expected`.
    ///
    /// Returns the matched [`MessageType`] on success, or an
    /// [`AapError::Protocol`] if the payload is too short or the message type
    /// does not match.
    fn expect_control_msg(&self, payload: &Bytes, expected: MessageType) -> Result<MessageType> {
        match parse_message_type(payload) {
            None => Err(AapError::Protocol(format!(
                "expected {:?} but payload has fewer than 2 bytes",
                expected
            ))),
            Some(Err(unknown)) => Err(AapError::Protocol(format!(
                "expected {:?} but got unknown message id 0x{:04X}",
                expected, unknown
            ))),
            Some(Ok(mt)) if mt != expected => Err(AapError::Protocol(format!(
                "expected {:?} but got {:?}",
                expected, mt
            ))),
            Some(Ok(mt)) => Ok(mt),
        }
    }
}
