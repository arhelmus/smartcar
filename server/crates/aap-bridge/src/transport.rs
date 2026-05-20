//! Bridge transport dispatcher.
//!
//! Picks between the BLE GATT impl ([`crate::gatt`], Linux only) and the TCP
//! impl ([`crate::tcp`], all platforms). The two transports carry the same
//! protobuf surface — clients written against [`ControlRequest`] /
//! [`ControlEvent`] are byte-identical on either pipe.

use std::net::SocketAddr;

use tokio::sync::{broadcast, mpsc};

use crate::{ControlEvent, ControlRequest, DeviceInfo};

/// Which transport carries the bridge control plane.
#[derive(Clone, Debug)]
pub enum BridgeTransport {
    /// TCP server with length-prefixed protobuf framing. Used for local dev
    /// so the iOS Simulator (which has no Bluetooth at all) can talk to a
    /// `smartcar-server` running on the same Mac.
    Tcp(SocketAddr),

    /// BLE GATT server via BlueZ. The production transport on the board.
    /// Linux only — on other hosts this resolves to a no-op with a warning,
    /// so the same binary can be built and run on the dev Mac.
    Ble,

    /// Bridge disabled. The task parks; no listener is opened.
    None,
}

/// Run the bridge with the selected transport. Returns when the transport
/// terminates; under normal operation it never does.
pub async fn run_bridge(
    transport: BridgeTransport,
    info: DeviceInfo,
    cmd_tx: mpsc::Sender<ControlRequest>,
    evt_tx: broadcast::Sender<ControlEvent>,
) -> anyhow::Result<()> {
    match transport {
        BridgeTransport::None => {
            tracing::info!("bridge: disabled");
            std::future::pending::<()>().await;
            Ok(())
        }
        BridgeTransport::Tcp(addr) => crate::tcp::run(addr, info, cmd_tx, evt_tx).await,
        BridgeTransport::Ble => {
            #[cfg(target_os = "linux")]
            {
                crate::gatt::run(info, cmd_tx, evt_tx).await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (info, cmd_tx, evt_tx);
                tracing::warn!(
                    "bridge: BLE requested but BlueZ unavailable on this platform — \
                     bridge disabled"
                );
                std::future::pending::<()>().await;
                Ok(())
            }
        }
    }
}
