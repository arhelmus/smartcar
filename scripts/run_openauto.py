#!/usr/bin/env python3
"""run_openauto.py — launch openauto, building it first if needed.

Usage:
    python3 scripts/run_openauto.py             # detached (default)
    python3 scripts/run_openauto.py --attached  # keep terminal attached
"""

import argparse
import glob
import os
import re
import shutil
import signal
import subprocess
import sys
from pathlib import Path

import common
from build_openauto import AUTOAPP, INSTALL_PREFIX, OPENAUTO_DIR, OPENAUTO_TCP_PORT, build_openauto

PID_FILE = common.REPO_ROOT / ".build" / "openauto.pid"
RUNTIME_ENV = INSTALL_PREFIX.parent / "runtime.env"


def _detect_qt_plugin_path() -> str:
    """Return extra QT_PLUGIN_PATH entries so Qt can find its multimedia plugins.

    On Nix, each package output gets its own store hash, so the qtmultimedia
    library and its plugins live under different hashes.  We scan the Nix store
    directly for plugin trees that contain the mediaservice plugins.
    """
    dirs: list[str] = []
    for plugin_dir in glob.glob("/nix/store/*-qtmultimedia-*/lib/qt-*/plugins"):
        if Path(plugin_dir, "mediaservice").exists():
            dirs.append(plugin_dir)
    return ":".join(dirs)


def _detect_gst_plugin_path() -> str:
    """Return GST_PLUGIN_PATH covering all Nix-built GStreamer plugin packages.

    The Nix-built Qt GStreamer multimedia backend (libgstmediaplayer.dylib) links
    against Nix GStreamer, so GStreamer must find its plugin registry in the same
    Nix store.  Without this, GstEngine::available() returns false and Qt falls
    back to the AVFoundation backend, which cannot decode from a QIODevice stream.
    """
    dirs: list[str] = []
    for gst_dir in glob.glob("/nix/store/*/lib/gstreamer-1.0"):
        if any(Path(gst_dir).glob("libgst*.dylib")):
            dirs.append(gst_dir)
    return ":".join(sorted(dirs))


def _runtime_env() -> dict:
    env = os.environ.copy()
    if RUNTIME_ENV.exists():
        for line in RUNTIME_ENV.read_text().splitlines():
            if "=" in line:
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip()

    extra_qt_plugins = _detect_qt_plugin_path()
    if extra_qt_plugins:
        existing = env.get("QT_PLUGIN_PATH", "")
        env["QT_PLUGIN_PATH"] = (existing + ":" + extra_qt_plugins).strip(":")

    gst_plugin_path = _detect_gst_plugin_path()
    if gst_plugin_path:
        existing = env.get("GST_PLUGIN_PATH", "")
        env["GST_PLUGIN_PATH"] = (existing + ":" + gst_plugin_path).strip(":")
        print(f"GST_PLUGIN_PATH set ({len(gst_plugin_path.split(':'))} dirs)", file=sys.stderr)

    env["QT_DEBUG_PLUGINS"] = "1"

    return env


def _kill_previous() -> None:
    if PID_FILE.exists():
        try:
            pid = int(PID_FILE.read_text().strip())
            os.kill(pid, signal.SIGTERM)
            print(f"Stopped previous openauto (PID {pid}).", file=sys.stderr)
        except (ProcessLookupError, ValueError):
            pass
        PID_FILE.unlink(missing_ok=True)

    # Best-effort: free the port in case a stale process holds it.
    lsof_bin = shutil.which("lsof")
    if lsof_bin:
        lsof = subprocess.run(
            [lsof_bin, "-ti", f"TCP:{OPENAUTO_TCP_PORT}", "-sTCP:LISTEN"],
            capture_output=True, text=True,
        )
        for pid in lsof.stdout.split():
            print(f"Killing PID {pid} on port {OPENAUTO_TCP_PORT} …", file=sys.stderr)
            subprocess.run(["kill", pid])


def run_openauto(attached: bool = False, clean: bool = False) -> int:
    if clean or not AUTOAPP.exists():
        build_openauto(rebuild=clean)

    _kill_previous()

    PID_FILE.parent.mkdir(parents=True, exist_ok=True)
    print(f"Launching {AUTOAPP} …", file=sys.stderr)

    env = _runtime_env()

    if attached:
        proc = subprocess.Popen([str(AUTOAPP)], cwd=str(OPENAUTO_DIR), env=env)
        PID_FILE.write_text(str(proc.pid))
        try:
            proc.wait()
        except KeyboardInterrupt:
            print("\nInterrupted — stopping …", file=sys.stderr)
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        finally:
            PID_FILE.unlink(missing_ok=True)
        return 0 if proc.returncode in (0, -15) else proc.returncode
    else:
        proc = subprocess.Popen(
            [str(AUTOAPP)],
            cwd=str(OPENAUTO_DIR),
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        PID_FILE.write_text(str(proc.pid))
        print(f"openauto running in background (PID {proc.pid}).", file=sys.stderr)
        return 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Launch openauto.")
    parser.add_argument("--attached", action="store_true", help="Keep terminal attached to the process.")
    parser.add_argument("--clean", action="store_true", help="Force a clean rebuild before launching.")
    args = parser.parse_args()
    raise SystemExit(run_openauto(attached=args.attached, clean=args.clean))
