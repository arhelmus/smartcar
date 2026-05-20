//! USB gadget lifecycle: configfs setup, FunctionFS mount, AOAP ep0 handler.
//!
//! # Two-persona handshake
//!
//! Android Auto requires a two-step USB setup:
//!
//! 1. **Initial gadget** (`12d1:107e`) — ep0-only.  The car head unit sends three
//!    AOAP vendor control requests on ep0 (Get-Protocol 51, Send-String 52×6,
//!    Start-Accessory 53) then triggers a USB re-enumeration.
//!
//! 2. **Accessory gadget** (`18d1:2d00`) — two bulk endpoints (EP1 IN, EP2 OUT)
//!    used for all AA protocol frames.
//!
//! [`run_handshake`] drives this sequence synchronously.  It is expected to run
//! inside `tokio::task::spawn_blocking`.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::descriptors;

// ── Logging helpers ──────────────────────────────────────────────────────────
//
// configfs writes are the most common silent-failure source in this module.
// Every `os error 22 (Invalid argument)` you've ever stared at started life as
// a single fs::write whose error message dropped the path. `write_attr` keeps
// the path in the error, so the journal line tells you exactly which sysfs
// node the kernel rejected.
//
// `flush_logs` forces the stdout BufWriter that tracing-subscriber writes into
// out to the journald pipe. The car kills Vbus the instant it dislikes our
// USB descriptor; without explicit flushes, the lines describing what we just
// tried to advertise sit in userspace memory and vanish with the process.
// We call it at the *risky moments* — right after each step that could be the
// last one the board ever executes.
fn flush_logs() {
    use std::io::Write;
    // stdout is the tracing-subscriber default writer (block-buffered when
    // piped to journald). stderr is unbuffered already; flushing it is a
    // no-op but harmless.
    let _ = std::io::stdout().lock().flush();
}

fn write_attr(path: impl AsRef<Path>, value: &str) -> io::Result<()> {
    let path = path.as_ref();
    debug!(path = %path.display(), value = %value.trim_end(), "configfs write");
    fs::write(path, value)
        .map_err(|e| io::Error::new(e.kind(), format!("configfs write {}: {e}", path.display())))
}

fn mkdir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    debug!(path = %path.display(), "configfs mkdir");
    fs::create_dir_all(path)
        .map_err(|e| io::Error::new(e.kind(), format!("configfs mkdir {}: {e}", path.display())))
}

// ── Gadget identity ───────────────────────────────────────────────────────────

const CONFIGFS_ROOT: &str = "/sys/kernel/config/usb_gadget";

#[allow(dead_code)] // Phase-1 (two-persona AOAP) gadget name; see `run_handshake` doc.
const GADGET_INIT: &str = "aap-init";
const GADGET_ACC: &str = "aap";

#[allow(dead_code)] // Phase-1 (two-persona AOAP) FunctionFS mount; see `run_handshake` doc.
const FFS_MOUNT_INIT: &str = "/dev/ffs-aap-init";
const FFS_MOUNT_ACC: &str = "/dev/ffs-aap";

// USB descriptor strings the head unit reads when enumerating the gadget.
// We impersonate a Google Pixel so HUs that whitelist on VID + manufacturer
// strings (factory firmware in real cars) accept the connection. A previous
// revision used the openauto/aap-server historical Huawei IDs
// (`0x12d1:0x107e`); at least one production HU rejected that as
// "unsupported USB device" without even probing for AOAP.
//
// Current strategy: skip the two-persona AOAP handshake entirely and present
// the accessory persona (0x18d1:0x2d00) directly — see `run_handshake`.
const MANUFACTURER: &str = "Google";
const PRODUCT: &str = "Pixel 8 Pro";
const SERIAL: &str = "AKDPV1234567";

// ── FunctionFS ep0 event constants (linux/usb/functionfs.h) ──────────────────

