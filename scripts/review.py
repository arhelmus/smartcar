#!/usr/bin/env python3
"""review.py — run all project checks in parallel.

Wired in two places:
  - `.githooks/pre-push`  → runs on every `git push`
  - `make review`         → manual invocation

Checks:
  - cargo fmt / clippy / test / audit             (host)
  - cross check (x86_64-unknown-linux-gnu)        (non-Linux hosts only — exercises
                                                   the Linux-only dep tree, e.g. bluer
                                                   + libdbus-sys, that macOS skips
                                                   via cfg(target_os = "linux"))
  - flutter pub get --enforce-lockfile / dart format /
    flutter analyze / flutter test                (mobile/, server/flutter-ui/)

Each check runs in its own thread; the actual work is a subprocess so the GIL
is irrelevant. Per-check stdout+stderr go to a temp log; only the logs of
failed checks are printed at the end, with an actionable hint where useful.

Usage:
    python3 scripts/review.py           # run everything
    python3 scripts/review.py --no-cross  # skip the slow Linux cross-check
"""

from __future__ import annotations

import argparse
import concurrent.futures
import dataclasses
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable

# Pull in .env.local (CARGO_TARGET_DIR especially — keeps the cross artifacts
# in the shared dir across checkouts) and trigger the init-stamp guard.
import common  # noqa: F401

REPO_ROOT = Path(__file__).resolve().parent.parent
SERVER = REPO_ROOT / "server"
MOBILE = REPO_ROOT / "mobile"
EMBED = SERVER / "flutter-ui"
BOARD = REPO_ROOT / "board"


@dataclasses.dataclass
class Check:
    """One unit of work in the review run."""

    name: str
    run: Callable[[Path], int]  # takes a log path, returns exit code
    hint: str = ""


# ── Check runners ─────────────────────────────────────────────────────────────


def shell(cmd: str, cwd: Path, env: dict[str, str] | None = None) -> Callable[[Path], int]:
    """A Check.run that executes one shell command, merging stdout+stderr into the log."""

    def runner(log_path: Path) -> int:
        merged_env = os.environ.copy()
        if env:
            merged_env.update(env)
        with log_path.open("w") as log:
            log.write(f"$ {cmd}\n")
            log.flush()
            return subprocess.call(
                cmd,
                shell=True,
                cwd=cwd,
                stdout=log,
                stderr=subprocess.STDOUT,
                env=merged_env,
            )

    return runner


def sequence(cmds: list[str], cwd: Path) -> Callable[[Path], int]:
    """A Check.run that runs commands in order, stopping at the first failure."""

    def runner(log_path: Path) -> int:
        with log_path.open("w") as log:
            for cmd in cmds:
                log.write(f"$ {cmd}\n")
                log.flush()
                rc = subprocess.call(
                    cmd, shell=True, cwd=cwd, stdout=log, stderr=subprocess.STDOUT
                )
                if rc != 0:
                    return rc
        return 0

    return runner


# ── Check catalog ─────────────────────────────────────────────────────────────


def build_checks(*, skip_cross: bool) -> list[Check]:
    cargo_manifest = f"--manifest-path {SERVER}/Cargo.toml"
    checks: list[Check] = [
        Check(
            "cargo fmt",
            shell(f"cargo fmt --all --check {cargo_manifest}", REPO_ROOT),
            hint=f"cargo fmt --all {cargo_manifest}",
        ),
        Check(
            "cargo clippy",
            shell(
                f"cargo clippy --all-targets --all-features {cargo_manifest} -- -D warnings",
                REPO_ROOT,
            ),
        ),
        Check("cargo test", shell(f"cargo test {cargo_manifest}", REPO_ROOT)),
        Check("cargo audit", shell(f"cargo audit --file {SERVER}/Cargo.lock", REPO_ROOT)),
    ]

    # Linux dep tree on non-Linux hosts (slow; needs Docker).
    if platform.system() != "Linux" and not skip_cross:
        checks.append(
            Check(
                "cross check (x86_64-linux-gnu)",
                shell(
                    "cross check -p smartcar-server --features openssl-vendored "
                    "--target x86_64-unknown-linux-gnu",
                    SERVER,
                    env={"DOCKER_DEFAULT_PLATFORM": "linux/amd64"},
                ),
                hint="is Docker running? Pass --no-cross to skip locally.",
            )
        )

    # Flutter projects — sequenced internally, parallel across each other.
    flutter_steps = [
        "flutter pub get --enforce-lockfile",
        "dart format --output=none --set-exit-if-changed .",
        "flutter analyze",
        "flutter test",
    ]
    for label, path in (("flutter (mobile)", MOBILE), ("flutter (flutter-ui)", EMBED)):
        checks.append(Check(label, sequence(flutter_steps, path)))

    # Ansible — syntax-check is cheap, lint catches FQCN/var/name drift.
    # Auto-skips when ansible-playbook is missing (same shape as cross-check on
    # Linux); install via brew/apt — see init.py _check_ansible.
    if shutil.which("ansible-playbook"):
        ansible_cmds = ["ansible-playbook site.yml --syntax-check"]
        if shutil.which("ansible-lint"):
            ansible_cmds.append("ansible-lint")
        checks.append(
            Check(
                "ansible (board)",
                sequence(ansible_cmds, BOARD),
                hint="brew install ansible ansible-lint  (or re-run `make init`)",
            )
        )

    return checks


# ── Driver ────────────────────────────────────────────────────────────────────


def run_one(check: Check) -> tuple[Check, int, Path]:
    fd, path_str = tempfile.mkstemp(prefix="review-", suffix=".log")
    os.close(fd)
    log = Path(path_str)
    rc = check.run(log)
    return check, rc, log


def main() -> int:
    parser = argparse.ArgumentParser(description="Run project checks in parallel.")
    parser.add_argument(
        "--no-cross",
        action="store_true",
        help="Skip the cross-compile Linux check (slow; needs Docker).",
    )
    args = parser.parse_args()

    checks = build_checks(skip_cross=args.no_cross)
    print(f"review: running {len(checks)} checks in parallel…", flush=True)

    failed: list[tuple[Check, int, Path]] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(checks)) as pool:
        futures = [pool.submit(run_one, c) for c in checks]
        for f in concurrent.futures.as_completed(futures):
            check, rc, log = f.result()
            status = "OK  " if rc == 0 else "FAIL"
            print(f"  [{status}] {check.name}", flush=True)
            if rc == 0:
                log.unlink(missing_ok=True)
            else:
                failed.append((check, rc, log))

    if failed:
        print("\nreview: failures —")
        for check, rc, log in failed:
            print(f"\n── {check.name} (exit {rc}) ──")
            if check.hint:
                print(f"hint: {check.hint}")
            try:
                print(log.read_text())
            finally:
                log.unlink(missing_ok=True)
        return 1

    print("\nreview: all checks passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
