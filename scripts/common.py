"""Shared constants and helpers for smartcar run scripts."""

from __future__ import annotations

import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST  = REPO_ROOT / "server" / "Cargo.toml"

DEFAULT_TARGET = "127.0.0.1:5278"  # native openauto TCP port
DEFAULT_LOG    = "info"


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
