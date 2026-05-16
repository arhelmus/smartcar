# 00 — Overview & Roles

This is the map. It fixes the actor/role vocabulary, sketches the happy-path
bring-up, previews the frame header, and points at where each fact is proven.
Every claim here is the *summary* of a sibling doc (`01`–`12`); those docs are
authoritative on detail and this overview must not contradict them. Conventions
(big-endian, `→ HU`/`← HU`, hex ids, citation style) are in
[`README.md`](README.md).

## Actors

| Actor | AA role | In AACS | In smartcar |
|-------|---------|---------|-------------|
| Head unit (HU) | TLS *client* on the wire; **consumer** of projected UI; the active party *during* version + TLS bring-up | the USB/TCP peer (`openauto`-style) | the `openauto` emulator |
| Projection source ("server" in our docs) | app-layer TLS server-state (`SSL_accept`); **producer** of video/audio; the active party *after* auth | `AAServer` | `smartcar-server` |
| Local client | n/a (AACS-specific) | `AAClient`/`GetEvents` over a Unix `SOCK_SEQPACKET` socket | n/a (smartcar drives its own encoder) |

Terminology trap: at the **TLS** layer the projection source calls
`SSL_accept()` (holds TLS *server*-state) **but the head unit drives the
handshake as the TLS client and presents the client cert** — the server only
reacts (see `04-tls-auth.md`). At the **USB** layer the projection source is
the AOAP *accessory*, not the host (see `01-physical-transport.md`). "Server"
in this folder always means **projection source**, the role we implement; it is
*not* the wire-protocol initiator until `AuthComplete`.

## End-to-end sequence (happy path)

Arrows use the README convention: `→ HU` = server→head unit, `← HU` =
head unit→server. `[plain]`/`[enc]` = frame encryption flag. This is a
condensed spine; the fully annotated, blocking-aware traces are in
`12-sequences.md`.

```
                                         server = projection source

== USB: two personas, AOAP mode switch (ctrl 51/52/53) ==    (01-physical-transport.md)
  · initial gadget 12d1:107e (MassStorage+FFS); HU sends ctrl 51/52/53
  · server replies AOAP proto 2, then re-enumerates as 18d1:2d00 (FFS only)
  · open FunctionFS ep0/ep1/ep2; spawn pumps  — no AA frame until here

← HU  ch0  VersionRequest        [plain]  raw u16 major,minor   (03-control-channel.md)
→ HU  ch0  VersionResponse       [plain]  raw (1, 5, match=0)   accept iff major==1

   == TLS handshake: N request/response round trips, count not fixed ==  (04-tls-auth.md)
← HU  ch0  SslHandshake          [plain]  TLS ClientHello (opaque)
→ HU  ch0  SslHandshake          [plain]  ServerHello / Cert / KeyExch …
            ( feed→SSL_accept(-1/WANT_READ is normal)→drain writeBio→send,
              repeats until SSL_accept succeeds; no explicit "done" frame )
← HU  ch0  SslHandshake          [plain]  client flight …
→ HU  ch0  SslHandshake          [plain]  server flight …

← HU  ch0  AuthComplete          [plain]  no body — presence is the trigger
            *** server becomes the ACTIVE party here ***
→ HU  ch0  ServiceDiscoveryRequest  [enc]  ← FIRST encrypted frame of session
                                            pb{model="AAServer", manufacturer="TAG"}
← HU  ch0  ServiceDiscoveryResponse [enc]  pb{ channels:[ {channel_id, sub} … ] }
            server builds channel_id → handler map     (05-service-discovery.md)

   == per feature channel chN, when first needed (video: eager on first
      encoded sample; input: lazy on first local subscription) ==  (06/07/08, 12)
→ HU  chN  ChannelOpenRequest    [enc, Specific]  pb{channel_id=chN, unknown=0}
            ( server thread BLOCKS on a no-timeout CV until the response )
← HU  chN  ChannelOpenResponse   [enc, Specific]  pb{status} (value not inspected)
→ HU  chN  channel-specific setup [enc]  video: SetupRequest/StartIndication;
← HU  chN  channel-specific resp  [enc]  input: HandshakeRequest/Response
→ HU  chN  channel data           [enc]  video media → HU; input events ← HU

   == steady state ==
← HU  ch0  PingRequest           [enc]  pb{timestamp=T}  (HU drives ping)
→ HU  ch0  PingResponse          [enc]  pb{timestamp=T}  (same T echoed back)
```

Notes (each summarised from the cited sibling doc):

