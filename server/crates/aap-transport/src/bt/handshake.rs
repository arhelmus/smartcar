//! AAW WiFi-credentials handshake — phone-side.
//!
//! Direction is **reversed** vs. aa-proxy-rs (which impersonates the head
//! unit): we send what aa-proxy-rs reads and read what aa-proxy-rs sends.
//!
//! Order, confirmed against aa-proxy-rs/src/bluetooth.rs:
//!
//! 1. HU → phone: `WIFI_START_REQUEST { ip_address, port }`        — HU's AA listener address
//! 2. phone → HU: `WIFI_INFO_REQUEST {}`                            — empty; "give me your WiFi creds"
//! 3. HU → phone: `WIFI_INFO_RESPONSE { ssid, key, bssid, sec, ap_type }` — credentials
//! 4. phone → HU: `WIFI_START_RESPONSE { status = SUCCESS }`        — ack
//! 5. phone → HU: `WIFI_CONNECTION_STATUS { status = SUCCESS }`     — final status
//!
//! Some HUs additionally exchange `WIFI_VERSION_REQUEST/RESPONSE` (ids 4/5)
//! before step 1. We absorb either incoming message-id and echo back a
//! reasonable response — there is no documented field meaning, just four
//! ints — so a slightly chattier HU doesn't deadlock us.

use std::net::SocketAddr;
use std::time::Duration;

use prost::Message as ProstMessage;
use tracing::{info, warn};

use super::error::BtError;
use super::proto::aaw::{
    MessageId, Status, WifiConnectionStatus, WifiInfoRequest, WifiInfoResponse, WifiStartRequest,
    WifiStartResponse, WifiVersionRequest, WifiVersionResponse,
};
use super::rfcomm::Framed;
use super::wpa::WifiCreds;

/// Per-step RFCOMM read timeout. Long enough for slow HU firmware (BMW iDrive
/// has been observed to stall 1–3 s between steps), short enough to bail if
/// the HU has wandered off.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// Output of a successful handshake: where to TCP-connect once we're on the
/// HU's WiFi, plus the credentials to give wpa_supplicant.
pub struct HandshakeOutcome {
    /// HU's listening address on the WiFi network it will create.
    pub hu_tcp_target: SocketAddr,
    /// Credentials for the HU's WiFi AP (we are the STA).
    pub creds: WifiCreds,
}

/// Run the phone-side AAW handshake on an open RFCOMM stream and return what
/// to do with the result.
pub async fn run_phone_side(framed: &mut Framed) -> Result<HandshakeOutcome, BtError> {
    // ── Step 1 (with optional version exchange) ───────────────────────────────
    let start_req: WifiStartRequest = loop {
        let (id, body) = framed.recv(STEP_TIMEOUT).await?;
        match id {
            MessageId::WifiStartRequest => {
                let req = WifiStartRequest::decode(body.as_ref())?;
                info!(ip = %req.ip_address, port = req.port, "aaw: WifiStartRequest from HU");
                break req;
            }
            MessageId::WifiVersionRequest => {
                info!("aaw: optional WifiVersionRequest seen, replying");
                let _ = WifiVersionRequest::decode(body.as_ref())?;
                // Field meanings are not documented; aa-proxy-rs answers with
                // four small ints and an empty string. Mirror that.
                let resp = WifiVersionResponse {
                    unknown_value_a: 1,
                    unknown_value_b: 2,
                    unknown_value_c: None,
                    unknown_value_d: 0,
                };
                framed.send(MessageId::WifiVersionResponse, &resp).await?;
            }
            other => {
                warn!(?other, "aaw: unexpected message during step 1; ignoring");
            }
        }
    };

    // ── Step 2 ───────────────────────────────────────────────────────────────
    framed
        .send(MessageId::WifiInfoRequest, &WifiInfoRequest {})
        .await?;

    // ── Step 3 ───────────────────────────────────────────────────────────────
    let info_resp: WifiInfoResponse = loop {
        let (id, body) = framed.recv(STEP_TIMEOUT).await?;
        match id {
            MessageId::WifiInfoResponse => break WifiInfoResponse::decode(body.as_ref())?,
            other => warn!(?other, "aaw: unexpected message during step 3; ignoring"),
        }
    };
    info!(
        ssid = %info_resp.ssid,
        bssid = %info_resp.bssid,
        security = ?info_resp.security_mode(),
        "aaw: HU WifiInfoResponse"
    );

    // ── Step 4 ───────────────────────────────────────────────────────────────
    let start_resp = WifiStartResponse {
        ip_address: None,
        port: None,
        status: Status::Success as i32,
    };
    framed
        .send(MessageId::WifiStartResponse, &start_resp)
        .await?;

    // ── Step 5 ───────────────────────────────────────────────────────────────
    let conn_status = WifiConnectionStatus {
        status: Status::Success as i32,
        error_message: None,
    };
    framed
        .send(MessageId::WifiConnectionStatus, &conn_status)
        .await?;

    // Compose outcome.
    let hu_tcp_target: SocketAddr = format!("{}:{}", start_req.ip_address, start_req.port)
        .parse()
        .map_err(|e| BtError::Framing(format!("HU WifiStartRequest had bad ip/port: {e}")))?;

    let creds = WifiCreds {
        ssid: info_resp.ssid.clone(),
        psk: info_resp.password.clone(),
        bssid: info_resp.bssid.clone(),
        security: info_resp.security_mode(),
    };

    Ok(HandshakeOutcome {
        hu_tcp_target,
        creds,
    })
}
