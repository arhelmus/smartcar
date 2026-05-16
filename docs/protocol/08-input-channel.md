# 08 — Input Channel

The input channel carries **user input from the head unit's touchscreen and
hard buttons back to the projected app**. It is the one channel whose data
flows *toward* the server: the HU is the physical input device, the server
relays each event to its registered local clients.

Two properties make this channel different from video (`07-video-channel.md`):

1. **Lazy open.** Video is brought up eagerly during boot. Input is opened
   *only when a local client first subscribes* to it
   (`InputChannelHandler::handleMessageFromClient`,
   `AAServer/src/InputChannelHandler.cpp:67`). No client ⇒ no channel.
2. **Handshake, not Setup.** Instead of video's Setup Request/Response it does a
   `HandshakeRequest`/`HandshakeResponse` pair that negotiates the set of hard
   buttons the source advertises.

Prereqs: generic channel open (`06-channel-lifecycle.md`), `available_buttons`
discovered in the `InputChannel` service-discovery entry
(`05-service-discovery.md`), and the local client socket that triggers the open
and receives the fan-out (`11-aacs-client-socket.md`).

> Direction note (see `00-overview.md`): "→ HU" = server→head unit, "← HU" =
> head unit→server. Open/handshake requests go **→ HU**; every steady-state
> input `Event` is **← HU**.

## Message-id table

Channel-local message ids, first 2 bytes of the decrypted payload, big-endian.
Values from `InputChannelMessageType` (`AACS/include/enums.h:49` — note this
enum lives in the repo-root `include/`, not under `AAServer/`).

| Id (hex) | Name (AACS) | Name (AAProto / aasdk) | Dir | Payload |
|----------|-------------|------------------------|-----|---------|
| `0x0000` | `None` | `NONE` | — | (unused sentinel) |
| `0x8001` | `Event` | `INPUT_EVENT_INDICATION` | ← HU | `InputEvent` |
| `0x8002` | `HandshakeRequest` | `BINDING_REQUEST` | → HU | `InputChannelHandshakeRequest` |
| `0x8003` | `HandshakeResponse` | `BINDING_RESPONSE` | ← HU | `BindingResponse` (status) |

Plus the inherited generic control ids on this channel — `ChannelOpenRequest`
(`0x0007`, → HU) / `ChannelOpenResponse` (`0x0008`, ← HU) — handled by the base
`ChannelHandler` before any input-specific dispatch (`06-channel-lifecycle.md`).

> Name alignment: AACS calls `0x8002`/`0x8003` *Handshake* Request/Response;
> aasdk/AAProto call the same ids *Binding* Request/Response. They are the same
> wire messages. The `0x8003` body is a status enum (`BindingResponse.status`,
> `BindingResponseMessage.proto`); AACS only checks *that* it arrived, never the
> body (`InputChannelHandler.cpp:54`).

## Lifecycle

```
Local client (Unix sock)        Server                         Head Unit (HU)
      |                            |                                 |
      |  subscribe to input ch --> |                                 |
      |                            |  register clientId               |
      |                            |  -- ChannelOpenRequest 0x0007 -> |
      |                            |  <- ChannelOpenResponse 0x0008 - |
      |                            |  -- HandshakeRequest  0x8002 --> |   carries
      |                            |     {available_buttons[]}        |   advertised
      |                            |  <- HandshakeResponse 0x8003 --- |   buttons
      |                            |  (handleMessageFromClient returns)|
      |                            |                                 |
      |                            |   == steady state ==             |
      |                            |  <- Event 0x8001 (InputEvent) -- |  user touches
      | <-- raw InputEvent ------- |  fan-out to all reg. clients     |  / presses key
      |                            |  <- Event 0x8001 ... ----------- |
      |                            |                                 |
      |  client disconnects -----> |  deregister clientId             |
```

### 1. Open (lazy, client-triggered)

`handleMessageFromClient(clientId, channelId, specific, data)`
(`InputChannelHandler.cpp:67`) runs the whole bring-up synchronously on the
subscribing client's call:

1. `registered_clients.insert(clientId)` — record the subscriber
   (`:71`).
2. `ChannelHandler::openChannel()` — generic `ChannelOpenRequest 0x0007` →
   wait for `ChannelOpenResponse 0x0008` (`:72`; base at
   `ChannelHandler.cpp:12`). Sent with `Specific` flag set.
3. `gotHandshakeResponse = false` (`:73`).
4. `sendHandshakeRequest()` (`:74`).
5. `expectHandshakeResponse()` — blocks on the condvar until `0x8003`
   arrives (`:75`, `:40`).

The `data` argument from the client's subscribe packet is ignored — the
subscribe message is purely a trigger. The same call serves additional
subscribers (each re-runs open/handshake but the HU tolerates the repeat;
the practical contract is "first subscriber opens it").

### 2. HandshakeRequest (→ HU, `0x8002`)

`sendHandshakeRequest()` (`InputChannelHandler.cpp:22`):

