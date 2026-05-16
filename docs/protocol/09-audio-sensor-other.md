# 09 — Audio, Sensor & Other Channels (catalog / reference)

> **Fidelity warning.** This is a **breadth / reference** document, deliberately
> lower-fidelity than [`07-video-channel.md`](07-video-channel.md) and
> [`08-input-channel.md`](08-input-channel.md). Those two channels are the
> *only* ones AACS's `AAServer` actually parses; **every other channel in this
> file is handled by AACS's `DefaultChannelHandler`, which is pure
> byte-passthrough** — it never decodes the protobufs. So for the channels
> below the authoritative schema source is **aasdk + AAProto**, not AACS
> behaviour. Treat the message-id / protobuf facts here as
> *canonical-schema-derived* but **unverified against a running `openauto`
> session**. Field semantics, optionality, and which side initiates have **not**
> been observed on the wire for these channels in this project yet.

## Scope

A catalog of every "other" channel a projection source (smartcar's role — see
[`00-overview.md`](00-overview.md)) may encounter beyond video and input:

- **Audio sink** (head unit plays our audio: media / system / speech / guidance / telephony) — *prioritised, more detail*
- **Sensor** (we feed the head unit car/phone telemetry) — *prioritised, more detail*
- Audio source / microphone (head unit captures mic, we relay)
- Bluetooth (pairing handshake)
- Navigation status
- Media playback status / Media browser
- Phone status
- Generic notification
- Vendor extension
- Radio, Wifi projection (enumerated for completeness)
- Control-channel-adjacent flows: **Shutdown**, **Audio Focus**, **Navigation Focus**

Cross-references:

- Channel discovery / the `Channel` oneof that advertises these — [`05-service-discovery.md`](05-service-discovery.md).
- The generic `ChannelOpenRequest/Response` open→setup→start state machine applies to **all** channels below — [`06-channel-lifecycle.md`](06-channel-lifecycle.md).
- The consolidated message-id ↔ protobuf ↔ direction table — [`10-message-catalog.md`](10-message-catalog.md) (this doc feeds it).

## Conventions

- Multi-byte ints on the wire are **big-endian**; message id = first 2 bytes (BE) of the decrypted payload.
- Direction: **→ HU** = projection source (us) to head unit; **← HU** = head unit to us. (Same convention as [`00-overview.md`](00-overview.md); arrows below are described from our/server perspective, *unverified*.)
- "Channel msg ids" are the per-channel id enums. "Control msg ids" (focus/shutdown) live in the channel-0 namespace.
- Schema citations: `aasdk/include/aasdk/Channel/...` for the channel inventory, `AAProto/<File>.proto` for message/enum schema. Hex ids are taken verbatim from the AAProto `*MessageIdsEnum.proto` / `ControlMessageIdsEnum.proto` files.
- **AACS status** column is the same for nearly all rows — see the passthrough note next.

## AACS passthrough (the one behavioural fact we *can* assert)

`AACS/AAServer/src/DefaultChannelHandler.cpp` is the handler bound to any
channel that is not video or input. Both directions are verbatim byte bridges
with **no protobuf decode**:

- HU → client: `sendToClient(-1, message.channel, message.flags & Specific, message.content)`
- client → HU: re-frames with `EncryptionType::Encrypted | FrameType::Bulk` (+ `Specific` if set) and `sendToHeadunit(...)`.

So "**AACS status: passthrough only**" below means: AACS relays the raw
encrypted-then-decrypted payload between the head unit and its local clients
untouched; it provides **zero** reference behaviour for these channels. smartcar
must implement semantics from the AAProto schema directly.

---

## Channel inventory (aasdk)

aasdk groups channels under `aasdk/include/aasdk/Channel/`. Subdirs present in
this checkout (`server/third_party/aasdk/include/aasdk/Channel/`):

