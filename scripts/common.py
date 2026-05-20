"""Shared constants and helpers for smartcar run scripts."""

from __future__ import annotations

import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST  = REPO_ROOT / "server" / "Cargo.toml"

DEFAULT_TARGET = "127.0.0.1:5278"  # native openauto TCP port
DEFAULT_LOG    = "info"

SERVER_PID_FILE = REPO_ROOT / ".smartcar-server.pid"
SERVER_LOG_FILE = REPO_ROOT / "smartcar-server.log"


# ── .env.local loader ─────────────────────────────────────────────────────────

def _load_env_local() -> None:
    """Parse .env.local in the repo root and inject into os.environ.

    Existing environment variables take precedence (they are not overwritten),
    so shell exports or CI variables always win over the file.
    """
    env_file = REPO_ROOT / ".env.local"
    if not env_file.exists():
        return
    for line in env_file.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip()
        if key and key not in os.environ:
            os.environ[key] = value

_load_env_local()


# ── make-init freshness check ────────────────────────────────────────────────

def _check_init_stamp() -> None:
    """Verify scripts/.init records the current init.py version.

    Catches the "I pulled main but forgot to re-run make init" case where
    init.py grew a new step (a prerequisite, a patch, a seeded file) that
    later scripts now assume. Importing `common` from anywhere triggers
    this — runs before any other check so the failure mode is the actionable
    one ("run make init") and not a downstream symptom.
    """
    from init import INIT_VERSION  # canonical version lives in init.py

    stamp = REPO_ROOT / "scripts" / ".init"
    try:
        recorded = stamp.read_text().strip()
    except FileNotFoundError:
        print(
            f"ERROR: scripts/.init missing — run `make init` "
            f"(init.py is at v{INIT_VERSION}).",
            file=sys.stderr,
        )
        raise SystemExit(1)
    if recorded != str(INIT_VERSION):
        print(
            f"ERROR: scripts/.init is stale (records v{recorded}, init.py is "
            f"at v{INIT_VERSION}) — re-run `make init`.",
            file=sys.stderr,
        )
        raise SystemExit(1)

_check_init_stamp()


def _require_env(key: str) -> str:
    """Return env[key] stripped, or die loud if it is unset / blank.

    .env.local is the single source of truth for board / link config; the
    scripts don't carry built-in defaults so a missing key never silently
    points at the wrong host or interface.
    """
    value = os.environ.get(key, "").strip()
    if not value:
        print(
            f"ERROR: {key} not set — add it to .env.local "
            "(see .env.local.example).",
            file=sys.stderr,
        )
        raise SystemExit(1)
    return value


# ── Local server lifecycle ───────────────────────────────────────────────────

def stop_local_server() -> None:
    """Terminate the local smartcar-server started by run_server.py, if any.

    Reads SERVER_PID_FILE, SIGTERMs the process (escalating to SIGKILL after
    ~2 s), and removes the pid file.  No-op when nothing is running.  Used by
    run_server.py before relaunching and by run_board.py so the laptop server
    does not contend with the board for the openauto connection.
    """
    if not SERVER_PID_FILE.exists():
        return
    try:
        pid = int(SERVER_PID_FILE.read_text().strip())
    except ValueError:
        SERVER_PID_FILE.unlink(missing_ok=True)
        return

    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        SERVER_PID_FILE.unlink(missing_ok=True)
        return

    print(f"Stopping local server (pid {pid})…", file=sys.stderr)
    try:
        os.kill(pid, signal.SIGTERM)
        for _ in range(20):  # wait up to 2 s
            time.sleep(0.1)
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
        else:
            os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    SERVER_PID_FILE.unlink(missing_ok=True)


# ── Laptop USB-Ethernet IP discovery ─────────────────────────────────────────

def _find_ip_by_mac(mac: str) -> str:
    """Return the IPv4 address of the local interface whose MAC matches *mac*.

    Parses `ifconfig` output (macOS / Linux).  Returns an empty string if the
    interface is not found or has no IPv4 address.
    """
    mac = mac.lower()
    try:
        out = subprocess.check_output(["ifconfig"], text=True, stderr=subprocess.DEVNULL)
    except (FileNotFoundError, subprocess.CalledProcessError):
        return ""

    # Split into per-interface blocks (lines not starting with whitespace begin
    # a new interface on both macOS and Linux ifconfig).
    current_mac = ""
    current_ip  = ""
    for line in out.splitlines():
        if not line[:1].isspace():
            # New interface block — flush previous if it matched.
            current_mac = ""
            current_ip  = ""
        stripped = line.strip()
        m = re.search(r"\bether\s+([0-9a-f:]{17})\b", stripped)
        if m:
            current_mac = m.group(1).lower()
        m = re.search(r"\binet\s+(\d+\.\d+\.\d+\.\d+)\b", stripped)
        if m:
            current_ip = m.group(1)
        if current_mac == mac and current_ip:
            return current_ip

    return ""


