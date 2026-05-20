#!/usr/bin/env python3
"""run_server.py — build and run the smartcar-server Rust binary.

Usage:
    python3 scripts/run_server.py [--target HOST:PORT] [--release] [--log LEVEL] [--attached]

By default the server runs detached (build is shown in the terminal; the
binary runs in the background).  Pass --attached to keep it in the
foreground instead.
"""

import argparse
import os
import subprocess
import sys

import common


def _kill_previous() -> None:
    """Terminate any server instance recorded in the PID file."""
    common.stop_local_server()


def main() -> int:
    parser = argparse.ArgumentParser(description="Build and run smartcar-server.")
    parser.add_argument(
        "--target", default=common.DEFAULT_TARGET, metavar="HOST:PORT",
        help=f"Head-unit address (default: {common.DEFAULT_TARGET}).",
    )
    build_mode = parser.add_mutually_exclusive_group()
    build_mode.add_argument("--debug",   dest="release", action="store_false", help="Build in debug mode (default).")
    build_mode.add_argument("--release", dest="release", action="store_true",  help="Build in release mode.")
    parser.set_defaults(release=False)
    parser.add_argument(
        "--log", default=common.DEFAULT_LOG, metavar="LEVEL",
        help=f"RUST_LOG level (default: {common.DEFAULT_LOG}).",
    )
    parser.add_argument(
        "--attached", action="store_true",
        help="Run in the foreground instead of detaching.",
    )
    parser.add_argument(
        "--testkit", action="store_true",
        help="Use the synthetic testkit producers (drops the flutter feature "
             "and passes --testkit to the binary). Default builds with Flutter.",
    )
    args = parser.parse_args()
    flutter = not args.testkit

    env = {**os.environ, "RUST_LOG": args.log}

    _kill_previous()
    common.sweep_cargo_cache()

    if args.attached:
        cmd = common.cargo_run_cmd(release=args.release, target=args.target, flutter=flutter)
        print(f"Starting smartcar-server (attached): {' '.join(cmd)}", file=sys.stderr)
        try:
            subprocess.run(cmd, check=True, env=env, cwd=str(common.REPO_ROOT))
        except subprocess.CalledProcessError as exc:
            print(f"ERROR: server exited with code {exc.returncode}.", file=sys.stderr)
            return exc.returncode
        except KeyboardInterrupt:
            print("\nInterrupted — server stopped.", file=sys.stderr)
        return 0

    # --- detached mode ---
    # Build in the foreground so the user sees compile progress and errors.
    build_cmd = common.cargo_build_cmd(release=args.release, flutter=flutter)
    print(f"Building smartcar-server: {' '.join(build_cmd)}", file=sys.stderr)
    result = subprocess.run(build_cmd, cwd=str(common.REPO_ROOT))
    if result.returncode != 0:
        print(f"ERROR: build failed (exit {result.returncode}).", file=sys.stderr)
        return result.returncode

    binary = common.server_binary_path(release=args.release)
    run_cmd = [str(binary), "--target", args.target]
    if args.testkit:
        run_cmd.append("--testkit")
    print(f"Starting smartcar-server (detached): {' '.join(run_cmd)}", file=sys.stderr)
    print(f"Logs: {common.SERVER_LOG_FILE}", file=sys.stderr)

    log_fh = common.SERVER_LOG_FILE.open("w")
    proc = subprocess.Popen(
        run_cmd, env=env, cwd=str(common.REPO_ROOT),
        stdout=log_fh, stderr=log_fh,
        start_new_session=True,
    )
    common.SERVER_PID_FILE.write_text(str(proc.pid))
    print(f"Server started (pid {proc.pid}).", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
