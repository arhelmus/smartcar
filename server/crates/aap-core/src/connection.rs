//! Android Auto control-channel state machine.
//!
//! [`Connection`] drives the full AA setup sequence (version negotiation, TLS
//! upgrade, service discovery, channel open) and then enters a steady-state
//! dispatch loop that routes data frames to the appropriate service.

use bytes::Bytes;
use prost::Message;
use tracing::{debug, info, warn};

use aap_contracts::{AapError, Result};
use aap_contracts::{ChannelId, FrameFlags, MessageType, Transport};

use crate::control::{encode_control, parse_message_type, proto_body};
use crate::registry::ServiceRegistry;

// ── Wire constants ────────────────────────────────────────────────────────────

/// Major version of the AA protocol we support.
const AA_VERSION_MAJOR: u16 = 1;
/// Minor version of the AA protocol we advertise.
const AA_VERSION_MINOR: u16 = 1;

// ── Connection ────────────────────────────────────────────────────────────────

/// Drives the Android Auto control-channel protocol over a [`Transport`].
///
/// Create one instance per accepted connection and call [`Connection::run`].
/// The instance is consumed when `run` returns.
pub struct Connection<T: Transport> {
    transport: T,
    registry: ServiceRegistry,
}

impl<T: Transport> Connection<T> {
    /// Wrap a transport and a service registry into a new connection.
    pub fn new(transport: T, registry: ServiceRegistry) -> Self {
        Self {
            transport,
            registry,
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
        self.service_discovery().await?;
        self.open_channels().await?;
        self.dispatch_loop().await
    }

    // ── Version negotiation ───────────────────────────────────────────────────

    /// Receive the head unit's `VersionRequest` and reply with `VersionResponse`.
    async fn handshake_version(&mut self) -> Result<()> {
        let frame = self.transport.recv_frame().await?;
        let mt = self.expect_control_msg(&frame.payload, MessageType::VersionRequest)?;
        debug!(?mt, "received VersionRequest");

        // The head unit sends: [major_hi, major_lo, minor_hi, minor_lo].
        // We reply with its major + our minor, and status=MATCH (0x0000).
        let body = proto_body(&frame.payload);
        let peer_major = if body.len() >= 2 {
            u16::from_be_bytes([body[0], body[1]])
        } else {
            AA_VERSION_MAJOR
        };

        // Build VersionResponse: [major_hi, major_lo, minor_hi, minor_lo, status_hi, status_lo]
        // This message has no protobuf encoding — it is a raw 6-byte payload.
        let mut payload = Vec::with_capacity(8);
        let msg_id = MessageType::VersionResponse.as_u16();
        payload.push((msg_id >> 8) as u8);
        payload.push((msg_id & 0xFF) as u8);
        payload.push((peer_major >> 8) as u8);
        payload.push((peer_major & 0xFF) as u8);
        payload.push((AA_VERSION_MINOR >> 8) as u8);
        payload.push((AA_VERSION_MINOR & 0xFF) as u8);
        // status = 0x0000 (MATCH)
        payload.push(0x00);
        payload.push(0x00);

        let resp = aap_contracts::Frame::control_bulk(ChannelId::Control, Bytes::from(payload));
        self.transport.send_frame(resp).await?;
        info!(
            "version negotiation complete (peer major={peer_major}, our minor={AA_VERSION_MINOR})"
        );
        Ok(())
    }

    // ── TLS handshake ─────────────────────────────────────────────────────────

    /// Exchange `SslHandshake` frames and drive the TLS upgrade.
    ///
    /// [`Transport::upgrade_tls`] is currently a `todo!()` in
    /// `aap-transport`; wrapping the call here means the binary compiles and
    /// the `todo` panic surfaces at runtime rather than blocking compilation.
    async fn handshake_tls(&mut self) -> Result<()> {
        // Receive the head unit's first SslHandshake frame (TLS ClientHello).
        let frame = self.transport.recv_frame().await?;
        self.expect_control_msg(&frame.payload, MessageType::SslHandshake)?;
        debug!("received SslHandshake frame from head unit");

        // Delegate the full TLS state machine to the transport.
        // upgrade_tls() will feed/drain TLS frames internally; it surfaces any
        // error (including the current todo!()) as TransportError::Tls.
        match self.transport.upgrade_tls().await {
            Ok(()) => {
                info!("TLS upgrade successful");
                Ok(())
            }
            Err(e) => {
                // Surface TLS errors as AapError::Transport so callers can handle
                // or log them consistently.
                Err(AapError::Transport(e))
            }
        }
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

    /// Receive `ServiceDiscoveryRequest` and respond with the list of services.
    async fn service_discovery(&mut self) -> Result<()> {
        let frame = self.transport.recv_frame().await?;
        self.expect_control_msg(&frame.payload, MessageType::ServiceDiscoveryRequest)?;

        let body = proto_body(&frame.payload);
        let req = aap_proto::ServiceDiscoveryRequest::decode(body)
            .map_err(|e| AapError::Protocol(format!("ServiceDiscoveryRequest decode: {e}")))?;
        info!(
            device_name = %req.device_name,
            device_brand = %req.device_brand,
            "service discovery request"
        );

        // Build channel descriptors from registered services.
        let channels = self.build_channel_descriptors();

        let resp = aap_proto::ServiceDiscoveryResponse {
            channels,
            head_unit_name: "Smartcar".into(),
            car_model: "Generic".into(),
            car_year: "2024".into(),
            car_serial: "SC-0001".into(),
            left_hand_drive_vehicle: true,
            headunit_manufacturer: "Smartcar".into(),
            headunit_model: "v0.1".into(),
            sw_build: env!("CARGO_PKG_VERSION").into(),
            sw_version: env!("CARGO_PKG_VERSION").into(),
            can_play_native_media_during_vr: false,
            hide_clock: None,
        };

        let frame = encode_control(MessageType::ServiceDiscoveryResponse, &resp);
        self.transport.send_frame(frame).await?;
        info!(
            "service discovery response sent ({} channel(s))",
            resp.channels.len()
        );
        Ok(())
    }

    /// Build a [`Vec`] of proto `ChannelDescriptor`s from the service registry.
    ///
    /// Each service's [`aap_contracts::ServiceDescriptor::descriptor_bytes`]
    /// contains a prost-encoded `ChannelDescriptor` body (less the
    /// `channel_id` field) as produced by that service's crate.  Here we
    /// reconstruct the full descriptor so the `channel_id` field is always set
    /// from the registry key — we don't trust the opaque bytes to carry it.
    fn build_channel_descriptors(&self) -> Vec<aap_proto::ChannelDescriptor> {
        self.registry
            .descriptors()
            .into_iter()
            .map(|desc| {
                // Try to decode the service's opaque descriptor bytes into a
                // ChannelDescriptor.  If decoding fails fall back to a minimal
                // descriptor that at least carries the channel_id so the head
                // unit knows the channel exists.
                let mut cd =
                    aap_proto::ChannelDescriptor::decode(desc.descriptor_bytes).unwrap_or_default();
                cd.channel_id = aap_proto::bridge::channel_id_to_u32(desc.channel);
                cd
            })
            .collect()
    }

    // ── Channel open ──────────────────────────────────────────────────────────

    /// Process all `ChannelOpenRequest` frames until the head unit stops sending
    /// them.
    ///
    /// The AA spec does not include an explicit end-of-open-requests marker, so
    /// we peek at incoming frames: any non-`ChannelOpenRequest` control message
    /// is buffered via the loop peeking and handled by [`Self::dispatch_loop`].
    async fn open_channels(&mut self) -> Result<()> {
        loop {
            let frame = self.transport.recv_frame().await?;

            let Some(mt_result) = parse_message_type(&frame.payload) else {
                return Err(AapError::Protocol("empty control frame payload".into()));
            };

            match mt_result {
                Ok(MessageType::ChannelOpenRequest) => {
                    let body = proto_body(&frame.payload);
                    let req = aap_proto::ChannelOpenRequest::decode(body).map_err(|e| {
                        AapError::Protocol(format!("ChannelOpenRequest decode: {e}"))
                    })?;

                    let channel_id_raw = req.channel_id as u8;
                    info!(
                        channel = channel_id_raw,
                        priority = req.priority,
                        "channel open request"
                    );

                    // Reply with status=OK (0).
                    let resp = aap_proto::ChannelOpenResponse {
                        status: 0, // enums::status::Enum::Ok
                    };
                    let resp_frame = encode_control(MessageType::ChannelOpenResponse, &resp);
                    self.transport.send_frame(resp_frame).await?;
                }

                // Any other message signals the end of the channel-open phase.
                // Handle it inside the dispatch loop.
                Ok(mt) => {
                    debug!(
                        ?mt,
                        "end of channel-open phase; dispatching first steady-state frame"
                    );
                    self.handle_control_frame(mt, &frame.payload).await?;
                    return Ok(());
                }

                Err(unknown) => {
                    warn!(
                        id = unknown,
                        "unknown message type in channel-open phase; skipping"
                    );
                }
            }
        }
    }

    // ── Steady-state dispatch loop ────────────────────────────────────────────

    /// Read frames forever and dispatch them to services or handle control messages.
    async fn dispatch_loop(&mut self) -> Result<()> {
        loop {
            let frame = self.transport.recv_frame().await?;

            if frame.flags.contains(FrameFlags::CONTROL) {
                // Control-channel frame.
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
            } else {
                // Data frame — dispatch to the registered service.
                if let Err(e) = self.dispatch_data_frame(frame).await {
                    warn!("error dispatching data frame: {e}");
                }
            }
        }
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
                // Reply with GAIN (1) by default — W7 will implement proper focus.
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
    /// matching service.  Sends back any frames returned by the service.
    async fn dispatch_data_frame(&mut self, frame: aap_contracts::Frame) -> Result<()> {
        let channel = frame.channel;

        // Data frames have [msg_id_hi, msg_id_lo, <proto body>] too, matching
        // the per-channel convention used by aap-contracts Service::handle.
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
                warn!(
                    ?channel,
                    message_id, "no service registered for channel; dropping frame"
                );
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
