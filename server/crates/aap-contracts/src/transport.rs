//! Abstract byte-and-frame transport.
//!
//! Implemented by `aap-transport` (TCP today, USB later). Trait is
//! object-safe via `async-trait` so the rest of the stack can hold
//! `Box<dyn Transport>` without committing to a concrete impl.

use async_trait::async_trait;
use thiserror::Error;

use crate::frame::Frame;

/// Errors at the transport layer.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Peer closed the connection.
    #[error("connection closed")]
    Closed,

    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// TLS error during handshake or stream operation.
    #[error("tls: {0}")]
    Tls(String),

    /// Malformed frame on the wire.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),

    /// `upgrade_tls` called in an invalid state.
    #[error("invalid state: {0}")]
    InvalidState(&'static str),
}

/// Abstract bidirectional frame transport.
///
/// Implementations are responsible for:
/// - Wire framing (channel/flags/len/payload).
/// - Multi-frame reassembly: callers see only complete logical messages.
/// - Optional TLS upgrade via [`Transport::upgrade_tls`], to be invoked exactly
///   once between version negotiation and service discovery. After upgrade,
///   the implementation transparently wraps/unwraps TLS for any frame whose
///   `flags` contain `ENCRYPTED`.
#[async_trait]
pub trait Transport: Send {
    /// Read the next complete logical frame.
    async fn recv_frame(&mut self) -> Result<Frame, TransportError>;

    /// Write a complete logical frame. Implementation may fragment.
    async fn send_frame(&mut self, frame: Frame) -> Result<(), TransportError>;

    /// Perform the inline TLS handshake using AA-flavoured SSL handshake
    /// frames. Must be called exactly once, after `VersionResponse` and
    /// before any service discovery.
    async fn upgrade_tls(&mut self) -> Result<(), TransportError>;
}
