# 06 — Generic Channel Lifecycle

How every feature channel (video, input, media, sensor, …) is brought from
"announced in Service Discovery" to "usable", **before** any channel-specific
setup. This is the shared `ChannelHandler` base behaviour; VIDEO ([07]) and
INPUT ([08]) layer extra handshakes *on top* of what is described here.

Behavioural reference: `AACS/AAServer/src/ChannelHandler.cpp`. Schema:
`AACS/proto/ChannelOpenRequest.proto`, `AACS/proto/ChannelOpenResponse.proto`.

## The two messages

Both use the channel's own id (not control channel 0). Message id is the first
2 bytes (BE) of the decrypted payload — same slot as control ids ([02]).

| Dir | Msg id | Name | Payload (proto2) |
|-----|--------|------|------------------|
| → HU | `0x0007` | `ChannelOpenRequest` | `unknown_field:int32=1`, `channel_id:int32=2` |
| ← HU | `0x0008` | `ChannelOpenResponse` | `status:int32=1` |

`MessageType::ChannelOpenRequest = 7`, `ChannelOpenResponse = 8`
(`AACS/include/enums.h:28-29`).

`ChannelOpenRequest` is built with `channel_id` = this handler's channel id and
`unknown_field` = `0` (`ChannelHandler.cpp:34-37`). Note the proto field
*numbers* are `unknown_field=1`, `channel_id=2`; the value written into
`unknown_field` is just `0`. Do not read meaning into `unknown_field` beyond
"server sends 0". The response carries a single `status` int; the server only
checks that a `ChannelOpenResponse` *arrived* — it does not branch on
`status`'s value (`ChannelHandler.cpp:25-28`). Don't infer status semantics.

## Generic state machine

```
   ┌──────────┐  openChannel()         ┌──────────────────────┐
   │  CREATED │ ─────────────────────▶ │  AWAITING_OPEN_RESP   │
   └──────────┘  send ChannelOpenReq   └──────────┬───────────┘
   (handler built     id 0x07,                    │ ← HU ChannelOpenResponse
    from Svc Disc,    Specific flag,               │   (id 0x08, any status)
    see [05])         channel = chN                ▼
                                        ┌──────────────────────┐
                                        │       OPEN/USABLE     │
                                        └──────────┬───────────┘
                                                   │ channel-specific
                                                   │ setup ([07]/[08]) — or
                                                   ▼ none (DefaultChannelHandler)
                                        ┌──────────────────────┐
                                        │   STREAMING / PASS-   │
                                        │   THROUGH             │
                                        └──────────────────────┘
```

The CREATED → AWAITING → OPEN transition is what *every* channel shares. The
last box is where specialisations diverge.

## Synchronous / blocking model

`openChannel()` is **blocking** (`ChannelHandler.cpp:12-16`):

```
openChannel():
  gotChannelOpenResponse = false
  sendChannelOpenRequest()        // → HU, id 0x07
  expectChannelOpenResponse()     // BLOCKS on a condition variable
```

`expectChannelOpenResponse()` does `cv.wait(lk, [=]{ return
gotChannelOpenResponse; })` (`ChannelHandler.cpp:48-51`) — the calling thread
parks until the head unit's response is received. There is **no timeout** and
**no spurious-wakeup escape**: the predicate only flips when a
`ChannelOpenResponse` actually arrives. `disconnected()` is a no-op in the base
(`ChannelHandler.cpp:53`) — it touches neither `gotChannelOpenResponse` nor
`cv`, so a peer disconnect does **not** wake the waiter. Nothing in the base
class can ever release a thread blocked in `openChannel()` other than an
inbound `ChannelOpenResponse`.

> **Deadlock hazard (real smartcar concern).** AACS gets away with this only
> because `openChannel()` is invoked from threads it is willing to wedge: in
> AACS, `VideoChannelHandler` calls it from the **GStreamer streaming thread**
> on the first sample (`VideoChannelHandler.cpp:30-32`) and
> `InputChannelHandler` from the **client-socket handler thread** on first
> client registration (`InputChannelHandler.cpp:67-76`). If the HU never sends
> `ChannelOpenResponse` (or the link drops mid-open), that thread is parked
> forever — and the subclasses then do a *second* no-timeout
> `cv.wait` for their own handshake (Setup / input Handshake), so the hazard is
> two serial unbounded waits, not one. smartcar owns the equivalent
> server-initiated open path and **must** add a timeout and a
> disconnect-driven cancellation that flips the predicate and notifies the CV;
> a straight port of this base class will hang on any unresponsive or
> disappearing head unit.

The wake-up: inbound HU messages are dispatched by `AaCommunicator` to
`handleMessageFromHeadunit` for the target channel. The base implementation
(`ChannelHandler.cpp:18-32`) takes the mutex, reads the 2-byte BE message id,
and **iff** it is `ChannelOpenResponse` sets `gotChannelOpenResponse = true`,
returns `true` ("handled"), then `cv.notify_all()` releases the blocked
`openChannel()`. Any other message id → returns `false` (not handled here).

