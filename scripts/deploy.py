#!/usr/bin/env python3
"""deploy.py — one-shot deploy: build, rsync, ansible, restart, healthcheck.

Pipeline:
  1. Cross-build smartcar-server (release by default) and rsync it +
     Flutter assets to the board.
  2. Apply the ansible playbook in board/ so any drift in unit files,
     gadget config, or system state is reconciled.
  3. Materialize or clear the transient runtime override at
     /run/smartcar-deploy/runtime.env, daemon-reload, then
     `systemctl restart smartcar-server.service`.
  4. Health-check: poll `systemctl show` until the unit is active +
     running with zero restarts, or fail with the last 40 journal lines.

Prerequisites the script does NOT take care of (they need sudo or hardware):
  - Laptop USB-Ethernet IP must be assigned. Run first:
        python3 scripts/assign_board.py
  - Board must be in CAR mode. The smartcar-server unit is gated by
    ConditionPathExists=/run/usb-car-mode; without it a restart silently
    no-ops. Set the jumper, or trigger a single car-mode boot with:
        ssh root@$BOARD_HOST 'touch /var/lib/smartcar/car-mode-once && reboot'

Usage:
    python3 scripts/deploy.py                                  # full deploy
    python3 scripts/deploy.py --check                          # ansible --check --diff, no restart
    python3 scripts/deploy.py --debug                          # debug build
    python3 scripts/deploy.py --skip-build                     # use binary already on board
    python3 scripts/deploy.py --runtime-args "--log debug"     # one-shot args, evaporate on reboot
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

# Pulls .env.local into os.environ + runs the init-stamp guard.
import common

REPO_ROOT = Path(__file__).resolve().parent.parent
BOARD = REPO_ROOT / "board"

REQUIRED_ENV = ("BOARD_HOST", "BOARD_USER", "BOARD_MAC", "BOARD_MAC_DEV")

# /run is tmpfs — the override evaporates on reboot, never persisted.
# Ansible templates the unit with EnvironmentFile=-<this path> (the leading
# '-' makes it optional, so the unit boots fine when no override is set).
RUNTIME_OVERRIDE_DIR  = "/run/smartcar-deploy"
RUNTIME_OVERRIDE_FILE = f"{RUNTIME_OVERRIDE_DIR}/runtime.env"

HEALTHCHECK_TIMEOUT_SEC  = 10
HEALTHCHECK_INTERVAL_SEC = 0.5


def _ssh(board: str, user: str, cmd: str, *, capture: bool = False) -> subprocess.CompletedProcess:
    args = ["ssh", "-o", "BatchMode=yes", f"{user}@{board}", cmd]
    if capture:
        return subprocess.run(args, capture_output=True, text=True)
    return subprocess.run(args)


def _preflight() -> int:
    missing = [k for k in REQUIRED_ENV if not os.environ.get(k, "").strip()]
    if missing:
        print(
            f"ERROR: missing {' '.join(missing)} in .env.local "
            "(see .env.local.example).",
            file=sys.stderr,
        )
        return 1
    if not shutil.which("ansible-playbook"):
        print("ERROR: ansible-playbook not found — run `make init`.", file=sys.stderr)
        return 1

    # Laptop interface — sudo to assign so it's its own script (kept separate).
    laptop_ip = common.laptop_usb_ip()
    if not laptop_ip:
        print(
            "ERROR: laptop USB-Ethernet interface has no IP — the board is not reachable.\n"
            "       Run first:  make assign   (or `sudo python3 scripts/assign_board.py`)",
            file=sys.stderr,
        )
        return 1
    return 0


def _run_ansible(check: bool) -> int:
    cmd = ["ansible-playbook", "site.yml"]
    if check:
        cmd += ["--check", "--diff"]
    print(f"  + {' '.join(cmd)} (cwd={BOARD})", file=sys.stderr)
    return subprocess.call(cmd, cwd=BOARD)


def _write_or_clear_runtime_override(board: str, user: str, runtime_args: str) -> int:
    """(Re)write or remove the transient EnvironmentFile on the board.

    A previous deploy might have left an override behind; running without
    `--runtime-args` means "use the unit as-is" so we always clear when no
    args are given, never inherit from a prior session.
    """
    if runtime_args:
        # Escape single quotes for embedding inside a single-quoted shell string.
        escaped = runtime_args.replace("'", "'\\''")
        remote_cmd = (
            f"mkdir -p {RUNTIME_OVERRIDE_DIR} && "
            f"printf 'DEPLOY_EXTRA_ARGS=%s\\n' '{escaped}' > {RUNTIME_OVERRIDE_FILE}"
        )
        print(f"  + transient runtime args: {runtime_args}", file=sys.stderr)
    else:
        remote_cmd = f"rm -f {RUNTIME_OVERRIDE_FILE}"
        print("  + clearing any prior transient runtime override", file=sys.stderr)
    return _ssh(board, user, remote_cmd).returncode


def _healthcheck(board: str, user: str) -> int:
    """Poll systemctl until active+running with zero restarts, else fail loud."""
    deadline = time.monotonic() + HEALTHCHECK_TIMEOUT_SEC
    last = {"ActiveState": "?", "SubState": "?", "NRestarts": "0", "Result": "?"}

    while time.monotonic() < deadline:
        result = _ssh(
            board, user,
            "systemctl show smartcar-server.service "
            "-p ActiveState -p SubState -p NRestarts -p Result",
            capture=True,
        )
        if result.returncode != 0:
            print(f"  ✗ systemctl show failed: {result.stderr.strip()}", file=sys.stderr)
            return result.returncode
        last = dict(line.split("=", 1) for line in result.stdout.strip().splitlines() if "=" in line)

        active   = last.get("ActiveState", "?")
        sub      = last.get("SubState", "?")
        try:
            nrestart = int(last.get("NRestarts", "0") or 0)
        except ValueError:
            nrestart = 0
        result_s = last.get("Result", "?")

        if active == "active" and sub == "running" and nrestart == 0:
            print(f"  ✓ healthy: ActiveState=active SubState=running NRestarts=0",
                  file=sys.stderr)
            return 0
        # Fast-path: terminal failure states; no point in waiting longer.
        if active == "failed" or result_s in {"core-dump", "exit-code", "signal", "oom-kill"}:
            break
        time.sleep(HEALTHCHECK_INTERVAL_SEC)

    print(
        f"\n  ✗ unhealthy after {HEALTHCHECK_TIMEOUT_SEC}s: "
        f"ActiveState={last.get('ActiveState')} SubState={last.get('SubState')} "
        f"NRestarts={last.get('NRestarts')} Result={last.get('Result')}",
        file=sys.stderr,
    )

    # Diagnose the most common "looks broken but isn't a crash" case first.
    cond = _ssh(
        board, user,
        "test -e /run/usb-car-mode && echo car || echo dev",
        capture=True,
    ).stdout.strip()
    if cond == "dev":
        print(
            "    Board is in DEV mode — smartcar-server.service is gated by\n"
            "    ConditionPathExists=/run/usb-car-mode and will not start.\n"
            "    Trigger a single car-mode boot with:\n"
            f"      ssh {user}@{board} 'touch /var/lib/smartcar/car-mode-once && reboot'\n"
            "    Or jumper pin 37→39 and power-cycle. See docs/board-setup.md.",
            file=sys.stderr,
        )
        return 1

    print("\n  Last 40 journal lines for smartcar-server:", file=sys.stderr)
    _ssh(board, user, "journalctl -u smartcar-server -n 40 --no-pager")
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Build, deploy, provision, restart, and health-check the board.",
    )
    parser.add_argument(
        "--check", action="store_true",
        help="ansible --check --diff; skip build, rsync, restart, healthcheck.",
    )
    parser.add_argument(
        "--skip-build", action="store_true",
        help="Skip cross-build + rsync — use the binary already on the board.",
    )
    build_mode = parser.add_mutually_exclusive_group()
    build_mode.add_argument("--debug",   dest="release", action="store_false",
                            help="Cross-compile a debug build.")
    build_mode.add_argument("--release", dest="release", action="store_true",
                            help="Cross-compile a release build (default).")
    parser.set_defaults(release=True)
    parser.add_argument(
        "--runtime-args", default="", metavar="STR",
        help='Extra args appended to ExecStart via /run/smartcar-deploy/runtime.env. '
             'Transient — evaporates on reboot, never persisted. Example: '
             '--runtime-args "--log debug --bridge tcp".',
    )
    args = parser.parse_args()

    rc = _preflight()
    if rc != 0:
        return rc

    board = common.BOARD_HOST
    user  = common.BOARD_USER

    # ── 1/4: build + rsync ───────────────────────────────────────────────────
    if args.check:
        print("\n[1/4] build + rsync … skipped (--check)", file=sys.stderr)
    elif args.skip_build:
        print("\n[1/4] build + rsync … skipped (--skip-build)", file=sys.stderr)
    else:
        print("\n[1/4] cross-build + rsync …", file=sys.stderr)
        common.stop_local_server()  # don't let the laptop server hold openauto
        rc = common.cross_build_and_deploy(board, user, args.release)
        if rc != 0:
            return rc

    # ── 2/4: ansible ────────────────────────────────────────────────────────
    print("\n[2/4] ansible-playbook …", file=sys.stderr)
    rc = _run_ansible(check=args.check)
    if rc != 0:
        return rc
    if args.check:
        print("\n[3/4] systemd restart … skipped (--check)", file=sys.stderr)
        print("[4/4] healthcheck … skipped (--check)", file=sys.stderr)
        return 0

    # ── 3/4: runtime override + daemon-reload + restart ──────────────────────
    print("\n[3/4] systemd reload + restart …", file=sys.stderr)
    rc = _write_or_clear_runtime_override(board, user, args.runtime_args)
    if rc != 0:
        return rc
    rc = _ssh(
        board, user,
        "systemctl daemon-reload && systemctl restart smartcar-server.service",
    ).returncode
    if rc != 0:
        return rc

    # ── 4/4: healthcheck ────────────────────────────────────────────────────
    print("\n[4/4] healthcheck …", file=sys.stderr)
    return _healthcheck(board, user)


if __name__ == "__main__":
    raise SystemExit(main())
