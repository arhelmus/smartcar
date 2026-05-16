# 05 — Service Discovery

The single request/response exchange on channel 0 that tells the server which
**channels** the head unit offers, with each channel's per-feature config.
Actors and the "server" = projection-source convention are in
[`00-overview.md`](00-overview.md); framing in [`02-framing.md`](02-framing.md);
TLS/auth in [`04-tls-auth.md`](04-tls-auth.md); the per-channel open handshake
that follows is [`06-channel-lifecycle.md`](06-channel-lifecycle.md).

Behavioural reference: AACS `AAServer/src/AaCommunicator.cpp`. Schema reference:
AACS's vendored `proto/*.proto` (what AACS actually decodes) cross-checked
against canonical `AAProto/*.proto`. The two schemas are wire-compatible on the
fields below; **name** differences are noted, no **field-number** differences
were found in the service-discovery path.

---

## 1. Position in the bring-up

Triggered the instant auth completes. `AaCommunicator.cpp:237-239`: on receiving
control message `AuthComplete`, the server immediately calls
`sendServiceDiscoveryRequest()`. The head unit answers with
`ServiceDiscoveryResponse`, dispatched at `AaCommunicator.cpp:240-242` to
`handleServiceDiscoveryResponse()`.

```
← HU  ch0  AuthComplete
→ HU  ch0  ServiceDiscoveryRequest    (server initiates; ENCRYPTED)
← HU  ch0  ServiceDiscoveryResponse   (server builds channel-id → handler map)
```

Both messages are control-channel (channel `0`), 2-byte BE message id prefix
then a serialized protobuf. Direction arrows are from the head unit's POV;
"→ HU" = server-sent. **From Service Discovery onward all payloads are
TLS-encrypted** — the request goes out with `EncryptionType::Encrypted`
(`AaCommunicator.cpp:103`), unlike the plaintext Version/SSL handshake traffic.

---

## 2. ServiceDiscoveryRequest (→ HU)

Built and sent by `sendServiceDiscoveryRequest()`,
`AaCommunicator.cpp:95-104`. The server sets exactly two string fields and
hard-codes both literals (order in source is `set_manufacturer` then
`set_model`; wire order is irrelevant):

| Field (AACS `ServiceDiscoveryRequest.proto`) | # | Type | AACS value | Canonical name (`ServiceDiscoveryRequestMessage.proto`) |
|---|---|---|---|---|
| `model` | 4 | `required string` | `"AAServer"` (`:98` `set_model`) | `device_name` |
| `manufacturer` | 5 | `required string` | `"TAG"` (`:97` `set_manufacturer`) | `device_brand` |

Same field numbers (4, 5), different field *names* between AACS's vendored proto
and canonical AAProto — irrelevant on the wire (protobuf keys on number). There
is **no field 1–3**; numbering starts at 4 in both schemas. smartcar may
substitute its own brand/model strings; they are informational, not
behaviour-gating.

Wire layout of the decrypted payload:

```
[ msgid:2 BE = ServiceDiscoveryRequest ][ protobuf: f4=model, f5=manufacturer ]
```

---

## 3. ServiceDiscoveryResponse (← HU) — top level

`ServiceDiscoveryResponse.proto`: `repeated Channel channels = 1`. AACS only
reads field 1. The canonical `ServiceDiscoveryResponseMessage.proto` defines
many more head-unit-description fields on the same message
(`head_unit_name=2`, `car_model=3`, `car_year=4`, `car_serial=5`,
`left_hand_drive_vehicle=6`, `headunit_manufacturer=7`, `headunit_model=8`,
`sw_build=9`, `sw_version=10`, `can_play_native_media_during_vr=11`,
`hide_clock=12`). AACS ignores all of these; a real head unit will populate
them and a server may parse them for UI/telemetry, but none are required to
proceed. The whole raw response is also stashed verbatim in
`serviceDescriptor` (`AaCommunicator.cpp:108`) and re-exported to AACS local
clients (see [`11-aacs-client-socket.md`](11-aacs-client-socket.md)).

---

## 4. The `Channel` message and its sub-message catalog

`Channel.proto`. Every `Channel` carries `channel_id` plus **at most one**
feature sub-message. It is modelled as a list of `optional` fields (not a
protobuf `oneof`) but used as a tagged union — AACS picks the handler by
testing presence (`has_*`).

