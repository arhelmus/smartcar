#!/usr/bin/env python3
"""run_board.py — build, deploy, and run smartcar-server on the ARM board.

Usage:
    python3 scripts/run_board.py [options]

Options:
    --laptop-ip IP    Laptop's USB-Ethernet IP (auto-detected from LAPTOP_USB_MAC if omitted).
    --board HOST      Board SSH address (default: BOARD_HOST in .env.local or 10.55.0.1).
    --user USER       SSH user (default: BOARD_USER in .env.local or root).
    --log LEVEL       RUST_LOG level (default: info).
    --debug           Cross-compile a debug build.
    --release         Cross-compile a release build (default).
    --attached        Keep SSH open; Ctrl-C stops the server.
                      Default: detached (server keeps running after SSH exits).

Prerequisites on the laptop:
    cargo install cross   # Rust cross-compilation tool
    Docker running        # cross uses Docker for the sysroot

Prerequisites on the board (Armbian):
    apt-get install -y libssl3

Environment / .env.local:
    BOARD_HOST       Board SSH address
    BOARD_USER       Board SSH user
    LAPTOP_USB_MAC   Laptop-side USB Ethernet MAC for auto IP detection
    LAPTOP_USB_IP    Explicit laptop IP (skips MAC lookup)
"""

import argparse
import subprocess
import sys

import common


def _ssh(board: str, user: str, cmd: str, *, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(["ssh", "-o", "BatchMode=yes", f"{user}@{board}", cmd], check=check)


def main() -> int:
    parser = argparse.ArgumentParser(description="Build, deploy, and run smartcar-server on the ARM board.")
    parser.add_argument("--board", default=common.BOARD_HOST, metavar="HOST",
                        help=f"Board SSH host (default: {common.BOARD_HOST}).")
    parser.add_argument("--user", default=common.BOARD_USER, metavar="USER",
                        help=f"Board SSH user (default: {common.BOARD_USER}).")
    parser.add_argument("--laptop-ip", default=None, metavar="IP",
                        help="Laptop USB-Ethernet IP. Auto-detected from LAPTOP_USB_MAC when omitted.")
    parser.add_argument("--log", default=common.DEFAULT_LOG, metavar="LEVEL",
                        help=f"RUST_LOG level (default: {common.DEFAULT_LOG}).")
    parser.add_argument("--attached", action="store_true",
                        help="Keep SSH open; server stops when SSH session ends.")
    build_mode = parser.add_mutually_exclusive_group()
    build_mode.add_argument("--debug",   dest="release", action="store_false", help="Debug build.")
    build_mode.add_argument("--release", dest="release", action="store_true",  help="Release build (default).")
    parser.set_defaults(release=True)
    args = parser.parse_args()

    # Bring up the laptop side of the USB-Ethernet link before anything else,
    # so the board is reachable and IP auto-detection below succeeds.
    rc = common.assign_board_ip()
    if rc != 0:
        return rc

    laptop_ip = args.laptop_ip or common.laptop_usb_ip()
    if not laptop_ip:
        print("ERROR: could not determine laptop USB-Ethernet IP.", file=sys.stderr)
        print("       Set LAPTOP_USB_MAC in .env.local, or pass --laptop-ip <ip>.", file=sys.stderr)
        return 1
    if not args.laptop_ip:
        print(f"Auto-detected laptop USB-Ethernet IP: {laptop_ip}", file=sys.stderr)

    # The board takes over the openauto connection — make sure a local
    # smartcar-server (started by run_server.py) isn't holding it.
    common.stop_local_server()

    rc = common.cross_build_and_deploy(args.board, args.user, args.release)
    if rc != 0:
        return rc

    # ── Run ──────────────────────────────────────────────────────────────────
    target = f"{laptop_ip}:5278"

    # On the board the iOS-app bridge runs over BLE (the dev-default TCP makes
    # sense only on the Mac where the Simulator lives).
    bridge_flag = "--bridge ble"

    if args.attached:
        ssh_cmd = (
            f"RUST_LOG={args.log} {common.BOARD_SERVER_BINARY} "
            f"{bridge_flag} --target {target}"
        )
        print(f"Starting smartcar-server → openauto at {target} (attached) …", file=sys.stderr)
        try:
            _ssh(args.board, args.user, ssh_cmd)
        except KeyboardInterrupt:
            print("\nInterrupted.", file=sys.stderr)
        return 0

    # Detached: stop any previous instance, start under nohup.
    kill_cmd = (
        f"if [ -f {common.BOARD_PID_FILE} ]; then"
        f"  pid=$(cat {common.BOARD_PID_FILE});"
        f"  kill \"$pid\" 2>/dev/null;"
        f"  rm -f {common.BOARD_PID_FILE};"
        f"  echo \"Stopped previous server (pid $pid)\";"
        f"fi"
    )
    _ssh(args.board, args.user, kill_cmd, check=False)

    start_cmd = (
        f"nohup env RUST_LOG={args.log}"
        f" {common.BOARD_SERVER_BINARY} {bridge_flag} --target {target}"
        f" > {common.BOARD_LOG_FILE} 2>&1 &"
        f" echo $! > {common.BOARD_PID_FILE} &&"
        f" echo \"Started pid $(cat {common.BOARD_PID_FILE})\""
    )
    print(f"Starting smartcar-server → openauto at {target} (detached) …", file=sys.stderr)
    _ssh(args.board, args.user, start_cmd)

    print(f"\nServer running on {args.board}.", file=sys.stderr)
    print(f"  Logs:  ssh {args.user}@{args.board} 'tail -f {common.BOARD_LOG_FILE}'", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
