#!/usr/bin/env python3
"""init.py — one-time developer setup.

Run once after cloning:
    make init
"""

import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CERTS_DIR = REPO_ROOT / "server" / "certs"


def _run(args: list[str], **kwargs) -> None:
    print(f"  + {' '.join(args)}", file=sys.stderr)
    try:
        subprocess.run(args, check=True, **kwargs)
    except subprocess.CalledProcessError as exc:
        print(f"\nERROR: command failed (exit {exc.returncode})", file=sys.stderr)
        raise SystemExit(1) from exc


def _check_nix() -> None:
    print("Checking Nix …", file=sys.stderr)
    if shutil.which("nix-shell"):
        print("  nix-shell OK.", file=sys.stderr)
        return
    # DeterminateSystems installs to a fixed path not always on PATH yet
    if Path("/nix/var/nix/profiles/default/bin/nix-shell").exists():
        print("  nix-shell found at /nix/var/nix/profiles/default/bin/nix-shell.", file=sys.stderr)
        print("  Add /nix/var/nix/profiles/default/bin to PATH if not already set.", file=sys.stderr)
        return
    print(
        "  WARNING: Nix not found — required to build openauto.\n"
        "  Install with the DeterminateSystems installer:\n"
        "    curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install",
        file=sys.stderr,
    )


def _check_submodules() -> None:
    print("Checking git submodules …", file=sys.stderr)
    result = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "submodule", "status"],
        capture_output=True, text=True, check=True,
    )
    if any(line.startswith("-") for line in result.stdout.splitlines()):
        _run(["git", "-C", str(REPO_ROOT), "submodule", "update", "--init", "--recursive"])
    else:
        print("  Submodules OK.", file=sys.stderr)


def _check_cargo() -> None:
    print("Checking Rust toolchain …", file=sys.stderr)
    if shutil.which("cargo"):
        print("  Cargo OK.", file=sys.stderr)
    else:
        print("  WARNING: cargo not found — install Rust via https://rustup.rs/", file=sys.stderr)


def _check_certs() -> None:
    print("Checking TLS certificates …", file=sys.stderr)
    key = CERTS_DIR / "server.key"
    crt = CERTS_DIR / "server.crt"
    if key.exists() and crt.exists():
        print("  Certs OK.", file=sys.stderr)
        return
    print("  No certs found — generating self-signed cert …", file=sys.stderr)
    CERTS_DIR.mkdir(parents=True, exist_ok=True)
    _run([
        "openssl", "req", "-x509", "-newkey", "rsa:4096",
        "-keyout", str(key), "-out", str(crt),
        "-days", "365", "-nodes", "-subj", "/CN=localhost",
    ], capture_output=True)
    print("  Generated server/certs/server.key and server/certs/server.crt.", file=sys.stderr)


def main() -> int:
    _check_nix()
    _check_submodules()
    _check_cargo()
    _check_certs()
    print("\nInit complete.", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
