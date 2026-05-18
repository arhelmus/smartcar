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

use tracing::{debug, info, warn};

use super::descriptors;

// ── Gadget identity ───────────────────────────────────────────────────────────

const CONFIGFS_ROOT: &str = "/sys/kernel/config/usb_gadget";

const GADGET_INIT: &str = "aap-init";
const GADGET_ACC: &str = "aap";

const FFS_MOUNT_INIT: &str = "/dev/ffs-aap-init";
const FFS_MOUNT_ACC: &str = "/dev/ffs-aap";

const MANUFACTURER: &str = "TAG";
const PRODUCT: &str = "AAServer";
const SERIAL: &str = "TAGAAS";

// ── FunctionFS ep0 event constants (linux/usb/functionfs.h) ──────────────────

const FUNCTIONFS_BIND: u8 = 0;
const FUNCTIONFS_ENABLE: u8 = 2;
const FUNCTIONFS_SETUP: u8 = 4;

// ── AOAP control-transfer request codes ──────────────────────────────────────

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

/// Run the full two-persona AOAP handshake and return the open bulk endpoints.
///
/// Returns `(gadget_guard, ep1_tx, ep2_rx)` where:
/// - `gadget_guard` — keeps the accessory gadget alive; drop it to disable USB.
/// - `ep1_tx`       — write-only file for outbound AA frames (board → host).
/// - `ep2_rx`       — read-only file for inbound AA frames (host → board).
///
/// **Blocking**: must be called from a blocking thread (e.g. `spawn_blocking`).
pub fn run_handshake() -> io::Result<(GadgetHandle, fs::File, fs::File)> {
    // ── Phase 1: initial gadget — ep0 only, receives AOAP requests ────────
    info!("USB: setting up initial gadget ({GADGET_INIT})");
    setup_gadget(GADGET_INIT, 0x12d1, 0x107e, FFS_MOUNT_INIT)?;
    let init_guard = GadgetHandle {
        gadget_name: GADGET_INIT.to_owned(),
        ffs_mount: FFS_MOUNT_INIT.to_owned(),
    };

    let mut ep0 = write_and_enable(
        GADGET_INIT,
        FFS_MOUNT_INIT,
        &descriptors::initial_descriptors(),
    )?;

    info!("USB: waiting for AOAP handshake from host");
    wait_for_aoap(&mut ep0)?;
    info!("USB: AOAP handshake done; switching to accessory persona");

    // Close ep0 before tearing down the mount, then drop the guard.
    drop(ep0);
    drop(init_guard);

    // ── Phase 2: accessory gadget — EP1 IN + EP2 OUT ─────────────────────
    info!("USB: setting up accessory gadget ({GADGET_ACC})");
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

    Ok((acc_guard, ep1, ep2))
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Create the configfs gadget hierarchy and mount FunctionFS.
fn setup_gadget(name: &str, vid: u16, pid: u16, ffs_mount: &str) -> io::Result<()> {
    let g = format!("{CONFIGFS_ROOT}/{name}");

    // Gadget root and device-level attributes.
    fs::create_dir_all(&g)?;
    fs::write(format!("{g}/idVendor"), format!("0x{vid:04x}\n"))?;
    fs::write(format!("{g}/idProduct"), format!("0x{pid:04x}\n"))?;
    fs::write(format!("{g}/bcdUSB"), "0x0200\n")?;
    fs::write(format!("{g}/bcdDevice"), "0x0100\n")?;

    // Language strings.
    fs::create_dir_all(format!("{g}/strings/0x409"))?;
    fs::write(format!("{g}/strings/0x409/manufacturer"), MANUFACTURER)?;
    fs::write(format!("{g}/strings/0x409/product"), PRODUCT)?;
    fs::write(format!("{g}/strings/0x409/serialnumber"), SERIAL)?;

    // Configuration.
    fs::create_dir_all(format!("{g}/configs/c.1/strings/0x409"))?;
    fs::write(
        format!("{g}/configs/c.1/strings/0x409/configuration"),
        "Config\n",
    )?;
    fs::write(format!("{g}/configs/c.1/MaxPower"), "500\n")?;

    // FunctionFS function (must exist before mount).
    fs::create_dir_all(format!("{g}/functions/ffs.{name}"))?;

    // Mount FunctionFS — this creates ep0 in the mount directory.
    fs::create_dir_all(ffs_mount)?;
    let status = Command::new("mount")
        .args(["-t", "functionfs", name, ffs_mount])
        .status()?;
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("mount functionfs {name} → {ffs_mount} failed: {status}"),
        ));
    }

    debug!("FunctionFS mounted at {ffs_mount}");
    Ok(())
}