def laptop_usb_ip() -> str:
    """Return the laptop's USB-Ethernet IP, found by matching BOARD_MAC.

    BOARD_MAC is the host-side MAC the board's g_ether gadget advertises
    (`/etc/modprobe.d/g_ether.conf` → `host_addr`), so it identifies which of
    the laptop's interfaces is the USB link to the board regardless of the
    rotating `enX` name macOS assigns it. Aborts if BOARD_MAC is unset;
    returns an empty string when no matching interface is found (caller
    decides whether that's fatal).
    """
    return _find_ip_by_mac(_require_env("BOARD_MAC"))


# ── Board USB-Ethernet IP assignment ─────────────────────────────────────────


def _find_iface_by_mac(mac: str) -> str | None:
    """Return the interface name whose ether address matches *mac*."""
    mac = mac.lower()
    try:
        out = subprocess.check_output(["ifconfig"], text=True, stderr=subprocess.DEVNULL)
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None

    current_iface = ""
    for line in out.splitlines():
        if not line[:1].isspace():
            current_iface = line.split(":")[0]
        elif re.search(rf"\bether\s+{re.escape(mac)}\b", line.strip()):
            return current_iface
    return None


def assign_board_ip(check: bool = False) -> int:
    """Assign the laptop's USB-Ethernet IP so the board is reachable.

    Finds the interface whose MAC matches BOARD_MAC (set by the board's
    g_ether gadget) and assigns it 10.55.0.2/24 via `sudo ifconfig` —
    idempotent, a no-op when already assigned. The board sits at the fixed
    g_ether gadget address 10.55.0.1, so the laptop side is paired to it.
    With *check* only reports the current state. Returns 0 on success /
    nothing to do, non-zero on error.
    """
    ip = _require_env("LAPTOP_HOST")
    mask = "24"
    mac = _require_env("BOARD_MAC")

    iface = _find_iface_by_mac(mac)
    if not iface:
        print(f"ERROR: no interface found with MAC {mac}.", file=sys.stderr)
        print("       Is the USB cable plugged in and the board powered on?", file=sys.stderr)
        return 1

    print(f"Found interface: {iface} (MAC {mac})", file=sys.stderr)

    out = subprocess.check_output(["ifconfig", iface], text=True)
    m = re.search(r"\binet\s+(\d+\.\d+\.\d+\.\d+)\b", out)
    current_ip = m.group(1) if m else None

    if check:
        print(f"Current IP: {current_ip}" if current_ip else "No IP assigned.", file=sys.stderr)
        return 0

    if current_ip == ip:
        print(f"Already assigned {ip} — nothing to do.", file=sys.stderr)
        return 0

    print(f"Assigning {ip}/{mask} to {iface} …", file=sys.stderr)
    result = subprocess.run(["sudo", "ifconfig", iface, f"{ip}/{mask}"])
    if result.returncode != 0:
        print("ERROR: ifconfig failed.", file=sys.stderr)
        return result.returncode
    return 0


# ── Board config (populated from .env.local / environment) ───────────────────

CROSS_TARGET = "aarch64-unknown-linux-gnu"

BOARD_HOST = _require_env("BOARD_HOST")
BOARD_USER = _require_env("BOARD_USER")

BOARD_SERVER_BINARY = "/usr/local/bin/smartcar-server"
BOARD_PID_FILE      = "/tmp/smartcar-server.pid"
BOARD_LOG_FILE      = "/tmp/smartcar-server.log"


# ── Build helpers ─────────────────────────────────────────────────────────────

def cargo_build_cmd(release: bool, flutter: bool = True) -> list[str]:
    cmd = [
        "cargo", "build",
        "--manifest-path", str(MANIFEST),
        "--bin", "smartcar-server",
    ]
    # Flutter is always compiled in (no longer a cargo feature); `flutter`
    # only selects the runtime producer via `--testkit`.
    if release:
        cmd.append("--release")
    return cmd


def server_binary_path(release: bool) -> Path:
    profile = "release" if release else "debug"
    return REPO_ROOT / "server" / "target" / profile / "smartcar-server"


