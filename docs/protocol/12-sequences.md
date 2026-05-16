# 12 — End-to-End Sequences

This is the **aggregator** doc. Docs 01–11 define formats; this one stitches
them into annotated end-to-end traces and is authoritative on **ordering,
timing, blocking, and who-initiates-what** across a whole session. Message
formats, enum values, and field layouts are **not** re-derived here — every
format detail cross-refs the owning area doc.

Behavioural ground truth (cross-checked for ordering): AACS
`AAServer/src/AaCommunicator.cpp` (`handleMessageContent` /
`handleChannelMessage` dispatch, `getMessage` ep1 writer, `dataPump` threads)
and `AAServer/main.cpp` (client wiring, `gotMessage` fan-out).

## Conventions (recap, full detail in [README](README.md) / [00](00-overview.md))

- Multi-byte ints **big-endian**; message/enum ids in **hex**.
- Lanes, left→right: **Local Client** | **Server** | **Head Unit (HU)**.
  A **USB** lane is added for the cold-boot prologue only.
- `→ HU` = server→head unit; `← HU` = head unit→server. The Server lane is in
  the middle; arrows point at the HU lane.
- Each step is annotated `[ch N · plain|enc · Spec? · type · id]`:
  - `ch N` — frame channel byte (`0` = control).
  - `plain|enc` — `EncryptionType` flag bit ([02] §2): `plain` pre-auth,
    `enc` from `ServiceDiscoveryRequest` onward.
  - `Spec?` — `MessageTypeFlags::Specific` (flags bit `0x04`); `-` if Control.
  - `type` — `FrameType` ([02] §2): `Bulk` single-frame, or
    `First`/`Last`/intermediate for a fragmented message.
  - `id` — 2-byte BE message id of the *decrypted* payload (hex).
- **Blocking waits** are marked `⟦BLOCKS: <thread> on <cv>⟧`.

### Thread model used by every trace (from [01] §4, cross-checked)

| Thread | Loop | Blocks on |
|--------|------|-----------|
| **ep2-RX** | `read(ep2)` → `handleMessage` → `handleMessageContent` → dispatch | the wire (`read`), then **runs the whole dispatch inline** — including any handler `openChannel()` CV wait |
| **ep1-TX** | `getMessage` (drains send queue) → `write(ep1)` | `sendQueueNotEmpty` (≤1 s), one frame per call; a mid-flight fragmented message stays at the **front** (mutated in place: `sendQueue.front().offset += maxSize`) until `pop_front()`, so it **serialises all outbound traffic and keeps a message's fragments contiguous** |
| **ep0** | control events only | `read(ep0)`; throws on `FUNCTIONFS_SUSPEND` |
| producer threads | gst sample / local-client packet → `sendMessage` (enqueue only) | nothing — fire-and-forget into the send queue |

The single load-bearing hazard threaded through every scenario:
`AaCommunicator.cpp` runs `handleChannelMessage` **under the global mutex `m`**
and **on the ep2-RX thread**, and `openChannel()` blocks that very thread on a
no-timeout condition variable ([06] §"Synchronous / blocking model"). So a
handler's blocking open is only ever unblocked by a *later* inbound frame —
which the **same** ep2-RX thread must read. AACS gets away with it because the
producer that calls `openChannel()` is *not* ep2-RX (it is a gst sample thread
or a local-client reader thread); ep2-RX stays free to read the response. This
ordering invariant is the spine of scenarios 2, 3 and 5.

---

## Scenario 1 — Cold boot to ready

USB mode switch → enumerate → ch0 Version → TLS (N round trips) →
AuthComplete → Service Discovery → handler map built.