const FUNCTIONFS_BIND: u8 = 0;
const FUNCTIONFS_ENABLE: u8 = 2;
const FUNCTIONFS_SETUP: u8 = 4;

// ── AOAP control-transfer request codes ──────────────────────────────────────

// AOAP vendor request codes — only the two-persona flow (`wait_for_aoap`)
// consumes these.  The skip-handshake variant in `run_handshake` doesn't
// touch them.
#[allow(dead_code)]
const AOAP_GET_PROTOCOL: u8 = 51;
#[allow(dead_code)]
const AOAP_SEND_STRING: u8 = 52;
#[allow(dead_code)]
const AOAP_START_ACCESSORY: u8 = 53;

// ── GadgetHandle ─────────────────────────────────────────────────────────────

/// RAII guard that disables and tears down the USB gadget + FunctionFS mount
/// when dropped.
pub struct GadgetHandle {
    gadget_name: String,
    ffs_mount: String,
}

impl Drop for GadgetHandle {
    fn drop(&mut self) {
        if let Err(e) = cleanup(&self.gadget_name, &self.ffs_mount) {
            warn!("USB gadget cleanup error ({}): {e}", self.gadget_name);
        }
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Bring up the AOAP accessory gadget and return the open bulk endpoints.
///
/// **Skip-handshake variant.** Production car HUs whitelist Google's
/// VID (`0x18d1`) and reject the Huawei initial-gadget ID
/// (`0x12d1:0x107e`) as "unsupported USB device" without ever probing
/// for AOAP. We therefore skip the two-persona dance entirely and
/// present `0x18d1:0x2d00` (Google AOAP accessory) directly; the HU
/// recognizes the accessory persona on first sight, no mode-switch
/// control transfers needed.
///
/// If you hit a HU that *requires* the mode-switch handshake, revert to
/// the two-persona flow: re-add the `setup_gadget(GADGET_INIT,
/// 0x18d1, 0x4ee1, …)` + `wait_for_aoap()` calls — both still live in
/// this module (marked `#[allow(dead_code)]`) — and drop the initial
/// gadget before this Phase-2 block.
///
/// Returns `(gadget_guard, ep1_tx, ep2_rx)` where:
/// - `gadget_guard` — keeps the accessory gadget alive; drop it to disable USB.
/// - `ep1_tx`       — write-only file for outbound AA frames (board → host).
/// - `ep2_rx`       — read-only file for inbound AA frames (host → board).
///
/// **Blocking**: must be called from a blocking thread (e.g. `spawn_blocking`).
pub fn run_handshake() -> io::Result<(GadgetHandle, fs::File, fs::File)> {
    // ── Accessory gadget — EP1 IN + EP2 OUT ───────────────────────────────
    info!("USB: setting up accessory gadget ({GADGET_ACC}) — skip-handshake");
    setup_gadget(GADGET_ACC, 0x18d1, 0x2d00, FFS_MOUNT_ACC)?;
    let acc_guard = GadgetHandle {
        gadget_name: GADGET_ACC.to_owned(),
        ffs_mount: FFS_MOUNT_ACC.to_owned(),
    };

    let mut ep0_acc = write_and_enable(
        GADGET_ACC,
        FFS_MOUNT_ACC,
        &descriptors::accessory_descriptors(),
    )?;

    info!("USB: waiting for host to enumerate accessory gadget");
    wait_for_enable(&mut ep0_acc)?;
    drop(ep0_acc);
    info!("USB: host enumerated; opening bulk endpoints");

    let ep1 = fs::File::options()
        .write(true)
        .open(format!("{FFS_MOUNT_ACC}/ep1"))?;
    let ep2 = fs::File::options()
        .read(true)
        .open(format!("{FFS_MOUNT_ACC}/ep2"))?;

    // Last sync point before handing the bulk endpoints back to the async
    // transport. Anything that happens after this is governed by the bulk
    // flow's own per-frame logs.
    flush_logs();
    Ok((acc_guard, ep1, ep2))
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Create the configfs gadget hierarchy and mount FunctionFS.
fn setup_gadget(name: &str, vid: u16, pid: u16, ffs_mount: &str) -> io::Result<()> {
    info!(
        gadget = name,
        vid = format!("0x{vid:04x}"),
        pid = format!("0x{pid:04x}"),
        manufacturer = MANUFACTURER,
        product = PRODUCT,
        serial = SERIAL,
        ffs_mount,
        "USB: configuring gadget"
    );
    let g = format!("{CONFIGFS_ROOT}/{name}");

    // Gadget root and device-level attributes.
    mkdir(&g)?;
    write_attr(format!("{g}/idVendor"), &format!("0x{vid:04x}\n"))?;
    write_attr(format!("{g}/idProduct"), &format!("0x{pid:04x}\n"))?;
    write_attr(format!("{g}/bcdUSB"), "0x0200\n")?;
    write_attr(format!("{g}/bcdDevice"), "0x0100\n")?;

    // Language strings.
    mkdir(format!("{g}/strings/0x409"))?;
    write_attr(format!("{g}/strings/0x409/manufacturer"), MANUFACTURER)?;
    write_attr(format!("{g}/strings/0x409/product"), PRODUCT)?;
    write_attr(format!("{g}/strings/0x409/serialnumber"), SERIAL)?;

    // Configuration.
    mkdir(format!("{g}/configs/c.1/strings/0x409"))?;
    write_attr(
        format!("{g}/configs/c.1/strings/0x409/configuration"),
        "Config\n",
    )?;
    write_attr(format!("{g}/configs/c.1/MaxPower"), "500\n")?;

    // FunctionFS function (must exist before mount).
    mkdir(format!("{g}/functions/ffs.{name}"))?;

    // Mount FunctionFS — this creates ep0 in the mount directory.
    mkdir(ffs_mount)?;
    info!(
        source = name,
        target = ffs_mount,
        "USB: mounting FunctionFS"
    );
    let output = Command::new("mount")
        .args(["-t", "functionfs", name, ffs_mount])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "mount -t functionfs {name} -> {ffs_mount} failed ({}): {}",
            output.status,
            stderr.trim()
        )));
    }
    debug!("FunctionFS mounted at {ffs_mount}");
    Ok(())
}

