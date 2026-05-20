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
OPENAUTO_DIR = REPO_ROOT / "server" / "third_party" / "openauto"
PATCHES_DIR = REPO_ROOT / "scripts" / "patches" / "openauto"
OPENAUTO_PATCH_COMMIT = "smartcar: macos patches"


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
    # A leading '-' marks an uninitialized submodule, '+' a checked-out commit
    # that differs from the one recorded. Either way we (re)sync. The update
    # is idempotent and cheap when everything is already in place, so run it
    # unconditionally rather than relying on the status prefix heuristic.
    needs_work = any(
        line and line[0] in "-+" for line in result.stdout.splitlines()
    )
    _run(["git", "-C", str(REPO_ROOT), "submodule", "update", "--init", "--recursive"])
    if needs_work:
        print("  Submodules synced.", file=sys.stderr)
    else:
        print("  Submodules OK.", file=sys.stderr)


def _check_cargo() -> None:
    print("Checking Rust toolchain …", file=sys.stderr)
    if shutil.which("cargo"):
        print("  Cargo OK.", file=sys.stderr)
    else:
        print("  WARNING: cargo not found — install Rust via https://rustup.rs/", file=sys.stderr)


def _check_cross() -> None:
    print("Checking cross-compilation tools …", file=sys.stderr)

    if not shutil.which("rustup"):
        print("  WARNING: rustup not found — install from https://rustup.rs/", file=sys.stderr)
        print("           or: brew install rustup-init && rustup-init", file=sys.stderr)
        return

    if not shutil.which("cross"):
        print("  WARNING: 'cross' not found — install with: cargo install cross", file=sys.stderr)
        return

    # The cross Docker image for aarch64 is amd64-only; cross mounts the
    # x86_64-unknown-linux-gnu toolchain from the host into the container.
    # On Apple Silicon this toolchain must be force-installed.
    HOST_TOOLCHAIN = "stable-x86_64-unknown-linux-gnu"
    installed = subprocess.run(
        ["rustup", "toolchain", "list"], capture_output=True, text=True,
    ).stdout
    if HOST_TOOLCHAIN in installed:
        print("  cross + host toolchain OK.", file=sys.stderr)
        return

    print(f"  Installing host toolchain '{HOST_TOOLCHAIN}' for cross (Apple Silicon, one-time) …",
          file=sys.stderr)
    _run(["rustup", "toolchain", "add", HOST_TOOLCHAIN, "--force-non-host", "--profile", "minimal"])


def _check_cargo_audit() -> None:
    print("Checking cargo-audit …", file=sys.stderr)
    if shutil.which("cargo-audit"):
        print("  cargo-audit OK.", file=sys.stderr)
        return
    if not shutil.which("cargo"):
        print("  WARNING: cargo not found — skipping cargo-audit install.", file=sys.stderr)
        return
    print("  cargo-audit not found — installing …", file=sys.stderr)
    _run(["cargo", "install", "cargo-audit"])


def _check_env_local() -> None:
    print("Checking .env.local …", file=sys.stderr)
    target = REPO_ROOT / ".env.local"
    template = REPO_ROOT / ".env.local.example"
    if target.exists():
        print("  .env.local OK.", file=sys.stderr)
        return
    if not template.exists():
        print("  WARNING: .env.local.example missing — skipping.", file=sys.stderr)
        return
    shutil.copy(template, target)
    print(f"  Created .env.local from {template.name}.", file=sys.stderr)
    print("  Edit .env.local to fill in your local values (BOARD_HOST, BOARD_MAC, …).",
          file=sys.stderr)


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


def _setup_git_hooks() -> None:
    print("Configuring git hooks …", file=sys.stderr)
    result = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "config", "core.hooksPath"],
        capture_output=True, text=True,
    )
    if result.stdout.strip() == ".githooks":
        print("  Hooks already configured.", file=sys.stderr)
        return
    _run(["git", "-C", str(REPO_ROOT), "config", "core.hooksPath", ".githooks"])
    print("  Hooks configured — .githooks/ is now active.", file=sys.stderr)


def _patch_openauto() -> None:
    print("Checking openauto patches …", file=sys.stderr)
    result = subprocess.run(
        ["git", "-C", str(OPENAUTO_DIR), "log", "--oneline", f"--grep={OPENAUTO_PATCH_COMMIT}"],
        capture_output=True, text=True,
    )
    if result.returncode == 0 and result.stdout.strip():
        print("  Patches already applied.", file=sys.stderr)
        return

    patch_files = sorted(PATCHES_DIR.glob("*.patch"))
    if not patch_files:
        print("  No patches to apply.", file=sys.stderr)
        return

    print(f"  Applying {len(patch_files)} patch(es) …", file=sys.stderr)
    for patch in patch_files:
        _run(["git", "-C", str(OPENAUTO_DIR), "apply", str(patch)])

    _run(["git", "-C", str(OPENAUTO_DIR), "add", "-A"])
    _run(["git", "-C", str(OPENAUTO_DIR), "commit", "-m", OPENAUTO_PATCH_COMMIT])
    print("  Patches committed locally.", file=sys.stderr)


def _quiet_openauto_pointer() -> None:
    """Stop git/IDE flagging the openauto submodule pointer as a change.

    `_patch_openauto` commits the macOS patches *inside* the submodule, which
    necessarily moves its HEAD away from the commit the parent repo records.
    That divergence is intentional and must never be committed to the parent,
    so tell git to ignore this submodule's state entirely (local config only —
    not shared via .gitmodules). Idempotent.
    """
    print("Quieting openauto submodule pointer …", file=sys.stderr)
    _run([
        "git", "-C", str(REPO_ROOT), "config",
        'submodule.server/third_party/openauto.ignore', "all",
    ])
    print("  submodule.server/third_party/openauto.ignore = all", file=sys.stderr)


def main() -> int:
    _check_nix()
    _check_submodules()
    _patch_openauto()
    _quiet_openauto_pointer()
    _check_cargo()
    _check_cargo_audit()
    _check_cross()
    _check_env_local()
    _check_certs()
    _setup_git_hooks()
    print("\nInit complete.", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
