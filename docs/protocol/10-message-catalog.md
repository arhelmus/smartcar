# 10 — Message & Enum Catalog

The single cross-referenced reference for every Android Auto **message id**,
its **protobuf**, **direction**, and the **value enums** they carry. This doc
**aggregates** the tables introduced by the per-area docs (03–09); it does
**not** re-explain flows — follow the area-doc links for behaviour, state
machines, and the AACS reference-implementation quirks.

Authority for raw numbering: `AACS/include/enums.h` (behavioural / what AACS
actually dispatches) and the canonical `server/third_party/AAProto/*Enum.proto`
/ `*MessageIdsEnum.proto` schemas. **Where the prose area docs and the raw enum
files disagree, the raw enum file wins and the discrepancy is noted inline.**

---

## 1. How to read this catalog

A decrypted payload always begins with a **2-byte big-endian message id**
(`be16_to_cpu(payload[0..2])`, `AaCommunicator.cpp:153,:224`; see
[`02-framing.md`](02-framing.md) §5). That id is **not globally unique** — its
namespace is selected by **two** pieces of frame context:

1. **`channel` byte** (frame header byte 0). Channel `0` = the **control
   channel** → ids are `MessageType` (`enums.h:21-37`). Any other channel = a
   feature channel → ids are that channel's per-feature enum (AV / Input /
   Sensor / Bluetooth …).
2. The **`Specific` flag** (frame flags bit 2, `0x04`) distinguishes a
   channel-specific message from a control-style message *within* a feature
   channel; it does **not** change the id's numeric value but is part of how
   AACS frames generic vs. channel-specific traffic (e.g. `ChannelOpenRequest`
   is sent `Specific`, video `SetupRequest` is **not**). See
   [`02-framing.md`](02-framing.md) §2 and [`06-channel-lifecycle.md`](06-channel-lifecycle.md).

Consequences worth internalising:

- The **same** numeric id means different things per namespace. `0x8001` is
  `StartIndication` on an AV channel, `Event` on an input channel,
  `SENSOR_START_REQUEST` on a sensor channel, `PAIRING_REQUEST` on bluetooth.
- The generic open ids `0x0007`/`0x0008` ride the **feature channel's** byte
  (not ch0) even though they share numbering with the control `MessageType`
  enum — see [`06-channel-lifecycle.md`](06-channel-lifecycle.md).
- The high bit `0x8000` of an AV/Input id conventionally marks a "channel
  control" message; clear = a media/data payload
  ([`07-video-channel.md`](07-video-channel.md) §3).

Direction convention (from [`00-overview.md`](00-overview.md)): **→ HU** =
projection source (smartcar / `AAServer`) → head unit; **← HU** = head unit →
source; **↔ HU** = both. All ids hex, all multi-byte ints big-endian.

"AACS support" column legend:
- **handled** — `AAServer` parses/acts on it with dedicated logic.
- **arrival-only** — id recognised, unblocks a condvar, body **not** parsed.
- **pass-through** — relayed verbatim to/from AACS local clients
  (`DefaultChannelHandler`); zero protocol semantics.
- **throws** — hits the final `else` in `handleMessageContent` →
  `throw std::runtime_error("Unhandled message type")` (`AaCommunicator.cpp:246`).
- **n/a (aasdk-only)** — id exists only in aasdk/AAProto; AACS has no enum
  entry and never produces or consumes it.

---

## 2. Master message-id table

### 2.1 Control channel (channel `0` — `MessageType`)

Source: `AACS/include/enums.h:21-37`; cross-check
`AAProto/ControlMessageIdsEnum.proto` (`ControlMessage.Enum`). Routing rules &
behaviour: [`03-control-channel.md`](03-control-channel.md).