/// Write descriptor + strings blobs to ep0, link function into config, enable gadget.
///
/// Returns the open ep0 file (needed for reading events).
fn write_and_enable(gadget_name: &str, ffs_mount: &str, descs: &[u8]) -> io::Result<fs::File> {
    let ep0_path = format!("{ffs_mount}/ep0");
    debug!(path = %ep0_path, "opening ep0");
    let mut ep0 = fs::File::options().read(true).write(true).open(&ep0_path)?;

    // Write descriptor blob then strings blob — two separate write(2) calls.
    ep0.write_all(descs).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("ep0 write descriptors ({} bytes): {e}", descs.len()),
        )
    })?;
    let strs = descriptors::strings();
    ep0.write_all(&strs).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("ep0 write strings ({} bytes): {e}", strs.len()),
        )
    })?;
    debug!(
        descs_bytes = descs.len(),
        strings_bytes = strs.len(),
        "FunctionFS descriptors written to {ep0_path}"
    );

    // Symlink function into config AFTER descriptors are written.
    let g = format!("{CONFIGFS_ROOT}/{gadget_name}");
    let src = format!("{g}/functions/ffs.{gadget_name}");
    let dst = format!("{g}/configs/c.1/ffs.{gadget_name}");
    if !Path::new(&dst).exists() {
        debug!(src = %src, dst = %dst, "symlink function into config");
        std::os::unix::fs::symlink(&src, &dst)
            .map_err(|e| io::Error::new(e.kind(), format!("symlink {src} -> {dst}: {e}")))?;
    }

    // Enable gadget by binding to the first available UDC. This is THE most
    // common car-failure point: the kernel rejects the descriptor here with
    // an `EINVAL` if anything (VID/PID, descriptor layout, endpoint config)
    // doesn't pass its checks.
    let udc = find_udc()?;
    info!(gadget = gadget_name, udc = %udc, "USB: binding gadget to UDC");
    let udc_path = format!("{g}/UDC");
    fs::write(&udc_path, format!("{udc}\n")).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "UDC bind failed ({udc_path} <- '{udc}'): {e}. \
                 Common causes: another gadget already bound (g_ether-load.service \
                 ran in car mode by mistake?), kernel rejected the descriptor \
                 (idProduct/idVendor not accepted as-is), or the UDC driver isn't ready."
            ),
        )
    })?;
    info!(gadget = gadget_name, udc = %udc, "USB gadget enabled");
    // CRITICAL: from this point on the host can see us and may yank Vbus at
    // any instant. Push every preceding log line out to journald NOW, while
    // we still have power.
    flush_logs();

    Ok(ep0)
}

