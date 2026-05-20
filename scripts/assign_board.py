#!/usr/bin/env python3
"""assign_board.py — assign the laptop's USB-Ethernet IP after the board boots.

Finds the interface whose MAC matches BOARD_MAC from .env.local and assigns
it the standard laptop IP (10.55.0.2/24) via ifconfig.

Usage:
    python3 scripts/assign_board.py          # assign
    python3 scripts/assign_board.py --check  # check current state, don't change

Requires sudo for ifconfig.
"""

from __future__ import annotations

import argparse
import sys

import common


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Assign laptop USB-Ethernet IP so the board is reachable after boot."
    )
    parser.add_argument("--check", action="store_true",
                        help="Only check the current state; don't assign anything.")
    args = parser.parse_args()

    rc = common.assign_board_ip(check=args.check)
    if rc == 0 and not args.check:
        print(f"SSH: ssh {common.BOARD_USER}@{common.BOARD_HOST}", file=sys.stderr)
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
