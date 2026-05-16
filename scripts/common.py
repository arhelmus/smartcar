"""Shared constants and helpers for smartcar run scripts."""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST  = REPO_ROOT / "server" / "Cargo.toml"

DEFAULT_TARGET = "127.0.0.1:5278"  # native openauto TCP port
DEFAULT_LOG    = "info"

SERVER_PID_FILE = REPO_ROOT / ".smartcar-server.pid"
SERVER_LOG_FILE = REPO_ROOT / "smartcar-server.log"


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
