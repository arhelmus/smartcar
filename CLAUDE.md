# smartcar — AI Agent Guide

## What this project is

**smartcar** is a custom Android Auto projection source written in Rust. It implements the phone/server side of the Android Auto (AA) wire protocol and connects to a head unit — either `openauto` running in Docker for local development, or a real car head unit in production.

Key roles:
- **`server/`** — Rust workspace; the AA projection source. The top-level crates are `aap-core` (protocol engine), `aap-transport` (TCP/TLS), `aap-video` (GStreamer encoder), and `aap-flutter` (embedded UI renderer).
- **`docker/`** — `openauto` head-unit emulator + Docker Compose stack; VNC-accessible on port 5900.
- **`scripts/`** — Python orchestration (stdlib only); `run_stack.py` brings up the full dev environment.
- **`apps/`** — future iOS/Android client apps (currently placeholders).
- **`server/third_party/openauto`** — vendored `openauto` source used as a reference.

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
| https://github.com/tomasz-grobelny/AACS | Server (`AAServer`) — **behavioural ground truth** | _(cloned externally; see `/tmp/aa_investigate/AACS` if still present)_ |
| https://github.com/opencardev/aasdk | Client lib — framing, message-id enums, full channel catalog | `server/third_party/` (partial) |
| https://github.com/opencardev/AAProto | Protocol — canonical protobuf schemas | _(external)_ |

When validating a protocol doc claim:
1. Check AACS `AAServer/src/` for **behavioural** truth (state machines, call order, blocking).
2. Check `AAProto/*.proto` for **schema** truth (field numbers, enum values).
3. Use `aasdk/include/aasdk/Channel/**` for the **full channel catalog** (channels AACS doesn't implement).
4. The authoritative enum file is `AACS/include/enums.h` (repo root `include/`), **not** `AAServer/include/enums.h`.
