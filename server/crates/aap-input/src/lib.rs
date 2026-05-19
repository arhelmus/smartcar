//! `InputService` — Android Auto input channel (head unit → phone).
//!
//! The head unit is the physical input device (touchscreen / hard keys). It
//! relays each user interaction to us on the input channel. Unlike video this
//! is the one channel whose *data* flows toward the phone.
//!
//! # Message flow (verified against the vendored openauto/aasdk emulator)
//!
//! ```text
//! generic ChannelOpenRequest/Response  (aap-core opens the channel)
//! Phone → HU : KEY_BINDING_REQUEST  (0x8002)  empty keycode list
//! HU    → Phone : KEY_BINDING_RESPONSE (0x8003) status   ← unblocks input
//! HU    → Phone : INPUT_REPORT       (0x8001)  InputReport (touch / keys)
//! ```
//!
//! openauto does **not** emit any touch events until it receives the
//! `KEY_BINDING_REQUEST`; that request is sent by
//! `aap_core::Connection::post_channel_init`. This service handles the
//! inbound `0x8003` (log only) and `0x8001` (decode + forward) messages.
//!
//! `InputReport` (newer `aap_protobuf` schema spoken by openauto) is
//! wire-compatible with the AAProto `InputEventIndication` / `TouchEvent` /
//! `TouchLocation` types: identical field numbers, and `PointerAction`
//! values equal `TouchAction` values.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use prost::Message;
use tracing::{debug, info, warn};

use aap_contracts::{ChannelId, Frame, Service, ServiceDescriptor, ServiceError};

// ── Input-channel message ids (aap_protobuf InputMessageId) ───────────────────

/// HU → Phone: a touch / key `InputReport`.
const MSG_INPUT_REPORT: u16 = 0x8001;
/// Phone → HU: key-binding request (sent by `aap-core`, not this service).
const MSG_KEY_BINDING_REQUEST: u16 = 0x8002;
/// HU → Phone: key-binding response — arrival means the HU started input.
const MSG_KEY_BINDING_RESPONSE: u16 = 0x8003;

// ── PointerAction / TouchAction values (wire-identical) ───────────────────────
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const ACTION_MOVED: i32 = 2;
const ACTION_POINTER_DOWN: i32 = 5;
const ACTION_POINTER_UP: i32 = 6;

/// A single touch transition delivered to a [`PointerSink`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerPhase {
    /// Finger touched the surface.
    Down,
    /// Finger moved while down.
    Move,
    /// Finger left the surface.
    Up,
}

/// Receives decoded pointer transitions from the input channel.
///
/// Implemented by the renderer (e.g. the Flutter embedder) so head-unit
/// touches drive the projected UI. Coordinates are in the negotiated video
/// resolution's pixel space — which, for every catalog config (zero margin,
/// 160 dpi → pixel ratio 1.0), maps 1:1 onto the renderer surface.
pub trait PointerSink: Send + Sync {
    /// Deliver one pointer transition. `x`/`y` are display pixels;
    /// `timestamp_us` is the head unit's monotonic clock in microseconds.
    fn pointer(&self, phase: PointerPhase, x: f64, y: f64, timestamp_us: u64);
}

/// A [`PointerSink`] that just logs — used when no renderer is wired
/// (testkit / `--features flutter` off) so the protocol path is still
/// exercised and touches are visible in the log.
pub struct LogPointerSink;

impl PointerSink for LogPointerSink {
    fn pointer(&self, phase: PointerPhase, x: f64, y: f64, timestamp_us: u64) {
        info!(
            ?phase,
            x, y, timestamp_us, "input: touch (no renderer wired)"
        );
    }
}

/// Android Auto input service: decodes head-unit touch reports and forwards
/// them to a [`PointerSink`].
pub struct InputService {
    sink: Arc<dyn PointerSink>,
}

impl InputService {
    /// Create an input service that forwards decoded touches to `sink`.
    pub fn new(sink: Arc<dyn PointerSink>) -> Self {
        Self { sink }
    }

    fn forward_touch(&self, report: &aap_proto::msgs::InputEventIndication) {
        let Some(touch) = report.touch_event.as_ref() else {
            return;
        };
        let phase = match touch.touch_action {
            ACTION_DOWN | ACTION_POINTER_DOWN => PointerPhase::Down,
            ACTION_MOVED => PointerPhase::Move,
            ACTION_UP | ACTION_POINTER_UP => PointerPhase::Up,
            other => {
                debug!(action = other, "input: unknown touch action; dropping");
                return;
            }
        };

        // The pointer that changed state is at `action_index`; fall back to
        // the first location if the index is out of range.
        let loc = touch
            .touch_location
            .get(touch.action_index as usize)
            .or_else(|| touch.touch_location.first());
        let Some(loc) = loc else {
            debug!("input: touch event with no locations; dropping");
            return;
        };

        debug!(
            ?phase,
            x = loc.x,
            y = loc.y,
            pointer_id = loc.pointer_id,
            "input: forwarding touch"
        );
        self.sink
            .pointer(phase, loc.x as f64, loc.y as f64, report.timestamp);
    }
}

#[async_trait]
impl Service for InputService {
    fn channel(&self) -> ChannelId {
        ChannelId::InputSource
    }

    fn descriptor(&self) -> ServiceDescriptor {
        use aap_proto::data::{ChannelDescriptor, InputChannel};

        let cd = ChannelDescriptor {
            channel_id: 0,
            sensor_channel: None,
            av_channel: None,
            input_channel: Some(InputChannel {
                supported_keycodes: vec![],
                touch_screen_config: None,
                touch_pad_config: None,
            }),
            av_input_channel: None,
            bluetooth_channel: None,
            navigation_channel: None,
            media_info_channel: None,
            vendor_extension_channel: None,
        };
        let mut buf = Vec::with_capacity(cd.encoded_len());
        cd.encode(&mut buf).expect("encoding into Vec never fails");
        ServiceDescriptor {
            channel: ChannelId::InputSource,
            descriptor_bytes: Bytes::from(buf),
        }
    }

    async fn handle(
        &mut self,
        message_id: u16,
        payload: Bytes,
    ) -> Result<Vec<Frame>, ServiceError> {
        match message_id {
            MSG_INPUT_REPORT => {
                let report = aap_proto::msgs::InputEventIndication::decode(payload)
                    .map_err(|e| ServiceError::InvalidPayload(format!("InputReport: {e}")))?;
                self.forward_touch(&report);
                Ok(vec![])
            }
            MSG_KEY_BINDING_RESPONSE => {
                info!("input: key-binding response received — head unit now sending input");
                Ok(vec![])
            }
            MSG_KEY_BINDING_REQUEST => {
                // We send this; receiving it would be a protocol error.
                warn!("input: unexpected inbound KEY_BINDING_REQUEST; ignoring");
                Ok(vec![])
            }
            unknown => {
                warn!(
                    message_id = unknown,
                    "input: unsupported message id; dropping"
                );
                Ok(vec![])
            }
        }
    }
}
