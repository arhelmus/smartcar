#!/usr/bin/env python3
"""run_server.py — build and run the smartcar-server Rust binary.

Usage:
    python3 scripts/run_server.py [--target HOST:PORT] [--release] [--log LEVEL]
"""

import argparse
import os
import subprocess
import sys

import common


def main() -> int:
    parser = argparse.ArgumentParser(description="Build and run smartcar-server.")
    parser.add_argument(
        "--target", default=common.DEFAULT_TARGET, metavar="HOST:PORT",
        help=f"Head-unit address (default: {common.DEFAULT_TARGET}).",
    )
    parser.add_argument("--release", action="store_true", help="Build in release mode.")
    parser.add_argument(
        "--log", default=common.DEFAULT_LOG, metavar="LEVEL",
        help=f"RUST_LOG level (default: {common.DEFAULT_LOG}).",
    )
    args = parser.parse_args()

    env = {**os.environ, "RUST_LOG": args.log}
    cmd = common.cargo_run_cmd(release=args.release, target=args.target)
    print(f"Starting smartcar-server: {' '.join(cmd)}", file=sys.stderr)

    try:
        subprocess.run(cmd, check=True, env=env, cwd=str(common.REPO_ROOT))
    except subprocess.CalledProcessError as exc:
        print(f"ERROR: server exited with code {exc.returncode}.", file=sys.stderr)
        return exc.returncode
    except KeyboardInterrupt:
        print("\nInterrupted — server stopped.", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