| aasdk subdir | Channel role | Covered below |
|---|---|---|
| `Control/` | control channel (ch 0) | focus/shutdown rows below; main spec [`03-control-channel.md`](03-control-channel.md) |
| `MediaSink/Video/` | head unit displays our video | [`07-video-channel.md`](07-video-channel.md) |
| `MediaSink/Audio/` | head unit plays our audio (4 stream variants) | **§ Audio Sink** |
| `MediaSource/Audio/` | head unit captures mic → us | **§ Audio Source / Mic** |
| `InputSource/` | head unit touch/buttons → us | [`08-input-channel.md`](08-input-channel.md) |
| `SensorSource/` | we push sensor/telemetry → head unit | **§ Sensor** |
| `Bluetooth/` | BT pairing handshake | catalog row |
| `NavigationStatus/` | turn-by-turn nav status | catalog row |
| `MediaPlaybackStatus/` | now-playing metadata/state | catalog row |
| `MediaBrowser/` | browsable media library | catalog row |
| `PhoneStatus/` | phone/call status | catalog row |
| `Radio/` | radio (tuner) control | catalog row |
| `WifiProjection/` | Wi-Fi projection bring-up | catalog row |
| `GenericNotification/` | generic notification | catalog row |
| `VendorExtension/` | OEM-specific opaque channel | catalog row |

> aasdk ships only C++ service classes (`*Service.hpp` / `I*Service.hpp` /
> `I*ServiceEventHandler.hpp`) per subdir — the wire ids/schemas it uses live in
> AAProto's `*MessageIdsEnum.proto` and per-channel `*.proto`, which is what's
> cited below. The MediaSink/Audio subdir has a `AudioMediaSinkService.hpp`
> plus exactly **four** concrete channel classes under
> `aasdk/include/aasdk/Channel/MediaSink/Audio/Channel/`:
> `GuidanceAudioChannel.hpp`, `MediaAudioChannel.hpp`, `SystemAudioChannel.hpp`,
> `TelephonyAudioChannel.hpp` (there is **no** `SpeechAudioChannel` class — the
> `SPEECH` `AudioType` value has no dedicated aasdk class in this checkout).

---

## § Audio Sink (prioritised)

The head unit *plays* audio we produce. Audio sink **reuses the AV-channel
message-id namespace and protobufs** — it is the same wire machinery as the
video channel, just with `stream_type = AUDIO`. There is **one channel per
audio stream type**; the head unit advertises each as a separate `AVChannel` in
Service Discovery, distinguished by `audio_type`.

### Stream types

Audio streams are differentiated by `AudioType` (`AAProto/AudioTypeEnum.proto`):

| `AudioType` | value | typical use |
|---|---|---|
| `NONE` | 0 | unset |
| `SPEECH` | 1 | TTS / nav guidance prompts |
| `SYSTEM` | 2 | UI / notification sounds |
| `MEDIA` | 3 | music / general media |
| `ALARM` | 4 | alarms |

Verbatim from `AAProto/AudioTypeEnum.proto` (`message AudioType.Enum`). aasdk's
four MediaSink/Audio channel classes (`GuidanceAudioChannel`,
`MediaAudioChannel`, `SystemAudioChannel`, `TelephonyAudioChannel`) do **not**
map 1:1 onto these five enum values — the enum classifies the stream, the
classes are aasdk's own grouping (no `SpeechAudioChannel`/`AlarmAudioChannel`
class exists in this checkout, and call audio is `TelephonyAudioChannel`). The
`AVChannel` descriptor carries `available_while_in_call` (field 5, optional
bool) to flag streams that survive a phone call.

### Service-discovery descriptor

`AAProto/AVChannelData.proto` — `message AVChannel` (shared with video):

| field | # | proto2 | meaning |
|---|---|---|---|
| `stream_type` | 1 | required | `AVStreamType.Enum` — `NONE=0`, `AUDIO=1`, `VIDEO=3` (`AAProto/AVStreamTypeEnum.proto`); audio = `AUDIO`(1) |
| `audio_type` | 2 | optional | `AudioType.Enum` (table above) — classifies the audio stream |
| `audio_configs` | 3 | repeated | `AudioConfig` (PCM params, see below) |
| `video_configs` | 4 | repeated | `VideoConfig` — unused for audio |
| `available_while_in_call` | 5 | optional | bool |

`AAProto/AudioConfigData.proto` — `message AudioConfig`:

| field | # | type | meaning |
|---|---|---|---|
| `sample_rate` | 1 | required uint32 | e.g. 16000 / 48000 Hz |
| `bit_depth` | 2 | required uint32 | e.g. 16 |
| `channel_count` | 3 | required uint32 | 1 = mono, 2 = stereo |

### Message ids (AV namespace — shared with video)

`AAProto/AVChannelMessageIdsEnum.proto`, `message AVChannelMessage` (these are
the *same* ids used by the video channel; see [`07-video-channel.md`](07-video-channel.md)
for the video-side detail):

