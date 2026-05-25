#!/usr/bin/env python3
"""assign_board.py — assign the laptop's USB-Ethernet IP after the board boots.

Finds the interface whose MAC matches BOARD_MAC from .env.local and assigns
it the standard laptop IP (10.55.0.2/24) via ifconfig.

Usage:
    python3 scripts/assign_board.py          # assign once
    python3 scripts/assign_board.py --check  # check current state, don't change
    python3 scripts/assign_board.py --watch  # poll every 5s and bind when the board appears

Requires sudo for ifconfig.
"""

from __future__ import annotations

import argparse
import os
import sys
import time

import common

WATCH_INTERVAL_SECONDS = 5


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Assign laptop USB-Ethernet IP so the board is reachable after boot."
    )
    parser.add_argument("--check", action="store_true",
                        help="Only check the current state; don't assign anything.")
    parser.add_argument("--watch", action="store_true",
                        help=f"Poll every {WATCH_INTERVAL_SECONDS}s and bind the board "
                             "whenever it appears unbound. Runs until interrupted.")
    args = parser.parse_args()

    if args.watch and args.check:
        parser.error("--watch and --check are mutually exclusive")

    if args.watch:
        return _watch()

    rc = common.assign_board_ip(check=args.check)
    if rc == 0 and not args.check:
        print(f"SSH: ssh {common.BOARD_USER}@{common.BOARD_HOST}", file=sys.stderr)
    return rc


def _watch() -> int:
    if os.geteuid() != 0:
        _reexec_with_sudo()  # does not return

    print(
        f"Watching for board every {WATCH_INTERVAL_SECONDS}s (Ctrl+C to stop)…",
        file=sys.stderr,
    )
    try:
        while True:
            common.assign_board_ip()
            time.sleep(WATCH_INTERVAL_SECONDS)
    except KeyboardInterrupt:
        print("\nStopped.", file=sys.stderr)
        return 0


def _reexec_with_sudo() -> None:
    """Replace this process with `sudo -E python <script> <args…>`.

    Hands control to sudo so it can prompt for a password on the user's
    terminal, then runs the same invocation as root. Never returns.
    """
    script = os.path.abspath(sys.argv[0])
    argv = ["sudo", "-E", sys.executable, script, *sys.argv[1:]]
    print(f"Re-running under sudo: {' '.join(argv[2:])}", file=sys.stderr)
    os.execvp("sudo", argv)


if __name__ == "__main__":
    raise SystemExit(main())
