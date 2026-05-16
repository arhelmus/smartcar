# 03 — Control Channel (channel 0)

Channel `0` is the **control channel**. It carries the entire session bring-up
(version negotiation, TLS handshake, auth, service discovery) and all
steady-state housekeeping (ping liveness, navigation/audio focus). It is *not*
a feature channel: it has no `ChannelOpenRequest/Response` lifecycle (see
`06-channel-lifecycle.md`); it exists for the whole session.

All ints on the wire are **big-endian**. Framing (header, encryption bit,
fragmentation) is in `02-framing.md`; the **decrypted** payload always begins
with a 2-byte BE **message id** (`MessageType`). Behavioural source of truth is
AACS `AAServer/src/AaCommunicator.cpp`; the enum numbering used here is from
`AACS/include/enums.h:21` (`enum MessageType`) — note this header lives at
`AACS/include/`, **not** `AAServer/include/`. `AAProto/ControlMessageIdsEnum.proto`
(`message ControlMessage.Enum`, package `gb.xxy.trial.proto.ids`) is the
schema-level cross-check: it agrees on every id AACS's `MessageType` defines and
additionally declares `NONE = 0x0000` plus the two Shutdown ids
(`0x000f`/`0x0010`) that AACS's enum omits and never dispatches (see table).
Conversely `VoiceSessionRequest = 0x11` *is* in AACS's enum (`enums.h:34`) and in
AAProto, but AACS still has no dispatch arm for it.

"→ HU" = server → head unit. "← HU" = head unit → server. The server is the
**active party after auth**: it sends ServiceDiscoveryRequest unprompted on
`AuthComplete` and answers HU pings.

## Control message-id table (`MessageType`)

| Id (hex) | Name | Dir | Encryption | Payload | AACS behaviour |
|----------|------|-----|------------|---------|----------------|
| `0x0001` | VersionRequest | ← HU | Plain | raw: `u16 major`, `u16 minor` (BE; **not** protobuf) | parsed, `handleVersionRequest` (`AaCommunicator.cpp:85`) |
| `0x0002` | VersionResponse | → HU | Plain | raw: `u16 major`, `u16 minor`, `u16 matchCode` (BE) | sent, `sendVersionResponse` (`AaCommunicator.cpp:75`) |
| `0x0003` | SslHandshake | ↔ HU | Plain | raw: opaque TLS handshake bytes | `handleSslHandshake` — details in `04-tls-auth.md` |
| `0x0004` | AuthComplete | ← HU | Plain | not parsed (presence-only trigger) | triggers `sendServiceDiscoveryRequest()` (`AaCommunicator.cpp:237`) |
| `0x0005` | ServiceDiscoveryRequest | → HU | Encrypted | protobuf `ServiceDiscoveryRequest` | sent by server — see `05-service-discovery.md` |
| `0x0006` | ServiceDiscoveryResponse | ← HU | Encrypted | protobuf `ServiceDiscoveryResponse` | `handleServiceDiscoveryResponse` (`AaCommunicator.cpp:240`) — see `05` |
| `0x0007` | ChannelOpenRequest | (per-channel) | Encrypted | protobuf `ChannelOpenRequest` | **not handled on ch0** — lives on feature channels, see `06-channel-lifecycle.md` |
| `0x0008` | ChannelOpenResponse | (per-channel) | Encrypted | protobuf `ChannelOpenResponse` | **not handled on ch0** — see `06-channel-lifecycle.md` |
| `0x000b` | PingRequest | ← HU | Encrypted | protobuf `PingRequest` { `int64 timestamp = 1` } | `handlePingRequest` (`AaCommunicator.cpp:243`,`252`) |
| `0x000c` | PingResponse | → HU | Encrypted | protobuf `PingResponse` { `int64 timestamp = 1` } | sent by `handlePingRequest` (`AaCommunicator.cpp:265`) |
| `0x000d` | NavigationFocusRequest | → HU | Encrypted | protobuf `NavigationFocusRequest` { `required uint32 type = 1` } — schema ref only | **not parsed by ch0 dispatch**; only the *Response* id is special-cased |
| `0x000e` | NavigationFocusResponse | ← HU | Encrypted | protobuf `NavigationFocusResponse` { `required uint32 type = 1` } | re-routed to channel handler, *not* control, **not parsed** by AACS — see routing rules below (`AaCommunicator.cpp:229`) |
| `0x000f` | ShutdownRequest | — | — | protobuf `ShutdownRequest` { `required ShutdownReason.Enum reason = 1` } (per AAProto) | **NOT in AACS `MessageType`**; defined only in `ControlMessageIdsEnum.proto:40`. No dispatch arm → throws. |
| `0x0010` | ShutdownResponse | — | — | protobuf `ShutdownResponse` { } (empty, per AAProto) | **NOT in AACS `MessageType`**; defined only in `ControlMessageIdsEnum.proto:41`. No dispatch arm → throws. |
| `0x0011` | VoiceSessionRequest | — | — | not parsed (no AAProto schema file) | id defined (`enums.h:34`, also `ControlMessageIdsEnum.proto:42`) but **no dispatch arm** in `handleMessageContent` → falls to "Unhandled message type" throw |
| `0x0012` | AudioFocusRequest | → HU | Encrypted | protobuf `AudioFocusRequest` { `required AudioFocusType.Enum audio_focus_type = 1` } — schema ref only | **not parsed by ch0 dispatch**; only the *Response* id is special-cased |
| `0x0013` | AudioFocusResponse | ← HU | Encrypted | protobuf `AudioFocusResponse` { `required AudioFocusState.Enum audio_focus_state = 1` } | re-routed to channel handler, *not* control, **not parsed** by AACS — see routing rules below (`AaCommunicator.cpp:227`) |