```
USB / ep0        Local Client      Server                              Head Unit (HU)
   |                  |               |                                      |
   |  == persona 1: 12d1:107e, ep0-only, MassStorage+FFS ([01] §1-3) ==      |
   |  <--- ctrl 51 Get-Protocol ----------------------------------------- HU |
   |  ---- 02 00  (AOAP proto 2, LE u16) [ep0] ------------------------->     |
   |  <--- ctrl 52 Send-String (×0..n, wIndex; logged, ignored) --------- HU |
   |  <--- ctrl 53 Start-Accessory ------------------------------------- HU  |
   |  (ModeSwitcher returns 0 → ep0 loop ends → teardown persona 1)          |
   |                                                                         |
   |  == USB re-enumeration → persona 2: 18d1:2d00, FFS only ([01] §1) ==    |
   |  AaCommunicator::setup: write descriptors+strings to ep0,               |
   |  open ep1/ep2, mainGadget->enable(udc); spawn ep0/ep1/ep2 pumps         |
   |                  |               |                                      |
   |                  |     [ch 0 · plain · - · Bulk · 0x0001] VersionRequest|
   |                  |               | <----- raw u16 major,minor --------- HU
   |                  |               |  handleVersionRequest: accept iff major==1
   |                  |     [ch 0 · plain · - · Bulk · 0x0002] VersionResponse
   |                  |               | ---- raw (1,5,match=0) ----------->   |   ([03] §Version)
   |                  |               |                                      |
   |                  |               |  == TLS handshake: N round trips ([04]) ==
   |                  |               |  initializeSsl(): SSL_new, mem BIOs,  |
   |                  |               |  SSL_set_accept_state                 |
   |                  |     [ch 0 · plain · - · Bulk · 0x0003] SslHandshake  |
   |                  |               | <-- TLS ClientHello (opaque) ------- HU
   |                  |               |  BIO_write(readBio); SSL_accept →     |
   |                  |               |  -1/WANT_READ (normal); drain writeBio|
   |                  |     [ch 0 · plain · - · Bulk · 0x0003] SslHandshake  |
   |                  |               | -- ServerHello/Cert/KeyExch ------>   |
   |                  |               | <-- SslHandshake (client flight) --- HU   ┐ repeats:
   |                  |               | -- SslHandshake (server flight) -->   |   │ feed→accept
   |                  |               | <-- SslHandshake ... --------------- HU   │ →drain→send
   |                  |               | -- SslHandshake ... -------------->   |   ┘ until SSL_accept OK
   |                  |               |  (last server flight still sent Plain;|
   |                  |               |   no explicit "done" from server)     |
   |                  |               |                                      |
   |                  |     [ch 0 · plain · - · Bulk · 0x0004] AuthComplete  |
   |                  |               | <-- (no body; presence = trigger) -- HU
   |                  |               |  *** server becomes ACTIVE PARTY ***  |
   |                  |               |  sendServiceDiscoveryRequest()        |
   |                  |   [ch 0 · ENC · - · Bulk · 0x0005] ServiceDiscoveryRequest  ← FIRST encrypted frame
   |                  |               | -- pb{model="AAServer"(f4),         |
   |                  |               |       manufacturer="TAG"(f5)} ---->  |   ([05] §2)
   |                  |   [ch 0 · enc · - · Bulk · 0x0006] ServiceDiscoveryResponse
   |                  |               | <-- pb{ channels:[Channel{id,sub}…]}  HU
   |                  |               |  handleServiceDiscoveryResponse:     |
   |                  |               |  build channelHandlers[id] map;      |
   |                  |               |  stash raw resp in serviceDescriptor |
   |                  |               |                                      |
   |               == READY: ch0 up, encrypted, handler map built ==          |
```

**Ordering / blocking notes**

- The USB prologue is **strictly serial** and finishes *before* any AA frame:
  `53 Start-Accessory` tears down persona 1, re-enumeration brings up persona
  2, and only then are ep1/ep2 opened ([01] §2–4). Descriptors+strings are
  written to ep0 **before** `mainGadget->enable` and before ep1/ep2 exist.
- Version + entire TLS handshake are **`plain`**. The flip to `enc` is exactly
  at `ServiceDiscoveryRequest` ([02] §5, [03], [04]) — that frame is the
  first ciphertext of the session.
- TLS is **N request/response round trips, not one**: each `handleSslHandshake`
  call does one `feed → SSL_accept → drain writeBio → send` cycle;
  `SSL_accept` returning `-1`/`WANT_READ` is the normal "need peer's next
  flight" state for every round but the last ([04] §"One round trip"). The
  count N depends on the TLS 1.2 flight count, not fixed by AA.
- **Who initiates:** HU drives Version (sends Request first) and drives the
  TLS handshake (TLS client). The server becomes the **active initiator the
  instant `AuthComplete` arrives** — `sendServiceDiscoveryRequest()` is
  unprompted ([03] §AuthComplete, [04] §AuthComplete).
