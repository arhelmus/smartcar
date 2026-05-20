# smartcar

Custom Android Auto projection source written in Rust. Connects to `openauto`
(head unit emulator) for local development; to a real car head unit in
production.

## Layout

- `server/` — Rust workspace (the AA projection source).
- `server/flutter-ui/` — head-unit Flutter UI embedded in `smartcar-server` via `aap-flutter`.
- `mobile/` — phone-side Flutter app (iOS + Android), talks to the board over the `aap-bridge` control channel.
- `shell.nix` — hermetic Nix environment for building openauto natively on macOS.
- `scripts/` — Python orchestration (`init.py`, `build_openauto.py`, `run_openauto.py`, `run_server.py`, `review.py`).
- `scripts/patches/openauto/` — local patches applied to the openauto submodule at build time.
- `docs/protocol/` — Android Auto wire protocol documentation.

## Quickstart

```sh
make init
```

## Pre-push checks (`make review`)

`make review` runs every check the pre-push git hook runs: `cargo
fmt / clippy / test / audit`, a cross-compile to `x86_64-unknown-linux-gnu`
when on non-Linux hosts (catches Linux-only deps like `bluer` /
`libdbus-sys` that the macOS host build skips), and the Flutter
`pub get --enforce-lockfile / format / analyze / test` pipeline for both
`mobile/` and `server/flutter-ui/`. All checks run in parallel.

```sh
make review                       # full run
make review ARGS=--no-cross       # skip the Docker-backed Linux check
```

The pre-push hook is a thin shim over `scripts/review.py` — anything you
can pass to the script you can pass via `ARGS=` to the Makefile target.
