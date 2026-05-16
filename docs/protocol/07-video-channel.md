# 07 — Video (AV sink) Channel

The video channel is the projected display: the **server** (projection
source / producer) opens it, negotiates a setup, waits for the head unit to
grant video focus, then pumps a continuous H264 byte-stream to the head unit
which renders it full-screen.

This is one of only two channels with a real server-side reference
implementation (`AAServer/src/VideoChannelHandler.cpp`), so this doc is
high-fidelity. Where AACS hand-builds raw payload bytes instead of using the
protobuf, both the raw bytes **and** the schema they encode are given, with the
distinction called out explicitly.

Prereqs: [05 — service discovery](05-service-discovery.md) (this channel's id
and `MediaChannel`/`VideoConfig` come from the `ServiceDiscoveryResponse`
catalog), [06 — channel lifecycle](06-channel-lifecycle.md) (the generic
`ChannelOpenRequest`/`Response` this layers on; behavioural reference is
`AAServer/src/ChannelHandler.cpp`), and
[02 — framing](02-framing.md) (frame header, fragmentation, `total_len`).

Conventions: multi-byte ints are **big-endian**; "→ HU" = server to head unit,
"← HU" = head unit to server; message ids are the first 2 BE bytes of the
decrypted payload; `MediaMessageType` values are from
`AACS/include/enums.h:39-47`.

---

## 1. Channel identity & negotiated config (recap from 05)

The video channel is **not** channel 0. Its integer id is assigned by the head
unit in the `ServiceDiscoveryResponse`: AACS finds the `MediaChannel` whose
`media_type == Video (3)` and binds that `channel_id` to a
`VideoChannelHandler` (`AAServer/src/AaCommunicator.cpp:116-118`). All ids and
flags below are relative to that channel number; this doc writes it `chN`.

The negotiated capability set lives in service discovery, not in this channel:

- `MediaChannel` (`AACS/proto/MediaChannel.proto`) — `media_type` =
  `MediaStreamType.Video` = `3`; carries `repeated VideoConfig video_configs`.
- `VideoConfig` (`AACS/proto/VideoConfig.proto`) — per-config:
  `video_resolution` (`VideoResolution.Enum`: `H480=1`, `H720=2`, `H1080=3`),
  `video_fps` (`VideoFps.Enum`: `F30=1`, `F60=2`), `margin_width`,
  `margin_height`, `dpi`, optional `additional_depth`. Each entry's **index**
  in `video_configs` is the `config_index` referenced by Setup Request /
  Setup Response / Start Indication below.

aasdk's mirror schemas (`server/third_party/AAProto/`) are identical modulo
package/naming: `data.VideoConfig` (`VideoConfigData.proto`),
`enums.VideoResolution` (`_480p=1`, `_720p=2`, `_1080p=3`),
`enums.VideoFPS` (`_30=1`, `_60=2`), `enums.AVStreamType.VIDEO=3`.

> **Protocol vs. AACS encoder.** What the *protocol* carries is a chosen
> `config_index` into the negotiated `video_configs`. What AACS's gstreamer
> pipeline *happens to* produce is a fixed **800×480, baseline-profile,
> byte-stream H264 @ 30fps** stream (`x264enc` with `speed-preset=1`,
> `key-int-max=25`; caps `video/x-h264, stream-format=byte-stream,
> profile=baseline, width=800, height=480, framerate=30/1`,
> `VideoChannelHandler.cpp:73-81`). That resolution/codec is an AACS
> implementation choice, **not** a protocol constant — a conformant source
> must emit whatever matches the `VideoConfig` it selects. AACS does not even
> read the negotiated config back; it always sends `config_index = 3` (see
> §3) and always encodes 800×480.

---

## 2. Lifecycle state machine

The video channel layers a setup/start sub-handshake on top of the generic
channel open. The server is the active party throughout.