Flagged gaps:
- **`0x0009`, `0x000a`** — no enum value in AACS or AAProto. Unused id space.
- **`0x000f` / `0x0010` (Shutdown*)** — schema exists in AAProto only; AACS has
  no `MessageType` entry and no dispatch arm, so an inbound shutdown on ch0
  hits the final `else` and `throw std::runtime_error("Unhandled message
  type: ...")` (`AaCommunicator.cpp:247`). Flag for `smartcar` if graceful
  teardown is needed.
- **`0x0011` VoiceSessionRequest** — id defined but never dispatched; same
  unhandled-throw fate.
- **`0x000d` NavigationFocusRequest / `0x0012` AudioFocusRequest** — only the
  matching *Response* ids are recognised by the ch0 dispatcher. AACS never
  parses the Request bodies itself; the actual request payload schema for these
  is whatever a channel handler/client produces.

## Message routing rules

Two functions decide how an inbound decrypted message is handled:
`handleMessageContent` (`AaCommunicator.cpp:220`) is the entry point;
`handleChannelMessage` (`AaCommunicator.cpp:150`) is the channel-forwarding
path. The message id is read as `be16` from the first 2 payload bytes
(`AaCommunicator.cpp:223-224`).

Decision order in `handleMessageContent`:

1. **`message.channel != 0`** → `handleChannelMessage` (feature channel; not
   this doc — see `02`/`06`).
2. **channel 0 + id == `AudioFocusResponse` (`0x13`)** → `handleChannelMessage`
   (`AaCommunicator.cpp:227`). Even though it arrives on ch0, it is *not*
   treated as control.
3. **channel 0 + id == `NavigationFocusResponse` (`0x0e`)** →
   `handleChannelMessage` (`AaCommunicator.cpp:229`). Same: ch0 but routed as a
   channel message.
4. **channel 0 + id == `VersionRequest` (`0x01`)** → `handleVersionRequest`.
5. **channel 0 + id == `SslHandshake` (`0x03`)** → `handleSslHandshake`
   (see `04-tls-auth.md`).
6. **channel 0 + id == `AuthComplete` (`0x04`)** → `sendServiceDiscoveryRequest()`
   (no payload parse; presence is the trigger).
7. **channel 0 + id == `ServiceDiscoveryResponse` (`0x06`)** →
   `handleServiceDiscoveryResponse` (see `05-service-discovery.md`).
8. **channel 0 + id == `PingRequest` (`0x0b`)** → `handlePingRequest`.
9. **anything else** → `throw std::runtime_error("Unhandled message type: " + id)`
   (`AaCommunicator.cpp:247`). This is a hard failure, not a silent drop —
   covers Shutdown*, VoiceSessionRequest, and any unknown id on ch0.

Inside `handleChannelMessage` for a **channel-0** message (the focus-response
case, path 2/3 above): it does **not** dispatch to a control handler. Instead it
calls `gotMessage(-1, message.channel, message.flags & Specific, msg)`
(`AaCommunicator.cpp:166`) — i.e. it emits the raw message to local
clients/listeners with `clientId == -1` and the `Specific` flag taken from
header flags bit 2 (`MessageTypeFlags::Specific = 0x04`, `enums.h:18`). For
`channel != 0`, `handleChannelMessage` instead invokes
`channelHandlers[channel]->handleMessageFromHeadunit(message)` and throws
`aa_runtime_error` if the handler reports it unhandled (`AaCommunicator.cpp:156-164`).

Net effect on the control channel:
- Bring-up + ping ids are consumed **internally** by `AaCommunicator`.
- Focus *responses* on ch0 are **passed through** to local clients (the
  projection app decides focus policy), never interpreted by the communicator.
- Everything else on ch0 is a fatal protocol error.

## Bring-up messages that live on ch0

These run once, in order, on channel 0 before any feature channel exists.
Full detail is in the cross-referenced docs; only the ch0-dispatch facts are here.

### Version negotiation (`0x01` / `0x02`)

