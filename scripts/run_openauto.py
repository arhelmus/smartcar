#!/usr/bin/env python3
"""run_openauto.py — launch openauto, building it first if needed.

Usage:
    nix-shell --pure --run "python3 scripts/run_openauto.py"
"""

import subprocess
import sys

from build_openauto import AUTOAPP, OPENAUTO_DIR, OPENAUTO_TCP_PORT, build_openauto


def run_openauto() -> int:
    """Ensure the binary exists, free the port if needed, then launch."""
    if not AUTOAPP.exists():
        print("Binary not found — building …", file=sys.stderr)
        build_openauto()

    # Kill any process already holding the port.
    lsof = subprocess.run(
        ["lsof", "-ti", f"TCP:{OPENAUTO_TCP_PORT}", "-sTCP:LISTEN"],
        capture_output=True, text=True,
    )
    for pid in lsof.stdout.split():
        print(f"Killing PID {pid} on port {OPENAUTO_TCP_PORT} …", file=sys.stderr)
        subprocess.run(["kill", pid])

    print(f"Launching {AUTOAPP} …", file=sys.stderr)
    proc = subprocess.Popen([str(AUTOAPP)], cwd=str(OPENAUTO_DIR))
    try:
        proc.wait()
    except KeyboardInterrupt:
        print("\nInterrupted — stopping …", file=sys.stderr)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    return 0 if proc.returncode in (0, -15) else proc.returncode


if __name__ == "__main__":
    raise SystemExit(run_openauto())