So the open is a strict request/response round-trip on the channel itself, run
synchronously by whichever thread invoked `openChannel()`.

## Frame flags for ChannelOpenRequest

The request frame's flag byte is exactly
(`ChannelHandler.cpp:42-45`, `AACS/include/enums.h:5-19`):

```
FrameType::Bulk (0b11) | EncryptionType::Encrypted (0x08) | MessageTypeFlags::Specific (0x04)
= 0x0F
```

i.e. single (non-fragmented) frame, TLS-encrypted, channel-specific. The frame
channel byte is the feature channel id `chN`, not `0`. See [02] for the flag
bit layout and the `total_len` rule (not present here — it's a `Bulk` frame).
`ChannelOpenResponse` arrives from the HU similarly encrypted + `Specific`.

## Subclass extension contract

A specialised handler overrides `handleMessageFromHeadunit` but **must call the
base first** and short-circuit if the base consumed the message
(`InputChannelHandler.cpp:45-47`, `VideoChannelHandler.cpp:170-171`):

```
SubclassHandler::handleMessageFromHeadunit(msg):
  if ChannelHandler::handleMessageFromHeadunit(msg):   // base eats ChannelOpenResponse
      return true                                       // → unblocks openChannel()
  ... handle channel-specific message ids (Setup/Start/Handshake/...) ...
```

Division of labour: the **base** owns only `ChannelOpenResponse` (id `0x08`)
and the open CV; the **subclass** owns everything after.

**When the generic open actually runs differs per subclass — it is not a
client-driven uniform step:**

- `VideoChannelHandler::openChannel()` (`VideoChannelHandler.cpp:127-133`)
  calls `ChannelHandler::openChannel()` (generic blocking open), then runs its
  own blocking `SetupRequest`/`expectSetupResponse` handshake. It is triggered
  by the **first GStreamer sample** in `new_sample` (`VideoChannelHandler.cpp:30-32`),
  not by a client message. Before that point `channelOpened == false` and
  Video's `handleMessageFromHeadunit` (`VideoChannelHandler.cpp:164-169`)
  **bypasses the base entirely**, relaying every HU message straight to local
  clients like a Default channel — so the base open-CV path only becomes live
  once `openChannel()` has set `channelOpened = true`.
- `InputChannelHandler` calls `ChannelHandler::openChannel()` from
  `handleMessageFromClient` (`InputChannelHandler.cpp:67-76`) on the **first
  client registration**, then layers a *second* independent blocking CV
  (`gotHandshakeResponse`, `sendHandshakeRequest`/`expectHandshakeResponse`)
  for its own input handshake.

Where these handler objects are constructed from the Service Discovery channel
catalog is covered in [05] (`AaCommunicator.cpp:117-127`).

## DefaultChannelHandler — pass-through (no specialised logic)

Channels with no dedicated handler get `DefaultChannelHandler`
(`AACS/AAServer/src/DefaultChannelHandler.cpp`), also used for control channel
`0`. It does **not** override `handleMessageFromHeadunit` to add channel logic
— it is a bidirectional bridge to AACS local clients:

- **HU → client**: forwards the raw message to all local clients via
  `sendToClient(-1, …)` (clientId `-1` = broadcast), propagating the `Specific`
  bit from the frame flags as `message.flags & MessageTypeFlags::Specific`
  (`DefaultChannelHandler.cpp:14-19`). This override **unconditionally returns
  `true` and never calls the base**, so for a Default channel
  `ChannelOpenResponse` is just relayed to the client like any other message
  rather than driving an open CV — Default channels are never opened via the
  blocking `openChannel()` path at all. (This is the same pre-open relay shape
  Video uses while `channelOpened == false`; see the subclass section above.)
- **Client → HU**: re-emits to the head unit with
  `EncryptionType::Encrypted | FrameType::Bulk`, OR-ing in
  `MessageTypeFlags::Specific` only if the client marked the message specific
  (`DefaultChannelHandler.cpp:21-30`).

It carries no state machine of its own beyond the generic base; it is purely
the AACS client fan-out path (see [11] for the local client socket protocol).

## Cross-references

- [02] `02-framing.md` — frame header, `EncryptionType`/`FrameType`/`MessageTypeFlags` bits, the `0x0F` flag composition, `total_len` rule.
- [05] `05-service-discovery.md` — where each channel id and its handler type come from.
- [07] `07-video-channel.md` — video setup handshake layered on top of this open.
- [08] `08-input-channel.md` — input handshake layered on top of this open.

[02]: ./02-framing.md
[05]: ./05-service-discovery.md
[07]: ./07-video-channel.md
[08]: ./08-input-channel.md
