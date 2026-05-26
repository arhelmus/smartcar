//! BlueZ adapter + paired-device helpers.
//!
//! AAW pairing flow is **car-initiated**: the user opens Android Auto
//! Wireless setup on the head unit, the HU scans BR/EDR, the operator picks
//! `smartcar` on the car's UI, the HU sends the pair request, and BlueZ
//! accepts it via the Just Works agent registered here. The board never
//! initiates pairing (`bluetoothctl pair`), and the operator never has to
//! type a BD_ADDR anywhere.
//!
//! After pairing, BlueZ caches the bond. Subsequent boots find the paired
//! peer automatically by scanning the adapter's paired-device list for one
//! that exposes the AAWG SDP profile UUID.

use std::time::{Duration, Instant};

use bluer::{
    agent::{Agent, AgentHandle},
    rfcomm::{Profile, ProfileHandle, Stream as RfcommStream},
    Adapter, Device, Session,
};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::error::BtError;
use super::rfcomm::AAWG_PROFILE_UUID;

/// Adapter alias advertised on BR/EDR (and used as the BLE local name if/when
/// we add LE advertising). Cars see this in their AA Wireless pair list.
const ADVERTISED_NAME: &str = "smartcar";

/// Bundle returned by [`open_adapter`]. Caller keeps it alive for as long as
/// the board should be advertising / acceptive — drop all fields to
/// unregister.
pub struct AdapterBundle {
    /// Kept alive so the BlueZ D-Bus connection stays open for the lifetime
    /// of the bundle. Dropping the Session closes everything else with it.
    pub _session: Session,
    pub adapter: Adapter,
    /// Just Works pairing agent. Keeps incoming pair requests auto-accepted
    /// for as long as it's alive.
    pub _agent: AgentHandle,
    /// Background task that turns the registered AAWG `Profile`'s incoming
    /// connection-request stream into a plain `mpsc<Stream>` channel.
    /// Both inbound (car-initiated) and outbound (via
    /// `device.connect_profile()`) RFCOMM connections route through here,
    /// so [`BtTransport::connect`] doesn't need to distinguish which
    /// direction the link came up — first stream to arrive wins.
    pub _aawg_drainer: JoinHandle<()>,
    /// AAWG RFCOMM streams established through the registered Profile.
    /// Recv on this once we've triggered `device.connect_profile(AAWG_UUID)`
    /// (or while waiting for the HU to initiate inbound). Buffered at 4
    /// so a quick second connect attempt doesn't drop the first stream.
    pub aawg_streams: mpsc::Receiver<RfcommStream>,
}