| Id | Name (AACS) / canonical | Dir | Enc | Protobuf (AAProto) | AACS support | Doc |
|----|-------------------------|-----|-----|--------------------|--------------|-----|
| `0x0001` | `VersionRequest` / `VERSION_REQUEST` | ← HU | Plain | **raw**: `u16 major`, `u16 minor` (not protobuf) | handled (`AaCommunicator.cpp:85`) | [03](03-control-channel.md) |
| `0x0002` | `VersionResponse` / `VERSION_RESPONSE` | → HU | Plain | **raw**: `u16 major`,`u16 minor`,`u16 matchCode` | handled (`:75`) | [03](03-control-channel.md) |
| `0x0003` | `SslHandshake` / `SSL_HANDSHAKE` | ↔ HU | Plain | **raw**: opaque TLS bytes | handled (`:270`) | [03](03-control-channel.md) / [04](04-tls-auth.md) |
| `0x0004` | `AuthComplete` / `AUTH_COMPLETE` | ← HU | Plain | **none** (presence-only trigger) | handled — triggers SvcDisc (`:237`) | [03](03-control-channel.md) |
| `0x0005` | `ServiceDiscoveryRequest` / `SERVICE_DISCOVERY_REQUEST` | → HU | Enc | `ServiceDiscoveryRequestMessage.proto` (AACS: `ServiceDiscoveryRequest.proto`) | handled (`:95`) | [05](05-service-discovery.md) |
| `0x0006` | `ServiceDiscoveryResponse` / `SERVICE_DISCOVERY_RESPONSE` | ← HU | Enc | `ServiceDiscoveryResponseMessage.proto` | handled (`:240`) | [05](05-service-discovery.md) |
| `0x0007` | `ChannelOpenRequest` / `CHANNEL_OPEN_REQUEST` | (per-channel) | Enc | `ChannelOpenRequestMessage.proto` | **not on ch0** — feature channels only | [06](06-channel-lifecycle.md) |
| `0x0008` | `ChannelOpenResponse` / `CHANNEL_OPEN_RESPONSE` | (per-channel) | Enc | `ChannelOpenResponseMessage.proto` | **not on ch0** — feature channels only | [06](06-channel-lifecycle.md) |
| `0x0009` | — undefined — | — | — | none | **no enum value** (AACS or AAProto) — unused id space | — |
| `0x000a` | — undefined — | — | — | none | **no enum value** (AACS or AAProto) — unused id space | — |
| `0x000b` | `PingRequest` / `PING_REQUEST` | ← HU | Enc | `PingRequestMessage.proto` `{int64 timestamp=1}` | handled (`:252`) | [03](03-control-channel.md) |
| `0x000c` | `PingResponse` / `PING_RESPONSE` | → HU | Enc | `PingResponseMessage.proto` `{int64 timestamp=1}` | handled (`:265`) | [03](03-control-channel.md) |
| `0x000d` | `NavigationFocusRequest` / `NAVIGATION_FOCUS_REQUEST` | → HU | Enc | `NavigationFocusRequestMessage.proto` `{uint32 type=1}` | **not dispatched on ch0** (only Response special-cased) | [03](03-control-channel.md) / [09](09-audio-sensor-other.md) |
| `0x000e` | `NavigationFocusResponse` / `NAVIGATION_FOCUS_RESPONSE` | ← HU | Enc | `NavigationFocusResponseMessage.proto` `{uint32 type=1}` | pass-through (re-routed to client, `:229`) | [03](03-control-channel.md) / [09](09-audio-sensor-other.md) |
| `0x000f` | *(none in AACS)* / `SHUTDOWN_REQUEST` | — | — | `ShutdownRequestMessage.proto` `{reason:ShutdownReason}` | **throws** — not in `enums.h`, only `ControlMessageIdsEnum.proto:40` | [03](03-control-channel.md) / [09](09-audio-sensor-other.md) |
| `0x0010` | *(none in AACS)* / `SHUTDOWN_RESPONSE` | — | — | `ShutdownResponseMessage.proto` (empty body) | **throws** — not in `enums.h`, only `ControlMessageIdsEnum.proto:41` | [03](03-control-channel.md) / [09](09-audio-sensor-other.md) |
| `0x0011` | `VoiceSessionRequest` / `VOICE_SESSION_REQUEST` | — | — | (voice/assistant session trigger; not parsed) | **throws** — id defined `enums.h:34` but **no dispatch arm** | [03](03-control-channel.md) / [09](09-audio-sensor-other.md) |
| `0x0012` | `AudioFocusRequest` / `AUDIO_FOCUS_REQUEST` | → HU | Enc | `AudioFocusRequestMessage.proto` `{audio_focus_type:AudioFocusType}` | **not dispatched on ch0** (only Response special-cased) | [03](03-control-channel.md) / [09](09-audio-sensor-other.md) |
| `0x0013` | `AudioFocusResponse` / `AUDIO_FOCUS_RESPONSE` | ← HU | Enc | `AudioFocusResponseMessage.proto` `{audio_focus_state:AudioFocusState}` | pass-through (re-routed to client, `:227`) | [03](03-control-channel.md) / [09](09-audio-sensor-other.md) |

> **Discrepancy noted (raw-enum-wins):** [`03-control-channel.md`](03-control-channel.md)'s
> prose lists Shutdown `0x0f`/`0x10` rows in the `MessageType` table for
> completeness, but the raw `enums.h:21-37` enum has **no** `Shutdown*`
> members and **no** `0x0011` dispatch — so AACS hard-`throw`s these. The
> raw-enum status (**throws**) is authoritative; treat the prose row as a gap
> annotation, not a "handled" claim. Likewise `0x0009`/`0x000a` exist in
> **no** enum file — do not emit them.

### 2.2 Generic channel-open (any feature channel; `MessageType` numbering)

These two ids share the control `MessageType` numbering but ride the
**feature channel's** byte, never ch0. Source `enums.h:28-29`. Behaviour /
state machine: [`06-channel-lifecycle.md`](06-channel-lifecycle.md).