- **Who initiates flips at `AuthComplete`.** The HU is the active party for
  Version + the entire TLS handshake (it sends first, it is the TLS client).
  The server becomes the driver the instant `AuthComplete` arrives and from
  then on initiates Service Discovery and every channel open
  (`03`/`04`/`05`/`12`).
- **The encryption boundary is exact.** Version and *all* `SslHandshake`
  frames are `plain`; `ServiceDiscoveryRequest` is the first `enc` frame and
  everything after stays encrypted (`02-framing.md` §5, `04-tls-auth.md`).
- **TLS is N round trips, not one.** Each `handleSslHandshake` does one
  feed→`SSL_accept`→drain→send cycle; `SSL_accept` returning `-1` with
  `SSL_ERROR_WANT_READ` is the *normal* per-round state. N tracks the TLS 1.2
  flight count, not anything AA-defined (`04-tls-auth.md`).
- **Direction on the input channel inverts.** Open/handshake go `→ HU`, but the
  steady-state data (`InputEvent`) flows `← HU` (the HU is the input device);
  video media flows `→ HU` (`08-input-channel.md`).
- **Channel ids are head-unit-assigned in the Response**, never fixed by spec;
  everything downstream keys off `channel_id` (`05-service-discovery.md`).
- **Ping is HU-driven.** The server never originates pings and has no liveness
  timeout of its own — it only echoes the inbound `timestamp`
  (`03-control-channel.md`).

## Frame anatomy (preview — full detail in `02-framing.md`)

```
 byte:  0        1        2        3       [4    5    6    7]            N
      +--------+--------+--------+--------+----+----+----+----+   ...  +----+
      |channel | flags  |    length (BE)  |     total_len (BE)   | payload  |
      +--------+--------+--------+--------+----+----+----+----+   ...  +----+
        u8       u8        u16                 u32, FIRST frame ONLY

 length    = byte count of THIS frame's payload (ciphertext when encrypted),
             excludes header and total_len
 total_len = full reassembled, post-decrypt payload size; present ONLY on a
             FIRST frame, so the header is 4 bytes normally / 8 on a FIRST frame

 flags byte:   bit 7..4   bit 3        bit 2        bits 1..0
              [ unused ] [ enc 0x08 ] [ spec 0x04 ] [ frame type ]
   frame type (flags & 0x3):  1 = First   2 = Last   3 = Bulk
   Bulk == First|All (== 0x3) is "first AND last" = a single self-contained
   frame; it is NOT a distinct 4th code point. A *true* First is detected by
   masked equality (flags & 0x3) == 1, never a bit test (02-framing.md §2).
```

Caveat to carry forward: AACS's *inbound* parser does **not** branch on frame
type and never skips `total_len` — it assumes single-frame (`Bulk`) inbound and
has no reassembly. `smartcar` must implement real **per-channel** reassembly
(`02-framing.md` §6, `12-sequences.md` §"Implications").

During Version + the TLS handshake, `payload` is plaintext; from
`ServiceDiscoveryRequest` onward it is TLS ciphertext. The **decrypted**
payload begins with a 2-byte big-endian **message id**. That id's namespace
depends on the `channel` byte: on channel `0` it is `MessageType` (control);
on a feature channel it is that channel's enum (e.g. `MediaMessageType`,
`InputChannelMessageType`) — same slot, disambiguated by channel
(`02-framing.md` §5, `03-control-channel.md`, `10-message-catalog.md`).

## Where each fact comes from

AACS is the **behavioural** ground truth; AAProto is the **schema** reference;
aasdk is the **breadth/cross-check** source (per `README.md`). Paths below are
relative to the AACS checkout root (cloned at `/tmp/aa_investigate/AACS`).

- Bring-up order, framing, TLS, fragmentation, dispatch: `AAServer/src/AaCommunicator.cpp`.
- Message-id / flag enum values: `include/enums.h` — note this is at the
  **checkout root** `include/`, *not* `AAServer/include/` (the docs sometimes
  write `AAServer/include/enums.h` as a logical name; the physical file is
  `AACS/include/enums.h`).
- Generic channel lifecycle (open CV, blocking model): `AAServer/src/ChannelHandler.cpp` (`06-channel-lifecycle.md`).
- Channel handler state machines: `AAServer/src/{VideoChannelHandler,InputChannelHandler,DefaultChannelHandler}.cpp`.
- USB mode switch & descriptors: `AAServer/src/{ModeSwitcher,descriptors}.cpp`.
- End-to-end **ordering / blocking / who-initiates** is owned by `12-sequences.md`,
  cross-checked against `AaCommunicator.cpp` + `AAServer/main.cpp`.
