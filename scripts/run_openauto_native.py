#!/usr/bin/env python3
"""run_openauto_native.py — build and/or launch openauto as a native macOS binary.

Must be run inside the repo's Nix shell:

    nix-shell --pure --run "python3 scripts/run_openauto_native.py [--rebuild]"

Or enter the shell first:

    nix-shell --pure
    python3 scripts/run_openauto_native.py [--rebuild]
"""

import argparse
import multiprocessing
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import List, Optional

import common

# ── Paths ──────────────────────────────────────────────────────────────────────

THIRD_PARTY    = common.REPO_ROOT / "server" / "third_party"
AASDK_DIR      = THIRD_PARTY / "aasdk"
OPENAUTO_DIR   = THIRD_PARTY / "openauto"
AASDK_BUILD    = AASDK_DIR    / "build-macos"
OPENAUTO_BUILD = OPENAUTO_DIR / "build-macos"
INSTALL_PREFIX = common.REPO_ROOT / ".build" / "native"
AUTOAPP        = OPENAUTO_BUILD / "bin" / "autoapp"
PATCHES_DIR    = common.REPO_ROOT / "scripts" / "patches" / "openauto"

OPENAUTO_TCP_PORT = 5278

# ── Internal helpers ───────────────────────────────────────────────────────────

def _run(cmd: List[str], cwd: Optional[str] = None) -> None:
    print(f"+ {' '.join(cmd)}", file=sys.stderr)
    subprocess.run(cmd, check=True, cwd=cwd or str(common.REPO_ROOT))


def _cmake_prefix_path() -> str:
    nix_paths = os.environ.get("CMAKE_PREFIX_PATH", "")
    parts = [str(INSTALL_PREFIX)]
    if nix_paths:
        parts.append(nix_paths)
    return ";".join(parts)


def _check_nix() -> bool:
    if not os.environ.get("IN_NIX_SHELL"):
        print(
            "ERROR: not running inside a Nix shell.\n"
            "Enter the shell first:\n"
            "  nix-shell --pure\n"
            "or run directly:\n"
            "  nix-shell --pure --run 'python3 scripts/run_openauto_native.py'",
            file=sys.stderr,
        )
        return False
    return True


def _check_submodules() -> bool:
    ok = True
    for d in (AASDK_DIR, OPENAUTO_DIR):
        if not (d / "CMakeLists.txt").exists():
            print(f"Submodule not initialised: {d}", file=sys.stderr)
            ok = False
    if not ok:
        print(
            "Run:  git submodule update --init"
            " server/third_party/aasdk server/third_party/openauto",
            file=sys.stderr,
        )
    return ok


def _apply_patches() -> None:
    """Apply all .patch files from PATCHES_DIR to the openauto source tree.

    Each patch is applied with --check first; if it fails the patch is already
    applied (idempotent), so we skip it silently.
    """
    for patch in sorted(PATCHES_DIR.glob("*.patch")):
        check = subprocess.run(
            ["git", "apply", "--check", str(patch)],
            cwd=str(OPENAUTO_DIR),
            capture_output=True,
        )
        if check.returncode != 0:
            print(f"Patch already applied (skipping): {patch.name}", file=sys.stderr)
            continue
        print(f"Applying patch: {patch.name}", file=sys.stderr)
        subprocess.run(
            ["git", "apply", str(patch)],
            cwd=str(OPENAUTO_DIR),
            check=True,
        )


def _blkid_flags() -> List[str]:
    stub = os.environ.get("BLKID_STUB", "")
    if not stub:
        return []
    return [
        f"-DBLKID_INCLUDE_DIRS={stub}/include",
        f"-DBLKID_LIBRARIES={stub}/lib/libblkid.a",
    ]


def _openssl_link_flags() -> str:
    try:
        return subprocess.check_output(
            ["pkg-config", "--libs", "openssl"], text=True
        ).strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return "-lssl -lcrypto"


