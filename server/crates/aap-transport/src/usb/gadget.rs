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

// configfs writes are the most common silent-failure source in this module.
// Every `os error 22 (Invalid argument)` you've ever stared at started life as
// a single fs::write whose error message dropped the path. `write_attr` keeps
// the path in the error, so the journal line tells you exactly which sysfs
// node the kernel rejected.
//
// USB bring-up checkpoints use `tracing::info!(target: "flight", …)` — see the
// `flight` target docs in smartcar-server/src/main.rs.
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

// Phase-1 (initial / "MTP-like") gadget — used so the HU sees a
// recognisable phone first and drives the AOAP mode-switch handshake
// (vendor requests 51/52/53 on ep0).
const GADGET_INIT: &str = "aap-init";
const GADGET_ACC: &str = "aap";

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

// AOAP vendor request codes consumed by `wait_for_aoap` during Phase 1.
const AOAP_GET_PROTOCOL: u8 = 51;
const AOAP_SEND_STRING: u8 = 52;
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
/// **Two-persona variant.** Real Android phones expose themselves first as
/// an ordinary device (MTP / file-transfer class on a Pixel) and only
/// after the head unit drives the AOAP mode-switch handshake on ep0 do
/// they disconnect and re-enumerate as the AOAP accessory persona. We
/// mirror that.
///
/// 1. **Phase 1** — bring up `aap-init` as `0x18d1:0x4ee1` (Pixel MTP),
///    ep0-only via FunctionFS with `FUNCTIONFS_ALL_CTRL_RECIP` so the
///    device-directed AOAP vendor requests reach userspace. Drive
///    `wait_for_aoap` until the HU sends `AOAP_START_ACCESSORY` (req 53)
///    and the bus is reset.
/// 2. **Phase 2** — tear down `aap-init` (Drop on the guard unbinds UDC,
///    umounts FunctionFS, removes configfs nodes) and bring up `aap` as
///    `0x18d1:0x2d00` (Google AOAP accessory) with EP1 IN + EP2 OUT.
///    Wait for `FUNCTIONFS_ENABLE` from the re-enumerated host, hand
///    EP1/EP2 back to the caller.
///
/// History: an earlier "skip-handshake" variant presented `0x18d1:0x2d00`
/// directly without Phase 1.  That worked against `openauto` and against a
/// Mac host (which accepts any descriptor) but caused Audi MMI 2022 to
/// reboot — the HU's USB state machine apparently can't reach the
/// accessory persona without seeing the mode-switch first. AACS uses the
/// same direct approach and is reportedly rejected by several modern HUs
/// (2022 Chevy Bolt, 2023 Mercedes — see github.com/tomasz-grobelny/AACS
/// issue #32), which lines up.
///
/// Returns `(gadget_guard, ep1_tx, ep2_rx)` where:
/// - `gadget_guard` — keeps the accessory gadget alive; drop it to disable USB.
/// - `ep1_tx`       — write-only file for outbound AA frames (board → host).
/// - `ep2_rx`       — read-only file for inbound AA frames (host → board).
///
/// **Blocking**: must be called from a blocking thread (e.g. `spawn_blocking`).
pub fn run_handshake() -> io::Result<(GadgetHandle, fs::File, fs::File)> {
    tracing::info!(target: "flight", "run_handshake: entered");

    // ── Phase 1: initial MTP-like persona for AOAP mode-switch ────────────
    info!("USB: phase 1 — setting up initial gadget ({GADGET_INIT}) as Pixel MTP (0x18d1:0x4ee1)");
    setup_gadget(GADGET_INIT, 0x18d1, 0x4ee1, FFS_MOUNT_INIT)?;
    tracing::info!(target: "flight", "run_handshake: phase 1 setup_gadget done");
    let init_guard = GadgetHandle {
        gadget_name: GADGET_INIT.to_owned(),
        ffs_mount: FFS_MOUNT_INIT.to_owned(),
    };

    let (mut ep0_init, ep1_idle, ep2_idle, ep3_idle) = write_and_enable_initial(
        GADGET_INIT,
        FFS_MOUNT_INIT,
        &descriptors::initial_descriptors(),
    )?;
    tracing::info!(target: "flight", "run_handshake: phase 1 write_and_enable_initial done (UDC bound as MTP)");

    info!("USB: phase 1 — waiting for AOAP vendor requests 51/52/53 from HU");
    wait_for_aoap(&mut ep0_init)?;
    tracing::info!(target: "flight", "run_handshake: phase 1 wait_for_aoap returned (Start-Accessory received)");

    // Tear down Phase 1 explicitly. Drop order: ep0 + idle bulk/intr
    // file handles first (closes those fds — kernel requires this before
    // unbinding the function), then the guard (whose Drop unbinds UDC,
    // umounts FunctionFS, removes configfs hierarchy). The HU sees
    // device-removed at this point and prepares to re-enumerate as the
    // accessory persona.
    drop(ep0_init);
    drop(ep1_idle);
    drop(ep2_idle);
    drop(ep3_idle);
    drop(init_guard);
    tracing::info!(target: "flight", "run_handshake: phase 1 teardown complete (UDC unbound, configfs cleared)");

    // ── Phase 2: accessory persona — EP1 IN + EP2 OUT ─────────────────────
    info!("USB: phase 2 — setting up accessory gadget ({GADGET_ACC}) as 0x18d1:0x2d00");
    setup_gadget(GADGET_ACC, 0x18d1, 0x2d00, FFS_MOUNT_ACC)?;
    tracing::info!(target: "flight", "run_handshake: phase 2 setup_gadget done");
    let acc_guard = GadgetHandle {
        gadget_name: GADGET_ACC.to_owned(),
        ffs_mount: FFS_MOUNT_ACC.to_owned(),
    };

    // write_and_enable returns *all three* FunctionFS file handles. The
    // ordering is critical and matches AACS: ep1/ep2 must be opened AFTER
    // descriptors are written (so the kernel has created the endpoint
    // nodes) but BEFORE the UDC is bound (after bind, the kernel hands
    // those endpoints over to the host stack and the file nodes disappear
    // from the FunctionFS mount). An earlier revision opened ep1/ep2 only
    // after FUNCTIONFS_ENABLE, which fails with ENOENT on a real host.
    let (mut ep0_acc, ep1, ep2) = write_and_enable(
        GADGET_ACC,
        FFS_MOUNT_ACC,
        &descriptors::accessory_descriptors(),
    )?;
    tracing::info!(target: "flight", "run_handshake: phase 2 write_and_enable done (ep0/ep1/ep2 open, UDC bound)");

    info!("USB: phase 2 — waiting for host to enumerate accessory gadget");
    wait_for_enable(&mut ep0_acc)?;
    tracing::info!(target: "flight", "run_handshake: phase 2 wait_for_enable returned (FUNCTIONFS_ENABLE received)");
    drop(ep0_acc);

    // Last sync point before handing the bulk endpoints back to the async
    // transport. Anything that happens after this is governed by the bulk
    // flow's own per-frame logs.
    tracing::info!(target: "flight", "run_handshake: ep0 dropped, returning Ok with ep1/ep2");
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
    tracing::info!(target: "flight", "setup_gadget: entered");

    // Gadget root and device-level attributes.
    mkdir(&g)?;
    tracing::info!(target: "flight", "setup_gadget: gadget root mkdir done");
    write_attr(format!("{g}/idVendor"), &format!("0x{vid:04x}\n"))?;
    write_attr(format!("{g}/idProduct"), &format!("0x{pid:04x}\n"))?;
    write_attr(format!("{g}/bcdUSB"), "0x0200\n")?;
    write_attr(format!("{g}/bcdDevice"), "0x0100\n")?;
    tracing::info!(target: "flight", "setup_gadget: device-level attrs (VID/PID/bcd*) written");

    // Language strings.
    mkdir(format!("{g}/strings/0x409"))?;
    write_attr(format!("{g}/strings/0x409/manufacturer"), MANUFACTURER)?;
    write_attr(format!("{g}/strings/0x409/product"), PRODUCT)?;
    write_attr(format!("{g}/strings/0x409/serialnumber"), SERIAL)?;
    tracing::info!(target: "flight", "setup_gadget: lang strings 0x409 written");

    // Configuration.
    mkdir(format!("{g}/configs/c.1/strings/0x409"))?;
    write_attr(
        format!("{g}/configs/c.1/strings/0x409/configuration"),
        "Config\n",
    )?;
    write_attr(format!("{g}/configs/c.1/MaxPower"), "500\n")?;
    tracing::info!(target: "flight", "setup_gadget: config c.1 written");

    // FunctionFS function (must exist before mount).
    mkdir(format!("{g}/functions/ffs.{name}"))?;
    tracing::info!(target: "flight", "setup_gadget: ffs function dir created");

    tracing::info!(target: "flight", "setup_gadget: configfs hierarchy written");

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
        tracing::info!(target: "flight", "setup_gadget: mount -t functionfs FAILED status={} stderr={}",
            output.status,
            stderr.trim());
        return Err(io::Error::other(format!(
            "mount -t functionfs {name} -> {ffs_mount} failed ({}): {}",
            output.status,
            stderr.trim()
        )));
    }
    tracing::info!(target: "flight", "setup_gadget: FunctionFS mounted");
    debug!("FunctionFS mounted at {ffs_mount}");
    Ok(())
}