| Id | Name | Dir | Frame flags | Protobuf | AACS support | Doc |
|----|------|-----|-------------|----------|--------------|-----|
| `0x0007` | `ChannelOpenRequest` | → HU | `Bulk\|Enc\|Specific` (`0x0F`) | `ChannelOpenRequestMessage.proto` `{unknown_field=1 (sent 0), channel_id=2}` | handled — server sends (`ChannelHandler.cpp:34-37`) | [06](06-channel-lifecycle.md) |
| `0x0008` | `ChannelOpenResponse` | ← HU | `Enc\|Specific` | `ChannelOpenResponseMessage.proto` `{status:int32=1}` | arrival-only — status **not** inspected (`ChannelHandler.cpp:25-28`) | [06](06-channel-lifecycle.md) |

### 2.3 AV / Media channel (`MediaMessageType` / `AVChannelMessage`)

Used by **video sink** *and* **all audio sink/source** channels (same wire
machinery, distinguished by service-discovery `stream_type`/`audio_type`).
Source: AACS `enums.h:39-47` (`MediaMessageType`); canonical
`AAProto/AVChannelMessageIdsEnum.proto` (`AVChannelMessage.Enum`) — superset.
Video behaviour: [`07-video-channel.md`](07-video-channel.md); audio:
[`09-audio-sensor-other.md`](09-audio-sensor-other.md).

| Id | Name (AACS) / canonical | Dir | Frame flags | Payload (AACS) / schema | AACS support | Doc |
|----|-------------------------|-----|-------------|-------------------------|--------------|-----|
| `0x0000` | `MediaWithTimestampIndication` / `AV_MEDIA_WITH_TIMESTAMP_INDICATION` | → HU | `Bulk\|Enc` | **raw**: `[8B BE pts µs][raw H264/PCM]` (hand-built, no protobuf) | handled — server sends (`VideoChannelHandler.cpp:38`) | [07](07-video-channel.md) / [09](09-audio-sensor-other.md) |
| `0x0001` | `MediaIndication` / `AV_MEDIA_INDICATION` | → HU | `Bulk\|Enc` | **raw**: `[raw H264/PCM]` (no timestamp) | handled — server sends | [07](07-video-channel.md) / [09](09-audio-sensor-other.md) |
| `0x8000` | `SetupRequest` / `SETUP_REQUEST` | → HU | `Bulk\|Enc` (no Specific) | `AVChannelSetupRequestMessage.proto` `{config_index}`; AACS hardcodes bytes `08 03` | handled — server sends (hardcoded) | [07](07-video-channel.md) |
| `0x8001` | `StartIndication` / `START_INDICATION` | → HU | `Bulk\|Enc` (no Specific) | `AVChannelStartIndicationMessage.proto` `{session,config}`; AACS hardcodes `08 00 10 00` | handled — server sends (hardcoded) | [07](07-video-channel.md) |
| `0x8002` | *(none in AACS)* / `STOP_INDICATION` | (→ HU) | — | `AVChannelStopIndicationMessage.proto` | n/a (aasdk-only) — not in `enums.h` | [07](07-video-channel.md) / [09](09-audio-sensor-other.md) |
| `0x8003` | `SetupResponse` / `SETUP_RESPONSE` | ← HU | — | `AVChannelSetupResponseMessage.proto` `{media_status:AVChannelSetupStatus, max_unacked, configs[]}` | arrival-only — not parsed (`VideoChannelHandler.cpp:178`) | [07](07-video-channel.md) |
| `0x8004` | `MediaAckIndication` / `AV_MEDIA_ACK_INDICATION` | ← HU | — | `AVMediaAckIndicationMessage.proto` `{session:int32, value:uint32}` | arrival-only — recognised, not acted on (`:184`) | [07](07-video-channel.md) |
| `0x8005` | *(none in AACS)* / `AV_INPUT_OPEN_REQUEST` | (→ HU, mic) | — | `AVInputOpenRequestMessage.proto` | n/a (aasdk-only) — audio-source/mic | [09](09-audio-sensor-other.md) |
| `0x8006` | *(none in AACS)* / `AV_INPUT_OPEN_RESPONSE` | (← HU, mic) | — | `AVInputOpenResponseMessage.proto` | n/a (aasdk-only) — audio-source/mic | [09](09-audio-sensor-other.md) |
| `0x8007` | *(none in AACS)* / `VIDEO_FOCUS_REQUEST` | (→ HU) | — | `VideoFocusRequestMessage.proto` | n/a (aasdk-only) — never sent by AACS | [07](07-video-channel.md) |
| `0x8008` | `VideoFocusIndication` / `VIDEO_FOCUS_INDICATION` | ← HU | — | `VideoFocusIndicationMessage.proto` `{focus_mode:VideoFocusMode, unrequested:bool}` | arrival-only — body ignored, always starts (`:181`) | [07](07-video-channel.md) |

