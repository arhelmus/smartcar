//! TCP/USB transport with TLS upgrade for Android Auto projection.
//!
//! This crate implements the [`Transport`] trait from `aap-contracts` over a
//! TCP connection. It provides:
//!
//! - Wire-format frame encoding and decoding ([`codec`]).
//! - Multi-frame reassembly keyed by channel ([`TcpTransport`]).
//! - A TLS upgrade hook (stub — full handshake implemented in a later work item).

#![warn(missing_docs)]

mod codec;
mod tcp;

pub use tcp::TcpTransport;
