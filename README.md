# smartcar

Custom Android Auto projection source written in Rust. Connects to `openauto`
(head unit emulator) for local development; to a real car head unit in
production.

## Layout

- `server/` — Rust workspace (the projection source).
- `apps/ios`, `apps/android` — future client apps (placeholders).
- `docker/` — openauto emulator container + compose.
- `scripts/` — Python orchestration (stdlib only).
- `docs/protocol/` — Android Auto wire protocol documentation.

## Quickstart

```sh
git submodule update --init --recursive
cd server && cargo check --workspace
```
