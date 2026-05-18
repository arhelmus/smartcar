//! TCP/USB transports with TLS upgrade for Android Auto projection.
//!
//! Transports available:
//!
//! - [`TcpTransport`] — TCP connection; used for local development against
//!   the openauto emulator.
//! - [`UsbTransport`] — USB FunctionFS gadget with AOAP two-persona handshake;
//!   Linux only; used when the board is connected directly to a car head unit
//!   or to a laptop running openauto in USB host mode.
//!
//! Both implement the [`Transport`] trait from `aap-contracts` with the same
//! frame codec, multi-frame reassembly, and TLS semantics.

#![warn(missing_docs)]

mod codec;
mod tcp;
mod tls;

#[cfg(target_os = "linux")]
mod usb;

pub use tcp::TcpTransport;

#[cfg(target_os = "linux")]
pub use usb::UsbTransport;