```
state: (constructed)
   |  pipeline → PLAYING; pre-open frames from HU are passed through
   |  to the AACS local client untouched (VideoChannelHandler.cpp:163-169)
   v
state: FIRST SAMPLE
   |  first encoded H264 sample from gstreamer triggers openChannel()
   |  (VideoChannelHandler.cpp:30-32)
   v
   |  → chN  ChannelOpenRequest         (generic, msg id 0x0007, Specific)
   |  ← chN  ChannelOpenResponse        (generic, msg id 0x0008)  ── blocks
   v
state: CHANNEL OPEN
   |  → chN  SetupRequest      0x8000  payload 08 03
   |  ← chN  SetupResponse     0x8003               ── blocks
   v
state: SETUP DONE
   |  ← chN  VideoFocusIndication 0x8008  (HU grants focus)
   |  → chN  StartIndication   0x8001  payload 08 00 10 00
   v
state: STREAMING
   |  → chN  MediaWithTimestampIndication 0x0000  (pts present)  ┐ repeats
   |  → chN  MediaIndication            0x0001  (pts == -1)      │ per
   |  ← chN  MediaAckIndication         0x8004  (HU flow ack)    ┘ frame
   v
  (steady state until teardown)
```

Two synchronous blocking points are enforced with a mutex+condvar
(`VideoChannelHandler.h:9-11`): `expectSetupResponse()` blocks the
sample-producing thread until `SetupResponse` arrives
(`VideoChannelHandler.cpp:147-150`); the generic
`expectChannelOpenResponse()` blocks until `ChannelOpenResponse`
(`ChannelHandler.cpp:48-51`). So the first encoded frame is not transmitted
as media until open **and** setup have both completed.

> **Pre-open pass-through.** Until `channelOpened` is set true, *any* message
> from the head unit on this channel is forwarded verbatim to the AACS local
> client and consumed (`VideoChannelHandler.cpp:163-169`). This is AACS
> client-fan-out plumbing, not AA protocol — a standalone source can ignore
> it. `channelOpened` flips true at the **top** of `openChannel()`
> (`VideoChannelHandler.cpp:127-133`) so it is already set before the generic
> open round-trip runs.

---

## 3. Message-id table (channel `chN`)

`MediaMessageType` (`AACS/include/enums.h:39-47`); identical numbering in
aasdk `ids.AVChannelMessage` (`AVChannelMessageIdsEnum.proto`). Generic
open/close ids are `MessageType` (`enums.h:21-37`), namespaced by channel.
Bit `0x8000` set ⇒ "channel control" message; clear ⇒ media payload.

| Hex id | Name | Dir | Frame flags | Payload (AACS) | Schema it encodes |
|--------|------|-----|-------------|----------------|-------------------|
| `0x0007` | ChannelOpenRequest | → HU | `Bulk\|Enc\|Specific` | protobuf `ChannelOpenRequest{field1=0, channel_id=chN}` | `ChannelOpenRequest.proto` (§4.1) |
| `0x0008` | ChannelOpenResponse | ← HU | — | (status; AACS only checks arrival) | `ChannelOpenResponse.proto` |
| `0x8000` | SetupRequest | → HU | `Bulk\|Enc` (not Specific) | **hardcoded** `08 03` | `AVChannelSetupRequest{config_index}` |
| `0x8003` | SetupResponse | ← HU | — | not parsed by AACS | `AVChannelSetupResponse{media_status, max_unacked, configs[]}` |
| `0x8008` | VideoFocusIndication | ← HU | — | not parsed by AACS | `VideoFocusIndication{focus_mode, unrequested}` |
| `0x8001` | StartIndication | → HU | `Bulk\|Enc` (not Specific) | **hardcoded** `08 00 10 00` | `AVChannelStartIndication{session, config}` |
| `0x0000` | MediaWithTimestampIndication | → HU | `Bulk\|Enc` | `[8B BE pts/1000][raw H264 bytes]` | (header is hand-built, not protobuf) |
| `0x0001` | MediaIndication | → HU | `Bulk\|Enc` | `[raw H264 bytes]` (no timestamp) | (hand-built) |
| `0x8004` | MediaAckIndication | ← HU | — | consumed, not parsed | `AVMediaAckIndication{session, value}` |
| `0x8002` | StopIndication | (← HU) | — | not handled by AACS | (aasdk `STOP_INDICATION`) |
| `0x8007` | VideoFocusRequest | (→ HU) | — | not sent by AACS | (aasdk `VIDEO_FOCUS_REQUEST`) |
| `0x8005`/`0x8006` | AVInputOpen Req/Resp | n/a | — | not used on video sink | (aasdk; touch input is its own channel — see 08) |

