#!/usr/bin/env python3
"""board_provision.py — run the board/ ansible playbook with .env.local loaded.

Thin wrapper around `ansible-playbook`. Imports `common` to pull
.env.local into the environment, asserts that the board-addressing vars
(BOARD_HOST / BOARD_USER / BOARD_MAC) are present, then execs
ansible-playbook in board/. Extra args after `--` are passed straight
through, so `--check`, `--diff`, `-l <host>`, `--tags <tag>` all work.

Usage:
    python3 scripts/board_provision.py                # apply
    python3 scripts/board_provision.py -- --check --diff
    python3 scripts/board_provision.py -- -l orangepi-dev --tags usb_gadget
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

# Pull in .env.local. common.py also runs the init-stamp guard, so a stale
# checkout fails fast with "run make init" before we touch ansible.
import common  # noqa: F401

REPO_ROOT = Path(__file__).resolve().parent.parent
BOARD = REPO_ROOT / "board"

REQUIRED_ENV = ("BOARD_HOST", "BOARD_USER", "BOARD_MAC", "BOARD_MAC_DEV")


def main() -> int:
    if not shutil.which("ansible-playbook"):
        print(
            "ERROR: ansible-playbook not found — run `make init` or "
            "`brew install ansible`.",
            file=sys.stderr,
        )
        return 1

    missing = [k for k in REQUIRED_ENV if not os.environ.get(k, "").strip()]
    if missing:
        print(
            f"ERROR: missing {' '.join(missing)} in .env.local "
            "(see .env.local.example).",
            file=sys.stderr,
        )
        return 1

    # Drop the leading '--' separator if present so users can write
    # `... -- --check --diff` to disambiguate ansible flags from ours.
    extra = sys.argv[1:]
    if extra and extra[0] == "--":
        extra = extra[1:]

    cmd = ["ansible-playbook", "site.yml", *extra]
    print(f"+ cd {BOARD} && {' '.join(cmd)}", file=sys.stderr)
    return subprocess.call(cmd, cwd=BOARD)


if __name__ == "__main__":
    raise SystemExit(main())