> Note: `MediaMessageType` (`enums.h:39-47`) omits `0x8002`, `0x8005`,
> `0x8006`, `0x8007` — those are aasdk-only ids
> (`AVChannelMessageIdsEnum.proto`). AACS only **sends** `0x0007/0x8000/
> 0x8001/0x0000/0x0001` and only **reacts to** `0x0008/0x8003/0x8008/0x8004`
> on the video channel; everything else is pass-through.

### 2.4 Input channel (`InputChannelMessageType` / `InputChannelMessage`)

Source: AACS `enums.h:49-54`; canonical
`AAProto/InputChannelMessageIdsEnum.proto`. Behaviour:
[`08-input-channel.md`](08-input-channel.md). (Generic open `0x0007`/`0x0008`
also apply here — see §2.2.)

| Id | Name (AACS) / canonical | Dir | Frame flags | Protobuf | AACS support | Doc |
|----|-------------------------|-----|-------------|----------|--------------|-----|
| `0x0000` | `None` / `NONE` | — | — | (unused sentinel) | n/a | [08](08-input-channel.md) |
| `0x8001` | `Event` / `INPUT_EVENT_INDICATION` | ← HU | `Bulk\|Enc` | `InputEventIndicationMessage.proto` (AACS: `InputEvent.proto`) | handled — verbatim fan-out to clients (`InputChannelHandler.cpp:57`) | [08](08-input-channel.md) |
| `0x8002` | `HandshakeRequest` / `BINDING_REQUEST` | → HU | `Bulk\|Enc` (no Specific) | `BindingRequestMessage.proto` (AACS: `InputChannelHandshakeRequest`) | handled — server sends (`:22`) | [08](08-input-channel.md) |
| `0x8003` | `HandshakeResponse` / `BINDING_RESPONSE` | ← HU | — | `BindingResponseMessage.proto` `{status}` | arrival-only — status not inspected (`:54`) | [08](08-input-channel.md) |

> Name divergence: AACS *Handshake*Request/Response ≡ aasdk/AAProto
> *Binding*Request/Response — **same wire ids/messages** (see §4).

### 2.5 Sensor channel (`SensorChannelMessage`)

**No AACS `enums.h` entry** — AACS treats sensor channels as
`DefaultChannelHandler` pass-through. Source: canonical
`AAProto/SensorChannelMessageIdsEnum.proto`. Detail:
[`09-audio-sensor-other.md`](09-audio-sensor-other.md). Directions
**unverified on the wire** for this project.

| Id | Name | Dir (typical, unverified) | Protobuf | AACS support | Doc |
|----|------|---------------------------|----------|--------------|-----|
| `0x0000` | `NONE` | — | — | n/a | [09](09-audio-sensor-other.md) |
| `0x8001` | `SENSOR_START_REQUEST` | ← HU | `SensorStartRequestMessage.proto` `{sensor_type:SensorType=1, refresh_interval:int64=2}` | pass-through | [09](09-audio-sensor-other.md) |
| `0x8002` | `SENSOR_START_RESPONSE` | → HU | `SensorStartResponseMessage.proto` `{status:Status=1}` | pass-through | [09](09-audio-sensor-other.md) |
| `0x8003` | `SENSOR_EVENT_INDICATION` | → HU | `SensorEventIndicationMessage.proto` (repeated sensor sub-messages) | pass-through | [09](09-audio-sensor-other.md) |

### 2.6 Bluetooth channel (`BluetoothChannelMessage`)

No AACS `enums.h` entry — pass-through. Source: canonical
`AAProto/BluetoothChannelMessageIdsEnum.proto`. Directions unverified.

| Id | Name | Dir (unverified) | Protobuf | AACS support | Doc |
|----|------|------------------|----------|--------------|-----|
| `0x0000` | `NONE` | — | — | n/a | [09](09-audio-sensor-other.md) |
| `0x8001` | `PAIRING_REQUEST` | ← HU | `BluetoothPairingRequestMessage.proto` `{phone_address, pairing_method:BluetoothPairingMethod}` | pass-through | [09](09-audio-sensor-other.md) |
| `0x8002` | `PAIRING_RESPONSE` | → HU | `BluetoothPairingResponseMessage.proto` `{already_paired, status:BluetoothPairingStatus}` | pass-through | [09](09-audio-sensor-other.md) |
| `0x8003` | `AUTH_DATA` | ↔ HU | (bluetooth auth-data payload) | pass-through | [09](09-audio-sensor-other.md) |

### 2.7 Navigation / Audio focus (control channel `0`)