/// Write descriptor + strings blobs to ep0, link function into config, enable gadget.
///
/// Returns the open ep0 file (needed for reading events).
fn write_and_enable(gadget_name: &str, ffs_mount: &str, descs: &[u8]) -> io::Result<fs::File> {
    let ep0_path = format!("{ffs_mount}/ep0");
    let mut ep0 = fs::File::options().read(true).write(true).open(&ep0_path)?;

    // Write descriptor blob then strings blob — two separate write(2) calls.
    ep0.write_all(descs)?;
    ep0.write_all(&descriptors::strings())?;
    debug!("FunctionFS descriptors written to {ep0_path}");

    // Symlink function into config AFTER descriptors are written.
    let g = format!("{CONFIGFS_ROOT}/{gadget_name}");
    let src = format!("{g}/functions/ffs.{gadget_name}");
    let dst = format!("{g}/configs/c.1/ffs.{gadget_name}");
    if !Path::new(&dst).exists() {
        std::os::unix::fs::symlink(&src, &dst)?;
    }

    // Enable gadget by binding to the first available UDC.
    let udc = find_udc()?;
    fs::write(format!("{g}/UDC"), format!("{udc}\n"))?;
    info!("USB gadget {gadget_name} enabled on UDC {udc}");

    Ok(ep0)
}

/// Read ep0 events and respond to AOAP vendor requests.
///
/// Returns when `AOAP_START_ACCESSORY` (req 53) is received.
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
fn wait_for_enable(ep0: &mut fs::File) -> io::Result<()> {
    let mut event = [0u8; 12];
    loop {
        ep0.read_exact(&mut event)?;
        match event[8] {
            FUNCTIONFS_ENABLE => return Ok(()),
            FUNCTIONFS_BIND => debug!("acc ep0: BIND"),
            other => debug!("acc ep0: event type={other}"),
        }
    }
}

/// Return the name of the first available USB Device Controller.
fn find_udc() -> io::Result<String> {
    for entry in fs::read_dir("/sys/class/udc/")? {
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
/// Errors are logged but not propagated (called from Drop).
fn cleanup(gadget_name: &str, ffs_mount: &str) -> io::Result<()> {
    let g = format!("{CONFIGFS_ROOT}/{gadget_name}");

    // Disable gadget first.
    let _ = fs::write(format!("{g}/UDC"), "\n");

    // Remove function symlink.
    let _ = fs::remove_file(format!("{g}/configs/c.1/ffs.{gadget_name}"));

    // Remove config subdirs.
    let _ = fs::remove_dir(format!("{g}/configs/c.1/strings/0x409"));
    let _ = fs::remove_dir(format!("{g}/configs/c.1"));
    let _ = fs::remove_dir(format!("{g}/functions/ffs.{gadget_name}"));

    // Unmount FunctionFS.
    let status = Command::new("umount").arg(ffs_mount).status();
    if let Ok(s) = status {
        if !s.success() {
            warn!("umount {ffs_mount} exited with {s}");
        }
    }
    let _ = fs::remove_dir(ffs_mount);

    // Remove gadget strings and root.
    let _ = fs::remove_dir(format!("{g}/strings/0x409"));
    let _ = fs::remove_dir(&g);

    Ok(())
}