- **Blocking in this scenario:** none of the handler CVs. Each step is a
  reactive ep2-RX dispatch that enqueues a reply and returns; ep1-TX writes it.
  The `getMessage` ≤1 s wait on `sendQueueNotEmpty` is the only wait and it is
  benign. The blocking opens begin in Scenario 2.
- Channel `0`'s `DefaultChannelHandler` is created in the `AaCommunicator`
  ctor, **not** from the Response ([05] §5); the Response only adds feature
  handlers.

---

## Scenario 2 — Video bring-up & steady state

First gst sample triggers `openChannel()` on the video channel `chN` (id from
Scenario 1's Response — [05] §5, [07] §1). AACS opens video only when the first
encoded sample arrives ([07] §4.5 calls this "lazy on first encoded frame"; the
same mechanism is described as "eager" relative to *input* in Scenario 3 — the
distinction is purely "first sample" vs "first local subscription", not a
protocol constant). Two blocking points in series, then a per-frame media/ack
loop.

```
Local Client            Server (gst thread ‖ ep1-TX ‖ ep2-RX)        Head Unit (HU)
     |                          |                                          |
     |                          |  gst pipeline PLAYING; encodes H264       |
     |                          |  (AACS: 800x480 baseline @30 — impl       |
     |                          |   choice, not protocol; [07] §1)          |
     |                          |                                          |
     |   [RawData on ch chN, from local client, optional — see Scenario via 11] 
     |                          |  ── FIRST encoded sample (gst thread) ──  |
     |                          |  new_sample() → VideoChannelHandler::      |
     |                          |  openChannel(): channelOpened=true (top),  |
     |                          |  then ChannelHandler::openChannel()        |
     |  [ch chN · enc · Spec · Bulk · 0x0007] ChannelOpenRequest             |
     |                          | -- pb{channel_id=chN, unknown_field=0} --> |   ([06])
     |                          | ⟦BLOCKS: gst thread on open CV — no timeout⟧|
     |  [ch chN · enc · Spec · Bulk · 0x0008] ChannelOpenResponse            |
     |                          | <-- pb{status} (value NOT inspected) ----- HU
     |                          |  ep2-RX → base handler sets               |
     |                          |  gotChannelOpenResponse, notify_all →     |
     |                          |  gst thread wakes                          |
     |                          |                                          |
     |  [ch chN · enc · - · Bulk · 0x8000] SetupRequest                      |
     |                          | -- body 08 03 (=AVChannelSetupRequest      |
     |                          |    {config_index=3}, hardcoded; [07] §4.2) |
     |                          | ⟦BLOCKS: gst thread on setup CV⟧            |
     |  [ch chN · enc · - · Bulk · 0x8003] SetupResponse                     |
     |                          | <-- (body NOT parsed; arrival unblocks) -- HU
     |                          |  gst thread wakes; first sample now sendable|
     |                          |                                          |
     |  [ch chN · enc · - · Bulk · 0x8008] VideoFocusIndication              |
     |                          | <-- {focus_mode,unrequested} (NOT parsed;  HU
     |                          |  AACS starts on ANY 0x8008; [07] §4.4)     |
     |  [ch chN · enc · - · Bulk · 0x8001] StartIndication                   |
     |                          | -- body 08 00 10 00 (=Start{session=0,    |
     |                          |    config=0}, hardcoded) -------------->   |
     |                          |                                          |
     |              == STREAMING — per encoded frame, repeats ==             |
     |                          |  gst sample → new_sample(): build msg     |
     |  [ch chN · enc · - · Bulk · 0x0000] MediaWithTimestampIndication      |
     |                          | -- [8B BE pts/1000 µs][Annex-B H264] -->   |   ([07] §4.5)
     |   ── OR, only if buffer.pts == -1: ──                                  |
     |  [ch chN · enc · - · Bulk · 0x0001] MediaIndication                   |
     |                          | -- [Annex-B H264] (no timestamp) ------>   |
     |     (large sample → transport fragments: First(+4B BE total_len of    |
     |      PLAINTEXT) · intermediate(s) · Last; app always hands one Bulk —  |
     |      [02] §4/§7. Each frame's len = CIPHERTEXT slice size.)            |
     |  [ch chN · enc · - · Bulk · 0x8004] MediaAckIndication                |
     |                          | <-- {session,value} recognised, NOT acted  HU
     |                          |  on (no max_unacked throttle; [07] §4.6)   |
     |                          | ... loop continues ...                    |
```

**Ordering / blocking notes**

- **Strict serialisation of bring-up:** open CV **must** resolve before
  `SetupRequest` is sent, and the setup CV before any media. The first encoded
  sample is *not* transmitted until **both** waits return ([07] §2). Frames
  encoded in the meantime queue on the gst side / are dropped per AACS's
  single-`firstSample` latch ([07] §4.5).
- **Which thread blocks:** the **gst sample thread** (the producer), not
  ep2-RX. This is why it is safe: ep2-RX remains free to read
  `ChannelOpenResponse` / `SetupResponse` and fire the notify. If `openChannel`
  ever ran *on* ep2-RX, the response could never be read → deadlock ([06], and
  see Scenario 5 hazard).
- **`total_len` semantics on fragmented media:** the First frame's 4-byte BE
  `total_len` is the **plaintext** message size; each frame's 2-byte `length`
  is that frame's **ciphertext** size — they are not equal for encrypted
  multi-frame messages ([02] §3/§7). Reassembly is the framing layer's job.
- **Timestamp note:** `0x0000` carries an 8-byte BE integer = gst PTS
  (nanoseconds) **÷ 1000 = microseconds**, placed immediately after the 2-byte
  id, before the H264 bytes — *not* protobuf ([07] §4.5). `0x0001` omits it
  (used only when `pts == -1`).
- **RawData origin (cross-ref [11]):** when video is driven by an AACS local
  client rather than gst, the client first does `GetChannelNumberByChannelType`
  (Video=0 → id byte) then tunnels payloads via `RawData` packets on that id;
  HU replies come back as 2-byte-prefixed pushes. `smartcar`'s frames
  originate from its own encoder; the seam is identical (one `sendMessage`
  enqueue). The `Specific` bit on `RawData` becomes the AA Specific flag.
- **`Specific` flag asymmetry:** generic `ChannelOpenRequest` is sent **with**
  `Specific` (flags `0x0F`); `SetupRequest`/`StartIndication`/media are sent
  **without** it ([06] §"Frame flags", [07] §3). Preserve this exactly.

---

## Scenario 3 — Input event path

**Lazy** open: nothing happens until a local client subscribes (issues a
`RawData` packet on the input channel — [11] §"Registration model"). Then a
generic open + a `Handshake` (not Setup) pair, then verbatim event fan-out.

```
Client A   Client B        Server (client-reader thread ‖ ep1-TX ‖ ep2-RX)    HU
   |          |                  |                                            |
   | connect AF_UNIX SOCK_SEQPACKET → clientId=A ([11])                        |
   | [00 01 ..] GetChannelNumberByChannelType(Input=1) ->                      |
   | <- [id] single byte = chM   |                                            |
   | [01 chM 00 <trigger>] RawData --------------------->                      |
   |          |                  |  sendToChannel → InputChannelHandler::      |
   |          |                  |  handleMessageFromClient (client-reader thr)|
   |          |                  |  registered_clients.insert(A)               |
   |          |                  |  ChannelHandler::openChannel():             |
   | [ch chM · enc · Spec · Bulk · 0x0007] ChannelOpenRequest                  |
   |          |                  | -- pb{channel_id=chM, unknown=0} -------->  |
   |          |                  | ⟦BLOCKS: client-reader thr on open CV⟧      |
   | [ch chM · enc · Spec · Bulk · 0x0008] ChannelOpenResponse                 |
   |          |                  | <-- (status; arrival unblocks) ----------- HU
   |          |                  |  sendHandshakeRequest():                    |
   | [ch chM · enc · - · Bulk · 0x8002] HandshakeRequest                       |
   |          |                  | -- pb InputChannelHandshakeRequest          |
   |          |                  |    {available_buttons[] from Svc Disc       |
   |          |                  |     InputChannel entry — [05]/[08]} ----->  |
   |          |                  | ⟦BLOCKS: client-reader thr on handshake CV⟧ |
   | [ch chM · enc · - · Bulk · 0x8003] HandshakeResponse                      |
   |          |                  | <-- BindingResponse{status} (NOT inspected) HU
   |          |                  |  notify → handleMessageFromClient returns   |
   |          |                  |  (Client A's RawData call completes)        |
   |          |                  |                                            |
   |          | (Client B connects later, same GetChannelNumber + RawData →    |
   |          |  registered_clients.insert(B); open+handshake re-run, HU       |
   |          |  tolerates the repeat; practical contract: first opens it.     |
   |          |  [08] §"Open"/"Registration")                                  |
   |          |                  |                                            |
   |          |          == steady state — repeats per user input ==           |
   |          |                  | [ch chM · enc · - · Bulk · 0x8001] Event   |
   |          |                  | <-- InputEvent (touch | buttons; NOT       HU  user
   |          |                  |  parsed by server — verbatim relay) ------- HU  touches
   | <- [chM 00 <0x8001 + InputEvent…>]  fan-out to EVERY registered client    |
   |          | <- [chM 00 <0x8001 + InputEvent…>]  (set iteration; both A,B)  |
   |          |                  | ... continues ...                          |
```

**Ordering / blocking notes**

- **Lazy vs eager:** input opens **only on first local subscription**; video
  opens **eagerly on first encoded sample**. Do not open input at boot ([08]
  §1, [07] §6 item 7).
- **Handshake replaces Setup:** the post-open sub-handshake is
  `HandshakeRequest(0x8002)` / `HandshakeResponse(0x8003)` carrying
  `available_buttons`, not video's `0x8000`/`0x8003` Setup pair. Both
  responses are bare unblock signals — bodies not inspected ([08] §3).
- **Which thread blocks:** the **client-reader thread** (per local connection,
  [11] §Transport), not ep2-RX — same safety reasoning as Scenario 2. The
  whole open+handshake runs **synchronously inside the subscriber's `RawData`
  call**; that client's reader thread is parked until `0x8003`.
- **Fan-out & registration:** registration is **implicit** — issuing `RawData`
  on the channel makes that connection a `registered_clients` member and
  associates its `clientId` ([11] §"Registration model", [08]
  §"Registration"). `0x8001` events are forwarded **verbatim (id prefix
  included) to every registered client** — the server never parses
  `InputEvent`. Targeting in `main.cpp` is `clientId match || clientId==-1`;
  the handler's per-channel fan-out iterates the whole `registered_clients`
  set, so a second client B gets every event too.
- **Direction inversion:** this is the one channel whose steady-state data
  flows **← HU** (HU is the physical input device); open/handshake still go
  **→ HU** ([08] header note).

---

## Scenario 4 — Steady-state interleave

ch0 ping/pong concurrent with video media + input events, all multiplexed over
the single ep1/ep2 endpoint pair.

```
Local Client        Server (ep1-TX writer  ‖  ep2-RX reader)        Head Unit (HU)
     |                     |                                              |
     |     ── all inbound frames arrive on the SAME ep2 stream, parsed     |
     |        one frame at a time by the single ep2-RX thread ──           |
     |                     | [ch 0   · enc · - · Bulk · 0x000b] PingRequest|
     |                     | <-- pb{timestamp=T} (HU drives ping) -------- HU
     |                     |  handlePingRequest: echo SAME T               |
     |                     | [ch 0   · enc · - · Bulk · 0x000c] PingResponse
     |                     | -- pb{timestamp=T} ----------------------->   |   ([03] §Ping)
     |                     | [ch chN · enc · - · Bulk · 0x0000] Media ...  |
     |                     | -- (video frame, possibly fragmented) ---->   |
     |                     | [ch chN · enc · - · Bulk · 0x8004] MediaAck   |
     |                     | <-- (HU flow ack) -------------------------- HU
     |                     | [ch chM · enc · - · Bulk · 0x8001] Event      |
     |                     | <-- InputEvent ----------------------------- HU
     | <- [chM 00 <…>] (input fan-out to client)                            |
     |                     |  -- focus responses on ch0 (0x0e Nav / 0x13   |
     |                     |     Audio) are routed to gotMessage(-1,…),    |
     |                     |     i.e. broadcast to local clients, NEVER    |
     |                     |     interpreted by the communicator ([03])    |
```

**Multiplexing & serialisation invariants**

- **One RX thread, frame-at-a-time:** ep2-RX reads, and one `read()` may carry
  several frames; `dataPump` re-invokes `handleMessage` on the remainder
  (return value = `length+4` bytes consumed) until the buffer drains ([02] §6,
  [01] §4). Channels are demultiplexed purely by the `channel` header byte;
  there is no per-channel inbound thread. Dispatch (`handleMessageContent`)
  routes by channel then by message id ([03] §"Message routing rules").
- **One TX writer, total order:** every outbound message — ping responses,
  ChannelOpen requests, Setup, Start, every media frame — goes through
  `sendMessage` (enqueue) and is serialised by the **single ep1-TX thread**,
  one `getMessage` frame per `write` ([01] §4, [02] §4). Producers (gst, ping
  handler on ep2-RX, client-reader threads) only enqueue; they never touch the
  wire. So outbound frame order = send-queue FIFO order.
- **Fragmentation interleaving caveat (cross-ref [02] §4):** `getMessage`
  emits exactly **one frame per call**. For a multi-frame message it does *not*
  pop the queue; it mutates the **front** element in place
  (`sendQueue.front().offset += maxSize`, `sendQueue.front().flags &= ~Bulk`)
  and `pop_front()`s only on the full/`Last` frame ([02] §4 table, lines
  `:383-412`). There is **no `push_back`** — the in-flight message stays at the
  *front* of the deque until fully drained, so **AACS's own outbound fragments
  are contiguous**: another channel's frame cannot slip between two video
  fragments on the AACS wire. Producers that enqueue during the gap
  `push_back` to the *back*, behind the in-flight message. **However, the AA
  protocol does not require contiguity** — each frame self-identifies by
  `channel` + `FrameType`, a peer is free to interleave fragments of different
  channels, and aasdk's RX (`MessageInStream`) does full **per-channel**
  reassembly ([02] §6 note). So a conformant receiver must still keep a
  **per-channel reassembly buffer** and never assume contiguous fragments —
  the invariant is driven by the *protocol*, not by AACS's (contiguous)
  serialiser.
- **Ping is HU-driven:** the server never originates pings and has no liveness
  timeout of its own — it is purely reactive, echoing the inbound `timestamp`
  ([03] §Ping). The ping handler runs on ep2-RX and just enqueues the response.
- **Focus on ch0 is pass-through:** `0x0e`/`0x13` *responses* on ch0 are
  forwarded to local clients via `gotMessage(-1,…)` (broadcast), never
  interpreted; their `0x0d`/`0x12` request counterparts are not dispatched on
  ch0 at all ([03] §"Message routing rules" / §"Focus messages").

---

## Scenario 5 — Teardown / error paths (concise)

```
Local Client        Server                                   Head Unit (HU)
     | close() ----> |  SocketClient read()==0 → disconnected  |
     |               |  → aac.disconnected(clientId):           |
     |               |    InputChannelHandler.erase(clientId)   |
     |               |    *** channel NOT closed; no ChannelClose to HU ***
     |               |    (with 0 clients, 0x8001 still consumed,
     |               |     forwarded to nobody)  [08] §"Registration"
     |               |                                          |
ep0  |               | <== FUNCTIONFS_SUSPEND on ep0 =========== USB
     |               |  handleEp0Message throws → tears the      |
     |               |  ep0 pump down ([01] §4)                  |
     |               |                                          |
     |               | <-- ch0 unhandled id (e.g. 0x000f        HU
     |               |  Shutdown*, 0x0011 VoiceSessionReq, any   |
     |               |  unknown) ------------------------------- HU
     |               |  handleMessageContent final else →        |
     |               |  throw runtime_error("Unhandled message   |
     |               |  type") — HARD failure, ep2-RX unwinds,   |
     |               |  error signal fires ([03] §routing #9)    |
```

**Notes**

- **Client disconnect ≠ channel teardown.** `disconnected()` only erases the
  `clientId` from `registered_clients`; the AA channel and its handler stay
  up, and the base `ChannelHandler::disconnected()` is a **no-op** that does
  **not** wake a blocked `openChannel()` ([06], [08] §"Registration").
- **Blocking-open-with-no-timeout hazard.** `expectChannelOpenResponse()` /
  `expectSetupResponse()` / `expectHandshakeResponse()` use a CV with **no
  timeout**; a dropped/absent response parks the opener **forever**, and
  `disconnected()` will not release it ([06] §"Synchronous/blocking",
  [07] §2, [08] §"Open"). Fatal **iff** the opener is the ep2-RX thread (then
  the very thread that would read the response is parked → permanent
  deadlock). AACS avoids this only because the opener is always a producer
  thread (gst / client-reader), never ep2-RX.
- **Unhandled message id ⇒ `throw`.** ch0 ids with no dispatch arm
  (`Shutdown* 0x000f/0x0010`, `VoiceSessionRequest 0x0011`, anything unknown)
  hit `throw std::runtime_error("Unhandled message type")` — not a silent
  drop; it tears down the RX path ([03] §routing #9). A non-`Bulk` inbound
  frame is also effectively mis-handled — AACS's RX assumes single-frame and
  has no inbound accumulator ([02] §6).
- **ep0 `FUNCTIONFS_SUSPEND`** is the only ep0 protocol event AACS reacts to;
  it `throw`s out of `handleEp0Message`, ending the ep0 pump ([01] §4).

---

## Implications for smartcar

Ordering invariants a Rust implementation must preserve, distilled from the
traces above:

1. **Server is the active initiator post-auth.** Stay reactive through Version
   + TLS (HU drives), then flip: the instant `AuthComplete` arrives, send
   `ServiceDiscoveryRequest` unprompted, and from there drive every channel
   open/setup. (Scenario 1; [03]/[04]/[05].)
2. **Encrypt only after the handshake.** Version + all `SslHandshake` frames
   are `plain`; `ServiceDiscoveryRequest` is the first `enc` frame and
   everything after stays encrypted. The boundary is exact, not approximate.
   (Scenario 1; [02] §5, [04].)
3. **Never block the read thread on a no-timeout CV.** Run channel
   open/setup/handshake waits on a thread *other* than the one that reads
   inbound frames (or make them async), and **always** bound them with a
   timeout — AACS's design only works because the opener is a separate
   producer thread. Also wire `disconnected()` to wake any blocked opener.
   (Scenarios 2/3/5; [06].)
4. **Real reassembly is mandatory.** Implement a **per-channel** reassembly
   buffer: on `(flags & 0x3) == First` read the 4-byte BE `total_len`
   (plaintext size), accumulate following intermediate/`Last` frames until the
   post-decrypt buffer reaches `total_len`, then dispatch; process `Bulk`
   immediately. AACS's RX assumes single-frame and would mis-parse a
   fragmented inbound message — do **not** copy that. AACS's *outbound*
   serialiser happens to keep fragments contiguous (in-flight message stays at
   the queue front), but the **protocol permits interleaving** and a real peer
   (aasdk) may interleave fragments of different channels, so reassembly state
   **must** be keyed by channel regardless. (Scenarios 2/4; [02] §4/§6/§7.)
5. **Serialise all outbound on one writer; producers only enqueue.** Preserve
   the single send-queue → one-frame-per-write model so outbound order is
   well-defined and fragmentation is consistent. (Scenario 4; [01] §4, [02].)
6. **Honour the flag asymmetries.** `ChannelOpenRequest` carries `Specific`;
   `SetupRequest`/`StartIndication`/`HandshakeRequest`/media do **not**. Get
   `FrameType` masked-equality right (`Bulk == First|Last`; a true First is
   `(flags & 0x3) == First`). (Scenarios 2/3; [02] §2, [06], [07], [08].)
7. **Lazy vs eager open is per channel.** Video: eager on first encoded
   sample. Input: lazy on first local subscription. Decide deliberately;
   neither is a protocol constant but the HU expects the open *before* its
   channel-specific traffic. (Scenarios 2/3; [07] §6, [08] §1.)
8. **Decide graceful teardown explicitly.** AACS hard-`throw`s on
   `Shutdown*`/`VoiceSessionRequest`/unknown ch0 ids and never tears a channel
   down on client disconnect. `smartcar` should choose intentional behaviour
   (graceful shutdown handling, channel lifecycle on last unsubscribe) rather
   than inherit the throw. (Scenario 5; [03], [06], [08].)

### Cross-reference index

Prologue [01] · framing/reassembly [02] · ch0 ids & routing [03] · TLS round
trips [04] · channel→handler map [05] · blocking open CV [06] · video
setup/start/media [07] · input lazy-open/handshake/fan-out [08] · local client
tunnel & push fan-out [11].
