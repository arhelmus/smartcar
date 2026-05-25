# smartcar — AI Agent Guide

## What this project is

**smartcar** is a custom Android Auto projection source written in Rust. It implements the phone/server side of the Android Auto (AA) wire protocol and connects to a head unit — either `openauto` built and run natively (via `shell.nix`) for local development, or a real car head unit in production.

Key roles:
- **`server/`** — Rust workspace; the AA projection source. The top-level crates are `aap-core` (protocol engine), `aap-transport` (TCP/TLS), `aap-video` (GStreamer encoder), and `aap-flutter` (embedded UI renderer).
- **`server/flutter-ui/`** — Flutter head-unit UI embedded in `smartcar-server` via `aap-flutter`.
- **`mobile/`** — phone-side Flutter app (iOS + Android), talks to the board over the `aap-bridge` BLE control channel.
- **`shell.nix`** — hermetic Nix environment that provides Qt5 / GStreamer / libblkid / OpenSSL for the native openauto build.
- **`scripts/`** — Python orchestration (stdlib only): `init.py`, `build_openauto.py`, `run_openauto.py`, `run_server.py`, `deploy.py` (cross-build + rsync + ansible + restart + healthcheck), `assign_board.py` (sudo; assigns the laptop's USB-Ethernet IP), `review.py`.
- **`server/third_party/openauto`** — vendored `openauto` source, patched on `make init` from `scripts/patches/openauto/`.

## Running & testing locally

**Never hand-run `cargo run` for the server, and never start openauto by hand.** To exercise the end-to-end pipeline always use the orchestration scripts:

- **`python3 scripts/run_openauto.py`** — builds (if needed) and launches openauto natively under `nix-shell`. Listens on TCP `127.0.0.1:5278`. Start this first.
- **`python3 scripts/run_server.py`** — builds and runs `smartcar-server` against the local openauto.

Log behaviour for `run_server.py`:

- **Default (detached):** the build runs in the foreground, then the server runs in the background. **stdout+stderr are written to `smartcar-server.log` in the repo root** (`/Users/arhelmus/smartcar/smartcar-server.log`). Read that file to inspect logs after a run.
- **`--attached`:** runs the server in the foreground so logs stream live in the terminal (use this when you want to watch logs in place).
- `--log LEVEL` sets `RUST_LOG` (e.g. `--log info,aap_core=debug`); `--release` / `--debug` pick the build profile. The script kills any previous server instance via its PID file before starting.

So: to verify a change, run `run_openauto.py`, then `run_server.py` (add `--attached` for live logs, or read `smartcar-server.log` afterwards). Do not claim a runtime behaviour is fixed without checking that log.

## Android Auto protocol questions

For any question about the AA wire protocol, **consult `docs/protocol/` first**:

- `docs/protocol/README.md` — documentation plan, source-material table, conventions
- `docs/protocol/00-overview.md` — actors, end-to-end sequence, glossary
- `docs/protocol/01-physical-transport.md` — USB AOAP mode-switch, FunctionFS, TCP variant
- `docs/protocol/02-framing.md` — frame header, flag bits, fragmentation/reassembly
- `docs/protocol/03-control-channel.md` — ch0 messages, version negotiation, ping
- `docs/protocol/04-tls-auth.md` — `SSL_accept` BIO pump, `AuthComplete`, encryption boundary
- `docs/protocol/05-service-discovery.md` — channel catalog, handler map
- `docs/protocol/06-channel-lifecycle.md` — `ChannelOpenRequest/Response`, blocking model
- `docs/protocol/07-video-channel.md` — video setup, media frames, ack flow
- `docs/protocol/08-input-channel.md` — input handshake, `InputEvent`/`TouchEvent`/`ButtonsEvent`
- `docs/protocol/09-audio-sensor-other.md` — audio, sensor, bluetooth, navigation, vendor-ext
- `docs/protocol/10-message-catalog.md` — exhaustive message-id ↔ protobuf ↔ direction table
- `docs/protocol/11-aacs-client-socket.md` — AACS Unix socket API (protocol B)
- `docs/protocol/12-sequences.md` — annotated end-to-end traces, blocking hazards

## Protocol doc caveats — always validate against sources

**The protocol docs are reverse-engineered and may contain mistakes.** If a claim in `docs/protocol/` looks wrong, or you need to verify behaviour before implementing, check the reference implementations directly:

| Repo | Role | Local vendor path |
|------|------|-------------------|
| https://github.com/tomasz-grobelny/AACS | Server (`AAServer`) — **behavioural ground truth** | `server/third_party/AACS` |
| https://github.com/opencardev/aasdk | Client lib — framing, message-id enums, full channel catalog | `server/third_party/aasdk` |
| https://github.com/opencardev/AAProto | Protocol — canonical protobuf schemas | `server/third_party/AAProto` |
| https://github.com/aa-proxy/aa-proxy-rs | **Transport reference** — how to wire USB AOAP gadget + wireless AA on Linux (same hardware class: Pi Zero 2 W / Orange Pi Zero 2W) | `server/third_party/aa-proxy-rs` |

When validating a protocol doc claim:
1. Check AACS `AAServer/src/` for **behavioural** truth (state machines, call order, blocking).
2. Check `AAProto/*.proto` for **schema** truth (field numbers, enum values).
3. Use `aasdk/include/aasdk/Channel/**` for the **full channel catalog** (channels AACS doesn't implement).
4. The authoritative enum file is `AACS/include/enums.h` (repo root `include/`), **not** `AAServer/include/enums.h`.
5. For **transport-layer** questions — how the gadget claims the UDC, AOAP mode-switch sequencing, wireless AA WiFi/BT pairing dance, reconnection logic — read `aa-proxy-rs/src/`. It targets the same Pi-class single-board Linux boxes we run on (Pi Zero 2 W / Orange Pi Zero 2W). The closest working reference for the patterns smartcar-server reimplements:
   - `aoa.rs` + `usb_gadget.rs` + `usb_stream.rs` — USB AOAP gadget bring-up and the I/O loop on the UDC
   - `bluetooth.rs` + `bt_helper.rs` + `btle.rs` — wireless AA pairing and the BT/BLE handshake the phone expects
   - `proxy.rs` — the main wire-up tying transport → AA framing
   - `io_uring.rs` — io_uring-based async I/O on the gadget side (we don't use this yet; relevant if/when we move off blocking reads)
