//! Connection state machine and service registry for the Android Auto protocol.
//!
//! # Overview
//!
//! This crate provides the top-level protocol driver that connects the wire
//! transport ([`aap_contracts::Transport`]) with per-channel service
//! implementations ([`aap_contracts::Service`]).
//!
//! The entry point is [`Connection`], which drives the AA control-channel
//! handshake (version negotiation → TLS → service discovery → channel open)
//! and then dispatches data frames to the registered services.
//!
//! # Quick start
//!
//! ```no_run
//! # use aap_core::{Connection, ServiceRegistry};
//! # async fn example<T: aap_contracts::Transport>(transport: T) {
//! let registry = ServiceRegistry::new();
//! let conn = Connection::new(transport, registry);
//! conn.run().await.unwrap();
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod connection;
mod control;
mod registry;
mod video_encoder;

pub use connection::Connection;
pub use registry::ServiceRegistry;