Already enumerated in §2.1 (`0x000d`/`0x000e` nav focus,
`0x0012`/`0x0013` audio focus, `0x0011` voice session). Repeated here as a
namespace grouping for implementers; behaviour is **pass-through on ch0** for
the *Response* ids and **not dispatched** for the *Request* ids. Schemas:
`AudioFocusRequestMessage.proto` / `AudioFocusResponseMessage.proto` /
`NavigationFocusRequestMessage.proto` / `NavigationFocusResponseMessage.proto`.
Detail: [`03-control-channel.md`](03-control-channel.md) §"Focus messages",
[`09-audio-sensor-other.md`](09-audio-sensor-other.md) §"Control-channel-adjacent".

### 2.8 Shutdown (control channel `0`)

`0x000f` `SHUTDOWN_REQUEST` / `0x0010` `SHUTDOWN_RESPONSE` — see §2.1. Schema
exists **only** in AAProto (`ShutdownRequestMessage.proto` `{reason}`,
`ShutdownResponseMessage.proto` empty); **AACS has no `MessageType` entry and
no dispatch arm → inbound shutdown on ch0 hard-`throw`s**
(`AaCommunicator.cpp:246`). `smartcar` should add explicit graceful handling.
Detail: [`03-control-channel.md`](03-control-channel.md) §"Flagged gaps",
[`09-audio-sensor-other.md`](09-audio-sensor-other.md) §"Shutdown".

### 2.9 aasdk-only / unimplemented (no AACS reference, no id enum in checkout)

These channels exist in aasdk's service classes and/or AAProto descriptors but
have **no `*MessageIdsEnum.proto` shipped in this checkout** — message ids are
**deliberately not invented here**. All are `DefaultChannelHandler`
pass-through in AACS. Schemas/descriptors per
[`09-audio-sensor-other.md`](09-audio-sensor-other.md).

| Channel | Descriptor proto | Msg-id enum in checkout? | AACS support |
|---------|------------------|---------------------------|--------------|
| Audio source / Mic | `AVInputChannelData.proto` | reuses AV namespace (§2.3: `0x8005`/`0x8006`/`0x8004`/`0x0000`/`0x0001`) | pass-through |
| Navigation status | `NavigationChannelData.proto` | **no** — descriptor proto only | pass-through |
| Media playback status | `MediaChannelData.proto` (`MediaInfoChannel`) | **no** — descriptor proto only | pass-through |
| Media browser | (no standalone proto in checkout) | **no** | pass-through |
| Phone status | (no standalone proto in checkout) | **no** | pass-through |
| Generic notification | (no standalone proto in checkout) | **no** | pass-through |
| Radio | (no standalone proto in checkout) | **no** | pass-through |
| Wifi projection | (no standalone proto in checkout) | **no** | pass-through |
| Vendor extension | `VendorExtensionChannelData.proto` | **no** — opaque `data` bytes | pass-through |

---

## 3. Enum value catalog

Numeric values sourced from canonical `AAProto/*Enum.proto` (proto2, package
`gb.xxy.trial.proto.enums`). **Wire-compatibility caveat (per
[`05-service-discovery.md`](05-service-discovery.md) §6):** where AACS's
vendored protos use different *names* for these enums, the **numeric values
are identical** and protobuf keys on the number, so the wire is compatible —
only the generated symbol names differ. Name divergences are called out in §4.

### 3.1 `ButtonCode` (`ButtonCodeEnum.proto`)

Non-contiguous. Used in `InputChannel.available_buttons`, the input handshake,
and `ButtonEvent.scan_code`.

| Val | Name | Val | Name | Val | Name |
|-----|------|-----|------|-----|------|
| `0x00` | `NONE` | `0x13` | `UP` | `0x55` | `TOGGLE_PLAY` |
| `0x01` | `MICROPHONE_2` | `0x14` | `DOWN` | `0x57` | `NEXT` |
| `0x02` | `MENU` | `0x15` | `LEFT` | `0x58` | `PREV` |
| `0x03` | `HOME` | `0x16` | `RIGHT` | `0x7E` | `PLAY` |
| `0x04` | `BACK` | `0x17` | `ENTER` | `0x7F` | `PAUSE` |
| `0x05` | `PHONE` | `0x54` | `MICROPHONE_1` | `65536` | `SCROLL_WHEEL` |
| `0x06` | `CALL_END` | | | | |

> [`08-input-channel.md`](08-input-channel.md)'s table lists an extra
> `0x42 = UNKNOWN_1`. That value is **not** in canonical
> `ButtonCodeEnum.proto` (raw-enum-wins: not a defined `ButtonCode`). Treat
> `0x42` as undocumented/HU-specific, not a spec constant.

### 3.2 `TouchAction` (`TouchActionEnum.proto`)

Non-contiguous; `TouchEvent.touch_action`.

