#!/usr/bin/env python3
"""init.py — one-time developer setup.

Run once after cloning:
    make init

Bump `INIT_VERSION` whenever init.py grows a new step (new prerequisite,
new patch, changed seed). `common.py` imports this constant and refuses
to run if `scripts/.init` records an older value, prompting the user to
re-run `make init`.
"""

# Bump whenever init.py changes meaningfully.
INIT_VERSION = 6

import platform
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPTS_DIR = Path(__file__).resolve().parent
INIT_STAMP = SCRIPTS_DIR / ".init"
CERTS_DIR = REPO_ROOT / "server" / "certs"
OPENAUTO_DIR = REPO_ROOT / "server" / "third_party" / "openauto"
PATCHES_DIR = REPO_ROOT / "scripts" / "patches" / "openauto"
OPENAUTO_PATCH_COMMIT = "smartcar: macos patches"


def _die(message: str) -> None:
    """Print an ERROR + actionable hint and exit non-zero.

    Used for prerequisites that downstream scripts (cargo build,
    build_openauto, make deploy, review) hard-depend on. Soft warnings
    that the user can legitimately ignore stay as `WARNING:` prints
    without exiting.
    """
    print(f"  ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


def _run(args: list[str], **kwargs) -> None:
    print(f"  + {' '.join(args)}", file=sys.stderr)
    try:
        subprocess.run(args, check=True, **kwargs)
    except subprocess.CalledProcessError as exc:
        # When the caller passed capture_output=True, the subprocess's
        # stdout/stderr never reached the terminal — flush them here so the
        # actual failure (not just the exit code) shows up immediately.
        for stream in (exc.stdout, exc.stderr):
            if not stream:
                continue
            text = stream.decode(errors="replace") if isinstance(stream, bytes) else stream
            sys.stderr.write(text)
            if not text.endswith("\n"):
                sys.stderr.write("\n")
        print(f"\nERROR: command failed (exit {exc.returncode})", file=sys.stderr)
        raise SystemExit(1) from exc


def _check_nix() -> None:
    # openauto builds via shell.nix on macOS; on Linux it builds natively
    # via apt deps (_check_apt_deps), so nix is genuinely not needed.
    if platform.system() != "Darwin":
        return
    print("Checking Nix …", file=sys.stderr)
    if shutil.which("nix-shell"):
        print("  nix-shell OK.", file=sys.stderr)
        return
    # DeterminateSystems installs to a fixed path not always on PATH yet.
    # Don't silently accept this — build_openauto.py runs `nix-shell` and will
    # fail with a confusing error if PATH isn't updated.
    if Path("/nix/var/nix/profiles/default/bin/nix-shell").exists():
        _die(
            "nix-shell installed at /nix/var/nix/profiles/default/bin but not on PATH.\n"
            "         Add it to your shell:\n"
            "           export PATH=/nix/var/nix/profiles/default/bin:$PATH"
        )
    _die(
        "nix-shell not found — required to build openauto on macOS.\n"
        "         Install with the DeterminateSystems installer:\n"
        "           curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install"
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


def _check_apt_deps() -> None:
    """Install the Linux system libs cargo's `-sys` crates need to link.

    macOS dev hosts get these via nix-shell (or Homebrew) — no apt to run.
    On Linux, libdbus-1-dev / libssl-dev / protobuf-compiler are what
    libdbus-sys / openssl-sys / prost actually link against; without them
    cargo build fails late with a pkg-config error.
    """
    if platform.system() != "Linux":
        return
    print("Checking apt deps …", file=sys.stderr)
    if not shutil.which("apt-get"):
        print("  WARNING: apt-get not found — install protobuf-compiler, "
              "libdbus-1-dev, libssl-dev manually for your distro.", file=sys.stderr)
        return
    packages = ["protobuf-compiler", "libdbus-1-dev", "libssl-dev"]
    # `dpkg -s <pkg>` returns 0 iff installed; skip apt-get if nothing's missing.
    missing = [
        p for p in packages
        if subprocess.run(
            ["dpkg", "-s", p],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        ).returncode != 0
    ]
    if not missing:
        print(f"  {' '.join(packages)} OK.", file=sys.stderr)
        return
    print(f"  Installing {' '.join(missing)} …", file=sys.stderr)
    _run(["sudo", "apt-get", "update", "-qq"])
    _run(["sudo", "apt-get", "install", "-y", "--no-install-recommends", *missing])


def _check_cargo() -> None:
    print("Checking Rust toolchain …", file=sys.stderr)
    if shutil.which("cargo"):
        print("  Cargo OK.", file=sys.stderr)
        return
    _die(
        "cargo not found — install Rust via https://rustup.rs/\n"
        "         (or `brew install rustup-init && rustup-init` on macOS)"
    )


def _check_cross() -> None:
    print("Checking cross-compilation tools …", file=sys.stderr)

    if not shutil.which("rustup"):
        _die(
            "rustup not found — install from https://rustup.rs/\n"
            "         (or `brew install rustup-init && rustup-init` on macOS)"
        )

    if not shutil.which("cross"):
        _die("`cross` not found — install with:  cargo install cross")

    # The cross Docker image is amd64-only. On Apple Silicon we need to
    # force-install the x86_64-unknown-linux-gnu host toolchain so cross can
    # mount it into the container; on native Linux x86_64 hosts the matching
    # toolchain is already in place and nothing extra is needed.
    if platform.machine() != "arm64":
        print("  cross OK.", file=sys.stderr)
        return

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
    # _check_cargo runs first and dies if cargo is missing, so this is safe.
    print("Checking cargo-audit …", file=sys.stderr)
    if shutil.which("cargo-audit"):
        print("  cargo-audit OK.", file=sys.stderr)
        return
    print("  cargo-audit not found — installing …", file=sys.stderr)
    _run(["cargo", "install", "cargo-audit"])


def _check_cargo_sweep() -> None:
    # Same precondition as _check_cargo_audit.
    print("Checking cargo-sweep …", file=sys.stderr)
    if shutil.which("cargo-sweep"):
        print("  cargo-sweep OK.", file=sys.stderr)
        return
    print("  cargo-sweep not found — installing …", file=sys.stderr)
    _run(["cargo", "install", "cargo-sweep"])


def _check_ansible() -> None:
    """Check ansible-playbook and ansible-lint — used by board/ provisioning.

    Both are required: `scripts/deploy.py` runs the playbook, and the
    `ansible (board)` check in `scripts/review.py` (which the pre-push
    hook calls) runs `ansible-lint`. Install instructions differ by
    platform so we don't auto-install; we just fail loud with the right
    one-liner.
    """
    print("Checking Ansible …", file=sys.stderr)
    missing = [t for t in ("ansible-playbook", "ansible-lint") if not shutil.which(t)]
    if not missing:
        print("  ansible-playbook + ansible-lint OK.", file=sys.stderr)
        return
    if platform.system() == "Darwin":
        hint = "brew install ansible ansible-lint"
    elif platform.system() == "Linux":
        hint = "sudo apt-get install -y ansible-core && pipx install ansible-lint"
    else:
        hint = "pipx install ansible-core ansible-lint"
    _die(f"missing {' '.join(missing)} — install with:\n         {hint}")


def _check_env_local() -> None:
    print("Checking .env.local …", file=sys.stderr)
    target = REPO_ROOT / ".env.local"
    template = REPO_ROOT / ".env.local.example"
    if target.exists():
        print("  .env.local OK.", file=sys.stderr)
        return
    if not template.exists():
        # The template is checked into git; if it's missing, the checkout is
        # broken — re-cloning is the only sane fix.
        _die(
            ".env.local.example missing from the repo root — checkout looks corrupt.\n"
            "         Re-clone or `git checkout .env.local.example`."
        )
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
    patch_files = sorted(PATCHES_DIR.glob("*.patch"))
    if not patch_files:
        print("  No patches to apply.", file=sys.stderr)
        return

    # If our patch commit is already at HEAD, drop it so we re-apply the
    # current set of patches cleanly — this makes adding or removing a patch
    # take effect on the next `make init` without manual submodule surgery.
    head_subject = subprocess.run(
        ["git", "-C", str(OPENAUTO_DIR), "log", "-1", "--format=%s"],
        capture_output=True, text=True,
    ).stdout.strip()
    if head_subject == OPENAUTO_PATCH_COMMIT:
        print("  Resetting previous patch commit to re-apply …", file=sys.stderr)
        _run(["git", "-C", str(OPENAUTO_DIR), "reset", "--hard", "HEAD~1"])

    print(f"  Applying {len(patch_files)} patch(es) …", file=sys.stderr)
    for patch in patch_files:
        _run(["git", "-C", str(OPENAUTO_DIR), "apply", str(patch)])

    _run(["git", "-C", str(OPENAUTO_DIR), "add", "-A"])
    # Scope identity to this one commit so CI runners (no global git config)
    # can still create it; the commit lives only in the submodule, never
    # pushed, so any name/email is fine.
    _run([
        "git", "-C", str(OPENAUTO_DIR),
        "-c", "user.name=smartcar init",
        "-c", "user.email=init@smartcar.local",
        "commit", "-m", OPENAUTO_PATCH_COMMIT,
    ])
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


def _write_init_stamp() -> None:
    print("Writing init stamp …", file=sys.stderr)
    INIT_STAMP.write_text(f"{INIT_VERSION}\n")
    print(f"  scripts/.init = v{INIT_VERSION}", file=sys.stderr)


def main() -> int:
    _check_nix()
    _check_submodules()
    _patch_openauto()
    _quiet_openauto_pointer()
    _check_apt_deps()
    _check_cargo()
    _check_cargo_audit()
    _check_cargo_sweep()
    _check_cross()
    _check_ansible()
    _check_env_local()
    _check_certs()
    _setup_git_hooks()
    _write_init_stamp()
    print("\nInit complete.", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
