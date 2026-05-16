# 11 — AACS Local Client Socket API

**Protocol (B), AACS-specific.** Everything in docs 01–10 is the AA *wire*
protocol (A) spoken to the head unit. This file documents a *different*
protocol: the local IPC `AAServer` exposes so that out-of-process clients
(AACS ships `AAClient`/`GetEvents`) can drive AA channels without speaking AA
themselves. `smartcar` does **not** have to copy this; it is documented because
it is a working, minimal pattern for "re-export AA channels to a local
consumer", and it pins down exactly what AACS's own tools expect.

This is **not** the big-endian, length-prefixed frame format of [02]. It is a
flat datagram layout over `SOCK_SEQPACKET` — message boundaries are preserved
by the kernel, so there is **no length prefix and no framing header**. One
`write()` = one logical packet = one `read()`.

Behavioural reference: `AAServer/main.cpp`, `AAServer/src/SocketClient.cpp`,
`AAServer/src/SocketCommunicator.cpp`. Types: `AAServer/include/PacketType.h`,
`Packet.h`, `ChannelType.h`.

## Transport

| Property | Value | Source |
|----------|-------|--------|
| Address family | `AF_UNIX` | `SocketCommunicator.cpp:20` |
| Socket type | `SOCK_SEQPACKET` (reliable, ordered, message-boundary preserving) | `SocketCommunicator.cpp:20` |
| Path | `./socket` (relative to `AAServer`'s CWD) | `main.cpp:84` |
| Backlog | `listen(sock, 5)` | `SocketCommunicator.cpp:29` |
| Per-client | one `accept()` → one `SocketClient` → one dedicated reader thread | `SocketCommunicator.cpp:47-55`, `SocketClient.cpp:18-26` |
| Read buffer | 2 MiB single read; one datagram per `read()` | `SocketClient.cpp:29-47` |

`SOCK_SEQPACKET` is load-bearing: the wire layouts below have **no length
field** because each packet is delivered atomically with its original size.
A client must send each request as a single `write()` and read each response
as a single `read()`.

## Client → server request packet

Parsed in `SocketClient.cpp:52-57`. Flat, fixed 3-byte prefix + payload:

```
byte:   0            1               2          3 ...
      +-----------+---------------+----------+-----------+
      |packetType |channelNumber  | specific |  data...  |
      +-----------+---------------+----------+-----------+
         1 byte       1 byte        1 byte    rest of pkt
```

- `packetType` — a `PacketType` enum value (table below). Enum *order* is the
  wire value: `GetChannelNumberByChannelType=0`, `RawData=1`,
  `GetServiceDescriptor=2` (`PacketType.h:3-7`).
- `channelNumber` — meaning depends on `packetType` (an AA channel id for
  `RawData`; a `ChannelType` enum for `GetChannelNumberByChannelType`; ignored
  otherwise).
- `specific` — only meaningful for `RawData`: it becomes the AA "Specific"
  flag (flags bit 2, see [06]) on the message injected into the channel.
- `data` — everything after byte 2 (the AA payload for `RawData`; empty for
  the query types).

The three prefix bytes are read unconditionally, so every request is at least
3 bytes even when the type carries no payload.

## PacketType semantics & responses

Dispatched in `main.cpp:90-112`.

| `packetType` | Request meaning | `channelNumber` field | `data` field | Server response (`sendMessage`) |
|---|---|---|---|---|
| `0` `GetChannelNumberByChannelType` | "what AA channel id was assigned to this feature?" | a `ChannelType` enum: `Video=0`, `Input=1` (plus `MaxValue=2`, a non-channel sentinel) (`ChannelType.h:3`) | — | **1 byte**: the AA channel id (`main.cpp:91-96`) |
| `1` `RawData` | inject an AA message into / it is the bidirectional tunnel for an AA channel | the AA channel id | raw AA payload | no synchronous reply; channel traffic comes back as async pushes (below) |
| `2` `GetServiceDescriptor` | "give me the raw Service Discovery response" | — | — | the **raw `ServiceDiscoveryResponse` protobuf bytes** (`main.cpp:107-109`) |
| anything else | — | — | — | server throws `runtime_error("Unknown packetType")` → client's reader thread tears down (`main.cpp:110-111`, `SocketClient.cpp:22-24`) |

Notes:

- **`GetChannelNumberByChannelType`** maps the AACS-local `ChannelType` enum to
  the dynamic AA channel id that the head unit assigned in Service Discovery
  ([05]). The client passes the *enum* in the `channelNumber` byte —
  server-side it is cast `(ChannelType)p.channelNumber` (`main.cpp:93`) — and
  gets a single id byte back. The indirection exists because `ChannelType`
  (`Video=0`, `Input=1`; `MaxValue=2` is the bound, not a channel) is a
  *stable, AACS-client-local* identifier, whereas the AA wire channel id it
  resolves to is assigned dynamically by the head unit per session ([05]) and
  is **not** the same number space. This is how a client learns which
  `channelNumber` to use for subsequent `RawData` packets without depending on
  the head unit's assignment.
- **`GetServiceDescriptor`** returns the exact `ServiceDiscoveryResponse`
  protobuf as bytes — no AACS-specific wrapping. Decode it per [05] to
  enumerate channels/features. (`aac.getServiceDescriptor()`, `main.cpp:109`.)
- **`RawData`** is the actual data path. The `data` bytes are handed to
  `aac.sendToChannel(clientId, channelNumber, specific, data)` (`main.cpp:99`),
  i.e. they are framed/encrypted and sent on AA channel `channelNumber` with
  the given Specific flag. Replies/indications from that channel are *not*
  returned inline — they arrive as server→client pushes.

## Server → client push

Whenever a message arrives from the head unit, `AaCommunicator` fires
`gotMessage(clientId, channelNumber, specific, data)` and `main.cpp:68-83`
fans it out to clients. Push layout (built `main.cpp:72-75`):

```
byte:   0              1                    2 ...
      +---------------+--------------------+-----------+
      | channelNumber | specific (0x00 |   |  data...  |
      |               |          0xff)     |           |
      +---------------+--------------------+-----------+
```

- `channelNumber` — the AA channel id the message came in on.
- byte 1 — `0xff` if the AA Specific flag was set, else `0x00`
  (`main.cpp:74`). Note this is **not** the `PacketType`/3-byte request
  layout — pushes have a 2-byte prefix only. The push stream is asymmetric to
  the request stream; a client demultiplexes pushes purely by
  `channelNumber`.
- `data` — the decrypted AA payload (begins with the 2-byte BE message id per
  [02]/[03]).

Fan-out / addressing (`main.cpp:76-82`):

- Each connection is assigned an integer client id from a monotonically
  increasing counter (`int clientCount = 0;`, `main.cpp:86`) at connect time:
  the `newClient` handler does `clients.insert({scl, clientCount++})`
  (`main.cpp:88`), so the *first* client gets id `0`, the next `1`, etc. Here
  `clients` is `main.cpp`'s local `map<SocketClient*, int>` (`main.cpp:66`) —
  the id ledger — which is distinct from `SocketCommunicator`'s internal
  `set<SocketClient*>` lifetime registry (`SocketCommunicator.h:16`). This id
  is what gets passed to `aac.sendToChannel(...)` as the originating client for
  `RawData`.
- A message is delivered to a client iff `cl.second == clientId` **or**
  `clientId == -1` (broadcast). I.e. some head-unit traffic is targeted back
  to the specific client that owns/registered the channel, and `-1` means
  "all clients". Disconnected clients are skipped (the `client_disconnected_error`
  is swallowed, `main.cpp:80-81`).

### Registration model (input channel)

The per-client targeting above is how the **input channel** ([08]) routing
works: the client that drives input is the one whose `clientId` matches, so
input events from the head unit are pushed back to that same client rather
than broadcast. There is no explicit "register" packet — registration is
implicit: by issuing `RawData` on a channel a client becomes the
`clientId` associated with that channel's traffic on the AA side
(`main.cpp:99` passes `clients[scl]`). Cross-ref [08] for the input
handshake/event semantics that ride on top of this tunnel.

## Disconnect & cleanup

- Client closes / `read()` returns 0 → `SocketClient` fires `disconnected()`
  and the reader thread exits (`SocketClient.cpp:48-50`).
- `main.cpp:114-118`: on `disconnected`, the server calls
  `aac.disconnected(clients[scl])` (so the AA side can drop that client's
  channel ownership) and erases the client from the map.
- A failing `RawData` injection also force-disconnects: the exception path at
  `main.cpp:100-106` calls `aac.disconnected(...)`, erases the client, and
  rethrows (which unwinds the reader thread and `close()`s the fd,
  `SocketClient.cpp:21-24`).
- `SocketClient` destructor cancels and joins the reader thread, then
  `close(fd)` (`SocketClient.cpp:12-16`). `SocketCommunicator` destructor stops
  the accept loop, `close()`s the listen socket, `unlink()`s `./socket`, and
  deletes remaining clients (`SocketCommunicator.cpp:66-73`).
- `sendMessage` is a single raw `write()`; a short write throws, and `ECONNRESET`
  is mapped to `client_disconnected_error` (`SocketClient.cpp:62-72`).

## Example session

A client wants to feed/receive on the video channel.

```
client                                  AAServer (./socket)
  | connect AF_UNIX SOCK_SEQPACKET ----> | accept → clientId = N
  |                                      |
  | [02 .. ..]  GetServiceDescriptor --> |
  | <-- raw ServiceDiscoveryResponse pb  |   (decode per [05])
  |                                      |
  | [00 00 ..]  GetChannelNumberByChannelType,
  |             channelNumber = Video(0) -> |
  | <-- [id]    single channel-id byte   |   say id = 0x03
  |                                      |
  | [01 03 00 <aa payload...>] RawData --> | aac.sendToChannel(N, 3, false, payload)
  |                                      |   → framed+encrypted onto AA ch 3 ([06]/[07])
  |                                      |
  |        ... head unit replies on ch 3 ...
  | <-- [03 00 <aa payload...>]          |   push: channelNumber=3, specific=0x00
  | <-- [03 ff <aa payload...>]          |   push: same channel, Specific flag set
  |                                      |
  | (close)  ---------------------------> | disconnected → aac.disconnected(N)
```

Byte legend: request packets are `[packetType][channelNumber][specific][data]`
(3-byte prefix); pushes are `[channelNumber][specific 0x00|0xff][data]`
(2-byte prefix). All over `SOCK_SEQPACKET`, so each line is exactly one
datagram — no length fields anywhere.

## Relationship to `smartcar`

`smartcar` is free to design its own local-consumer API (gRPC, its own Unix
socket, in-process channels, …). The reusable ideas here, independent of the
exact bytes:

1. Resolve dynamic AA channel ids on behalf of the client (the
   `ChannelType → channel id` indirection) so consumers never depend on the
   head unit's assignment.
2. Expose the raw `ServiceDiscoveryResponse` instead of a re-modelled API, so
   the consumer can use the AA schema directly ([05]).
3. A single bidirectional "raw channel" tunnel + per-client targeting is
   enough to support stateful channels like input ([08]) without a bespoke
   per-channel IPC surface.
4. A message-boundary-preserving transport removes all framing code on the
   IPC hop; the only framing that matters is the AA wire framing ([02]).

Cross-refs: service discovery [05], video channel [07], input channel [08],
generic channel lifecycle [06], AA framing [02].
