//! Bluetooth (AAW) bootstrap that ends in a `TcpTransport`.
//!
//! Same shape as the USB transport at `src/usb/`: a `connect()` constructor
//! that performs all the system-level bring-up (discoverable + Just Works
//! agent → wait for car-initiated pair → RFCOMM → WiFi handshake → join
//! HU's hotspot → TCP connect) and yields a real [`Transport`]
//! implementation. From the rest of the stack's point of view the result is
//! indistinguishable from `TcpTransport::new(stream)` — once on the HU's
//! WiFi the AA wire protocol is plain TCP+TLS, byte-for-byte identical to
//! the openauto dev transport.
//!
//! # Operator flow
//!
//! Pairing is **car-initiated**: open the car's AA Wireless setup, pick
//! `Smartcar`, accept Just Works. No `--bt-target`, no `bluetoothctl pair`,
//! no env var. After the first pairing the bond is cached in BlueZ and
//! subsequent boots auto-resume.
//!
//! # System requirements (board side)
//!
//! - BlueZ ≥ 5.55 with the `Class=0x6c020c` (phone) tuning from the ansible
//!   `bluetooth` role.
//! - wpa_supplicant + nl80211 driver for the wlan0 interface.
//! - systemd-networkd configured to DHCP wlan0 (the `network` role ships
//!   `/etc/systemd/network/40-wlan0.network`).
//! - `iw`, `ip` userspace tools on PATH.
//!
//! Provisioned by `python3 scripts/board_provision.py` and documented in
//! `docs/board-setup.md`.

mod error;
mod handshake;
mod pair;
mod proto;
mod rfcomm;
mod wpa;

use std::time::Duration;

use tokio::net::TcpStream;
use tracing::info;

pub use error::BtError;

use crate::TcpTransport;

/// Configuration for [`BtTransport::connect`].
#[derive(Debug, Clone)]
pub struct BtConfig {
    /// How long to keep the adapter discoverable + Just Works agent
    /// registered while waiting for the operator to pair the car. Default
    /// 5 minutes — generous enough for the operator to walk to the car,
    /// open AA setup, and tap `Smartcar`, but bounded so a misconfigured
    /// board fails fast rather than burning power forever.
    pub pair_wait_timeout: Duration,
    /// Timeout for the BlueZ Device.Connect() warm-up after we found the
    /// paired AAW peer. Default 8 s.
    pub bt_connect_timeout: Duration,
    /// Timeout for the RFCOMM client open. Default 8 s.
    pub rfcomm_connect_timeout: Duration,
    /// Timeout for joining the HU's WiFi network. Default 25 s.
    pub wifi_join_timeout: Duration,
    /// Timeout for DHCP lease on wlan0. Default 15 s.
    pub dhcp_timeout: Duration,
    /// Timeout for the outbound TCP connect to the HU. Default 10 s.
    pub tcp_connect_timeout: Duration,
}

impl Default for BtConfig {
    fn default() -> Self {
        Self {
            pair_wait_timeout: Duration::from_secs(300),
            bt_connect_timeout: Duration::from_secs(8),
            rfcomm_connect_timeout: Duration::from_secs(8),
            wifi_join_timeout: Duration::from_secs(25),
            dhcp_timeout: Duration::from_secs(15),
            tcp_connect_timeout: Duration::from_secs(10),
        }
    }
}

/// Thin wrapper around the BT bootstrap that returns the eventual
/// `TcpTransport`. Kept as a struct (rather than a free function) so a
/// future version can hold the `bluer::Session` and tear it down on Drop.
pub struct BtTransport;

impl BtTransport {
    /// Run the full AAW bootstrap.
    ///
    /// Returns a ready-to-use `TcpTransport` connected to the head unit
    /// over the WiFi network the HU just brought up.
    pub async fn connect(cfg: BtConfig) -> Result<TcpTransport, BtError> {
        info!("bt: starting AAW bootstrap");

        // 1. BlueZ adapter: discoverable + Just Works agent registered.
        // The `_agent` handle keeps the agent alive while we're polling for
        // a paired peer; it drops at end of this function once we already
        // have a bond, which is when BlueZ no longer needs us listening
        // for pair requests.
        let (_session, adapter, _agent) = pair::open_adapter().await?;

        // 2. Wait for a paired AAW peer to appear. On first boot the
        // operator opens AA Wireless on the car and selects `Smartcar`;
        // BlueZ accepts Just Works via the agent above, the bond is
        // cached, and this function returns. On subsequent boots the bond
        // is already there and this returns immediately.
        let device = pair::wait_for_aaw_device(&adapter, cfg.pair_wait_timeout).await?;
        let car_addr = device.address();
        pair::warm_connect(&device, cfg.bt_connect_timeout).await;

        // 3. RFCOMM client to AAWG channel (default 8).
        let mut framed = rfcomm::Framed::connect(
            car_addr,
            rfcomm::AAWG_DEFAULT_RFCOMM_CHANNEL,
            cfg.rfcomm_connect_timeout,
        )
        .await?;
        info!(
            channel = rfcomm::AAWG_DEFAULT_RFCOMM_CHANNEL,
            %car_addr,
            "bt: RFCOMM open to HU"
        );

        // 4. Phone-side AAW handshake.
        let outcome = handshake::run_phone_side(&mut framed).await?;
        // RFCOMM is closed implicitly when `framed` drops at end of scope;
        // BlueZ tears the channel down cleanly.
        drop(framed);

        // 5. Bring up wlan0 STA, join HU's AP, wait for DHCP.
        let _our_ip =
            wpa::join_hu_network(&outcome.creds, cfg.wifi_join_timeout, cfg.dhcp_timeout)
                .await?;

        // 6. TCP connect outward to the HU's AA listener.
        info!(target = %outcome.hu_tcp_target, "bt: TCP connecting to HU");
        let stream = tokio::time::timeout(
            cfg.tcp_connect_timeout,
            TcpStream::connect(outcome.hu_tcp_target),
        )
        .await
        .map_err(|_| BtError::WifiJoin("TCP connect to HU timed out".into()))?
        .map_err(BtError::Io)?;

        info!("bt: AAW bootstrap complete — handing off to TcpTransport");
        Ok(TcpTransport::new(stream))
    }
}
