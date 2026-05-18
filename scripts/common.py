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
    """Return the laptop's USB-Ethernet IP.

    Resolution order:
      1. LAPTOP_USB_IP env var (explicit override)
      2. Auto-detect via LAPTOP_USB_MAC env var (MAC → ifconfig lookup)
      3. Empty string (caller must handle the missing case)
    """
    explicit = os.environ.get("LAPTOP_USB_IP", "").strip()
    if explicit:
        return explicit
    mac = os.environ.get("LAPTOP_USB_MAC", "").strip()
    if mac:
        return _find_ip_by_mac(mac)
    return ""


# ── Board config (populated from .env.local / environment) ───────────────────

CROSS_TARGET = "aarch64-unknown-linux-gnu"

BOARD_HOST = os.environ.get("BOARD_HOST", "10.55.0.1")
BOARD_USER = os.environ.get("BOARD_USER", "root")

BOARD_SERVER_BINARY = "/usr/local/bin/smartcar-server"
BOARD_PID_FILE      = "/tmp/smartcar-server.pid"
BOARD_LOG_FILE      = "/tmp/smartcar-server.log"


# ── Build helpers ─────────────────────────────────────────────────────────────

def cargo_build_cmd(release: bool) -> list[str]:
    cmd = [
        "cargo", "build",
        "--manifest-path", str(MANIFEST),
        "--bin", "smartcar-server",
    ]
    if release:
        cmd.append("--release")
    return cmd


def server_binary_path(release: bool) -> Path:
    profile = "release" if release else "debug"
    return REPO_ROOT / "server" / "target" / profile / "smartcar-server"


def cargo_cross_build_cmd(release: bool) -> list[str]:
    # Vendor OpenSSL statically: the cross toolchain links libssl 1.1 but the
    # board (Debian 13) ships only libssl.so.3, so a dynamic link never
    # resolves. Static vendoring makes the ARM binary self-contained.
    cmd = [
        "cross", "build", "--target", CROSS_TARGET,
        "--bin", "smartcar-server", "--features", "openssl-vendored",
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

    return 0


def cargo_run_cmd(release: bool, target: str) -> list[str]:
    cmd = [
        "cargo", "run",
        "--manifest-path", str(MANIFEST),
        "--bin", "smartcar-server",
    ]
    if release:
        cmd.append("--release")
    cmd += ["--", "--target", target]
    return cmd
