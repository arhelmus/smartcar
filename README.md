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

## Review

The repository has an automated review pipeline that runs on every `git
push` via the pre-push hook and can also be invoked manually:

```sh
make review
```
