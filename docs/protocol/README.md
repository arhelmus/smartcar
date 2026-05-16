# Android Auto Protocol — Server-Side Spec

Reverse-engineered documentation of the Android Auto (AA) wire protocol, written
from the perspective of the **projection-source / "server" role** — the side
that drives the head unit. This is the role `smartcar` implements, and the role
[AACS](https://github.com/tomasz-grobelny/AACS) (`AAServer`) implements in C++.

## Source material

| Repo | Role | What we mine it for |
|------|------|---------------------|
| [AACS](https://github.com/tomasz-grobelny/AACS) | server (`AAServer`) | authoritative server-side flow: handshake, framing, channel mgmt, client socket API |
| [aasdk](https://github.com/opencardev/aasdk) | client lib | cross-check framing/encryption, message-id enums, full channel catalog |
| [AAProto](https://github.com/opencardev/AAProto) | protocol | canonical protobuf schemas + enum values |

> Note on "server": in AACS the *head unit* is the TCP/USB peer; `AAServer`
> speaks the phone (Android Auto) side and re-exports it over a local Unix
> socket to its own *clients*. Two distinct protocols are in play and both are
> documented here: **(A)** the AA wire protocol with the head unit, and **(B)**
> the AACS local client socket protocol.

## How the conversation works (one-paragraph summary)

The phone enumerates as a USB Android Open Accessory (AOAP) device (or a TCP
peer). Over endpoints 1 (IN) / 2 (OUT) it exchanges length-prefixed **frames**:
`[channel:1][flags:1][len:2 BE]([total_len:4 BE] only on a multi-frame first
frame)][payload]`. Channel `0` is the **control channel**. The bring-up
sequence on channel 0 is: Version Request/Response → TLS handshake (server =
`SSL_accept`, phone presents the AA cert) → Auth Complete → Service Discovery
Request/Response. Service Discovery enumerates per-feature **channels** (video,
input, media, sensors, …), each with an integer id. The server then drives each
channel's open/setup sub-handshake and pumps media/input messages. All
post-handshake payloads are TLS-encrypted; the first 2 bytes of a decrypted
payload are a big-endian **message id**.

## Documentation plan — areas to cover

Each area becomes one numbered markdown file. Status: ✅ drafted · ☐ todo.

| # | File | Area | Scope |
|---|------|------|-------|
| 00 | `00-overview.md` | ✅ Overview & roles | actors, end-to-end sequence diagram, glossary |
| 01 | `01-physical-transport.md` | ✅ Physical transport | USB AOAP mode-switch (vid/pid `18d1:2d00`, control reqs 51/52/53), FunctionFS endpoints, TCP variant |
| 02 | `02-framing.md` | ✅ Frame format | header layout, `EncryptionType`/`FrameType`/`MessageTypeFlags` bits, multi-frame fragmentation & reassembly, `total_len` rule, max frame size |
| 03 | `03-control-channel.md` | ✅ Control channel (ch 0) | message-id enum, Version Request/Response, ping, focus messages, message routing rules |
| 04 | `04-tls-auth.md` | ✅ TLS & auth | server `SSL_accept` flow, SSLv23 + no-TLS1.3 + DH params, cert/key, `AuthComplete`, encrypt/decrypt over memory BIOs |
| 05 | `05-service-discovery.md` | ✅ Service discovery | `ServiceDiscoveryRequest/Response` protobuf, `Channel` oneof catalog, channel-id → handler mapping |
| 06 | `06-channel-lifecycle.md` | ✅ Generic channel lifecycle | `ChannelOpenRequest/Response`, the per-channel open → setup → start state machine, ack/`Specific` flag |
| 07 | `07-video-channel.md` | ✅ Video (AV sink) channel | `MediaChannel`/`VideoConfig`, Setup Request/Response, Video Focus, Start Indication, Media(WithTimestamp) Indication, MediaAck |
| 08 | `08-input-channel.md` | ✅ Input channel | `InputChannel` available buttons, Handshake Request/Response, `InputEvent`/`TouchEvent`/`ButtonsEvent`, registration model |
| 09 | `09-audio-sensor-other.md` | ✅ Other channels | audio sink/source, sensor, bluetooth, navigation, media-status, vendor-extension (catalog from aasdk; depth as needed) |
| 10 | `10-message-catalog.md` | ✅ Message & enum catalog | exhaustive message-id ↔ protobuf ↔ direction table, generated/cross-checked against AAProto |
| 11 | `11-aacs-client-socket.md` | ✅ AACS local client API | Unix `SOCK_SEQPACKET` socket, `PacketType` (GetChannelNumberByChannelType / RawData / GetServiceDescriptor), wire layout, client/headunit fan-out |
| 12 | `12-sequences.md` | ✅ End-to-end sequences | full annotated boot trace, video bring-up, input event path, error/teardown |

### Suggested order of work
1. **02, 03, 04, 05, 06** — the mandatory bring-up path (nothing works without these).
2. **07, 08** — the two channels AACS actually implements (highest fidelity available).
3. **01, 11, 12** — environment & integration glue.
4. **09, 10** — breadth/reference, largely mined from aasdk + AAProto.

## Conventions used in these docs
- Multi-byte integers on the wire are **big-endian** unless stated.
- "→ HU" = source/server to head unit; "← HU" = head unit to server.
- Message ids and enum values are given in hex as they appear on the wire.
- Code references point at AACS (`AAServer/...`) as the behavioural reference
  and AAProto (`*.proto`) as the schema reference.