AACS only ever **sends** `0x0007`, `0x8000`, `0x8001`, `0x0000`, `0x0001`,
and only **reacts to** `0x0008`, `0x8003`, `0x8008`, `0x8004` on this channel
(`VideoChannelHandler.cpp:163-190`, `ChannelHandler.cpp:18-32`). `0x8002`,
`0x8007`, `0x8005/6` are listed for completeness from aasdk's enum
(`AVChannelMessageIdsEnum.proto`) and are not exercised by the reference
server.

Frame-flag names: `Bulk = First|Last = 0x03`, `Enc(rypted) = 0x08`,
`Specific = 0x04` (`enums.h:5-19`). Note `SetupRequest`/`StartIndication` are
sent **without** the `Specific` bit, whereas the generic `ChannelOpenRequest`
**sets** it (`ChannelHandler.cpp:42-45`). See [02](02-framing.md) for the
header layout and the `total_len` rule.

---

## 4. Message details

### 4.1 ChannelOpenRequest / Response (generic, via 06)

`VideoChannelHandler::openChannel()` first calls
`ChannelHandler::openChannel()` (`VideoChannelHandler.cpp:127-133`), which
sends a real protobuf `ChannelOpenRequest` (`channel_id` = this channel,
field 1 `= 0`) prefixed with BE id `0x0007`, flags
`Bulk|Encrypted|Specific`, then blocks until any `0x0008`
`ChannelOpenResponse` arrives (`ChannelHandler.cpp:34-51`). AACS does **not**
inspect the response status — mere arrival unblocks it. See
[06](06-channel-lifecycle.md) for the generic semantics.

> **Field-1 name.** AACS's `AACS/proto/ChannelOpenRequest.proto` names the
> message `{ unknown_field:int32=1, channel_id:int32=2 }` and AACS calls
> `set_unknown_field(0)`. The canonical aasdk schema
> (`AAProto/ChannelOpenRequestMessage.proto`) names field 1 `priority` (and
> orders it `priority=1, channel_id=2`). Same wire bytes; differing field
> names — see [06](06-channel-lifecycle.md).

### 4.2 SetupRequest — `0x8000` → HU

`sendSetupRequest()` (`VideoChannelHandler.cpp:138-145`):

```
plainMsg = [80 00]  [08 03]
            \____/    \____/
            msg id    hand-built body
flags = Bulk | Encrypted        (NOT Specific)
```

The body `08 03` is **hardcoded bytes**, not a serialized protobuf. Decoded as
protobuf it is exactly `AVChannelSetupRequest{ config_index = 3 }`
(`AVChannelSetupRequestMessage.proto`): wire tag `0x08` = field 1, varint;
value `0x03`. So AACS unconditionally requests **config index 3** and never
consults the negotiated `video_configs`. A conformant source should serialize
`AVChannelSetupRequest` properly with the index of the `VideoConfig` it can
actually produce.

### 4.3 SetupResponse — `0x8003` ← HU

Schema `AVChannelSetupResponse{ media_status:AVChannelSetupStatus,
max_unacked, repeated configs }` (`AVChannelSetupResponseMessage.proto`;
`AVChannelSetupStatus`: `NONE=0`, `FAIL=1`, `OK=2`). AACS **does not parse
it** — `handleMessageFromHeadunit` only checks the id equals
`SetupResponse`, sets `gotSetupResponse=true`, and notifies the condvar
(`VideoChannelHandler.cpp:178-180,188`). This unblocks `expectSetupResponse()`
and lets the channel proceed to streaming. The AACS legacy
`MediaChannelSetupResponse.proto` (`unknown_field_1..3`) is the same message
with unnamed fields; prefer the aasdk schema.

A correct source should check `media_status == OK` and honour `max_unacked`
for flow control; AACS does neither.

### 4.4 VideoFocusIndication — `0x8008` ← HU → StartIndication — `0x8001` → HU

