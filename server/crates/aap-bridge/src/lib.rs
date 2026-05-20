//! Bridge between the iPhone app and `smartcar-server`.
//!
//! Carries protobuf-framed `ControlRequest` writes (phone → board) and
//! `ControlEvent` notifications (board → phone). Audio (A2DP) and internet
//! (PAN/BNEP) are separate classic-Bluetooth profiles handled by BlueZ at the
//! system level — this crate only owns the LE control/signalling channel
//! that coordinates them.
//!
//! Two transports are supported behind the same protobuf API:
//!
//! - **TCP** ([`BridgeTransport::Tcp`]) — for local dev so the iOS Simulator
//!   (which cannot do Bluetooth) can talk to a `smartcar-server` on the
//!   same Mac. Default mode of the binary.
//! - **BLE GATT** ([`BridgeTransport::Ble`]) — production transport, Linux
//!   only. Selected by the board's run/boot scripts via `--bridge ble`.
//!
//! # GATT layout (BLE transport)
//!
//! ```text
//! Service:  SERVICE_UUID
//! ├─ Command  (Write, Write-Without-Response) ── COMMAND_UUID — ControlRequest
//! ├─ Event    (Notify)                        ── EVENT_UUID   — ControlEvent
//! └─ Info     (Read)                          ── INFO_UUID    — Info
//! ```
//!
//! # Quick start
//!
//! ```no_run
//! use std::net::SocketAddr;
//! use aap_bridge::{run_bridge, BridgeTransport, DeviceInfo};
//! use tokio::sync::{broadcast, mpsc};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let (cmd_tx, _cmd_rx) = mpsc::channel(64);
//! let (evt_tx, _)       = broadcast::channel(64);
//!
//! let transport = BridgeTransport::Tcp("127.0.0.1:4789".parse::<SocketAddr>()?);
//!
//! run_bridge(
//!     transport,
//!     DeviceInfo {
//!         name: "Smartcar".into(),
//!         firmware_version: env!("CARGO_PKG_VERSION").into(),
//!         protocol_version: 1,
//!     },
//!     cmd_tx,
//!     evt_tx,
//! ).await
//! # }
//! ```

#![warn(missing_docs)]

use uuid::{uuid, Uuid};

#[cfg(target_os = "linux")]
mod gatt;
mod tcp;
mod transport;

pub use transport::{run_bridge, BridgeTransport};

/// Prost-generated protobuf types for the bridge wire format.
pub mod proto {
    #![allow(missing_docs)]
    include!(concat!(env!("OUT_DIR"), "/smartcar.bridge.v1.rs"));
}

pub use proto::{ControlEvent, ControlRequest, Info};

/// 128-bit UUID of the smartcar bridge GATT service (BLE transport only).
pub const SERVICE_UUID: Uuid = uuid!("7c63a8ee-3e84-4b8b-9a5d-a8d4d0d24c50");
/// Command characteristic — phone writes `ControlRequest` protobufs here.
pub const COMMAND_UUID: Uuid = uuid!("7c63a8ee-3e84-4b8b-9a5d-a8d4d0d24c51");
/// Event characteristic — board notifies `ControlEvent` protobufs here.
pub const EVENT_UUID: Uuid = uuid!("7c63a8ee-3e84-4b8b-9a5d-a8d4d0d24c52");
/// Info characteristic — read returns a fixed [`Info`] protobuf.
pub const INFO_UUID: Uuid = uuid!("7c63a8ee-3e84-4b8b-9a5d-a8d4d0d24c53");

/// Static device info served on the Info characteristic (BLE) or sent as the
/// first frame on connect (TCP).
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    /// Advertised local name (BLE) and `Info.device_name`.
    pub name: String,
    /// Build/firmware version reported to the iOS app.
    pub firmware_version: String,
    /// Bridge protocol version; bump on breaking proto changes.
    pub protocol_version: u32,
}

impl DeviceInfo {
    pub(crate) fn to_proto(&self) -> Info {
        Info {
            device_name: self.name.clone(),
            firmware_version: self.firmware_version.clone(),
            protocol_version: self.protocol_version,
        }
    }
}
