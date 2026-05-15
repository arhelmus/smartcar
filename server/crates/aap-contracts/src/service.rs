//! Per-channel service abstraction consumed by `aap-core`.

use async_trait::async_trait;
use bytes::Bytes;
use thiserror::Error;

use crate::{channel::ChannelId, frame::Frame};

/// Opaque service descriptor used to populate
/// `ServiceDiscoveryResponse.services[]` on the control channel.
///
/// The byte payload is the protobuf-encoded `Service` sub-message defined in
/// AAProto. `aap-contracts` deliberately does not depend on `aap-proto`, so
/// the bytes are produced by individual service crates (which depend on
/// `aap-proto`) and shuttled to the control plane opaquely.
#[derive(Debug, Clone)]
pub struct ServiceDescriptor {
    /// The channel this descriptor advertises.
    pub channel: ChannelId,
    /// Encoded `Service` protobuf body, less the channel id field.
    pub descriptor_bytes: Bytes,
}

/// Errors raised by a [`Service`] while handling an inbound message.
#[derive(Debug, Error)]
pub enum ServiceError {
    /// Inbound `message_id` is not recognised by this service.
    #[error("unsupported message: 0x{0:04X}")]
    UnsupportedMessage(u16),

    /// Payload failed to decode against the expected schema.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// Catch-all for internal failures inside the service implementation.
    #[error("internal: {0}")]
    Internal(String),
}

/// Implemented by every per-channel service (Video, Input, Sensor, …).
///
/// `aap-core` owns a registry keyed by `ChannelId`, dispatches inbound frames
/// to `handle`, and writes returned frames back via `Transport`.
///
/// Services are stateful and per-connection. Construct a fresh instance for
/// each accepted connection.
#[async_trait]
pub trait Service: Send {
    /// The channel this service serves.
    fn channel(&self) -> ChannelId;

    /// Descriptor for service discovery.
    fn descriptor(&self) -> ServiceDescriptor;

    /// Handle one inbound message, returning zero or more outbound frames.
    ///
    /// `message_id` is the channel-specific u16 stripped from the head of the
    /// inbound payload; `payload` is the remainder (the protobuf body).
    async fn handle(&mut self, message_id: u16, payload: Bytes)
        -> Result<Vec<Frame>, ServiceError>;
}