The head unit grants/revokes the on-screen video focus by sending
`VideoFocusIndication{ focus_mode:VideoFocusMode, unrequested:bool }`
(`VideoFocusIndicationMessage.proto`; `VideoFocusMode`: `NONE=0`,
`FOCUSED=1`, `UNFOCUSED=2`). AACS does not parse the body; on **any**
`0x8008` it immediately calls `sendStartIndication()`
(`VideoChannelHandler.cpp:181-183`) — it does not distinguish
focused/unfocused. A robust source should only start on `FOCUSED` and stop
the stream on `UNFOCUSED`.

`sendStartIndication()` (`VideoChannelHandler.cpp:152-161`):

```
plainMsg = [80 01]  [08 00 10 00]
            \____/    \_________/
            msg id    hand-built body
flags = Bulk | Encrypted        (NOT Specific)
```

`08 00 10 00` is again **hardcoded**, not protobuf-serialized. Decoded as
`AVChannelStartIndication` (`AVChannelStartIndicationMessage.proto`):
- `08 00` → field 1 (`session`) varint = `0`
- `10 00` → field 2 (`config`) varint = `0`

So AACS reports session `0` and config `0`. (Note the asymmetry: SetupRequest
hardcoded `config_index=3` but StartIndication hardcodes `config=0`; a real
source should send the session id the HU expects and the same config it set
up.)

### 4.5 Media frames — `0x0000` / `0x0001` → HU

Every encoded H264 sample pulled from the gstreamer `appsink` is shipped as
one media message (`new_sample`, `VideoChannelHandler.cpp:18-51`):

```
if buffer.pts == -1:
    msg = [00 01] [ raw H264 bytes ... ]                 # MediaIndication
else:
    msg = [00 00] [ 8B BE (pts/1000) ] [ raw H264 ... ]  # MediaWithTimestampIndication
flags = Encrypted | Bulk
```

- **Id** is 2 BE bytes (`pushBackInt16`, `enums.h` ids `0x0000`/`0x0001`).
- **Timestamp** (only for `0x0000`): `pushBackInt64(msg, buffer->pts / 1000)`
  (`VideoChannelHandler.cpp:38`) — a **64-bit big-endian** integer of the
  gstreamer PTS in **nanoseconds divided by 1000**, i.e. **microseconds**.
  There is no protobuf here; the 8-byte timestamp is the literal payload
  prefix immediately after the id, followed by the codec bytes.
- **Payload**: the raw H264 buffer copied verbatim
  (`VideoChannelHandler.cpp:40-43`) — Annex-B **byte-stream** format
  (NAL start codes), baseline profile, as produced by `x264enc`
  (`VideoChannelHandler.cpp:73-81`). The first frame(s) carry SPS/PPS because
  the pipeline's caps are `stream-format=byte-stream`.
- **Frame split**: a sample larger than ~2000 bytes (the transport's
  `maxSize`, `AaCommunicator.cpp:372`) is fragmented across multiple wire
  frames by the transport — the first frame gets `First`, carries the 4-byte
  BE `total_len` of the **plaintext** message, and the rest are
  intermediate/`Last` (`AaCommunicator.cpp:383-438`). The application always
  hands the transport a single `Bulk` message; reassembly is the framing
  layer's job — see [02](02-framing.md). (`total_len` is the plaintext size;
  each frame's 2-byte `length` is its **ciphertext** size.)

`pts == -1` (unknown timestamp) selects the no-timestamp `MediaIndication`
variant. In practice the AACS pipeline timestamps buffers
(`do-timestamp=TRUE` on the source), so `0x0000` is the common path and
`0x0001` is the fallback.

The very first sample (`firstSample` static decl
`VideoChannelHandler.cpp:20`; the `if (firstSample) openChannel()` guard at
`VideoChannelHandler.cpp:30-32`) triggers `openChannel()` *before* the message
is built — so channel open + setup happen lazily on first encoded frame, and
that sample is then sent only after both blocking waits return. Note
`firstSample` is reset *after* the send (`VideoChannelHandler.cpp:49`), so the
first encoded buffer is itself transmitted as the first media frame once the
two blocking waits return.

### 4.6 MediaAckIndication — `0x8004` ← HU