/// Write descriptor + strings blobs to ep0, open the bulk endpoints, then
/// enable the gadget by binding it to the UDC.
///
/// Returns `(ep0, ep1, ep2)` — ep0 for reading FunctionFS events, ep1 for
/// outbound writes, ep2 for inbound reads.
///
/// # Why this order
///
/// Matches AACS (`AaCommunicator::setup`): write descs/strings → open
/// ep1/ep2 → bind UDC. The bulk-endpoint file nodes (`ep1`, `ep2`) are
/// created by the kernel when the descriptor blob is parsed, and they
/// remain openable from userspace ONLY between then and the UDC bind.
/// After the bind, the host's enumeration starts using those endpoints
/// through the kernel function driver and the file nodes disappear from
/// the FunctionFS mount — attempting to open them after that returns
/// ENOENT (confirmed empirically against a Mac host).
fn write_and_enable(
    gadget_name: &str,
    ffs_mount: &str,
    descs: &[u8],
) -> io::Result<(fs::File, fs::File, fs::File)> {
    tracing::info!(target: "flight", "write_and_enable: entered");
    let ep0_path = format!("{ffs_mount}/ep0");
    debug!(path = %ep0_path, "opening ep0");
    let mut ep0 = fs::File::options().read(true).write(true).open(&ep0_path)?;
    tracing::info!(target: "flight", "write_and_enable: ep0 opened");

    // Write descriptor blob then strings blob — two separate write(2) calls.
    ep0.write_all(descs).map_err(|e| {
        tracing::info!(target: "flight", "write_and_enable: ep0 descs write FAILED: {e}");
        io::Error::new(
            e.kind(),
            format!("ep0 write descriptors ({} bytes): {e}", descs.len()),
        )
    })?;
    let strs = descriptors::strings();
    ep0.write_all(&strs).map_err(|e| {
        tracing::info!(target: "flight", "write_and_enable: ep0 strings write FAILED: {e}");
        io::Error::new(
            e.kind(),
            format!("ep0 write strings ({} bytes): {e}", strs.len()),
        )
    })?;
    tracing::info!(target: "flight", "write_and_enable: ep0 descriptors+strings written");
    debug!(
        descs_bytes = descs.len(),
        strings_bytes = strs.len(),
        "FunctionFS descriptors written to {ep0_path}"
    );

    // Open ep1/ep2 NOW — between descriptor processing and UDC bind. See
    // function-level comment for why this window is the only one where
    // the open succeeds on a real Mac host.
    let ep1_path = format!("{ffs_mount}/ep1");
    let ep1 = fs::File::options()
        .write(true)
        .open(&ep1_path)
        .map_err(|e| {
            tracing::info!(target: "flight", "write_and_enable: open {ep1_path} (write) FAILED: {e}");
            io::Error::new(e.kind(), format!("open {ep1_path} (write): {e}"))
        })?;
    tracing::info!(target: "flight", "write_and_enable: ep1 opened (pre-UDC-bind)");

    let ep2_path = format!("{ffs_mount}/ep2");
    let ep2 = fs::File::options()
        .read(true)
        .open(&ep2_path)
        .map_err(|e| {
            tracing::info!(target: "flight", "write_and_enable: open {ep2_path} (read) FAILED: {e}");
            io::Error::new(e.kind(), format!("open {ep2_path} (read): {e}"))
        })?;
    tracing::info!(target: "flight", "write_and_enable: ep2 opened (pre-UDC-bind)");

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
    tracing::info!(target: "flight", "write_and_enable: found UDC {udc}, about to bind");
    info!(gadget = gadget_name, udc = %udc, "USB: binding gadget to UDC");
    let udc_path = format!("{g}/UDC");
    fs::write(&udc_path, format!("{udc}\n")).map_err(|e| {
        tracing::info!(target: "flight", "write_and_enable: UDC bind FAILED: {e}");
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
    tracing::info!(target: "flight", "write_and_enable: UDC bind WRITE returned Ok (host can now see us)");
    info!(gadget = gadget_name, udc = %udc, "USB gadget enabled");
    // CRITICAL: from this point on the host can see us and may yank Vbus at
    // any instant. Push every preceding log line out to journald NOW, while
    // we still have power.

    Ok((ep0, ep1, ep2))
}

/// Phase-1 variant of `write_and_enable` for the MTP-like initial persona.
///
/// Writes descriptors, opens ep0 + the three MTP-style endpoints (bulk
/// IN, bulk OUT, interrupt IN — same as `initial_descriptors()`), then
/// binds the UDC. The bulk/interrupt endpoints are held open by this
/// function's caller but never serviced; they exist solely so the host
/// sees a phone-shaped device and triggers its AOAP-probe path on ep0.
///
/// Returns `(ep0, ep1_idle, ep2_idle, ep3_idle)`. The three `_idle`
/// handles are dropped when Phase 1 ends (and along with them the
/// FunctionFS endpoint allocations), so no fd leaks. Same ordering
/// invariants as `write_and_enable`: write descriptors → open all
/// endpoints → symlink function into config → bind UDC.
fn write_and_enable_initial(
    gadget_name: &str,
    ffs_mount: &str,
    descs: &[u8],
) -> io::Result<(fs::File, fs::File, fs::File, fs::File)> {
    tracing::info!(target: "flight", "write_and_enable_initial: entered");
    let ep0_path = format!("{ffs_mount}/ep0");
    debug!(path = %ep0_path, "opening ep0 (initial)");
    let mut ep0 = fs::File::options().read(true).write(true).open(&ep0_path)?;
    tracing::info!(target: "flight", "write_and_enable_initial: ep0 opened");

    ep0.write_all(descs).map_err(|e| {
        tracing::info!(target: "flight", "write_and_enable_initial: ep0 descs write FAILED: {e}");
        io::Error::new(
            e.kind(),
            format!(
                "ep0 (initial) write descriptors ({} bytes): {e}",
                descs.len()
            ),
        )
    })?;
    let strs = descriptors::strings();
    ep0.write_all(&strs).map_err(|e| {
        tracing::info!(target: "flight", "write_and_enable_initial: ep0 strings write FAILED: {e}");
        io::Error::new(
            e.kind(),
            format!("ep0 (initial) write strings ({} bytes): {e}", strs.len()),
        )
    })?;
    tracing::info!(target: "flight", "write_and_enable_initial: ep0 descriptors+strings written");

    // Open the three MTP-style data endpoints we declared. The kernel
    // requires every declared endpoint to be opened by userspace before
    // the function is bound to a UDC. They stay open for the lifetime
    // of Phase 1 and are dropped (closed) when run_handshake's
    // `drop(ep_idle_*)` runs. We never read or write them — the car may
    // try MTP transfers on them; those just sit in kernel buffers /
    // time out, which is fine because the AOAP probe happens on ep0
    // and is what we actually care about.
    let ep1_path = format!("{ffs_mount}/ep1");
    let ep1_idle = fs::File::options()
        .write(true)
        .open(&ep1_path)
        .map_err(|e| {
            tracing::info!(target: "flight", "write_and_enable_initial: open {ep1_path} (idle bulk IN) FAILED: {e}");
            io::Error::new(e.kind(), format!("open {ep1_path} (idle bulk IN): {e}"))
        })?;
    tracing::info!(target: "flight", "write_and_enable_initial: ep1 (idle bulk IN) opened");

    let ep2_path = format!("{ffs_mount}/ep2");
    let ep2_idle = fs::File::options()
        .read(true)
        .open(&ep2_path)
        .map_err(|e| {
            tracing::info!(target: "flight", "write_and_enable_initial: open {ep2_path} (idle bulk OUT) FAILED: {e}");
            io::Error::new(e.kind(), format!("open {ep2_path} (idle bulk OUT): {e}"))
        })?;
    tracing::info!(target: "flight", "write_and_enable_initial: ep2 (idle bulk OUT) opened");

    let ep3_path = format!("{ffs_mount}/ep3");
    let ep3_idle = fs::File::options()
        .write(true)
        .open(&ep3_path)
        .map_err(|e| {
            tracing::info!(target: "flight", "write_and_enable_initial: open {ep3_path} (idle interrupt IN) FAILED: {e}");
            io::Error::new(
                e.kind(),
                format!("open {ep3_path} (idle interrupt IN): {e}"),
            )
        })?;
    tracing::info!(target: "flight", "write_and_enable_initial: ep3 (idle interrupt IN) opened");

    let g = format!("{CONFIGFS_ROOT}/{gadget_name}");
    let src = format!("{g}/functions/ffs.{gadget_name}");
    let dst = format!("{g}/configs/c.1/ffs.{gadget_name}");
    if !Path::new(&dst).exists() {
        std::os::unix::fs::symlink(&src, &dst)
            .map_err(|e| io::Error::new(e.kind(), format!("symlink {src} -> {dst}: {e}")))?;
    }

    let udc = find_udc()?;
    tracing::info!(target: "flight", "write_and_enable_initial: found UDC {udc}, about to bind");
    info!(gadget = gadget_name, udc = %udc, "USB: binding initial gadget to UDC");
    let udc_path = format!("{g}/UDC");
    fs::write(&udc_path, format!("{udc}\n")).map_err(|e| {
        tracing::info!(target: "flight", "write_and_enable_initial: UDC bind FAILED: {e}");
        io::Error::new(
            e.kind(),
            format!("UDC bind (initial) failed ({udc_path} <- '{udc}'): {e}"),
        )
    })?;
    tracing::info!(target: "flight", "write_and_enable_initial: UDC bind WRITE returned Ok (host can now see MTP-like persona)",
    );

    Ok((ep0, ep1_idle, ep2_idle, ep3_idle))
}

/// Read ep0 events and respond to AOAP vendor requests.
///
/// Returns when `AOAP_START_ACCESSORY` (req 53) is received — the host
/// will then disconnect us and re-enumerate as the accessory persona,
/// which is what Phase 2 of `run_handshake` brings up.
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
            FUNCTIONFS_BIND => {
                tracing::info!(target: "flight", "wait_for_aoap: ep0 BIND");
                debug!("ep0: BIND");
            }
            FUNCTIONFS_ENABLE => {
                tracing::info!(target: "flight", "wait_for_aoap: ep0 ENABLE");
                debug!("ep0: ENABLE");
            }
            FUNCTIONFS_SETUP => match b_request {
                AOAP_GET_PROTOCOL => {
                    tracing::info!(target: "flight", "wait_for_aoap: AOAP 51 Get-Protocol, responding v2");
                    debug!("AOAP req 51: Get-Protocol → v2");
                    ep0.write_all(&[0x02, 0x00]).map_err(|e| {
                        tracing::info!(target: "flight", "wait_for_aoap: AOAP 51 response write FAILED: {e}");
                        e
                    })?;
                    tracing::info!(target: "flight", "wait_for_aoap: AOAP 51 response sent");
                }
                AOAP_SEND_STRING => {
                    let idx = u16::from_le_bytes([event[4], event[5]]);
                    tracing::info!(target: "flight", "wait_for_aoap: AOAP 52 Send-String idx={idx} (no reply)");
                    debug!("AOAP req 52: Send-String idx={idx} (no response needed)");
                    // OUT transfer — host sends string; no IN reply required.
                }
                AOAP_START_ACCESSORY => {
                    tracing::info!(target: "flight", "wait_for_aoap: AOAP 53 Start-Accessory, exiting Phase 1");
                    debug!("AOAP req 53: Start-Accessory");
                    return Ok(());
                }
                other => {
                    tracing::info!(target: "flight", "wait_for_aoap: unexpected SETUP bRequest={other} bRequestType={} wValue={} wIndex={} wLength={}",
                        event[0],
                        u16::from_le_bytes([event[2], event[3]]),
                        u16::from_le_bytes([event[4], event[5]]),
                        u16::from_le_bytes([event[6], event[7]]),);
                    debug!("ep0 SETUP: unknown bRequest={other}");
                }
            },
            other => {
                tracing::info!(target: "flight", "wait_for_aoap: unknown event_type={other}");
                debug!("ep0: unknown event type={other}");
            }
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
            last_heartbeat = Instant::now();
            continue;
        }

        ep0.read_exact(&mut event)?;
        events += 1;
        let evt = event[8];
        let since_last = last_heartbeat.elapsed();
        match evt {
            FUNCTIONFS_ENABLE => {
                tracing::info!(target: "flight", "wait_for_enable: ENABLE received");
                info!(
                    total_events = events,
                    elapsed_s = started.elapsed().as_secs(),
                    "USB: FUNCTIONFS_ENABLE received — host has enumerated us"
                );
                return Ok(());
            }
            FUNCTIONFS_BIND => {
                tracing::info!(target: "flight", "wait_for_enable: acc ep0 BIND");
                debug!(elapsed_ms = since_last.as_millis(), "acc ep0: BIND");
            }
            FUNCTIONFS_SETUP => {
                tracing::info!(target: "flight", "wait_for_enable: SETUP (pre-ENABLE) bRequest={} wValue={} wIndex={} wLength={}",
                    event[1],
                    u16::from_le_bytes([event[2], event[3]]),
                    u16::from_le_bytes([event[4], event[5]]),
                    u16::from_le_bytes([event[6], event[7]]),);
                debug!(
                    elapsed_ms = since_last.as_millis(),
                    bRequestType = event[0],
                    bRequest = event[1],
                    wValue = u16::from_le_bytes([event[2], event[3]]),
                    wIndex = u16::from_le_bytes([event[4], event[5]]),
                    wLength = u16::from_le_bytes([event[6], event[7]]),
                    "acc ep0: SETUP (unexpected before ENABLE)"
                );
            }
            other => {
                tracing::info!(target: "flight", "wait_for_enable: unknown event_type={other}");
                debug!(
                    elapsed_ms = since_last.as_millis(),
                    evt = other,
                    "acc ep0: event"
                );
            }
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
    tracing::info!(target: "flight", "cleanup: begin gadget={gadget_name}");
    debug!(gadget = gadget_name, ffs_mount, "USB: cleanup begin");
    let g = format!("{CONFIGFS_ROOT}/{gadget_name}");

    // Disable gadget first — empty UDC string unbinds from the UDC.
    if let Err(e) = fs::write(format!("{g}/UDC"), "\n") {
        debug!(error = %e, "cleanup: unbind UDC failed (already unbound?)");
    }
    tracing::info!(target: "flight", "cleanup: UDC unbound gadget={gadget_name}");

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

    tracing::info!(target: "flight", "cleanup: done gadget={gadget_name}");
    debug!(gadget = gadget_name, "USB: cleanup done");
    Ok(())
}