- **Inbound** `VersionRequest` is **plain** (pre-TLS). Payload is raw, not
  protobuf: two BE `u16`s after the id — `major`, `minor`
  (`handleVersionRequest`, `AaCommunicator.cpp:85-93`).
- Acceptance rule: **only `major == 1` is accepted**. Any other major →
  `throw std::runtime_error("unsupported version")` (`AaCommunicator.cpp:89-92`).
  The inbound `minor` is read but ignored for the accept decision.
- **Outbound** `VersionResponse` (`sendVersionResponse`, `AaCommunicator.cpp:75-83`)
  is built as four BE `u16`s: id `0x0002`, then the server's offered version
  **`(1, 5)`**, then a `matchCode` of **`0`** ("version match", per comment at
  `AaCommunicator.cpp:80`). Sent `Plain | Bulk` on channel 0. The `0` matches
  AAProto's `VersionResponseStatus.Enum`: `MATCH = 0`, `MISMATCH = 0xFFFF`
  (`AAProto/VersionResponseStatusEnum.proto:26-28`) — AACS only ever emits
  `MATCH`, never `MISMATCH` (a bad major throws instead of replying mismatch).
- So regardless of the head unit's requested minor, AACS replies `1.5` with
  match=0. `smartcar` should mirror: accept iff `major==1`, answer `(1,5,0)`.

### SSL handshake (`0x03`)

Plain `SslHandshake` frames carry opaque TLS bytes; `handleSslHandshake`
(`AaCommunicator.cpp:270`) feeds them through OpenSSL memory BIOs in
`SSL_accept` (server) state and replies with more `SslHandshake` frames.
Full flow, cert/key, DH params, `SSL_OP_NO_TLSv1_3`: **`04-tls-auth.md`**.

### AuthComplete (`0x04`)

A bare `AuthComplete` (no body parsed). On receipt the server immediately calls
`sendServiceDiscoveryRequest()` (`AaCommunicator.cpp:237-239`) — this is the
moment the server becomes the active driver. From this point ch0 traffic is
**encrypted**.

### Service discovery (`0x05` / `0x06`)

Server sends `ServiceDiscoveryRequest` (encrypted protobuf, manufacturer
`"TAG"`, model `"AAServer"`; `AaCommunicator.cpp:95-104`). HU replies
`ServiceDiscoveryResponse`; `handleServiceDiscoveryResponse`
(`AaCommunicator.cpp:106-140`) builds the channel-id → handler map. Channel
catalog & oneof details: **`05-service-discovery.md`**.

## Steady-state ch0 traffic

After bring-up, the only control-channel messages are:

### Ping (`0x0b` request ← HU, `0x0c` response → HU)

- The **head unit drives ping**; the server only answers.
  `handlePingRequest` (`AaCommunicator.cpp:252-268`):
  parse inbound `PingRequest` protobuf → take its `timestamp` →
  build `PingResponse` with the **same `timestamp` echoed back** →
  send as id `0x0c`, `Encrypted | Bulk`, channel 0.
- Both protobufs are a single field: `required int64 timestamp = 1`
  (`AAProto/PingRequestMessage.proto:23`, `PingResponseMessage.proto:23`).
  Note these AAProto messages live in package `gb.xxy.trial.proto.messages`;
  AACS uses its own vendored `PingRequest.pb.h`/`PingResponse.pb.h` with the
  same single-field shape.
- AACS does no liveness timeout of its own — it is purely reactive to HU pings.

### Focus messages

- **NavigationFocusResponse (`0x0e`) / AudioFocusResponse (`0x13`)** arrive on
  ch0 but are *forwarded to local clients*, not interpreted (see routing rules).
  Their request counterparts (`0x0d`, `0x12`) are **not** dispatched on ch0 by
  AACS; in practice the requests are produced/consumed by feature-channel logic
  or clients, so AACS treats focus as an opaque pass-through on the control
  channel. Payloads are the AAProto audio/navigation focus protobufs
  (`AudioFocusResponseMessage.proto` etc.) — schema reference only; AACS does
  not parse them on ch0.

## Implementation notes for `smartcar`

- Read the id as BE `u16` from decrypted payload byte 0; switch on it with the
  exact `enums.h` numbering above. Do **not** trust `0x09`/`0x0a` (undefined).
- VersionRequest/Response and SslHandshake are **plain**; everything from
  ServiceDiscoveryRequest onward (incl. ping/focus) is **encrypted**.
- VersionResponse is hand-packed BE `u16`s, not protobuf. Reply `(1,5,0)`,
  accept only `major==1`.
- Focus responses on ch0 are *not* the communicator's concern — surface them to
  the higher layer; don't try to interpret them in the control path.
- Decide explicitly what to do for Shutdown* / VoiceSessionRequest: AACS
  hard-throws. `smartcar` may want graceful handling instead.
