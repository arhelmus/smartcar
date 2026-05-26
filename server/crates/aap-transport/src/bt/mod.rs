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
//! `smartcar`, accept Just Works. No `--bt-target`, no `bluetoothctl pair`,
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
    /// 10 minutes per attempt — generous enough for an operator to drive
    /// up, open AA setup, and tap `smartcar`. When this elapses the unit
    /// exits and systemd restarts us (no `StartLimitBurst` on the bt
    /// branch of the unit template), so practically the board advertises
    /// for as long as it's powered.
    pub pair_wait_timeout: Duration,
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
            pair_wait_timeout: Duration::from_secs(600),
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

        // 1. BlueZ adapter: alias=smartcar, discoverable, Just Works agent
        // registered, AAWG profile in SDP. `bundle.aawg_streams` is the
        // mpsc receiver where the Profile's `NewConnection` callback
        // delivers any AAWG RFCOMM stream — inbound or outbound. The
        // whole bundle stays alive for the lifetime of `connect`.
        let mut bundle = pair::open_adapter().await?;
        let adapter = &bundle.adapter;

        // 2. Wait for a paired AAW peer to appear. On first boot the
        // operator opens AA Wireless on the car and selects `smartcar`;
        // BlueZ accepts Just Works via the agent, the bond is cached, and
        // this function returns. On subsequent boots the bond is already
        // there and this returns immediately.
        let device = pair::wait_for_aaw_device(adapter, cfg.pair_wait_timeout).await?;
        let car_addr = device.address();
        // Deliberately NOT calling `device.connect()` here — that's BlueZ's
        // "ConnectAllProfiles" entry point, and it walks every UUID the
        // peer advertises (HFP, HFP-AG, A2DP-sink, AVRCP, PBAP, MAP, …)
        // attempting to bring each up. Cars expect those profiles to be
        // *driven* by a real phone with handlers; arriving as a Class
        // 0x6c020c phone and asking for HFP-AG that we don't speak made
        // the Audi infotainment reboot mid-handshake (journal line:
        // `a2dp-sink profile connect failed for <bmw>: Protocol not
        // available` is the bluetoothd-side fingerprint). We only want
        // AAWG, so we jump straight to the targeted ConnectProfile below.

        // 3. Trigger BlueZ to open the AAWG RFCOMM connection.
        //
        // `device.connect_profile(AAWG_UUID)` is the idiomatic way to do
        // this with bluer: BlueZ resolves the channel from its on-disk
        // SDP cache (written at pair time), opens the RFCOMM connection
        // to whatever channel the car advertised, and delivers the
        // resulting `Stream` to our registered Profile's `NewConnection`
        // callback. We don't have to know the channel ourselves.
        //
        // Per BlueZ docs `ConnectProfile` blocks until the profile is
        // fully connected, so once the await returns, the Stream is
        // already queued in `aawg_streams`. We then `recv()` it with a
        // bounded timeout to cover the rare case where the callback
        // didn't fire after a successful return (e.g. car closed the
        // RFCOMM link before NewConnection delivery).
        //
        // If the car initiates inbound RFCOMM *before* we initiate
        // outbound, the Stream is already queued and `recv()` returns
        // immediately — same code path either direction.
        let stream = match bundle.aawg_streams.try_recv() {
            Ok(s) => {
                info!(%car_addr, "bt: AAWG stream from prior inbound connect");
                s
            }
            Err(_) => {
                pair::connect_aawg_profile(&device).await?;
                tokio::time::timeout(cfg.rfcomm_connect_timeout, bundle.aawg_streams.recv())
                    .await
                    .map_err(|_| BtError::ConnectTimeout)?
                    .ok_or_else(|| {
                        BtError::Framing(
                            "AAWG Profile channel closed without delivering a Stream".into(),
                        )
                    })?
            }
        };
        info!(%car_addr, "bt: RFCOMM stream ready, starting AAW handshake");

        let mut framed = rfcomm::Framed::from_stream(stream);

        // 4. Phone-side AAW handshake.
        let outcome = handshake::run_phone_side(&mut framed).await?;
        // RFCOMM is closed implicitly when `framed` drops at end of scope;
        // BlueZ tears the channel down cleanly.
        drop(framed);

        // 5. Bring up wlan0 STA, join HU's AP, wait for DHCP.
        let _our_ip =
            wpa::join_hu_network(&outcome.creds, cfg.wifi_join_timeout, cfg.dhcp_timeout).await?;

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