| Val | Name (AAProto) | AACS name | Meaning |
|-----|----------------|-----------|---------|
| `0` | `PRESS` | `Press` | finger down |
| `1` | `RELEASE` | `Release` | finger up |
| `2` | `DRAG` | `Drag` | move while down |
| `5` | `POINTER_DOWN` | `Down` | multi-touch: additional pointer down |
| `6` | `POINTER_UP` | `Up` | multi-touch: a pointer up |

### 3.3 `SensorType` (`SensorTypeEnum.proto`)

`SensorStartRequest.sensor_type` & `SensorChannel`'s `Sensor.type`.

| Val | Name | Val | Name | Val | Name |
|-----|------|-----|------|-----|------|
| 0 | `NONE` | 8 | `GEAR` | 16 | `DOOR` |
| 1 | `LOCATION` | 9 | `DIAGNOSTICS` | 17 | `LIGHT` |
| 2 | `COMPASS` | 10 | `NIGHT_DATA` | 18 | `TIRE` |
| 3 | `CAR_SPEED` | 11 | `ENVIRONMENT` | 19 | `ACCEL` |
| 4 | `RPM` | 12 | `HVAC` | 20 | `GYRO` |
| 5 | `ODOMETER` | 13 | `DRIVING_STATUS` | 21 | `GPS` |
| 6 | `FUEL_LEVEL` | 14 | `DEAD_RECONING` | | |
| 7 | `PARKING_BRAKE` | 15 | `PASSENGER` | | |

### 3.4 `AudioType` (`AudioTypeEnum.proto`)

`MediaChannel.audio_type` — selects which audio sink channel.

| Val | Name | Val | Name |
|-----|------|-----|------|
| 0 | `NONE` | 3 | `MEDIA` |
| 1 | `SPEECH` | 4 | `ALARM` |
| 2 | `SYSTEM` | | |

### 3.5 `VideoResolution` (`VideoResolutionEnum.proto`) & `VideoFPS` (`VideoFPSEnum.proto`)

Enum selectors, **not** raw pixel/fps values. `VideoConfig.video_resolution` /
`video_fps`.

| Res val | AAProto | AACS name | | FPS val | AAProto | AACS name |
|---------|---------|-----------|-|---------|---------|-----------|
| 0 | `NONE` | `None` | | 0 | `NONE` | `None` |
| 1 | `_480p` | `H480` | | 1 | `_30` | `F30` |
| 2 | `_720p` | `H720` | | 2 | `_60` | `F60` |
| 3 | `_1080p` | `H1080` | | | | |

### 3.6 `AVStreamType` (`AVStreamTypeEnum.proto`) & `AVChannelSetupStatus` (`AVChannelSetupStatusEnum.proto`)

| Stream val | AAProto | AACS (`MediaStreamType`) | | Setup-status val | Name |
|------------|---------|--------------------------|-|------------------|------|
| 0 | `NONE` | `None` | | 0 | `NONE` |
| 1 | `AUDIO` | `Audio` | | 1 | `FAIL` |
| 3 | `VIDEO` | `Video` | | 2 | `OK` |

> Note `AVStreamType` skips value `2` (no member) — `VIDEO = 3`. This is the
> value AACS gates the video handler on
> ([`05-service-discovery.md`](05-service-discovery.md) §5).

### 3.7 Video focus (`VideoFocusModeEnum.proto`, `VideoFocusReasonEnum.proto`)

`VideoFocusIndication.focus_mode`.

| Mode val | Name | | Reason val | Name |
|----------|------|-|------------|------|
| 0 | `NONE` | | 0 | `NONE` |
| 1 | `FOCUSED` | | 1 | `UNK_1` |
| 2 | `UNFOCUSED` | | 2 | `UNK_2` |

### 3.8 Audio focus (`AudioFocusTypeEnum.proto`, `AudioFocusStateEnum.proto`)

`AudioFocusRequest.audio_focus_type` / `AudioFocusResponse.audio_focus_state`.

| Type val | Name | | State val | Name |
|----------|------|-|-----------|------|
| 0 | `NONE` | | 0 | `NONE` |
| 1 | `GAIN` | | 1 | `GAIN` |
| 2 | `GAIN_TRANSIENT` | | 2 | `GAIN_TRANSIENT` |
| 3 | `GAIN_NAVI` | | 3 | `LOSS` |
| 4 | `RELEASE` | | 4 | `LOSS_TRANSIENT_CAN_DUCK` |
| | | | 5 | `LOSS_TRANSIENT` |
| | | | 6 | `GAIN_MEDIA_ONLY` |
| | | | 7 | `GAIN_TRANSIENT_GUIDANCE_ONLY` |

### 3.9 `ShutdownReason` (`ShutdownReasonEnum.proto`) & `Status` (`StatusEnum.proto`)

| Reason val | Name | | Status val | Name |
|------------|------|-|------------|------|
| 0 | `NONE` | | 0 | `OK` |
| 1 | `QUIT` | | 1 | `FAIL` |