/// Power the default adapter, make it discoverable + pairable, register a
/// Just Works agent, and register the AAWG `Profile` so cars filtering
/// their pair list by SDP UUID actually see us.
///
/// Why the AAWG profile: cars (BMW iDrive, Audi MMI, etc.) commonly do an
/// SDP search during their "Add Android Auto phone" scan and skip any
/// device that doesn't list `4de17a00-…`. The on-disk evidence was the
/// board's `bluetoothctl show` UUID list containing GATT/GAP/SIM Access/
/// PnP/AVRCP/DeviceInfo but no AAWG — iPhones (which list every device)
/// saw the board, cars (which filter) did not.
///
/// The registered Profile is the local handler BlueZ delivers AAWG RFCOMM
/// connections to. Both directions land here: inbound (HU initiates) and
/// outbound (we initiate via `device.connect_profile(AAWG_UUID)`). The
/// drainer task accepts each `ConnectRequest`, extracts the underlying
/// `Stream`, and forwards it through `aawg_streams` so callers don't
/// have to drive the bluer `ConnectRequest` API directly.
pub async fn open_adapter() -> Result<AdapterBundle, BtError> {
    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;

    if !adapter.is_powered().await? {
        info!("bt: adapter not powered, powering on");
        adapter.set_powered(true).await?;
    }

    // BlueZ advertises the *alias*, not the adapter's kernel name. Without
    // an explicit alias the car sees the hostname (e.g. `orangepizero2w`)
    // instead of `smartcar`. set_alias persists to /var/lib/bluetooth so
    // subsequent boots come up with the right name even before this point.
    adapter.set_alias(ADVERTISED_NAME.into()).await?;

    // Car initiates pairing. We have to be findable and acceptive:
    //   discoverable=on  — so the car's BT scan sees our advertisement.
    //   pairable=on      — so BlueZ doesn't refuse incoming pair requests.
    // DiscoverableTimeout/PairableTimeout in /etc/bluetooth/main.conf are
    // both 0, meaning "no timeout" — once we turn them on they stay on
    // for the life of the adapter session.
    adapter.set_pairable(true).await?;
    adapter.set_discoverable(true).await?;

    // Just Works agent. None-callbacks would make bluer reject every
    // request; instead we provide auto-accept handlers for the three the
    // car might invoke depending on its capabilities. The board has no
    // display/keyboard, so it acts as a NoInputNoOutput peripheral — the
    // user is verifying the pair on the car's screen, not ours.
    let agent = Agent {
        request_default: true,
        request_confirmation: Some(Box::new(|_req| async { Ok(()) }.boxed())),
        request_authorization: Some(Box::new(|_req| async { Ok(()) }.boxed())),
        authorize_service: Some(Box::new(|_req| async { Ok(()) }.boxed())),
        ..Default::default()
    };
    let agent_handle = session
        .register_agent(agent)
        .await
        .map_err(|e| BtError::Agent(e.to_string()))?;

    // AAWG profile. Two roles in one registration:
    //   1. SDP advertisement — BlueZ adds `4de17a00-…` to our local SDP
    //      record so cars filtering their pair list by AAWG see us.
    //   2. Local handler — BlueZ delivers every AAWG RFCOMM connection
    //      (incoming OR our own outbound via `device.connect_profile()`)
    //      to this Profile's `NewConnection` callback, which we expose
    //      as the `ProfileHandle` stream below.
    //
    // No `role` / `channel` fields:
    //   - `role` defaults to whatever BlueZ chooses; we don't need to
    //     restrict to Server-only since we handle both directions.
    //   - `channel` is left auto-picked. BlueZ's default profile manager
    //     already binds channel 8 (SIM Access / OBEX) on this image,
    //     so requesting 8 fails with `rfcomm_bind: Address already in
    //     use`. Our local channel is transparent — for *outbound* the
    //     car-side channel is what matters and BlueZ resolves it via
    //     SDP at `connect_profile` time.
    let profile = Profile {
        uuid: AAWG_PROFILE_UUID,
        name: Some("AA Wireless".into()),
        require_authentication: Some(false),
        require_authorization: Some(false),
        ..Default::default()
    };
    let mut profile_handle: ProfileHandle = session
        .register_profile(profile)
        .await
        .map_err(|e| BtError::Agent(format!("register_profile(AAWG): {e}")))?;

    // Drainer: accept every `ConnectRequest`, send the resulting `Stream`
    // to the bounded mpsc. Buffered at 4 so a quick second connect
    // attempt (e.g. HU retries inbound while we're processing the first
    // stream) doesn't drop the original. Sender drops on task end →
    // receiver sees EOF.
    let (tx, rx) = mpsc::channel::<RfcommStream>(4);
    let aawg_drainer = tokio::spawn(async move {
        while let Some(req) = profile_handle.next().await {
            let peer = req.device();
            match req.accept() {
                Ok(stream) => {
                    info!(addr = %peer, "bt: AAWG RFCOMM connection established");
                    if tx.send(stream).await.is_err() {
                        debug!(
                            "bt: AAWG stream consumer dropped — no one waiting; \
                             dropping inbound stream"
                        );
                    }
                }
                Err(e) => warn!(
                    addr = %peer,
                    error = %e,
                    "bt: accepting AAWG RFCOMM ConnectRequest failed"
                ),
            }
        }
    });

    info!(
        addr = %adapter.address().await?,
        alias = %adapter.alias().await?,
        aawg = %AAWG_PROFILE_UUID,
        "bt: adapter ready, discoverable + Just Works agent + AAWG profile registered"
    );
    Ok(AdapterBundle {
        _session: session,
        adapter,
        _agent: agent_handle,
        _aawg_drainer: aawg_drainer,
        aawg_streams: rx,
    })
}

/// Wait for an AAW-capable paired device to appear on the adapter.
///
/// The first time `smartcar-server --transport bt` runs on a board, this
/// function blocks while the operator pairs the car via its AA Wireless
/// setup. On subsequent boots it finds the bond immediately and returns.
///
/// Logs a one-shot "waiting" message and polls every 5 s. Bails after
/// `timeout` with [`BtError::NoAawPairedDevice`] so the systemd unit
/// restarts and tries again rather than spinning forever.
pub async fn wait_for_aaw_device(adapter: &Adapter, timeout: Duration) -> Result<Device, BtError> {
    let deadline = Instant::now() + timeout;
    let mut announced_wait = false;
    loop {
        if let Some(device) = find_aaw_device(adapter).await? {
            info!(addr = %device.address(), "bt: AAW-capable paired device found");
            return Ok(device);
        }
        if Instant::now() >= deadline {
            return Err(BtError::NoAawPairedDevice);
        }
        if !announced_wait {
            info!(
                "bt: no paired AAW peer yet — open the car's Android Auto \
                 Wireless setup and select `smartcar` to pair"
            );
            announced_wait = true;
        }
        sleep(Duration::from_secs(5)).await;
    }
}

