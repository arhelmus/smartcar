#!/usr/bin/env python3
"""assign_board.py — assign the laptop's USB-Ethernet IP after the board boots.

Finds the interface whose MAC matches LAPTOP_USB_MAC from .env.local and assigns
it the configured laptop IP (default 10.55.0.2/24) via ifconfig.

Usage:
    python3 scripts/assign_board.py        # uses .env.local values
    python3 scripts/assign_board.py --check  # check current state, don't change

Requires sudo for ifconfig.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys

import common


LAPTOP_IP   = "10.55.0.2"
LAPTOP_MASK = "24"


def _find_iface_by_mac(mac: str) -> str | None:
    """Return the interface name whose ether address matches *mac*."""
    mac = mac.lower()
    out = subprocess.check_output(["ifconfig"], text=True, stderr=subprocess.DEVNULL)

    current_iface = ""
    for line in out.splitlines():
        if not line[:1].isspace():
            current_iface = line.split(":")[0]
        elif re.search(rf"\bether\s+{re.escape(mac)}\b", line.strip()):
            return current_iface
    return None


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Assign laptop USB-Ethernet IP so the board is reachable after boot."
    )
    parser.add_argument("--check", action="store_true",
                        help="Only check the current state; don't assign anything.")
    parser.add_argument("--ip", default=LAPTOP_IP, metavar="IP",
                        help=f"IP to assign to the laptop interface (default: {LAPTOP_IP}).")
    parser.add_argument("--mask", default=LAPTOP_MASK, metavar="BITS",
                        help=f"Prefix length (default: {LAPTOP_MASK}).")
    args = parser.parse_args()

    mac = (common.os.environ.get("LAPTOP_USB_MAC") or "").strip()
    if not mac:
        print("ERROR: LAPTOP_USB_MAC not set — add it to .env.local.", file=sys.stderr)
        return 1

    iface = _find_iface_by_mac(mac)
    if not iface:
        print(f"ERROR: no interface found with MAC {mac}.", file=sys.stderr)
        print("       Is the USB cable plugged in and the board powered on?", file=sys.stderr)
        return 1

    print(f"Found interface: {iface} (MAC {mac})")

    # Check current IP on the interface.
    out = subprocess.check_output(["ifconfig", iface], text=True)
    m = re.search(r"\binet\s+(\d+\.\d+\.\d+\.\d+)\b", out)
    current_ip = m.group(1) if m else None

    if args.check:
        if current_ip:
            print(f"Current IP: {current_ip}")
        else:
            print("No IP assigned.")
        return 0

    if current_ip == args.ip:
        print(f"Already assigned {args.ip} — nothing to do.")
        print(f"SSH: ssh {common.BOARD_USER}@{common.BOARD_HOST}")
        return 0

    print(f"Assigning {args.ip}/{args.mask} to {iface} …")
    result = subprocess.run(
        ["sudo", "ifconfig", iface, f"{args.ip}/{args.mask}"],
    )
    if result.returncode != 0:
        print("ERROR: ifconfig failed.", file=sys.stderr)
        return result.returncode

    print(f"Done. SSH: ssh {common.BOARD_USER}@{common.BOARD_HOST}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