> `Status` is `OK=0 / FAIL=1` (used by `SensorStartResponse.status`). Note the
> **separate** `AVChannelSetupStatus` enum (§3.6) is *not* the same numbering
> (`OK=2` there) — don't conflate them.

### 3.10 `VersionResponseStatus` (`VersionResponseStatusEnum.proto`)

The `matchCode` field of `VersionResponse` (raw `u16`, §2.1 `0x0002`).

| Val | Name | Note |
|-----|------|------|
| `0x0000` | `MATCH` | AACS always sends this (`AaCommunicator.cpp:80`) |
| `0xFFFF` | `MISMATCH` | |

### 3.11 `Gear` (`GearEnum.proto`)

`SensorEventIndication.gear` (`GearData`).

| Val | Name | Val | Name |
|-----|------|-----|------|
| 0 | `NEUTRAL` | 9 | `NINTH` |
| 1–8 | `FIRST`..`EIGHTH` | 10 | `TENTH` |
| | | 100 | `DRIVE` |
| | | 101 | `PARK` |
| | | 102 | `REVERSE` |

### 3.12 `DrivingStatus` (`DrivingStatusEnum.proto`)

**Bitmask** (powers of two), `SensorEventIndication.driving_status`.

| Val | Name | Val | Name |
|-----|------|-----|------|
| 0 | `UNRESTRICTED` | 8 | `NO_CONFIG` |
| 1 | `NO_VIDEO` | 16 | `LIMIT_MESSAGE_LEN` |
| 2 | `NO_KEYBOARD_INPUT` | 31 | `FULLY_RESTRICTED` |
| 4 | `NO_VOICE_INPUT` | | |

### 3.13 Bluetooth (`BluetoothPairingMethodEnum.proto`, `BluetoothPairingStatusEnum.proto`)

| Method val | Name | | Status val | Name |
|------------|------|-|------------|------|
| 0 | `NONE` | | 0 | `NONE` |
| 1 | `UNK_1` | | 1 | `OK` |
| 2 | `A2DP` | | 2 | `FAIL` |
| 3 | `UNK_3` | | | |
| 4 | `HFP` | | | |

> Caveat: [`09-audio-sensor-other.md`](09-audio-sensor-other.md) restates
> `BluetoothPairingStatus` as `OK=1/FAIL=2` — that matches the raw enum
> (`NONE=0, OK=1, FAIL=2`). It is **not** the global `Status` enum (§3.9,
> `OK=0/FAIL=1`).

### 3.14 `HeadlightStatus` / `IndicatorStatus` (`HeadlightStatusEnum.proto` / `IndicatorStatusEnum.proto`)

Both: `STATE_0=0, STATE_1=1, STATE_2=2, STATE_3=3` (opaque state codes; present
in AAProto, not all wired into `SensorEventIndication` —
[`09-audio-sensor-other.md`](09-audio-sensor-other.md)).

---

## 4. Schema-divergence appendix

AACS ships its own **vendored** protos (`AACS/proto/*.proto`); aasdk/`smartcar`
use canonical `AAProto`. Across the area docs, the divergences are **name-only
— field/enum NUMBERS match, so the wire is compatible** (protobuf keys on
number; [`05-service-discovery.md`](05-service-discovery.md) §6). This table
consolidates every divergence the area docs found, so an implementer can use
either symbol set against the same bytes.

### 4.1 Message-id enum names

| AACS (`enums.h`) | Canonical (AAProto) | Ids | Source doc |
|------------------|---------------------|-----|------------|
| `MessageType` | `ControlMessage.Enum` | `0x01`–`0x13` (AACS lacks `0x0f/0x10`; both lack `0x09/0x0a`) | [03](03-control-channel.md) |
| `MediaMessageType` | `AVChannelMessage.Enum` | AACS subset of AV ids | [07](07-video-channel.md) |
| `InputChannelMessageType` | `InputChannelMessage.Enum` | `0x8001`–`0x8003` | [08](08-input-channel.md) |
| `MediaMessageType::SetupRequest/StartIndication/...` | `SETUP_REQUEST` / `START_INDICATION` / ... | same ids | [07](07-video-channel.md) |
| `MediaMessageType::VideoFocusIndication 0x8008` | `VIDEO_FOCUS_INDICATION 0x8008` | same id | [07](07-video-channel.md) |

### 4.2 Message / field name divergences (numbers identical)