> **Channel ids are assigned by the head unit in this Response**, not fixed by
> spec. The same feature can land on a different id between sessions or head
> units, and nothing constrains the head unit to a particular numbering. Code
> must look ids up from the parsed Response, never hard-code them. AACS records
> only the Video and Input ids it cares about, into a fixed two-slot array
> `channelTypeToChannelNumber` keyed by `enum ChannelType { Video=0, Input=1 }`
> (`ChannelType.h`; written at `:116`/`:120`); every other channel is reachable
> only via its `channel_id` key in `channelHandlers[]`.

| `Channel` field | # | Sub-message | Covered in |
|---|---|---|---|
| `channel_id` | 1 | `uint32` (required) — the wire channel number | — |
| `sensor_channel` | 2 | `SensorChannel` | [09](09-audio-sensor-other.md) |
| `media_channel` | 3 | `MediaChannel` (video **or** audio) | [07](07-video-channel.md) / [09](09-audio-sensor-other.md) |
| `input_channel` | 4 | `InputChannel` | [08](08-input-channel.md) |
| `media_input_channel` | 5 | `MediaInputChannel` (mic / AV input) | [09](09-audio-sensor-other.md) |
| `bluetooth_channel` | 6 | `BluetoothChannel` | [09](09-audio-sensor-other.md) |
| `navigation_channel` | 8 | `NavigationChannel` | [09](09-audio-sensor-other.md) |
| `unknown_channel_1` | 10 | `UnknownChannel1` (empty msg) | purpose unknown in source |
| `vendor_extension_channel` | 12 | `VendorExtensionChannel` | [09](09-audio-sensor-other.md) |
| `unknown_channel_2` | 13 | `UnknownChannel2` (empty msg) | purpose unknown in source |

Note: fields **7**, **9**, **11** are unassigned in AACS's `Channel.proto`.
Canonical `ChannelDescriptorData.proto` (message `ChannelDescriptor`) uses
fields 1,2,3,4,5,6,8,**9**,12; AACS `Channel` uses 1,2,3,4,5,6,8,**10**,12,**13**.

AACS-vendored ↔ canonical name divergence on shared field numbers:

| # | AACS `Channel.proto` | Canonical `ChannelDescriptor` | Divergence |
|---|---|---|---|
| 1 | `channel_id` | `channel_id` | none |
| 2 | `sensor_channel` | `sensor_channel` | none |
| 3 | `media_channel` (`MediaChannel`) | `av_channel` (`AVChannel`) | name |
| 4 | `input_channel` | `input_channel` | none |
| 5 | `media_input_channel` (`MediaInputChannel`) | `av_input_channel` (`AVInputChannel`) | name |
| 6 | `bluetooth_channel` | `bluetooth_channel` | none |
| 8 | `navigation_channel` | `navigation_channel` | none |
| 9 | *(unused)* | `media_infoChannel` (`MediaInfoChannel`, empty) | canonical-only |
| 10 | `unknown_channel_1` (empty) | *(unused)* | AACS-only |
| 12 | `vendor_extension_channel` | `vendor_extension_channel` | none |
| 13 | `unknown_channel_2` (empty) | *(unused)* | AACS-only |