| msg | hex id | proto | direction (typical, **unverified**) |
|---|---|---|---|
| `AV_MEDIA_WITH_TIMESTAMP_INDICATION` | `0x0000` | (raw AV payload, ts-prefixed) | → HU (we send audio frames) |
| `AV_MEDIA_INDICATION` | `0x0001` | (raw AV payload) | → HU |
| `SETUP_REQUEST` | `0x8000` | `AVChannelSetupRequestMessage.proto` (`config_index`) | → HU |
| `START_INDICATION` | `0x8001` | `AVChannelStartIndicationMessage.proto` (`session`, `config`) | → HU |
| `STOP_INDICATION` | `0x8002` | `AVChannelStopIndicationMessage.proto` | → HU |
| `SETUP_RESPONSE` | `0x8003` | `AVChannelSetupResponseMessage.proto` (`media_status`, `max_unacked`, `configs`) | ← HU |
| `AV_MEDIA_ACK_INDICATION` | `0x8004` | `AVMediaAckIndicationMessage.proto` (`session`, `value`) | ← HU |
| `AV_INPUT_OPEN_REQUEST` | `0x8005` | `AVInputOpenRequestMessage.proto` | (mic; see Audio Source) |
| `AV_INPUT_OPEN_RESPONSE` | `0x8006` | `AVInputOpenResponseMessage.proto` | (mic) |
| `VIDEO_FOCUS_REQUEST` | `0x8007` | `VideoFocusRequestMessage.proto` | video-only |
| `VIDEO_FOCUS_INDICATION` | `0x8008` | `VideoFocusIndicationMessage.proto` | video-only |

Flow mirrors video: open (ch-generic, [`06`](06-channel-lifecycle.md)) →
`SETUP_REQUEST(config_index)` → `SETUP_RESPONSE(media_status, max_unacked)` →
`START_INDICATION(session, config)` → stream `AV_MEDIA(_WITH_TIMESTAMP)_INDICATION`
buffers, pace-limited by `AV_MEDIA_ACK_INDICATION` (`max_unacked` window) →
`STOP_INDICATION`. **Audio-focus** (who owns the speaker) is negotiated on the
**control channel**, not here — see § Audio Focus.

**AACS status: passthrough only** — `DefaultChannelHandler` (audio is not the
video channel in AACS; only video + input are special-cased).

---

## § Sensor (prioritised)

We (projection source) push vehicle/phone telemetry to the head unit. aasdk:
`aasdk/include/aasdk/Channel/SensorSource/SensorSourceService.hpp`. The head
unit *subscribes* to specific sensors; we then stream readings for them.

### Message ids

`AAProto/SensorChannelMessageIdsEnum.proto`, `message SensorChannelMessage`:

| msg | hex id | proto | direction (**unverified**) |
|---|---|---|---|
| `NONE` | `0x0000` | — | — |
| `SENSOR_START_REQUEST` | `0x8001` | `SensorStartRequestMessage.proto` | ← HU (HU subscribes) |
| `SENSOR_START_RESPONSE` | `0x8002` | `SensorStartResponseMessage.proto` | → HU (we ack) |
| `SENSOR_EVENT_INDICATION` | `0x8003` | `SensorEventIndicationMessage.proto` | → HU (we stream data) |

### Subscription request / response

`AAProto/SensorStartRequestMessage.proto` — `message SensorStartRequestMessage`:

| field | # | type | meaning |
|---|---|---|---|
| `sensor_type` | 1 | required `SensorType.Enum` | which sensor (table below) |
| `refresh_interval` | 2 | required int64 | requested update period |

`AAProto/SensorStartResponseMessage.proto` — `message SensorStartResponseMessage`:

| field | # | type |
|---|---|---|
| `status` | 1 | required `Status.Enum` — `OK=0` / `FAIL=1` (`AAProto/StatusEnum.proto`) |

The Service-Discovery descriptor for this channel
(`AAProto/SensorChannelData.proto`, `message SensorChannel`) advertises which
sensors we offer: `repeated Sensor sensors` where `Sensor`
(`AAProto/SensorData.proto`) is just `{ required SensorType.Enum type = 1 }`.

### Sensor types

`AAProto/SensorTypeEnum.proto`, `message SensorType`:

| name | val | name | val |
|---|---|---|---|
| `NONE` | 0 | `ENVIRONMENT` | 11 |
| `LOCATION` | 1 | `HVAC` | 12 |
| `COMPASS` | 2 | `DRIVING_STATUS` | 13 |
| `CAR_SPEED` | 3 | `DEAD_RECONING` | 14 |
| `RPM` | 4 | `PASSENGER` | 15 |
| `ODOMETER` | 5 | `DOOR` | 16 |
| `FUEL_LEVEL` | 6 | `LIGHT` | 17 |
| `PARKING_BRAKE` | 7 | `TIRE` | 18 |
| `GEAR` | 8 | `ACCEL` | 19 |
| `DIAGNOSTICS` | 9 | `GYRO` | 20 |
| `NIGHT_DATA` | 10 | `GPS` | 21 |

### Sensor data family (`SensorEventIndication`)

`AAProto/SensorEventIndicationMessage.proto` — `message SensorEventIndication`:
**all fields are `repeated` and optional**; one indication may batch readings of
one (or several) sensor kinds. Field number → sub-message → proto file:

| # | field | sub-message proto | notes |
|---|---|---|---|
| 1 | `gps_location` | `GPSLocationData.proto` | `timestamp:uint64`, `latitude:int32`, `longitude:int32`, `accuracy:uint32` (all required) + opt `altitude/speed/bearing:int32`; lat/lon are scaled int32 |
| 2 | `compass` | `CompassData.proto` | |
| 3 | `speed` | `SpeedData.proto` | `speed:int32` (+opt `cruise_engaged`, `cruise_set_speed`) |
| 4 | `rpm` | `RPMData.proto` | |
| 5 | `odometer` | `OdometerData.proto` | |
| 6 | `fuel_level` | `FuelLevelData.proto` | |
| 7 | `parking_brake` | `ParkingBrakeData.proto` | |
| 8 | `gear` | `GearData.proto` | `Gear.Enum` (`GearEnum.proto`: `NEUTRAL=0`, `FIRST..TENTH=1..10`, `DRIVE=100`, `PARK=101`, `REVERSE=102`) |
| 9 | `diagnostics` | `DiagnosticsData.proto` | |
| 10 | `night_mode` | `NightModeData.proto` | `{ required bool is_night = 1 }` — drives day/night UI theme |
| 11 | `enviorment` *(sic, schema typo)* | `EnvironmentData.proto` | |
| 12 | `hvac` | `HVACData.proto` | |
| 13 | `driving_status` | `DrivingStatusData.proto` | `{ required int32 status = 1 }` — gates UI lockout while moving |
| 14 | `steering_wheel` | `SteeringWheelData.proto` | |
| 15 | `passenger` | `PassengerData.proto` | |
| 16 | `door` | `DoorData.proto` | |
| 17 | `light` | `LightData.proto` | |
| 19 | `accel` | `AccelData.proto` | (note: no field 18 in the message) |
| 20 | `gyro` | `GyroData.proto` | |

> Minimum useful set for a projection source: `NIGHT_DATA` (theme),
> `DRIVING_STATUS` (lockout), and usually `LOCATION`/`GPS` + `CAR_SPEED`. The
> head unit decides which it subscribes to via `SENSOR_START_REQUEST`; we only
> stream what was requested. Related enums also present in AAProto but not all
> wired into `SensorEventIndication`: `DrivingStatusEnum.proto`,
> `HeadlightStatusEnum.proto`, `IndicatorStatusEnum.proto`.

**AACS status: passthrough only** — `DefaultChannelHandler`.

---

## § Audio Source / Microphone

Head unit captures the mic and relays the PCM to us (e.g. for Google Assistant
/ phone call). aasdk:
`aasdk/include/aasdk/Channel/MediaSource/Audio/MicrophoneAudioChannel.hpp`,
`MediaSourceService.hpp`. Uses the **AV-channel namespace** again
(`AVChannelMessageIdsEnum.proto`) but the input-open sub-messages:

| msg | hex id | proto |
|---|---|---|
| `AV_INPUT_OPEN_REQUEST` | `0x8005` | `AAProto/AVInputOpenRequestMessage.proto` — `AVInputOpenRequest { open:bool=1, anc:bool=2, ec:bool=3, max_unacked:int32=4 }` |
| `AV_INPUT_OPEN_RESPONSE` | `0x8006` | `AAProto/AVInputOpenResponseMessage.proto` — `AVInputOpenResponse { session:int32=1, value:uint32=2 }` |
| `AV_MEDIA(_WITH_TIMESTAMP)_INDICATION` | `0x0001` / `0x0000` | raw PCM (← HU, mic frames) |
| `AV_MEDIA_ACK_INDICATION` | `0x8004` | `AAProto/AVMediaAckIndicationMessage.proto` — `AVMediaAckIndication { session:int32=1, value:uint32=2 }` |

Descriptor schema: `AAProto/AVInputChannelData.proto` — `AVInputChannel
{ stream_type:AVStreamType.Enum=1 (required), audio_config:AudioConfig=2
(required, single — note **not** `repeated`, unlike the sink's
`audio_configs`), available_while_in_call:bool=3 (optional) }`. Direction is
inverted vs the audio *sink*: data flows ← HU. **AACS status: passthrough
only.**

---

## § Control-channel-adjacent flows

These ride **channel 0** (control), not a dedicated feature channel. Ids from
`AAProto/ControlMessageIdsEnum.proto`. Full control-channel spec:
[`03-control-channel.md`](03-control-channel.md) — listed here because they're
"other" session-level flows a projection source must handle.

### Audio Focus

Negotiates which app owns the speaker; separate from the audio-sink data path.

| msg | hex id | proto |
|---|---|---|
| `AUDIO_FOCUS_REQUEST` | `0x0012` | `AAProto/AudioFocusRequestMessage.proto` — `audio_focus_type: AudioFocusType.Enum` |
| `AUDIO_FOCUS_RESPONSE` | `0x0013` | `AAProto/AudioFocusResponseMessage.proto` — `audio_focus_state: AudioFocusState.Enum` |
| `VOICE_SESSION_REQUEST` | `0x0011` | (voice/assistant session trigger) |

`AudioFocusType` (`AudioFocusTypeEnum.proto`): `NONE=0, GAIN=1,
GAIN_TRANSIENT=2, GAIN_NAVI=3, RELEASE=4`.
`AudioFocusState` (`AudioFocusStateEnum.proto`): `NONE=0, GAIN=1,
GAIN_TRANSIENT=2, LOSS=3, LOSS_TRANSIENT_CAN_DUCK=4, LOSS_TRANSIENT=5,
GAIN_MEDIA_ONLY=6, GAIN_TRANSIENT_GUIDANCE_ONLY=7`.

### Navigation Focus

| msg | hex id | proto |
|---|---|---|
| `NAVIGATION_FOCUS_REQUEST` | `0x000d` | `AAProto/NavigationFocusRequestMessage.proto` — `{ uint32 type = 1 }` |
| `NAVIGATION_FOCUS_RESPONSE` | `0x000e` | `AAProto/NavigationFocusResponseMessage.proto` — `{ uint32 type = 1 }` |

### Shutdown

| msg | hex id | proto |
|---|---|---|
| `SHUTDOWN_REQUEST` | `0x000f` | `AAProto/ShutdownRequestMessage.proto` — `reason: ShutdownReason.Enum` |
| `SHUTDOWN_RESPONSE` | `0x0010` | `AAProto/ShutdownResponseMessage.proto` — empty body |

`ShutdownReason` (`ShutdownReasonEnum.proto`): `NONE=0, QUIT=1`. Either side may
send the request; the peer replies with the empty response, then the transport
is torn down.

---

## § Remaining channels (catalog rows)

All rows below: **AACS status = passthrough only** (`DefaultChannelHandler`).
Schema is AAProto-derived and **unverified on the wire** in this project.