/// Find a paired AAW-capable peer on the adapter.
///
/// Scan each paired device's cached SDP UUID set for `AAWG_PROFILE_UUID`.
/// BlueZ populates this set during the post-pair SDP browse, so any car
/// that completed pairing successfully will match here.
///
/// Returns `Ok(None)` if nothing matches — caller decides whether to wait
/// or fail. (Previously had a channel-8 RFCOMM probe fallback for boards
/// where BlueZ's SDP browse silently dropped the AAWG UUID; that probe
/// only made sense back when we hardcoded channel 8 ourselves. Now that
/// the channel comes from BlueZ's SDP cache via `device.connect_profile`,
/// the probe couldn't pick a single channel to test anyway. If a real HU
/// pairs but never gets AAWG into its uuid set, the operator can re-pair
/// or run `bluetoothctl info <addr>` to inspect the cached UUIDs.)
async fn find_aaw_device(adapter: &Adapter) -> Result<Option<Device>, BtError> {
    for addr in adapter.device_addresses().await? {
        let device = adapter.device(addr)?;
        if !device.is_paired().await? {
            continue;
        }
        if let Some(uuids) = device.uuids().await? {
            if uuids.contains(&AAWG_PROFILE_UUID) {
                debug!(%addr, "bt: paired device exposes AAWG profile (SDP)");
                return Ok(Some(device));
            }
        }
    }
    Ok(None)
}

/// Trigger BlueZ to open an AAWG RFCOMM connection to the paired peer.
///
/// BlueZ handles the channel lookup internally via its on-disk SDP cache
/// (`/var/lib/bluetooth/<adapter>/cache/<peer>`) — we never need to know
/// the channel number ourselves. The resulting `Stream` arrives via the
/// `Profile` callback we registered in [`open_adapter`], reachable from
/// [`AdapterBundle::aawg_streams`].
///
/// Per BlueZ docs, `ConnectProfile` blocks until the profile is fully
/// connected, so when this future returns Ok the `NewConnection` callback
/// has already fired and the Stream is queued. The caller's responsibility
/// is to `recv()` it from `aawg_streams` (with a sensible timeout to cover
/// the rare case where the callback didn't actually fire — e.g. HU
/// rejected the L2CAP setup after BlueZ thought it was OK).
///
/// Retries on transient busy errors (`br-connection-busy`, `InProgress`).
/// These happen when the car is mid-handshake on another profile (HFP /
/// A2DP / PBAP fire up right after AAW pair) — typically clears within a
/// few seconds. We retry up to 6 times with 2 s backoff (≈12 s budget);
/// systemd `Restart=on-failure` covers anything beyond that.
pub async fn connect_aawg_profile(device: &Device) -> Result<(), BtError> {
    const MAX_ATTEMPTS: u32 = 6;
    const RETRY_BACKOFF: Duration = Duration::from_secs(2);
    // BlueZ's internal page-timeout is ~20 s, but in practice it can hang
    // much longer (observed indefinitely with Audi MMI when the HU dropped
    // BR/EDR ACL right after Just-Works pair). Force the issue at 12 s per
    // attempt so we get a clean failure → systemd restart → next round of
    // diagnostics, rather than a permanently-stuck process.
    const PER_ATTEMPT_DEADLINE: Duration = Duration::from_secs(12);

    for attempt in 1..=MAX_ATTEMPTS {
        info!(
            addr = %device.address(),
            attempt,
            "bt: ConnectProfile(AAWG)"
        );
        let result = tokio::time::timeout(
            PER_ATTEMPT_DEADLINE,
            device.connect_profile(&AAWG_PROFILE_UUID),
        )
        .await;
        let bluez_result = match result {
            Ok(r) => r,
            Err(_) => {
                warn!(
                    addr = %device.address(),
                    attempt,
                    deadline = ?PER_ATTEMPT_DEADLINE,
                    "bt: ConnectProfile hung past deadline — aborting attempt"
                );
                if attempt < MAX_ATTEMPTS {
                    sleep(RETRY_BACKOFF).await;
                    continue;
                }
                return Err(BtError::Framing(format!(
                    "ConnectProfile(AAWG) hung past {PER_ATTEMPT_DEADLINE:?} \
                     for {} attempt(s) — HU not responding to BR/EDR page \
                     (likely needs the BLE pre-step we don't implement yet; \
                     see server/third_party/aa-proxy-rs/src/btle.rs)",
                    attempt
                )));
            }
        };
        match bluez_result {
            Ok(()) => return Ok(()),
            Err(e) if is_transient_busy(&e) && attempt < MAX_ATTEMPTS => {
                warn!(
                    addr = %device.address(),
                    attempt,
                    error = %e,
                    "bt: ConnectProfile transient busy — retrying after backoff"
                );
                sleep(RETRY_BACKOFF).await;
            }
            Err(e) => {
                return Err(BtError::Framing(format!(
                    "ConnectProfile(AAWG) failed after {attempt} attempt(s): {e}"
                )));
            }
        }
    }
    unreachable!("loop returns or errors on the last iteration")
}

/// BlueZ surfaces transient-busy errors via several distinct strings
/// depending on which subsystem rejected the call. All of them mean
/// "another BR/EDR operation is in flight, try again shortly".
fn is_transient_busy(e: &bluer::Error) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    s.contains("br-connection-busy")
        || s.contains("in progress")
        || s.contains("operation already in progress")
}
