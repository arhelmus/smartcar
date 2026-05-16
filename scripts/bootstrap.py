#!/usr/bin/env python3
"""bootstrap.py — one-time developer setup.

Run once after cloning:
    python3 scripts/bootstrap.py
"""

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
    try:
        subprocess.run(["cargo", "--version"], check=True, capture_output=True)
        print("  Cargo OK.", file=sys.stderr)
    except FileNotFoundError:
        print("ERROR: cargo not found — install Rust via https://rustup.rs/", file=sys.stderr)
        raise SystemExit(1)


def _check_certs() -> None:
    print("Checking TLS certificates …", file=sys.stderr)
    certs = (
        list(CERTS_DIR.glob("*.crt")) +
        list(CERTS_DIR.glob("*.key")) +
        list(CERTS_DIR.glob("*.pem"))
    )
    if certs:
        print(f"  Found: {', '.join(f.name for f in certs)}", file=sys.stderr)
    else:
        print(
            "  WARNING: no certs in server/certs/. Generate with:\n"
            "    openssl req -x509 -newkey rsa:4096 -keyout server/certs/server.key \\\n"
            "        -out server/certs/server.crt -days 365 -nodes -subj '/CN=localhost'",
            file=sys.stderr,
        )


def main() -> int:
    _check_submodules()
    _check_cargo()
    _check_certs()
    print("\nBootstrap complete.", file=sys.stderr)
    print("  Build openauto:  nix-shell --pure --run 'python3 scripts/build_openauto.py'", file=sys.stderr)
    print("  Run openauto:    nix-shell --pure --run 'python3 scripts/run_openauto.py'", file=sys.stderr)
    print("  Run server:      python3 scripts/run_server.py", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