/// Read ep0 events and respond to AOAP vendor requests.
///
/// Returns when `AOAP_START_ACCESSORY` (req 53) is received.
///
/// Unused by the current skip-handshake `run_handshake`; kept so an HU that
/// requires the two-persona dance can be supported by re-wiring the caller.
#[allow(dead_code)]
/// The host will disconnect and re-enumerate after this request.
fn wait_for_aoap(ep0: &mut fs::File) -> io::Result<()> {
    // usb_functionfs_event is 12 bytes:
    //   [0..7]  union { usb_ctrlrequest (8 bytes) | u8 number }
    //   [8]     event type (u8)
    //   [9..11] padding
    //
    // usb_ctrlrequest layout:
    //   [0] bRequestType, [1] bRequest, [2..3] wValue, [4..5] wIndex, [6..7] wLength
    let mut event = [0u8; 12];

    loop {
        ep0.read_exact(&mut event)?;
        let event_type = event[8];
        let b_request = event[1]; // only meaningful for FUNCTIONFS_SETUP

        match event_type {
            FUNCTIONFS_BIND => debug!("ep0: BIND"),
            FUNCTIONFS_ENABLE => debug!("ep0: ENABLE"),
            FUNCTIONFS_SETUP => match b_request {
                AOAP_GET_PROTOCOL => {
                    debug!("AOAP req 51: Get-Protocol → v2");
                    ep0.write_all(&[0x02, 0x00])?; // AOAP protocol version 2 (LE16)
                }
                AOAP_SEND_STRING => {
                    let idx = u16::from_le_bytes([event[4], event[5]]);
                    debug!("AOAP req 52: Send-String idx={idx} (no response needed)");
                    // OUT transfer — host sends string; no IN reply required.
                }
                AOAP_START_ACCESSORY => {
                    debug!("AOAP req 53: Start-Accessory");
                    return Ok(());
                }
                other => debug!("ep0 SETUP: unknown bRequest={other}"),
            },
            other => debug!("ep0: unknown event type={other}"),
        }
    }
}

