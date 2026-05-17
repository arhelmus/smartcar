#!/usr/bin/env python3
"""deploy_board.py — cross-compile smartcar-server for ARM and deploy to board.

Usage:
    python3 scripts/deploy_board.py [--board HOST] [--user USER] [--debug|--release]

Prerequisites on the laptop:
    cargo install cross   # Rust cross-compilation tool
    Docker running        # cross uses Docker for the sysroot

Prerequisites on the board (Armbian):
    apt-get install -y libssl3

Environment variables (override CLI defaults):
    BOARD_HOST   Board SSH address (default: 10.55.0.1)
    BOARD_USER   Board SSH user    (default: root)
"""

import argparse
import sys

import common


def main() -> int:
    parser = argparse.ArgumentParser(description="Cross-compile and deploy smartcar-server to the ARM board.")
    parser.add_argument(
        "--board", default=common.BOARD_HOST, metavar="HOST",
        help=f"Board SSH host (default: {common.BOARD_HOST}).",
    )
    parser.add_argument(
        "--user", default=common.BOARD_USER, metavar="USER",
        help=f"Board SSH user (default: {common.BOARD_USER}).",
    )
    build_mode = parser.add_mutually_exclusive_group()
    build_mode.add_argument("--debug",   dest="release", action="store_false", help="Build debug.")
    build_mode.add_argument("--release", dest="release", action="store_true",  help="Build release (default).")
    parser.set_defaults(release=True)
    args = parser.parse_args()

    rc = common.cross_build_and_deploy(args.board, args.user, args.release)
    if rc == 0:
        print(f"\nDeployed. Run with:  python3 scripts/run_board.py", file=sys.stderr)
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
