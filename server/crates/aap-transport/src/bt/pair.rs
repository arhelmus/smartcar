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
    rfcomm::{Profile, ProfileHandle, ReqError, Role},
    Adapter, Device, Session,
};
use futures::{FutureExt, StreamExt};
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
    /// Background task that drains the AAWG `Profile`'s inbound RFCOMM
    /// queue (rejecting each, since real AAW flows are phone-as-client).
    /// Aborted on drop, which closes the registered profile.
    pub _aawg_drainer: JoinHandle<()>,
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
/// The real AAW flow is phone-as-RFCOMM-client (we connect outward to the
/// HU's channel 8 — see `BtTransport::connect`), so we never expect a
/// useful inbound connection on this Profile. A background drainer task
/// rejects each request to keep BlueZ's queue empty.
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

    // AAWG profile — strictly an SDP advertisement here. BlueZ will list
    // the AAWG UUID in our service record so cars filtering on it find us.
    //
    // `channel: None` lets BlueZ pick any free RFCOMM channel. We
    // originally requested channel 8 to mirror aa-proxy-rs (HU side), but
    // BlueZ's default profile manager already binds channel 8 for SIM
    // Access / OBEX / etc. on this image (`rfcomm_bind: Address already
    // in use`), and our local channel is *transparent* to the car anyway
    // — the car's SDP query reads whatever channel BlueZ assigned us, and
    // (more importantly) the real AAW data flow has us as RFCOMM *client*
    // outbound to the HU's channel 8, not the other direction.
    let profile = Profile {
        uuid: AAWG_PROFILE_UUID,
        name: Some("AA Wireless".into()),
        role: Some(Role::Server),
        require_authentication: Some(false),
        require_authorization: Some(false),
        ..Default::default()
    };
    let mut profile_handle: ProfileHandle = session
        .register_profile(profile)
        .await
        .map_err(|e| BtError::Agent(format!("register_profile(AAWG): {e}")))?;

    // Background drainer. Some HUs may try inbound RFCOMM (reverse role)
    // even though the canonical flow is phone-as-client; we reject them
    // and rely on our outbound `Framed::connect` in BtTransport::connect.
    // If a particular HU only works with inbound, the journal line below
    // will surface it and we'll switch to using the inbound stream.
    let aawg_drainer = tokio::spawn(async move {
        while let Some(req) = profile_handle.next().await {
            warn!(
                addr = %req.device(),
                "bt: inbound RFCOMM on AAWG channel — rejecting (phone-as-client \
                 flow expected); if your HU needs reverse role, this is the \
                 breadcrumb to act on"
            );
            req.reject(ReqError::Rejected);
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
/// Two-pass scan:
///
///  1. **Fast path (SDP UUID filter).** For each paired device, check whether
///     BlueZ has cached `AAWG_PROFILE_UUID` in `device.uuids()`. BlueZ
///     normally does an SDP browse right after a successful pair, so on the
///     happy path this pass returns the bond immediately.
///
///  2. **Fallback (RFCOMM probe).** Some HUs / older BlueZ versions don't
///     reliably push the AAWG record into the SDP descriptors `bluer`
///     exposes — we then see `uuids = Some({})` or a set missing the AAWG
///     UUID and would otherwise wait forever. Second pass tries to actually
///     `Stream::connect(addr, 8)` on each paired device (short 2 s timeout).
///     A successful TCP-style connect proves the peer accepts the AAWG
///     RFCOMM channel; we drop the probe stream and let the caller open
///     the real one. Connect-refused / -unreachable peers are silently
///     skipped.
///
/// Returns `Ok(None)` if both passes find nothing — caller decides whether
/// to wait or fail.
async fn find_aaw_device(adapter: &Adapter) -> Result<Option<Device>, BtError> {
    let mut paired_unresolved: Vec<Device> = Vec::new();

    // Pass 1: SDP UUID filter. Cheap; no over-the-air traffic.
    for addr in adapter.device_addresses().await? {
        let device = adapter.device(addr)?;
        if !device.is_paired().await? {
            continue;
        }
        match device.uuids().await? {
            Some(uuids) if uuids.contains(&AAWG_PROFILE_UUID) => {
                debug!(%addr, "bt: paired device exposes AAWG profile (SDP)");
                return Ok(Some(device));
            }
            _ => paired_unresolved.push(device),
        }
    }

    // Pass 2: RFCOMM probe. Skipped entirely if there are no paired devices
    // — keeps a board with zero bonds quiet between polling iterations.
    for device in paired_unresolved {
        let addr = device.address();
        if probe_rfcomm_aawg(addr).await {
            debug!(%addr, "bt: paired device accepts RFCOMM channel 8 (probe)");
            return Ok(Some(device));
        }
    }

    Ok(None)
}

/// Open + immediately close an RFCOMM connection on the AAWG channel as a
/// liveness probe. Returns true only if the channel accepted us. 2 s budget
/// per device — long enough for a real HU to reply, short enough that a
/// dead bond doesn't stall the polling loop.
async fn probe_rfcomm_aawg(addr: bluer::Address) -> bool {
    use super::rfcomm::AAWG_DEFAULT_RFCOMM_CHANNEL;
    let sa = bluer::rfcomm::SocketAddr::new(addr, AAWG_DEFAULT_RFCOMM_CHANNEL);
    let probe = tokio::time::timeout(Duration::from_secs(2), bluer::rfcomm::Stream::connect(sa));
    matches!(probe.await, Ok(Ok(_)))
}

/// Best-effort `Connect()` on the paired device so BlueZ brings up any
/// supporting profiles (PNP, GATT scan) before we open RFCOMM. Errors here
/// are non-fatal — RFCOMM connect will surface a clear failure if the
/// device is genuinely out of range or has gone to sleep.
pub async fn warm_connect(device: &Device, timeout: Duration) {
    debug!(addr = %device.address(), "bt: warm connect");
    match tokio::time::timeout(timeout, device.connect()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!(error = %e, "bt: warm connect returned error (continuing)"),
        Err(_) => warn!("bt: warm connect timed out (continuing)"),
    }
}