- Push `0x8002` big-endian (`pushBackInt16`, `:24`).
- Build `tag::aas::InputChannelHandshakeRequest`, copying every value of
  `available_buttons` into its repeated `available_buttons` field as a
  `ButtonCode.Enum` (`:29`–`:30`).
- Serialize, append to the message, send with
  `FrameType::Bulk | EncryptionType::Encrypted` (`:36`). Note: **no `Specific`
  flag** here (unlike the generic ChannelOpenRequest).

`available_buttons` is supplied to the handler's constructor
(`InputChannelHandler.cpp:15`) from the `InputChannel` entry in the
Service Discovery Response (`05-service-discovery.md`). It is the source's
declared set of hard keys; the handshake echoes it to the HU so the HU knows
which `ButtonCode`s it may emit.

```proto
// AACS proto/InputChannel.proto — service-discovery entry
message InputChannel {
    repeated ButtonCode.Enum available_buttons = 1;
    optional TouchConfig          screen_config  = 2;  // width,height
}
// the handshake re-sends just the button list:
message InputChannelHandshakeRequest {
    repeated ButtonCode.Enum available_buttons = 1;
}
```

> aasdk/AAProto names: the discovery entry is `InputChannelData.InputChannel`
> with `supported_keycodes` (= `available_buttons`),
> `touch_screen_config`, and an extra `touch_pad_config`; the handshake is
> `BindingRequest { repeated int32 scan_codes = 1; }`. The scalar list is wire-
> compatible with the `ButtonCode.Enum` list. `TouchConfig` is `{width,height}`
> in both.

### 3. HandshakeResponse (← HU, `0x8003`)

`handleMessageFromHeadunit` (`InputChannelHandler.cpp:45`):

1. Delegate to `ChannelHandler::handleMessageFromHeadunit` first — this
   consumes `ChannelOpenResponse 0x0008`; if the base handles it, return early
   (`:46`).
2. Read the BE message id from the first 2 bytes (`:52`–`:53`).
3. `0x8003` ⇒ set `gotHandshakeResponse = true`, mark handled (`:54`–`:56`);
   `cv.notify_all()` unblocks the waiting `handleMessageFromClient`.

The response body (a `BindingResponse.status`) is **not inspected** — arrival
alone completes the handshake. The channel is now in steady state.

### 4. Steady state — Event (← HU, `0x8001`)

`0x8001` ⇒ for every `clientId` in `registered_clients`, forward the **entire
message content verbatim** to that client:
`sendToClient(rc, channelId, 0x00, message.content)`
(`InputChannelHandler.cpp:57`–`:60`). The server does **not** parse the
`InputEvent` — it is a pure relay. Parsing is the local client's job
(`11-aacs-client-socket.md`); the forwarded buffer still has the `0x8001`
id prefix.

## InputEvent payload model

```proto
// AACS proto/InputEvent.proto
message InputEvent {
    optional uint64    timestamp     = 1;   // HU monotonic clock, µs
    optional TouchEvent  touch_event   = 3;
    optional ButtonsEvent buttons_event = 4;
}
```

Exactly one of `touch_event` / `buttons_event` is set per event; `timestamp`
is the HU's clock at the moment of input.

> aasdk: `InputEventIndication` (`InputEventIndicationMessage.proto`) adds
> `disp_channel` (field 2), `absolute_input_event` (5) and
> `relative_input_event` (6) for pointer/dial input, and names field 4
> `button_event` (singular, type `ButtonEvents`) where AACS uses
> `buttons_event` (type `ButtonsEvent`) — same field number, same wire bytes.
> aasdk also makes `timestamp` (field 1) `required`; AACS makes it `optional`.
> AACS handles only touch + buttons; the extra fields, if a HU sends
> them, are simply relayed unparsed by the verbatim fan-out.

### Touch

```proto
// proto/TouchEvent.proto
message TouchEvent {
    repeated TouchLocation touch_location = 1;   // one per active pointer
    required TouchAction   touch_action   = 3;
}
// proto/TouchLocation.proto
message TouchLocation {
    required uint32 x   = 1;
    required uint32 y   = 2;
    required uint32 pid = 3;   // pointer id (aasdk: pointer_id)
}
```

`touch_action` (`proto/TouchAction.proto`) — note non-contiguous values:

| Value | Name | Meaning |
|-------|------|---------|
| `0` | `Press` | finger down (single-touch press) |
| `1` | `Release` | finger up |
| `2` | `Drag` | move while down |
| `5` | `Down` | multi-touch: an additional pointer went down |
| `6` | `Up` | multi-touch: a pointer went up |

aasdk names these `PRESS/RELEASE/DRAG/POINTER_DOWN/POINTER_UP` with identical
numeric values, and adds an `action_index` (field 2) to `TouchEvent`
identifying which pointer in `touch_location[]` changed. AACS's `TouchEvent`
omits `action_index`; `pid` on each `TouchLocation` disambiguates pointers.

Coordinates are in the touchscreen space declared by `TouchConfig
{width,height}` in the `InputChannel` discovery entry — i.e. they are *display*
pixels of the projected surface, not normalized.

