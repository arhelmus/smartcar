//! wpa_supplicant driver — join the head unit's hotspot once we've received
//! its credentials over RFCOMM.
//!
//! Why subprocess and not the wpa_supplicant control socket directly: the
//! board uses systemd-managed wpa_supplicant when in dev mode (for its own
//! station mode on home WiFi), and the simplest way to swap that for an AAW
//! session is to stop the system unit, point wpa_supplicant at a tmpfile
//! config, run it backgrounded, and let systemd-networkd run DHCP via
//! `/etc/systemd/network/wlan0.network` (installed by the board bringup
//! script).
//!
//! This module is intentionally small and shells out — it's running on a
//! board we control with stable Debian packages; the gain from a native
//! wpactrl crate is not worth the dependency.

use std::net::Ipv4Addr;
use std::path::Path;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::{sleep, Instant};
use tracing::{info, warn};

use super::error::BtError;
use super::proto::wifiprojection::WifiSecurityMode;

/// Credentials lifted from a head-unit `WifiInfoResponse`.
#[derive(Debug, Clone)]
pub struct WifiCreds {
    pub ssid: String,
    pub psk: String,
    pub bssid: String,
    pub security: WifiSecurityMode,
}

const RUNTIME_CONF: &str = "/run/aaw-wpa.conf";
const WLAN_IF: &str = "wlan0";

/// Bring up `wlan0` as a station joined to `creds`'s SSID, run DHCP, and
/// return the IPv4 the head unit's DHCP server hands us.
pub async fn join_hu_network(
    creds: &WifiCreds,
    join_timeout: Duration,
    dhcp_timeout: Duration,
) -> Result<Ipv4Addr, BtError> {
    info!(
        ssid = %creds.ssid,
        bssid = %creds.bssid,
        security = ?creds.security,
        "wpa: joining HU network"
    );

    write_conf(creds).await?;

    // Stop any wpa_supplicant currently bound to wlan0 (systemd unit, manual
    // run from a previous AAW attempt, etc.). We don't care if it wasn't
    // running.
    run(
        "systemctl",
        &["stop", &format!("wpa_supplicant@{WLAN_IF}.service")],
    )
    .await
    .ok();
    run("pkill", &["-f", &format!("wpa_supplicant.*-i ?{WLAN_IF}")])
        .await
        .ok();

    // Bring the interface DOWN/UP cleanly so wpa_supplicant starts from a known
    // state. `ip link set ... up` is idempotent; the down/up cycle clears any
    // residual association state from a prior session.
    run("ip", &["link", "set", WLAN_IF, "down"]).await?;
    run("ip", &["link", "set", WLAN_IF, "up"]).await?;

    // Start wpa_supplicant in the background. Use -B (daemonize). -D nl80211 is
    // the right driver for the Allwinner H618's combo radio on recent kernels.
    run(
        "wpa_supplicant",
        &["-B", "-D", "nl80211", "-i", WLAN_IF, "-c", RUNTIME_CONF],
    )
    .await
    .map_err(|e| BtError::Wpa(format!("spawn wpa_supplicant: {e}")))?;

    // Poll for association.
    wait_for_association(join_timeout).await?;

    // systemd-networkd runs DHCP on wlan0 once carrier comes up (the .network
    // file is shipped by the bringup script). Poll for the IP.
    wait_for_ip(dhcp_timeout).await
}

async fn write_conf(creds: &WifiCreds) -> Result<(), BtError> {
    let body = match creds.security {
        WifiSecurityMode::Wpa2Personal
        | WifiSecurityMode::WpaPersonal
        | WifiSecurityMode::WpaWpa2Personal => format!(
            r#"ctrl_interface=DIR=/run/wpa_supplicant GROUP=netdev
update_config=1
ap_scan=1
network={{
    ssid="{ssid}"
    psk="{psk}"
    key_mgmt=WPA-PSK
    pairwise=CCMP TKIP
    group=CCMP TKIP
    scan_ssid=1
}}
"#,
            ssid = escape_quotes(&creds.ssid),
            psk = escape_quotes(&creds.psk),
        ),
        WifiSecurityMode::Open => format!(
            r#"ctrl_interface=DIR=/run/wpa_supplicant GROUP=netdev
update_config=1
ap_scan=1
network={{
    ssid="{ssid}"
    key_mgmt=NONE
    scan_ssid=1
}}
"#,
            ssid = escape_quotes(&creds.ssid),
        ),
        other => {
            return Err(BtError::Wpa(format!(
                "unsupported WifiSecurityMode for AAW: {other:?}"
            )))
        }
    };

    if Path::new(RUNTIME_CONF).exists() {
        let _ = tokio::fs::remove_file(RUNTIME_CONF).await;
    }
    tokio::fs::write(RUNTIME_CONF, body)
        .await
        .map_err(|e| BtError::Wpa(format!("write {RUNTIME_CONF}: {e}")))?;
    // Restrict — file has the PSK.
    let _ = run("chmod", &["600", RUNTIME_CONF]).await;
    Ok(())
}

fn escape_quotes(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

async fn wait_for_association(timeout: Duration) -> Result<(), BtError> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let out = Command::new("iw")
            .args(["dev", WLAN_IF, "link"])
            .output()
            .await
            .map_err(|e| BtError::Wpa(format!("iw dev link: {e}")))?;
        let s = String::from_utf8_lossy(&out.stdout);
        if !s.trim_start().starts_with("Not connected") && !s.is_empty() {
            info!("wpa: associated\n{}", s);
            return Ok(());
        }
        sleep(Duration::from_millis(500)).await;
    }
    warn!("wpa: never associated within timeout");
    Err(BtError::WifiTimeout)
}

async fn wait_for_ip(timeout: Duration) -> Result<Ipv4Addr, BtError> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let out = Command::new("ip")
            .args([
                "-4", "-o", "addr", "show", "dev", WLAN_IF, "scope", "global",
            ])
            .output()
            .await
            .map_err(|e| BtError::Wpa(format!("ip addr: {e}")))?;
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(ip) = parse_first_inet(&s) {
            info!(%ip, "wpa: got DHCP lease on wlan0");
            return Ok(ip);
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(BtError::WifiTimeout)
}

/// Parse the first IPv4 address from one or more `ip -4 -o addr show` lines.
/// Output is one line per address with `inet <ip>/<prefix>` somewhere in it.
fn parse_first_inet(s: &str) -> Option<Ipv4Addr> {
    for line in s.lines() {
        let mut toks = line.split_whitespace();
        while let Some(tok) = toks.next() {
            if tok == "inet" {
                if let Some(addr_prefix) = toks.next() {
                    if let Some((addr, _)) = addr_prefix.split_once('/') {
                        if let Ok(ip) = addr.parse::<Ipv4Addr>() {
                            return Some(ip);
                        }
                    }
                }
            }
        }
    }
    None
}

async fn run(cmd: &str, args: &[&str]) -> Result<(), BtError> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| BtError::Wpa(format!("spawn {cmd}: {e}")))?;
    if !out.status.success() {
        return Err(BtError::Subprocess {
            cmd: format!("{cmd} {}", args.join(" ")),
            status: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ip_from_one_line() {
        let s = "2: wlan0    inet 192.168.43.245/24 brd 192.168.43.255 scope global dynamic wlan0";
        assert_eq!(parse_first_inet(s), Some(Ipv4Addr::new(192, 168, 43, 245)));
    }

    #[test]
    fn returns_none_when_no_inet() {
        assert_eq!(parse_first_inet(""), None);
        assert_eq!(parse_first_inet("garbage line"), None);
    }
}
