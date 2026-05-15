#!/usr/bin/env python3
"""bootstrap.py — one-time developer setup: init submodules, verify tooling,
pre-build the openauto Docker image, and check TLS certificates.

Run once after cloning the repository:
    python3 scripts/bootstrap.py
"""

import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
COMPOSE_FILE = REPO_ROOT / "docker" / "docker-compose.yml"
CERTS_DIR = REPO_ROOT / "server" / "certs"


def _run(args: list[str], **kwargs) -> subprocess.CompletedProcess:
    """Run a subprocess, aborting with a human-readable message on failure."""
    print(f"  + {' '.join(args)}", file=sys.stderr)
    try:
        return subprocess.run(args, check=True, **kwargs)
    except subprocess.CalledProcessError as exc:
        print(
            f"\nERROR: command failed (exit {exc.returncode}):\n  {' '.join(args)}",
            file=sys.stderr,
        )
        raise SystemExit(1) from exc


def _check_submodules() -> None:
    print("Checking git submodules …", file=sys.stderr)
    result = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "submodule", "status"],
        capture_output=True,
        text=True,
        check=True,
    )
    uninitialized = [
        line for line in result.stdout.splitlines() if line.startswith("-")
    ]
    if uninitialized:
        print(
            f"  {len(uninitialized)} uninitialized submodule(s) found — initialising …",
            file=sys.stderr,
        )
        _run(
            ["git", "-C", str(REPO_ROOT), "submodule", "update", "--init", "--recursive"]
        )
    else:
        print("  Submodules OK.", file=sys.stderr)


def _check_docker() -> None:
    print("Checking Docker …", file=sys.stderr)
    try:
        subprocess.run(
            ["docker", "info"],
            check=True,
            capture_output=True,
        )
    except FileNotFoundError:
        print(
            "\nERROR: 'docker' not found. Install Docker Desktop or Docker Engine:\n"
            "  https://docs.docker.com/get-docker/",
            file=sys.stderr,
        )
        raise SystemExit(1)
    except subprocess.CalledProcessError:
        print(
            "\nERROR: 'docker info' failed. Is the Docker daemon running?",
            file=sys.stderr,
        )
        raise SystemExit(1)

    # Verify 'docker compose' (v2 plugin) is available.
    try:
        subprocess.run(
            ["docker", "compose", "version"],
            check=True,
            capture_output=True,
        )
    except subprocess.CalledProcessError:
        print(
            "\nERROR: 'docker compose' (v2) is not available.\n"
            "  Upgrade Docker Desktop or install the Compose plugin:\n"
            "  https://docs.docker.com/compose/install/",
            file=sys.stderr,
        )
        raise SystemExit(1)

    print("  Docker OK.", file=sys.stderr)


def _check_cargo() -> None:
    print("Checking Cargo (Rust toolchain) …", file=sys.stderr)
    try:
        subprocess.run(["cargo", "--version"], check=True, capture_output=True)
    except FileNotFoundError:
        print(
            "\nERROR: 'cargo' not found. Install Rust via rustup:\n"
            "  https://rustup.rs/",
            file=sys.stderr,
        )
        raise SystemExit(1)
    except subprocess.CalledProcessError:
        print(
            "\nERROR: 'cargo --version' failed. Check your Rust installation.",
            file=sys.stderr,
        )
        raise SystemExit(1)

    print("  Cargo OK.", file=sys.stderr)


def _build_docker() -> None:
    print("Pre-building openauto Docker image …", file=sys.stderr)
    _run(
        [
            "docker",
            "compose",
            "-f",
            str(COMPOSE_FILE),
            "build",
        ],
        cwd=str(REPO_ROOT),
    )
    print("  Docker image built.", file=sys.stderr)


def _check_certs() -> None:
    print("Checking TLS certificates …", file=sys.stderr)
    if not CERTS_DIR.exists():
        print(f"  WARNING: {CERTS_DIR} does not exist.", file=sys.stderr)
        _print_cert_instructions()
        return

    cert_files = list(CERTS_DIR.glob("*.crt")) + \
                 list(CERTS_DIR.glob("*.key")) + \
                 list(CERTS_DIR.glob("*.pem"))
    if not cert_files:
        print(
            f"  WARNING: No TLS certificates found in {CERTS_DIR}.",
            file=sys.stderr,
        )
        _print_cert_instructions()
    else:
        print(
            f"  Found {len(cert_files)} certificate file(s): "
            + ", ".join(f.name for f in cert_files),
            file=sys.stderr,
        )


def _print_cert_instructions() -> None:
    print(
        "\n  To generate self-signed certs for local development run:\n"
        "\n"
        "    mkdir -p server/certs && \\\n"
        "    openssl req -x509 -newkey rsa:4096 -keyout server/certs/server.key \\\n"
        "        -out server/certs/server.crt -days 365 -nodes \\\n"
        "        -subj '/CN=localhost'\n"
        "\n"
        "  (OpenSSL required; optional for development without TLS.)\n",
        file=sys.stderr,
    )


def main() -> int:
    print("=== smartcar bootstrap ===\n", file=sys.stderr)

    _check_submodules()
    _check_docker()
    _check_cargo()
    _build_docker()
    _check_certs()

    print(
        "\n=== Bootstrap complete ===\n"
        "  Start the stack:    python3 scripts/run_stack.py\n"
        "  Emulator only:      python3 scripts/run_emulator.py\n"
        "  Server only:        python3 scripts/run_server.py\n",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
