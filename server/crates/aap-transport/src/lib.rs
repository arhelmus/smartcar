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

// Bluetooth (AAW) bootstrap is Linux-only AND requires the aasdk submodule
// to be present so prost-build can generate the aaw protos at build time. The
// build script sets `aap_transport_no_aaw` when those protos are missing, so
// macOS dev builds and barebones CI both keep compiling.
#[cfg(all(target_os = "linux", not(aap_transport_no_aaw)))]
mod bt;

pub use tcp::TcpTransport;

#[cfg(target_os = "linux")]
pub use usb::UsbTransport;

#[cfg(all(target_os = "linux", not(aap_transport_no_aaw)))]
pub use bt::{BtConfig, BtTransport};
