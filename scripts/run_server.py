#!/usr/bin/env python3
"""run_server.py — build and run the smartcar-server Rust binary via Cargo.

Usage:
    python3 scripts/run_server.py [--target HOST:PORT] [--release] [--log LEVEL]

Options:
    --target HOST:PORT   Address to listen on (default: 127.0.0.1:5277).
    --release            Build in release mode.
    --log LEVEL          Set RUST_LOG env var (default: info).
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST = REPO_ROOT / "server" / "Cargo.toml"


def build_command(target: str, release: bool) -> list[str]:
    """Construct the cargo run command."""
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


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Build and run the smartcar-server binary.",
    )
    parser.add_argument(
        "--target",
        default="127.0.0.1:5277",
        metavar="HOST:PORT",
        help="Address for the server to listen on (default: 127.0.0.1:5277).",
    )
    parser.add_argument(
        "--release",
        action="store_true",
        help="Build in release mode.",
    )
    parser.add_argument(
        "--log",
        default="info",
        metavar="LEVEL",
        help="RUST_LOG level (default: info).",
    )
    args = parser.parse_args()

    env = os.environ.copy()
    env["RUST_LOG"] = args.log

    cmd = build_command(target=args.target, release=args.release)
    print(f"Starting smartcar-server: {' '.join(cmd)}", file=sys.stderr)
    print(f"  RUST_LOG={args.log}", file=sys.stderr)

    try:
        subprocess.run(cmd, check=True, env=env, cwd=str(REPO_ROOT))
    except subprocess.CalledProcessError as exc:
        print(
            f"\nERROR: server exited with code {exc.returncode}.",
            file=sys.stderr,
        )
        return 1
    except KeyboardInterrupt:
        print("\nInterrupted — server stopped.", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