Every field number that exists in both schemas carries the same semantic
channel — **no conflicting field-number reuse** between the two schemas (the
doc's earlier claim of "no conflict" is confirmed). The only number used by
exactly one schema each: canonical-only `9` (`media_infoChannel`, an empty
message), AACS-only `10`/`13`. `unknown_channel_1`/`_2` are empty placeholder
messages with no fields; their purpose is unknown in the AACS source — do not
interpret or act on them.

---

## 5. Server channel-id → handler dispatch

`handleServiceDiscoveryResponse()`, `AaCommunicator.cpp:106-140`. Iterates every
`Channel` in `sdr.channels()` and, by sub-message presence, instantiates one
handler keyed by `channel_id` into `channelHandlers[channel_id]`. The
classification ladder (`:112-128`, first match wins):

| Order | Test on `Channel` | Extra condition | Handler instantiated | Recorded type slot | Spec doc |
|---|---|---|---|---|---|
| 1 | `has_media_channel()` (`:113`) | `media_channel().media_type() == MediaStreamType_Enum_Video` (enum **3**) (`:114-115`) | `VideoChannelHandler(channel_id)` (`:117-118`) | `channelTypeToChannelNumber[Video] = channel_id` (`:116`) | [07](07-video-channel.md) |
| 2 | `has_input_channel()` (`:119`) | — | `InputChannelHandler(channel_id, available_buttons)` (`:122-124`) | `channelTypeToChannelNumber[Input] = channel_id` (`:120`) | [08](08-input-channel.md) |
| 3 | `else` — everything not matched above (`:125`) | — | `DefaultChannelHandler(channel_id)` (`:126-127`) | none | [09](09-audio-sensor-other.md) |

The first arm is a single compound `if (has_media_channel() &&
media_type==Video)`; an audio/None `media_channel` makes the whole condition
false and falls to the `else if` then the `else`. There is no separate audio
arm.

Consequences of this exact ladder:

- A `media_channel` with `media_type` **Audio (1)** or **None (0)** fails the
  Video test and falls through to `DefaultChannelHandler` — AACS has no real
  audio sink; audio channels are accepted but inertly handled.
- `sensor`, `media_input`, `bluetooth`, `navigation`, `vendor_extension`,
  `unknown_1/2`, and any channel with no recognized sub-message all collapse
  to `DefaultChannelHandler`.
- Only Video and Input get a feature handler; their config is read out of the
  Response at parse time (`InputChannelHandler` is constructed with the
  `available_buttons` list copied from `input_channel`, `:121-124`).
- Channel `0` is **not** in the Response; its `DefaultChannelHandler` is
  created in the constructor (`AaCommunicator.cpp:469`), not here.

After construction each handler's `sendToClient` / `sendToHeadunit` signals are
wired (`:129-138`) so it can pump messages once its channel opens
([`06-channel-lifecycle.md`](06-channel-lifecycle.md)).
`channelTypeToChannelNumber` is later queried by
`getChannelNumberByChannelType()` (`:142-148`) to resolve, e.g., "the video
channel id" for outbound setup.

---

## 6. Per-channel config payloads (projection-source relevant)

What a projection source must parse out of each sub-message it cares about.
Field numbers below are identical in AACS vendored protos and canonical
AAProto unless noted; only field *names* differ.

### 6.1 `MediaChannel` (field 3) — `MediaChannel.proto`

| Field | # | Type | Notes |
|---|---|---|---|
| `media_type` | 1 | `MediaStreamType.Enum` (required) | `None=0`, `Audio=1`, **`Video=3`** — the value AACS gates the video handler on (`:114-115`). Canonical `AVStreamType`: `NONE=0/AUDIO=1/VIDEO=3` (same numbers) |
| `audio_type` | 2 | `AudioType.Enum` (optional) | `None=0 Speech=1 System=2 Media=3 Alarm=4` |
| `audio_configs` | 3 | `repeated AudioConfig` | for audio-type channels |
| `video_configs` | 4 | `repeated VideoConfig` | for video; server picks one and echoes it in Setup ([07](07-video-channel.md)) |

Canonical equivalent is `AVChannel` (`AVChannelData.proto`): same fields 1–4
(`media_type`→`stream_type`, etc.) plus an extra `available_while_in_call = 5`
not present in AACS's copy.

`AudioConfig` (`AudioConfig.proto`): `sample_rate=1`, `bits_per_sample=2`
(canonical name `bit_depth`), `channel_count=3` — all `required uint32`, numbers
identical.

### 6.2 `VideoConfig` — `VideoConfig.proto`

The list element in `MediaChannel.video_configs`. Field numbers match canonical
`VideoConfigData.proto` exactly.

| Field | # | Type | Values |
|---|---|---|---|
| `video_resolution` | 1 | `VideoResolution.Enum` (required) | `None=0 H480=1 H720=2 H1080=3` (canonical `VideoResolution`: `_480p=1 _720p=2 _1080p=3`, same numbers) |
| `video_fps` | 2 | `VideoFps.Enum` (required) | `None=0 F30=1 F60=2` (canonical `VideoFPS`: `_30=1 _60=2`) |
| `margin_width` | 3 | `uint32` (required) | letterbox margin px |
| `margin_height` | 4 | `uint32` (required) | letterbox margin px |
| `dpi` | 5 | `uint32` (required) | display density |
| `additional_depth` | 6 | `uint32` (optional) | — |

Resolution/fps are **enum selectors**, not raw pixel dimensions. Detailed video
bring-up (Setup Request echoing a chosen config, Video Focus, Start) is
[`07-video-channel.md`](07-video-channel.md).

### 6.3 `InputChannel` (field 4) — `InputChannel.proto`

| Field | # | Type | Notes |
|---|---|---|---|
| `available_buttons` | 1 | `repeated ButtonCode.Enum` | head unit's hard-key set; AACS copies this verbatim into `InputChannelHandler` (`:121-124`) |
| `screen_config` | 2 | `TouchConfig` (optional) | touchscreen geometry |

`TouchConfig` (`TouchConfig.proto`): `width=1`, `height=2` (`required uint32`).
`ButtonCode.Enum` values (full AACS set): `NONE=0x00 MICROPHONE_2=0x01 MENU=0x02
HOME=0x03 BACK=0x04 PHONE=0x05 CALL_END=0x06 UP=0x13 DOWN=0x14 LEFT=0x15
RIGHT=0x16 ENTER=0x17 UNKNOWN_1=0x42 MICROPHONE_1=0x54 TOGGLE_PLAY=0x55
NEXT=0x57 PREV=0x58 PLAY=0x7E PAUSE=0x7F SCROLL_WHEEL=65536` (0x10000).

Canonical `InputChannelData.proto` differs in shape/names: `supported_keycodes=1`
(`repeated uint32`, untyped vs AACS's `ButtonCode.Enum` — wire-compatible
varints), `touch_screen_config=2`, and an extra `touch_pad_config=3` AACS does
not model. Field numbers 1–2 align. Full input handshake/event model is
[`08-input-channel.md`](08-input-channel.md).

### 6.4 Other sub-messages (parsed only as far as `DefaultChannelHandler`)

A projection source sees these in the Response but AACS does not act on their
contents (handler is `DefaultChannelHandler`). Schemas, for completeness:

- `SensorChannel` (f2): `repeated Sensor sensors = 1`; `Sensor{ type:1 =
  SensorType.Enum }` (`None=0 Location=1 … Gyro=20 GPS=21`). Canonical
  `SensorChannelData.proto` is field-identical (`sensors=1`).
  → [09](09-audio-sensor-other.md)
- `MediaInputChannel` (f5): `stream_type:1 (MediaStreamType.Enum, required)`,
  `audio_config:2 (AudioConfig, required)`. Canonical `AVInputChannel` keeps 1–2
  (`stream_type` typed as `AVStreamType.Enum`) and adds
  `available_while_in_call = 3 (optional bool)`. → [09](09-audio-sensor-other.md)
- `BluetoothChannel` (f6): `adapter_address:1 (required string)`,
  `available_profiles:2 (repeated BluetoothProfile.Enum: NONE=0 UNK_1=1 A2DP=2
  UNK_3=3 HFP=4)`. Canonical `BluetoothChannelData.proto` diverges in **both
  name and enum type** on field 2: `supported_pairing_methods` typed as
  `BluetoothPairingMethod.Enum` (not `available_profiles`/`BluetoothProfile`).
  Field 1 (`adapter_address`) matches. → [09](09-audio-sensor-other.md)
- `NavigationChannel` (f8): `minimum_interval_ms:1`, `type:2` (both
  `required uint32`). Canonical `NavigationChannelData.proto` adds a third
  `image_options = 3 (NavigationImageOptions, required)` AACS does not model.
  → [09](09-audio-sensor-other.md)
- `VendorExtensionChannel` (f12): `name:1 (required string)`,
  `package_white_list:2 (repeated string)`, `data:3 (optional bytes)`.
  Canonical `VendorExtensionChannelData.proto` is field-identical.
  → [09](09-audio-sensor-other.md)
- `UnknownChannel1` (f10) / `UnknownChannel2` (f13): empty messages, no
  fields; purpose unknown in the AACS source — do not interpret.

---

## 7. Quick reference

```
auth complete
  → ServiceDiscoveryRequest { model="AAServer"(f4), manufacturer="TAG"(f5) }   ENCRYPTED
  ← ServiceDiscoveryResponse { channels:[ Channel{ channel_id, <one sub-msg> } ... ] }

for each Channel:
   media_channel & media_type==Video(3) ─→ VideoChannelHandler   (doc 07)
   input_channel                        ─→ InputChannelHandler   (doc 08)
   anything else                        ─→ DefaultChannelHandler  (doc 09)
   key = channel_id (head-unit-assigned, NOT fixed)
```

Channel ids come from the head unit here and only here; everything downstream
keys off them via `channelHandlers[channel_id]` /
`getChannelNumberByChannelType()`.
