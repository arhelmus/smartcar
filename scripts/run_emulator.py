#!/usr/bin/env python3
"""run_emulator.py — start the openauto emulator container via Docker Compose.

Usage:
    python3 scripts/run_emulator.py [--detach] [--no-build]

Options:
    -d, --detach   Run container in detached (background) mode.
    --no-build     Skip the --build flag (use cached image).
"""

import argparse
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
COMPOSE_FILE = REPO_ROOT / "docker" / "docker-compose.yml"


def build_command(detach: bool, no_build: bool) -> list[str]:
    """Construct the docker compose up command."""
    cmd = [
        "docker",
        "compose",
        "-f",
        str(COMPOSE_FILE),
        "up",
        "openauto",
    ]
    if not no_build:
        cmd.append("--build")
    if detach:
        cmd.append("--detach")
    return cmd


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Start the openauto emulator container.",
    )
    parser.add_argument(
        "-d",
        "--detach",
        action="store_true",
        help="Run in detached (background) mode.",
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="Skip rebuilding the Docker image.",
    )
    args = parser.parse_args()

    cmd = build_command(detach=args.detach, no_build=args.no_build)
    print(f"Starting openauto emulator: {' '.join(cmd)}", file=sys.stderr)

    try:
        subprocess.run(cmd, check=True, cwd=str(REPO_ROOT))
    except subprocess.CalledProcessError as exc:
        print(
            f"\nERROR: emulator failed (exit {exc.returncode}).",
            file=sys.stderr,
        )
        return 1
    except KeyboardInterrupt:
        print("\nInterrupted — emulator stopped.", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