| Concept | AACS vendored | Canonical AAProto | Aligned field #s | Doc |
|---------|---------------|-------------------|------------------|-----|
| Svc-disc request fields | `ServiceDiscoveryRequest{model=4, manufacturer=5}` | `ServiceDiscoveryRequestMessage{device_name=4, device_brand=5}` | 4, 5 | [05](05-service-discovery.md) |
| Channel oneof: media | `Channel.media_channel=3` (`MediaChannel`) | `ChannelDescriptorData.av_channel=3` (`AVChannel`) | 3 | [05](05-service-discovery.md) |
| Channel oneof: media input | `Channel.media_input_channel=5` (`MediaInputChannel`) | `av_input_channel=5` (`AVInputChannel`) | 5 | [05](05-service-discovery.md) |
| AV channel extra field | (absent) | `AVChannel.available_while_in_call=5` | 5 (AACS omits) | [05](05-service-discovery.md) / [09](09-audio-sensor-other.md) |
| Canonical-only channel | (absent) | `media_infoChannel=9` | 9 (AACS omits) | [05](05-service-discovery.md) |
| AACS-only channels | `unknown_channel_1=10`, `unknown_channel_2=13` (empty) | (absent) | 10, 13 (canonical omits) | [05](05-service-discovery.md) |
| Audio config field | `AudioConfig.bits_per_sample=2` | `AudioConfig.bit_depth=2` | 2 | [05](05-service-discovery.md) / [09](09-audio-sensor-other.md) |
| Stream-type enum | `MediaStreamType{None=0,Audio=1,Video=3}` | `AVStreamType{NONE=0,AUDIO=1,VIDEO=3}` | values identical | [05](05-service-discovery.md) / [07](07-video-channel.md) |
| Resolution enum | `VideoResolution{None,H480,H720,H1080}` | `VideoResolution{NONE,_480p,_720p,_1080p}` | values 0–3 identical | [05](05-service-discovery.md) / [07](07-video-channel.md) |
| FPS enum | `VideoFps{None,F30,F60}` | `VideoFPS{NONE,_30,_60}` | values 0–2 identical | [05](05-service-discovery.md) / [07](07-video-channel.md) |
| Input discovery entry | `InputChannel{available_buttons=1 (ButtonCode.Enum), screen_config=2}` | `InputChannelData.InputChannel{supported_keycodes=1 (uint32), touch_screen_config=2, +touch_pad_config=3}` | 1, 2 (varint-compatible; #3 AACS omits) | [05](05-service-discovery.md) / [08](08-input-channel.md) |
| Input handshake msg | `InputChannelHandshakeRequest{available_buttons=1}` | `BindingRequest{scan_codes=1 (int32)}` | 1 (varint-compatible) | [08](08-input-channel.md) |
| Input handshake resp | `HandshakeResponse` (`BindingResponse{status}`) | `BindingResponseMessage{status}` | same | [08](08-input-channel.md) |
| Input event msg | `InputEvent{timestamp=1, touch_event=3, buttons_event=4}` | `InputEventIndication{... +disp_channel=2, +absolute=5, +relative=6}` | 1,3,4 (AACS omits 2,5,6) | [08](08-input-channel.md) |
| Touch event | `TouchEvent{touch_location=1, touch_action=3}` | `TouchEventData{... +action_index=2}` | 1, 3 (AACS omits 2) | [08](08-input-channel.md) |
| Touch location | `TouchLocation.pid=3` | `pointer_id=3` | 3 | [08](08-input-channel.md) |
| AV setup response | `MediaChannelSetupResponse{unknown_field_1..3}` | `AVChannelSetupResponse{media_status, max_unacked, configs}` | numbers identical | [07](07-video-channel.md) |
| Touch-action enum | `TouchAction{Press,Release,Drag,Down=5,Up=6}` | `TouchAction{PRESS,RELEASE,DRAG,POINTER_DOWN=5,POINTER_UP=6}` | values identical | [08](08-input-channel.md) |
| Video focus enum | (AACS: bytes not parsed) | `VideoFocusMode{NONE=0,FOCUSED=1,UNFOCUSED=2}` | n/a (AACS ignores body) | [07](07-video-channel.md) |

### 4.3 Schema typo to preserve

`SensorEventIndicationMessage.proto` field 11 is spelled **`enviorment`**
(sic) in canonical AAProto though the enum member is `ENVIRONMENT` —
[`09-audio-sensor-other.md`](09-audio-sensor-other.md). Match the misspelled
field name on the wire-generated struct; numbering (field 11) is what matters.

---

## See also

- [`02-framing.md`](02-framing.md) — where the 2-byte id sits; channel/flags context.
- [`03-control-channel.md`](03-control-channel.md) — control id routing & gaps.
- [`05-service-discovery.md`](05-service-discovery.md) — `Channel` oneof field numbers; schema-name divergences.
- [`06-channel-lifecycle.md`](06-channel-lifecycle.md) — generic `0x0007`/`0x0008` semantics.
- [`07-video-channel.md`](07-video-channel.md) / [`08-input-channel.md`](08-input-channel.md) / [`09-audio-sensor-other.md`](09-audio-sensor-other.md) — per-channel behaviour.
</content>
</invoke>
