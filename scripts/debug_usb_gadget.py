#!/usr/bin/env python3
"""debug_usb_gadget.py — full no-hands iteration loop for USB-gadget debugging.

Drops the one-shot trigger /var/lib/smartcar/car-mode-once on the board and
reboots into car mode.  The board-side `usb-mode-select.sh` consumes the
trigger before committing to car mode AND schedules a transient
systemd-run timer to reboot 30 s later, which brings the board back to dev
mode automatically.  This script:

  1. checks that the laptop USB-Ethernet IP is assigned (so the board is reachable)
  2. triggers the car-mode reboot
  3. polls SSH until the board returns in dev mode (after the auto-revert)
  4. dumps the previous boot's smartcar-server journal so this iteration's
     checkpoints (target=flight) land directly in the terminal

No jumper, no power-cycle, no walk to the car.

Usage:
    python3 scripts/debug_usb_gadget.py [--board HOST] [--user USER] [--timeout SECONDS]
"""

import argparse
import subprocess
import sys
import time
from typing import Optional

import common

# Sane default for the full cycle: 30 s car-mode window + boot back +
# slack. Tune up if the board boots slowly or you raise the auto-revert
# delay in /usr/local/sbin/usb-mode-select.sh.
DEFAULT_TIMEOUT_S = 120


def ssh(board: str, user: str, cmd: str, *, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5",
         f"{user}@{board}", cmd],
        check=check, capture_output=True, text=True,
    )


def read_boot_id(board: str, user: str) -> Optional[str]:
    """Return the board's current kernel boot_id, or None if unreachable."""
    try:
        r = ssh(board, user, "cat /proc/sys/kernel/random/boot_id", check=True)
        return r.stdout.strip() or None
    except subprocess.CalledProcessError:
        return None


def wait_for_new_boot(board: str, user: str, prev_boot_id: str, timeout_s: int) -> bool:
    """Poll until SSH responds AND the kernel boot_id differs from `prev_boot_id`.

    Naively waiting for SSH-up is wrong: when we trigger a reboot the shell
    returns instantly (the reboot is scheduled via `sleep 1`), so SSH is
    still alive for a moment — a plain "is ssh up?" check returns True
    immediately. Comparing boot_id is the only reliable transition signal.
    """
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        current = read_boot_id(board, user)
        if current and current != prev_boot_id:
            return True
        time.sleep(3)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Trigger a one-shot car-mode boot, wait for auto-revert, dump the flight log."
    )
    parser.add_argument("--board", default=common.BOARD_HOST, metavar="HOST",
                        help=f"Board SSH host (default: {common.BOARD_HOST}).")
    parser.add_argument("--user", default=common.BOARD_USER, metavar="USER",
                        help=f"Board SSH user (default: {common.BOARD_USER}).")
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT_S,
                        metavar="SECONDS",
                        help=f"Max seconds to wait for the board to return (default: {DEFAULT_TIMEOUT_S}).")
    args = parser.parse_args()

    # 1. Preflight USB-Ethernet so the board is reachable. assign_board_ip
    # needs sudo; deploy/debug scripts don't run it, the operator does once
    # per session via `make assign`.
    if not common.laptop_usb_ip():
        print(
            "ERROR: laptop USB-Ethernet interface has no IP — the board is not reachable.\n"
            "       Run first:  make assign   (or `sudo python3 scripts/assign_board.py`)",
            file=sys.stderr,
        )
        return 1

    # 2. Snapshot the current boot_id so we can detect the reboot.
    prev_boot_id = read_boot_id(args.board, args.user)
    if not prev_boot_id:
        print("ERROR: could not reach board (read /proc/sys/kernel/random/boot_id failed).",
              file=sys.stderr)
        return 1

    # 3. Drop the one-shot car-mode trigger. Journal persistence + boot
    # indexing handle per-iteration isolation — we read journalctl -b -1
    # after the auto-revert, so we naturally see only this iteration's
    # smartcar-server output without having to clear anything first.
    prep_cmds = [
        "mkdir -p /var/lib/smartcar",
        "touch /var/lib/smartcar/car-mode-once",
        "sync",
        "echo prep-ok",
    ]
    print(f"Preparing board (prev boot_id={prev_boot_id[:8]}…) …", file=sys.stderr)
    try:
        r = ssh(args.board, args.user, " && ".join(prep_cmds), check=True)
        if r.stdout.strip():
            print(r.stdout.strip(), file=sys.stderr)
    except subprocess.CalledProcessError as e:
        print(f"ERROR: prep SSH failed: {e.stderr}", file=sys.stderr)
        return e.returncode

    # 4. Trigger reboot as a separate, fire-and-forget SSH call. The
    # `(sleep 1; reboot) &` form returns control to our SSH session
    # immediately, then disconnects when reboot actually fires.
    print("Triggering reboot …", file=sys.stderr)
    ssh(args.board, args.user, "(sleep 1; reboot) &", check=False)

    # 5. Wait for the new boot_id to appear. Timing in practice:
    #     0..~5s   reboot, kernel comes up
    #     ~5..~35s car-mode boot, smartcar-server attempts
    #     ~35s     systemd-run timer fires → reboot
    #     ~35..~50s second reboot, dev mode, SSH ready, NEW boot_id
    print(f"Waiting up to {args.timeout}s for the auto-revert reboot …", file=sys.stderr)
    start = time.monotonic()
    if not wait_for_new_boot(args.board, args.user, prev_boot_id, args.timeout):
        elapsed = int(time.monotonic() - start)
        print(f"ERROR: board didn't return within {args.timeout}s "
              f"(waited {elapsed}s). Check it manually.", file=sys.stderr)
        return 2
    elapsed = int(time.monotonic() - start)
    print(f"New boot detected after {elapsed}s — board is back in dev mode.",
          file=sys.stderr)

    # 4. Dump the previous-boot smartcar-server journal so the iteration's
    # evidence is right here in the terminal. -b -1 = the boot just before
    # the auto-revert (the car-mode attempt). The flight-target checkpoints
    # (tracing target=flight, emitted at info) read as a clean timeline.
    print("\n=== journalctl -u smartcar-server -b -1 (car-mode attempt) ===\n",
          file=sys.stderr)
    try:
        r = ssh(args.board, args.user,
                "journalctl -u smartcar-server -b -1 --no-pager", check=True)
        sys.stdout.write(r.stdout)
        sys.stdout.flush()
    except subprocess.CalledProcessError as e:
        print(f"WARN: could not read previous-boot journal: {e.stderr}", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