`AVMediaAckIndication{ session:int32, value:uint32 }`
(`AVMediaAckIndicationMessage.proto`) is the head unit's flow-control /
delivery acknowledgement. AACS recognises the id and marks the message
handled, but **does not parse or act on it** — it does not throttle on
`max_unacked` (`VideoChannelHandler.cpp:184-186`). A real source should track
acked counts against the `max_unacked` from SetupResponse and stop sending
when the window is exhausted.

---

## 5. Annotated sequence diagram

Arrows are head-unit's POV; the server is the right column and the active
party. `chN` = negotiated video channel id (from service discovery, §1).

```
Head Unit (HU)                         Projection Source (server)
     |                                          |
     |        [video channel id chN already known from ServiceDiscoveryResponse — see 05]
     |                                          |
     |                                          |  gst pipeline PLAYING; encodes
     |                                          |  800x480 baseline H264 @30 (AACS choice)
     |                                          |
     |                                          |  ── first encoded sample ──
     |                                          |     new_sample() → openChannel()
     |                                          |
     |  <-- chN ChannelOpenRequest (0x0007) ----|  protobuf {channel_id, unknown=0}
     |          flags Bulk|Enc|Specific         |  (generic, see 06)
     |  --- chN ChannelOpenResponse (0x0008) -->|  ── server blocked until this arrives
     |                                          |     (status not inspected by AACS)
     |                                          |
     |  <-- chN SetupRequest (0x8000) ----------|  body 08 03  == AVChannelSetupRequest
     |          flags Bulk|Enc (no Specific)    |  {config_index = 3}  (hardcoded)
     |  --- chN SetupResponse (0x8003) -------->|  AVChannelSetupResponse{media_status,
     |                                          |  max_unacked, configs}; AACS only
     |                                          |  checks arrival → unblocks setup
     |                                          |
     |  --- chN VideoFocusIndication (0x8008) ->|  {focus_mode, unrequested};
     |                                          |  AACS ignores body, always starts
     |  <-- chN StartIndication (0x8001) -------|  body 08 00 10 00 ==
     |          flags Bulk|Enc (no Specific)    |  AVChannelStartIndication{session=0,
     |                                          |  config=0}  (hardcoded)
     |                                          |
     |                == STREAMING (repeats per encoded frame) ==
     |  <-- chN MediaWithTimestampIndication ---|  [00 00][8B BE pts/1000 µs]
     |          (0x0000) flags Bulk|Enc         |  [raw Annex-B H264 ...]
     |  <-- chN MediaIndication (0x0001) -------|  [00 01][raw H264]  (only if pts==-1)
     |          (large frames fragmented by transport; First frame carries
     |           4B BE total_len of plaintext — see 02)
     |  --- chN MediaAckIndication (0x8004) --->|  {session, value}; recognised,
     |                                          |  not acted on (no flow throttle)
     |  <-- ...                          -------|  ... continues ...
     |                                          |
```

---

## 6. Conformance notes (where AACS cuts corners)

A correct projection source differs from the AACS reference in these ways
(all attributable to `VideoChannelHandler.cpp`):

1. **Setup body is hardcoded `08 03`.** Serialize a real
   `AVChannelSetupRequest` with the `config_index` of a `VideoConfig` you can
   actually encode, instead of always 3.
2. **SetupResponse is not parsed.** Check `media_status == OK`; capture
   `max_unacked` and `configs[]`.
3. **Video focus is not honoured.** Only start on `focus_mode == FOCUSED`;
   stop/pause encoding on `UNFOCUSED`.
4. **StartIndication body is hardcoded `08 00 10 00`** (`session=0,
   config=0`). Echo the negotiated `config` and a correct `session`.
5. **MediaAck is ignored.** Implement the `max_unacked` send window so the
   head unit's decoder is not overrun.
6. **Resolution/codec are fixed at 800×480 baseline H264 @30** by the
   gstreamer pipeline. The protocol carries whatever the selected
   `VideoConfig` specifies — match the negotiated resolution/fps/dpi.
7. **Lazy open on first sample** is an AACS design choice, not a protocol
   requirement; a source may open/setup eagerly after service discovery.

Items 1–5 are protocol-relevant correctness gaps; 6–7 are AACS
implementation choices, not protocol constraints.