| Channel | aasdk header | Purpose | Key msg-id enum | Key protobuf(s) | Direction |
|---|---|---|---|---|---|
| **Bluetooth** | `Channel/Bluetooth/BluetoothService.hpp` | BT pairing handshake so HU can pair with phone for HFP/A2DP | `AAProto/BluetoothChannelMessageIdsEnum.proto`: `PAIRING_REQUEST=0x8001`, `PAIRING_RESPONSE=0x8002`, `AUTH_DATA=0x8003` | `BluetoothPairingRequestMessage.proto` (`phone_address`, `pairing_method`), `BluetoothPairingResponseMessage.proto` (`already_paired`, `status`), descriptor `BluetoothChannelData.proto` (`adapter_address`, `supported_pairing_methods`). `BluetoothPairingMethodEnum`: `NONE=0,UNK_1=1,A2DP=2,UNK_3=3,HFP=4`; `BluetoothPairingStatusEnum`: `NONE=0,OK=1,FAIL=2` | request ← HU, response → HU (unverified) |
| **Navigation status** | `Channel/NavigationStatus/NavigationStatusService.hpp` | Turn-by-turn nav state pushed to HU cluster/UI | (no dedicated `*MessageIdsEnum.proto` in this checkout — ids consolidated in [`10`](10-message-catalog.md)) | descriptor `NavigationChannelData.proto` (`minimum_interval_ms`, `type`, `image_options`), `NavigationImageOptionsData.proto` | → HU |
| **Media playback status** | `Channel/MediaPlaybackStatus/MediaPlaybackStatusService.hpp` | Now-playing metadata + play state | (consolidated in [`10`](10-message-catalog.md)) | `MediaChannelData.proto` (`MediaInfoChannel`, empty descriptor) | → HU |
| **Media browser** | `Channel/MediaBrowser/MediaBrowserService.hpp` | Browsable media library tree | (consolidated in [`10`](10-message-catalog.md)) | (browser node protos — not present as standalone files in this checkout) | bidirectional |
| **Phone status** | `Channel/PhoneStatus/PhoneStatusService.hpp` | Phone / call status to HU | (consolidated in [`10`](10-message-catalog.md)) | (phone-status protos — not present standalone) | → HU |
| **Generic notification** | `Channel/GenericNotification/GenericNotificationService.hpp` | Generic notification surface | (consolidated in [`10`](10-message-catalog.md)) | (not present standalone) | → HU |
| **Vendor extension** | `Channel/VendorExtension/VendorExtensionService.hpp` | OEM-specific opaque channel | (consolidated in [`10`](10-message-catalog.md)) | descriptor `VendorExtensionChannelData.proto` (`name`, `package_white_list`, opaque `data` bytes) | bidirectional, opaque |
| **Radio** | `Channel/Radio/RadioService.hpp` | Tuner / radio control | (consolidated in [`10`](10-message-catalog.md)) | (radio protos — not present standalone) | bidirectional |
| **Wifi projection** | `Channel/WifiProjection/WifiProjectionService.hpp` | Wi-Fi projection bring-up / handoff | (consolidated in [`10`](10-message-catalog.md)) | (wifi protos — not present standalone) | bidirectional |

> Where a per-channel `*MessageIdsEnum.proto` is **not** in this AAProto
> checkout, ids are intentionally **not invented here** — they will be
> reconciled in [`10-message-catalog.md`](10-message-catalog.md) (and ideally
> verified against an `openauto` capture). Several of these channels' message
> protos likewise aren't shipped as standalone files in
> `server/third_party/AAProto/`; only their service classes exist in aasdk and
> their descriptors in the `*ChannelData.proto` files cited above.

---

## Implementation note for smartcar

Priority order implied by "most likely next implemented": **Audio Sink**
(reuses the video channel's AV machinery — config index / setup / start / media
+ ack window) and **Sensor** (`NIGHT_DATA` + `DRIVING_STATUS` are effectively
required for a correct HU UI). Both, plus the **Audio Focus** and **Shutdown**
control flows, are well-specified by AAProto and only need their semantics
confirmed against the `openauto` emulator. The remaining rows can stay as
opaque passthrough (matching AACS behaviour) until a concrete feature needs
them.

## Sources

- Channel inventory: `server/third_party/aasdk/include/aasdk/Channel/**` (service class headers only).
- Message ids: `server/third_party/AAProto/{AVChannelMessageIdsEnum,SensorChannelMessageIdsEnum,BluetoothChannelMessageIdsEnum,ControlMessageIdsEnum}.proto`.
- Schemas/enums: the corresponding `server/third_party/AAProto/*.proto` files cited inline.
- Passthrough behaviour: `AACS/AAServer/src/DefaultChannelHandler.cpp` (the *only* AACS-observed fact in this doc).
- Cross-refs: [`03`](03-control-channel.md), [`05`](05-service-discovery.md), [`06`](06-channel-lifecycle.md), [`07`](07-video-channel.md), [`08`](08-input-channel.md), [`10`](10-message-catalog.md).