def cargo_cross_build_cmd(release: bool, flutter: bool = True) -> list[str]:
    # Vendor OpenSSL statically: the cross toolchain links libssl 1.1 but the
    # board (Debian 13) ships only libssl.so.3, so a dynamic link never
    # resolves. Static vendoring makes the ARM binary self-contained.
    features = "openssl-vendored"
    cmd = [
        "cross", "build", "--target", CROSS_TARGET,
        "--bin", "smartcar-server", "--features", features,
    ]
    if release:
        cmd.append("--release")
    return cmd


def server_cross_binary_path(release: bool) -> Path:
    profile = "release" if release else "debug"
    return REPO_ROOT / "server" / "target" / CROSS_TARGET / profile / "smartcar-server"


def cross_build_and_deploy(board: str, user: str, release: bool) -> int:
    """Cross-compile smartcar-server and rsync it to the board.

    Returns 0 on success, non-zero on failure.
    Prerequisites (cross, rustup toolchain) are set up by running: make init
    """
    for tool, hint in [("cross", "cargo install cross"), ("rsync", "brew install rsync")]:
        if not shutil.which(tool):
            print(f"ERROR: '{tool}' not found — run 'make init' to set up prerequisites.", file=sys.stderr)
            return 1

    build_cmd = cargo_cross_build_cmd(release=release)
    server_dir = str(REPO_ROOT / "server")
    profile = "release" if release else "debug"
    print(f"Cross-compiling ({profile}) for {CROSS_TARGET} …", file=sys.stderr)

    # Force amd64 — the cross image is amd64-only (runs via Rosetta on Apple Silicon).
    env = os.environ.copy()
    env["DOCKER_DEFAULT_PLATFORM"] = "linux/amd64"
    result = subprocess.run(build_cmd, cwd=server_dir, env=env)
    if result.returncode != 0:
        print(f"ERROR: cross build failed (exit {result.returncode}).", file=sys.stderr)
        return result.returncode

    binary = server_cross_binary_path(release=release)
    if not binary.exists():
        print(f"ERROR: expected binary not found at {binary}", file=sys.stderr)
        return 1
    print(f"Built: {binary} ({binary.stat().st_size // 1024} KiB)", file=sys.stderr)

    dest = f"{user}@{board}:{BOARD_SERVER_BINARY}"
    print(f"Deploying to {dest} …", file=sys.stderr)
    result = subprocess.run(["rsync", "-az", "--progress", str(binary), dest])
    if result.returncode != 0:
        print(f"ERROR: rsync failed (exit {result.returncode}).", file=sys.stderr)
        return result.returncode

    # Flutter runtime: the build script stages libflutter_engine.so,
    # flutter_assets/ and icudtl.dat next to the binary. Ship them alongside
    # the executable on the board — the binary's $ORIGIN rpath resolves the
    # engine .so, and resolve_flutter_paths() finds the bundle next to itself.
    # Absent (testkit build) → skipped; the server falls back at runtime.
    board_dir = BOARD_SERVER_BINARY.rsplit("/", 1)[0] + "/"
    staged = binary.parent
    for name in ("libflutter_engine.so", "icudtl.dat", "flutter_assets"):
        src = staged / name
        if not src.exists():
            print(f"NOTE: {name} not staged — skipping (testkit build?).", file=sys.stderr)
            continue
        print(f"Deploying {name} → {user}@{board}:{board_dir}{name} …", file=sys.stderr)
        if src.is_dir():
            subprocess.run(["ssh", f"{user}@{board}", "mkdir", "-p", f"{board_dir}{name}"])
            # Trailing slash: copy contents into board_dir/<name>/.
            rsync = ["rsync", "-az", "--delete", f"{src}/",
                     f"{user}@{board}:{board_dir}{name}/"]
        else:
            rsync = ["rsync", "-az", str(src), f"{user}@{board}:{board_dir}{name}"]
        result = subprocess.run(rsync)
        if result.returncode != 0:
            print(f"ERROR: rsync of {name} failed (exit {result.returncode}).", file=sys.stderr)
            return result.returncode

    return 0


def cargo_run_cmd(release: bool, target: str, flutter: bool = True) -> list[str]:
    cmd = [
        "cargo", "run",
        "--manifest-path", str(MANIFEST),
        "--bin", "smartcar-server",
    ]
    # Flutter is always compiled in; `flutter=False` only switches the
    # runtime producer to the synthetic testkit one.
    if release:
        cmd.append("--release")
    cmd += ["--", "--target", target]
    if not flutter:
        cmd.append("--testkit")
    return cmd