- AACS local client socket protocol (protocol B): `AAServer/main.cpp`,
  `AAServer/src/{SocketClient,SocketCommunicator}.cpp`,
  `AAServer/include/{Packet,PacketType,ChannelType}.h` (`11-aacs-client-socket.md`).
- Protobuf schemas & enum numbering: `AAProto/*.proto` (schema), cross-checked
  against AACS's vendored `proto/*.proto` (what AACS actually decodes; field
  numbers agree, some field *names* differ — see `05`/`10`).
- Full channel catalog & message-id enums for channels AACS doesn't implement: `aasdk/include/aasdk/Channel/**`.

## Glossary

- **Projection source** — the role this folder calls "server": the side that
  produces video/audio and drives the session after auth. `AAServer` in AACS,
  `smartcar-server` here. AOAP *accessory* on USB, TLS *server*-state at the
  app layer, but **not** the wire initiator until `AuthComplete`.
- **Head unit (HU)** — the wire peer that consumes the projection; TLS client;
  active party during Version + TLS bring-up; assigns channel ids.
- **AOAP** — Android Open Accessory Protocol; the USB mode the phone enters so
  it acts as an *accessory* to the head-unit host (`01-physical-transport.md`).
- **Control channel** — channel `0`; carries version, TLS, auth, service
  discovery, ping, focus. Has no open/close lifecycle (`03-control-channel.md`).
- **Channel** — a numbered logical stream for one feature (video, input, …),
  whose id is **assigned by the head unit in Service Discovery**
  (`05-service-discovery.md`).
- **Service Discovery** — the one `ServiceDiscoveryRequest`/`Response` exchange
  on ch0 that enumerates channels; the first encrypted traffic of the session.
- **AuthComplete** — bare ch0 message (id `0x04`) from the HU after the TLS
  handshake; its arrival flips the server to active party (`04-tls-auth.md`).
- **Frame** — one wire unit: `[channel][flags][length BE]([total_len BE] on a
  FIRST frame)[payload]` (`02-framing.md`).
- **Message** — a logical payload that may span one or more frames; after
  decryption it starts with a 2-byte BE message id.
- **Message id** — first 2 bytes (BE) of a *decrypted* payload; namespace is
  `MessageType` on ch0, per-channel enum on a feature channel.
- **`EncryptionType`** — flags bit 3 (`0x08`): `Plain`/`Encrypted`
  (`enums.h`).
- **`MessageTypeFlags` / Specific flag** — flags bit 2 (`0x04`): marks a
  "channel-specific" vs. control-style message within a channel; `Control = 0`.
- **`FrameType`** — flags bits 0–1: `First = 1`, `Last = 2`,
  `Bulk = First|Last = 3`. `Bulk` is a single self-contained frame, **not** a
  distinct 4th value; a true First is `(flags & 0x3) == 1`.
- **`total_len`** — 4-byte BE size of the fully reassembled, *post-decrypt*
  message; present **only** on a FIRST frame.
- **Reassembly** — joining `First` + intermediates + `Last` (per channel) back
  into one message. AACS's inbound path does **not** do this (assumes `Bulk`);
  `smartcar` must (`02-framing.md` §6).
- **`ChannelHandler`** — AACS base class owning the generic blocking
  open round-trip; `DefaultChannelHandler` is the pass-through used for ch0 and
  any channel without a feature handler (`06-channel-lifecycle.md`).
- **AACS local client** — an out-of-process consumer (`AAClient`/`GetEvents`)
  that drives AA channels over AACS's Unix `SOCK_SEQPACKET` socket
  ("protocol B"); AACS-specific, not part of the AA wire protocol
  (`11-aacs-client-socket.md`).

## See also

- `01-physical-transport.md` — USB AOAP mode switch, FunctionFS endpoints, TCP variant.
- `02-framing.md` — authoritative frame header, flag bits, fragmentation/`total_len`.
- `03-control-channel.md` — ch0 message-id namespace, version, ping, routing.
- `04-tls-auth.md` — `SSL_accept` BIO pump, the TLS round trips, `AuthComplete`.
- `05-service-discovery.md` — `ServiceDiscovery*`, channel catalog, handler map.
- `06-channel-lifecycle.md` — generic open round-trip and blocking model.
- `07-video-channel.md` / `08-input-channel.md` — the two channels AACS implements.
- `12-sequences.md` — authoritative end-to-end ordering, blocking, who-initiates.
- `README.md` — conventions, source-material table, doc plan.