/// Wait for `FUNCTIONFS_ENABLE` on the accessory gadget's ep0.
///
/// This fires once the host has enumerated the new gadget and enabled it.
/// If the host never enumerates us (e.g. a car HU that doesn't recognize our
/// VID/PID), this would otherwise block forever in silence. We log every ep0
/// event we receive and emit a periodic info-level heartbeat so the journal
/// makes that stuck state obvious.
fn wait_for_enable(ep0: &mut fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    info!("USB: waiting for FUNCTIONFS_ENABLE from host");
    let started = Instant::now();
    let mut last_heartbeat = started;
    let mut events = 0u32;
    let mut event = [0u8; 12];

    // Use a short read timeout via poll(2) so we can emit heartbeats while
    // also yielding on every ep0 event. ep0 doesn't natively block on a
    // deadline, so we poll first; if nothing arrives in ~3s we log and loop.
    let fd = ep0.as_raw_fd();
    loop {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pfd, 1, 3000) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        if ready == 0 {
            // Timeout: log heartbeat every 3s of silence.
            let elapsed = started.elapsed();
            info!(
                elapsed_s = elapsed.as_secs(),
                events_seen = events,
                "USB: still waiting for host enumeration (no ep0 event yet)"
            );
            flush_logs();
            last_heartbeat = Instant::now();
            continue;
        }

        ep0.read_exact(&mut event)?;
        events += 1;
        let evt = event[8];
        let since_last = last_heartbeat.elapsed();
        match evt {
            FUNCTIONFS_ENABLE => {
                info!(
                    total_events = events,
                    elapsed_s = started.elapsed().as_secs(),
                    "USB: FUNCTIONFS_ENABLE received — host has enumerated us"
                );
                flush_logs();
                return Ok(());
            }
            FUNCTIONFS_BIND => debug!(elapsed_ms = since_last.as_millis(), "acc ep0: BIND"),
            FUNCTIONFS_SETUP => debug!(
                elapsed_ms = since_last.as_millis(),
                bRequestType = event[0],
                bRequest = event[1],
                wValue = u16::from_le_bytes([event[2], event[3]]),
                wIndex = u16::from_le_bytes([event[4], event[5]]),
                wLength = u16::from_le_bytes([event[6], event[7]]),
                "acc ep0: SETUP (unexpected before ENABLE)"
            ),
            other => debug!(
                elapsed_ms = since_last.as_millis(),
                evt = other,
                "acc ep0: event"
            ),
        }
        last_heartbeat = Instant::now();
    }
}

/// Return the name of the first available USB Device Controller.
fn find_udc() -> io::Result<String> {
    if let Some(entry) = fs::read_dir("/sys/class/udc/")?.next() {
        return Ok(entry?.file_name().to_string_lossy().into_owned());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no USB Device Controller found in /sys/class/udc/ — \
         check that CONFIG_USB_GADGET and the UDC driver are loaded",
    ))
}

/// Disable and remove a gadget + its FunctionFS mount.
///
/// Errors at each step are logged at `debug!` and intentionally not
/// propagated — `cleanup` runs from `Drop` and we want best-effort teardown
/// even if the kernel state is partially gone (e.g. after a crash).
fn cleanup(gadget_name: &str, ffs_mount: &str) -> io::Result<()> {
    debug!(gadget = gadget_name, ffs_mount, "USB: cleanup begin");
    let g = format!("{CONFIGFS_ROOT}/{gadget_name}");

    // Disable gadget first — empty UDC string unbinds from the UDC.
    if let Err(e) = fs::write(format!("{g}/UDC"), "\n") {
        debug!(error = %e, "cleanup: unbind UDC failed (already unbound?)");
    }

    // Remove function symlink.
    let _ = fs::remove_file(format!("{g}/configs/c.1/ffs.{gadget_name}"));

    // Remove config subdirs.
    let _ = fs::remove_dir(format!("{g}/configs/c.1/strings/0x409"));
    let _ = fs::remove_dir(format!("{g}/configs/c.1"));
    let _ = fs::remove_dir(format!("{g}/functions/ffs.{gadget_name}"));

    // Unmount FunctionFS.
    match Command::new("umount").arg(ffs_mount).output() {
        Ok(out) if out.status.success() => debug!(target = ffs_mount, "umount ok"),
        Ok(out) => warn!(
            "umount {ffs_mount} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => warn!("umount {ffs_mount} failed to spawn: {e}"),
    }
    let _ = fs::remove_dir(ffs_mount);

    // Remove gadget strings and root.
    let _ = fs::remove_dir(format!("{g}/strings/0x409"));
    let _ = fs::remove_dir(&g);

    debug!(gadget = gadget_name, "USB: cleanup done");
    Ok(())
}
