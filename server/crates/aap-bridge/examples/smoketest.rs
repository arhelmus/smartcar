//! End-to-end smoke test for `aap-bridge`.
//!
//! Spawns the bridge with the selected transport and:
//!   * logs every decoded `ControlRequest` from a connected client,
//!   * emits a heartbeat `Status` event every 2 s so a subscribed client sees
//!     notifications without any board-side activity.
//!
//! ```sh
//! # TCP (default) — connect from anything that speaks the framing.
//! RUST_LOG=info cargo run --example smoketest -p aap-bridge
//!
//! # BLE — Linux only, probe from nRF Connect / LightBlue.
//! RUST_LOG=info cargo run --example smoketest -p aap-bridge -- --ble
//! ```

use std::env;
use std::time::Duration;

use aap_bridge::proto::{control_event::Evt, ControlEvent, Status};
use aap_bridge::{run_bridge, BridgeTransport, DeviceInfo};
use tokio::sync::{broadcast, mpsc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let transport = if env::args().any(|a| a == "--ble") {
        BridgeTransport::Ble
    } else {
        BridgeTransport::Tcp("127.0.0.1:4789".parse()?)
    };

    let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
    let (evt_tx, _) = broadcast::channel::<ControlEvent>(64);

    // Sink commands → log.
    tokio::spawn(async move {
        while let Some(req) = cmd_rx.recv().await {
            tracing::info!(?req, "smoketest: command");
        }
    });

    // Heartbeat events so a subscribed client sees something.
    let evt_tx_h = evt_tx.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            let evt = ControlEvent {
                evt: Some(Evt::Status(Status {
                    current_app: "smoketest".into(),
                    audio_connected: false,
                    net_connected: false,
                })),
            };
            // No subscribers is fine — broadcast returns Err but we don't care.
            let _ = evt_tx_h.send(evt);
        }
    });

    run_bridge(
        transport,
        DeviceInfo {
            name: "Smartcar".into(),
            firmware_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: 1,
        },
        cmd_tx,
        evt_tx,
    )
    .await
}
