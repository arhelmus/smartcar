# smartcar

Custom Android Auto projection source written in Rust. Connects to `openauto`
(head unit emulator) for local development; to a real car head unit in
production.

## Layout

- `server/` — Rust workspace (the AA projection source).
- `shell.nix` — hermetic Nix environment for building openauto natively on macOS.
- `scripts/` — Python orchestration (`init.py`, `build_openauto.py`, `run_openauto.py`, `run_server.py`).
- `scripts/patches/openauto/` — local patches applied to the openauto submodule at build time.
- `apps/ios`, `apps/android` — future client apps (placeholders).
- `docs/protocol/` — Android Auto wire protocol documentation.

## Quickstart

```sh
make init
```
