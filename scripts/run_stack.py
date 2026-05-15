#!/usr/bin/env python3
"""run_stack.py — start the full smartcar stack (emulator + server).

Launches the openauto emulator in detached mode, waits until it is running,
then starts the smartcar-server.  On Ctrl-C or server exit the emulator is
stopped via 'docker compose down'.

Usage:
    python3 scripts/run_stack.py [--target HOST:PORT] [--release] \
                                  [--log LEVEL] [--timeout SECONDS]

Options:
    --target HOST:PORT   Address for the server (default: 127.0.0.1:5277).
    --release            Build the server in release mode.
    --log LEVEL          RUST_LOG level (default: info).
    --timeout SECONDS    Seconds to wait for the emulator to be ready (default: 30).
"""

import argparse
import os
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
COMPOSE_FILE = REPO_ROOT / "docker" / "docker-compose.yml"
MANIFEST = REPO_ROOT / "server" / "Cargo.toml"


# ---------------------------------------------------------------------------
# Emulator helpers
# ---------------------------------------------------------------------------

def _start_emulator() -> None:
    """Start the openauto container in detached mode."""
    cmd = [
        "docker",
        "compose",
        "-f",
        str(COMPOSE_FILE),
        "up",
        "openauto",
        "--build",
        "--detach",
    ]
    print(f"Starting emulator (detached): {' '.join(cmd)}", file=sys.stderr)
    try:
        subprocess.run(cmd, check=True, cwd=str(REPO_ROOT))
    except subprocess.CalledProcessError as exc:
        print(
            f"\nERROR: Failed to start emulator (exit {exc.returncode}).",
            file=sys.stderr,
        )
        raise SystemExit(1) from exc


def _emulator_is_running() -> bool:
    """Return True when the openauto service reports as 'running'."""
    result = subprocess.run(
        [
            "docker",
            "compose",
            "-f",
            str(COMPOSE_FILE),
            "ps",
            "--format",
            "{{.Service}} {{.State}}",
        ],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )
    for line in result.stdout.splitlines():
        parts = line.strip().split()
        if len(parts) >= 2 and parts[0] == "openauto" and parts[1] == "running":
            return True
    return False


def _wait_for_emulator(timeout: int) -> bool:
    """Poll until the emulator is running or timeout (seconds) elapses."""
    print(
        f"Waiting for openauto to be ready (timeout {timeout}s) …",
        file=sys.stderr,
    )
    deadline = time.monotonic() + timeout
    poll_interval = 2.0
    while time.monotonic() < deadline:
        if _emulator_is_running():
            print("  openauto is running.", file=sys.stderr)
            return True
        time.sleep(poll_interval)
    return False


def _stop_emulator() -> None:
    """Stop the emulator with 'docker compose down'."""
    cmd = [
        "docker",
        "compose",
        "-f",
        str(COMPOSE_FILE),
        "down",
    ]
    print(f"\nStopping emulator: {' '.join(cmd)}", file=sys.stderr)
    subprocess.run(cmd, cwd=str(REPO_ROOT))


# ---------------------------------------------------------------------------
# Server helpers
# ---------------------------------------------------------------------------

def _build_server_command(target: str, release: bool) -> list[str]:
    cmd = [
        "cargo",
        "run",
        "--manifest-path",
        str(MANIFEST),
        "--bin",
        "smartcar-server",
    ]
    if release:
        cmd.append("--release")
    cmd += ["--", "--target", target]
    return cmd


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(
        description="Start the full smartcar stack (emulator + server).",
    )
    parser.add_argument(
        "--target",
        default="127.0.0.1:5277",
        metavar="HOST:PORT",
        help="Address for the server (default: 127.0.0.1:5277).",
    )
    parser.add_argument(
        "--release",
        action="store_true",
        help="Build the server in release mode.",
    )
    parser.add_argument(
        "--log",
        default="info",
        metavar="LEVEL",
        help="RUST_LOG level (default: info).",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=30,
        metavar="SECONDS",
        help="Seconds to wait for the emulator to become ready (default: 30).",
    )
    args = parser.parse_args()

    # 1. Start emulator in detached mode.
    _start_emulator()

    # 2. Wait for the emulator to be ready.
    if not _wait_for_emulator(args.timeout):
        print(
            f"\nERROR: openauto did not become ready within {args.timeout}s.",
            file=sys.stderr,
        )
        _stop_emulator()
        return 1

    # 3. Start the server.
    env = os.environ.copy()
    env["RUST_LOG"] = args.log

    server_cmd = _build_server_command(target=args.target, release=args.release)
    print(f"Starting server: {' '.join(server_cmd)}", file=sys.stderr)
    print(f"  RUST_LOG={args.log}", file=sys.stderr)

    server_proc: subprocess.Popen | None = None
    exit_code = 0
    try:
        server_proc = subprocess.Popen(
            server_cmd,
            env=env,
            cwd=str(REPO_ROOT),
        )
        server_proc.wait()
        if server_proc.returncode != 0:
            print(
                f"\nServer exited with code {server_proc.returncode}.",
                file=sys.stderr,
            )
            exit_code = 1
    except KeyboardInterrupt:
        print("\nInterrupted — shutting down …", file=sys.stderr)
        if server_proc is not None:
            server_proc.terminate()
            try:
                server_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server_proc.kill()
    finally:
        # 4. Always stop the emulator on exit.
        _stop_emulator()

    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
