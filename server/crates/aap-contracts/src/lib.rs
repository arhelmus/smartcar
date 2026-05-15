//! Wire-protocol-agnostic contracts shared across `aap-*` crates.
//!
//! This crate is deliberately tiny. It exists so that `aap-transport`,
//! `aap-proto`, `aap-core`, and service crates (`aap-video`, etc.) can be
//! developed and tested in isolation by depending only on traits and POD types
//! defined here.
//!
//! No I/O. No protobuf. No async runtime assumptions beyond `Send`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod channel;
pub mod frame;
pub mod message;
pub mod service;
pub mod transport;

pub use channel::ChannelId;
pub use frame::{Frame, FrameFlags, FrameType};
pub use message::MessageType;
pub use service::{Service, ServiceDescriptor, ServiceError};
pub use transport::{Transport, TransportError};

/// Top-level error type usable by binaries that compose multiple layers.
#[derive(Debug, thiserror::Error)]
pub enum AapError {
    /// Transport-level failure (TCP, USB, TLS, framing).
    #[error("transport: {0}")]
    Transport(#[from] TransportError),

    /// Service-level failure (bad message, internal logic).
    #[error("service: {0}")]
    Service(#[from] ServiceError),

    /// Protocol violation that doesn't fit the layered errors above.
    #[error("protocol: {0}")]
    Protocol(String),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, AapError>;