### Buttons

```proto
// proto/ButtonsEvent.proto
message ButtonsEvent  { repeated ButtonEvent button_events = 1; }
message ButtonEvent {
    required ButtonCode.Enum scan_code  = 1;
    required bool            is_pressed = 2;   // true=down, false=up
    required uint32          meta       = 3;   // modifier bitmask
    required bool            long_press = 4;
}
```

`ButtonCode.Enum` (`proto/ButtonsEvent.proto`) — values are on the wire as-is
and must match what the handshake advertised in `available_buttons`:

| Code (hex) | Name | | Code (hex) | Name |
|-----------|------|-|-----------|------|
| `0x00` | `NONE` | | `0x16` | `RIGHT` |
| `0x01` | `MICROPHONE_2` | | `0x17` | `ENTER` |
| `0x02` | `MENU` | | `0x42` | `UNKNOWN_1` † |
| `0x03` | `HOME` | | `0x54` | `MICROPHONE_1` |
| `0x04` | `BACK` | | `0x55` | `TOGGLE_PLAY` |
| `0x05` | `PHONE` | | `0x57` | `NEXT` |
| `0x06` | `CALL_END` | | `0x58` | `PREV` |
| `0x13` | `UP` | | `0x7E` | `PLAY` |
| `0x14` | `DOWN` | | `0x7F` | `PAUSE` |
| `0x15` | `LEFT` | | `0x10000` | `SCROLL_WHEEL` ‡ |

† `0x42 UNKNOWN_1` is **AACS-specific** (`proto/ButtonsEvent.proto:23`); it is
**not** present in canonical AAProto `ButtonCodeEnum.proto`. Treat it as a
head-unit/impl extension, not a canonical AA button code. Every other code in
the table matches AAProto `ButtonCodeEnum.proto` exactly (same names, same
values).

‡ `SCROLL_WHEEL` is literally `65536` in both protos (decimal in the `.proto`
source) = `0x10000`; shown in hex here for column consistency. It is the one
code outside the single-byte range.

aasdk's `ButtonEvent` (`ButtonEventData.proto`) is structurally identical but
uses a plain `uint32 scan_code` (no enum) and makes `meta`/`long_press`
*optional* (AACS makes all four fields `required`). `BindingRequest.scan_codes`
is the same list as `available_buttons`. `meta` is a modifier bitmask;
`long_press` distinguishes a held press from a tap on the same `scan_code`.

## Registration & fan-out semantics

| Event | Code | Effect |
|-------|------|--------|
| Local client subscribes | `handleMessageFromClient`, `InputChannelHandler.cpp:67` | `registered_clients.insert(clientId)`, then (re)run open + handshake |
| HU `Event 0x8001` arrives | `handleMessageFromHeadunit`, `:57`–`:60` | iterate `registered_clients`, `sendToClient` the raw buffer to each |
| Local client disconnects | `disconnected`, `:79` | `registered_clients.erase(clientId)` |

- Fan-out is to **all** registered clients (a set, `InputChannelHandler.h:14`);
  every subscriber gets every input event.
- The channel is **not** torn down when the last client leaves — only the
  client set empties. With no registered clients, incoming `0x8001` events are
  consumed (marked handled) but forwarded to nobody (`:57`–`:60`).
- All of `registered_clients`/handshake state is guarded by the handler's
  mutex `m`; the open path blocks the subscribing client's thread until
  `HandshakeResponse` (`:75`).

## smartcar implications

- Open the input channel lazily on first local subscription; do not eagerly
  open it at boot like video.
- Source the handshake's `available_buttons` from the same list put in the
  `InputChannel` service-discovery entry (`05-service-discovery.md`) — keep
  them identical or the HU may suppress unadvertised keys.
- Treat `HandshakeResponse 0x8003` as a bare unblock signal; do not depend on
  the status body.
- The server is a verbatim relay for `0x8001`: forward the decrypted payload
  (id prefix included) to subscribers; parse `InputEvent` only if smartcar's
  consumer needs it. Coordinates are display-space pixels per the discovery
  `TouchConfig`.

## Source references

- Handler: `AAServer/src/InputChannelHandler.cpp`,
  `AAServer/include/InputChannelHandler.h`
- Generic open base: `AAServer/src/ChannelHandler.cpp:12`
- Message ids: `AACS/include/enums.h:49` (`InputChannelMessageType`; repo-root
  `include/`, shared with the head-unit-side enums, not under `AAServer/`)
- Schemas (AACS): `proto/InputChannel.proto`, `InputEvent.proto`,
  `TouchEvent.proto`, `TouchLocation.proto`, `TouchAction.proto`,
  `TouchConfig.proto`, `ButtonsEvent.proto`
- Schemas (cross-check, AAProto): `InputChannelMessageIdsEnum.proto`,
  `InputEventIndicationMessage.proto`, `BindingRequestMessage.proto`,
  `BindingResponseMessage.proto`, `TouchEventData.proto`,
  `ButtonEventData.proto`, `InputChannelData.proto`