# PATH_SUFFIXES in Findaap_protobuf.cmake / Findaasdk.cmake resolve include dirs
# one level too deep — pre-set the cache vars to bypass their finder logic.
_FINDER_OVERRIDES: List[str] = [
    f"-DAAP_PROTOBUF_INCLUDE_DIR={INSTALL_PREFIX}/include",
    f"-DAAP_PROTOBUF_LIB_DIR={INSTALL_PREFIX}/lib/libaap_protobuf.a",
    f"-DAASDK_INCLUDE_DIR={INSTALL_PREFIX}/include/aasdk",
    f"-DAASDK_LIB_DIR={INSTALL_PREFIX}/lib/libaasdk.a",
]


# ── Public build / run API ─────────────────────────────────────────────────────

def build_openauto(rebuild: bool = False) -> None:
    """Build aasdk + openauto from source inside the active Nix shell."""
    if not _check_nix():
        raise SystemExit(1)
    if not _check_submodules():
        raise SystemExit(1)

    jobs = multiprocessing.cpu_count()

    if rebuild:
        for d in (AASDK_BUILD, OPENAUTO_BUILD, INSTALL_PREFIX):
            if d.exists():
                print(f"Removing {d} …", file=sys.stderr)
                shutil.rmtree(d)

    # ── aasdk ──
    print("─── Building aasdk ───", file=sys.stderr)
    _run([
        "cmake", "-S", str(AASDK_DIR), "-B", str(AASDK_BUILD),
        "-GNinja",
        "-DCMAKE_BUILD_TYPE=Release",
        f"-DCMAKE_INSTALL_PREFIX={INSTALL_PREFIX}",
        "-DAASDK_TEST=OFF",
        "-DAASDK_BENCHMARK=OFF",
        "-DSKIP_BUILD_PROTOBUF=ON",
        "-DSKIP_BUILD_ABSL=ON",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        f"-DCMAKE_PREFIX_PATH={_cmake_prefix_path()}",
    ])
    _run(["cmake", "--build", str(AASDK_BUILD), f"-j{jobs}"])

    # aasdk's install rule writes TLS certs to /etc/aasdk (needs root); treat as non-fatal.
    install_result = subprocess.run(
        ["cmake", "--install", str(AASDK_BUILD)], cwd=str(common.REPO_ROOT)
    )
    if install_result.returncode != 0:
        lib = INSTALL_PREFIX / "lib" / "libaasdk.a"
        if not lib.exists():
            print(f"ERROR: expected {lib} after install", file=sys.stderr)
            raise SystemExit(install_result.returncode)
        print("Note: /etc/aasdk cert install skipped (needs root) — artifacts OK", file=sys.stderr)

    # ── openauto ──
    print("─── Building openauto ───", file=sys.stderr)
    _apply_patches()
    stubs_header = common.REPO_ROOT / "scripts" / "openauto_macos_stubs.hpp"
    _run([
        "cmake", "-S", str(OPENAUTO_DIR), "-B", str(OPENAUTO_BUILD),
        "-GNinja",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DNOPI=ON",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        f"-DCMAKE_PREFIX_PATH={_cmake_prefix_path()}",
        f"-DCMAKE_CXX_FLAGS_INIT=-include {stubs_header}",
        f"-DCMAKE_EXE_LINKER_FLAGS={_openssl_link_flags()}",
    ] + _FINDER_OVERRIDES + _blkid_flags())
    _run(["cmake", "--build", str(OPENAUTO_BUILD), f"-j{jobs}"])


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


# ── CLI entry point ────────────────────────────────────────────────────────────

def main() -> int:
    if sys.platform != "darwin":
        print("This script only runs on macOS.", file=sys.stderr)
        return 1

    parser = argparse.ArgumentParser(
        description="Build (if needed) and run openauto inside the Nix shell.",
    )
    parser.add_argument("--rebuild", action="store_true", help="Force a clean rebuild.")
    parser.add_argument("--build-only", action="store_true", help="Build without launching.")
    args = parser.parse_args()

    if args.rebuild or args.build_only:
        if not _check_nix():
            return 1
        build_openauto(rebuild=args.rebuild)
        if args.build_only:
            return 0

    return run_openauto()


if __name__ == "__main__":
    raise SystemExit(main())
