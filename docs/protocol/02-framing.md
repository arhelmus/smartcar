# 02 — Frame Format & Fragmentation

The transport (`01-physical-transport.md`) delivers an opaque byte stream. Every
logical message rides inside one or more **frames**. This doc is the
authoritative definition of the frame header, the flag byte, multi-frame
fragmentation/reassembly, and the plaintext-vs-ciphertext payload boundary.

All multi-byte integers are **big-endian**. Code references are to the AACS
server (`AAServer/src/AaCommunicator.cpp`) and `AAServer/include/enums.h`
(physically `AACS/include/enums.h` in the checkout). Inbound = "← HU"
(head unit → server), outbound = "→ HU" (server → head unit).

## 1. Frame header

```
 byte:  0        1        2        3       [4    5    6    7]              N
      +--------+--------+--------+--------+----+----+----+----+   ...   +----+
      |channel | flags  |    length BE    |     total_len BE     | payload  |
      +--------+--------+--------+--------+----+----+----+----+   ...   +----+
        u8       u8       u16 (len of payload)   u32, FIRST-frame only
```

| Off | Size | Field       | Meaning |
|-----|------|-------------|---------|
| 0   | 1    | `channel`   | channel id; `0` = control channel (`03-control-channel.md`). Feature channels assigned in service discovery (`05-service-discovery.md`). |
| 1   | 1    | `flags`     | bitfield, see §2. |
| 2   | 2    | `length`    | byte count of the `payload` that follows in **this** frame. Does **not** count the header, and does **not** count the `total_len` field. |
| 4   | 4    | `total_len` | **present only on a FIRST frame** (§3). u32 = total size of the fully reassembled, *post-decryption* payload across all frames of this message. |
| 4 or 8 | `length` | `payload` | frame body. Plaintext during handshake, TLS ciphertext afterward (§5). |

Header is **4 bytes** for single/intermediate/last frames, **8 bytes** for a
FIRST frame.

Outbound header is written byte-by-byte at `AaCommunicator.cpp:428-436`:
`encBuf[0]=channel` (`:428`), `encBuf[1]=flags` (`:429`), `length` split
big-endian `encBuf[2]=length>>8` / `encBuf[3]=length&0xff` (`:430-431`),
`total_len` `encBuf[4..7]` only on a FIRST frame (`:433-436`). Inbound header is
parsed at `AaCommunicator.cpp:346-351`.

> Caveat: the inbound parser reads `length` via
> `be16_to_cpu(*(__u16*)(byteView+2))` (`:351`; a sign-extension-free raw copy
> `lengthRaw` at `:350` is computed but unused). It never reads or skips a
> `total_len` field on inbound frames, and the `4` in its bounds check / return
> is a hard-coded base-header size that does **not** widen to `8` for a FIRST
> frame — see §6.

> aasdk cross-check: aasdk splits the same wire bytes into two logically
> separate objects — a 2-byte `FrameHeader` (`FrameHeader::getSizeOf()==2`:
> `channel` + `flags`) and a `FrameSize` that is **2 bytes** (`SHORT`) or
> **6 bytes** (`EXTENDED`: 2-byte `frameSize` + 4-byte `totalSize`), chosen
> `EXTENDED` iff the frame type is `FIRST`
> (`MessageInStream.cpp:97-98`, `FrameSize::getSizeOf` `FrameSize.cpp:73-75`).
> `2 + 2 = 4` and `2 + 6 = 8` — byte-for-byte identical to AACS's header /
> FIRST-header sizes and field order. The `total_len`/`length` split below is
> therefore not an AACS quirk; it is the shared wire format.

## 2. Flag byte

`enums.h` defines three independent enums that are OR'd into the single `flags`
byte:

```
 bit:  7  6  5  4   3        2        1  0
      [ unused    ] [enc]   [spec]   [frame type]
```

| Bits | Enum | Values | Source |
|------|------|--------|--------|
| 3 (`0x08`) | `EncryptionType` | `Plain = 0`, `Encrypted = 1<<3 = 0x08` | `enums.h:5-8` |
| 2 (`0x04`) | `MessageTypeFlags` | `Control = 0`, `Specific = 1<<2 = 0x04` | `enums.h:16-19` |
| 1-0 (`0x03`) | `FrameType` | `First = 1`, `Last = 2`, `Bulk = First\|Last = 3` | `enums.h:10-14` |

Key non-obvious fact: **`Bulk == 0x3 == First | Last`**. It is not a distinct
fourth code point — it is "first *and* last", i.e. a single self-contained
frame. This makes the frame-type field a 2-bit value with four meanings:

| `flags & 0x3` | `FrameType` | Meaning |
|---------------|-------------|---------|
| `0x0` | (none) | not used by AACS |
| `0x1` | `First`  | first frame of a multi-frame message (carries `total_len`) |
| `0x2` | `Last`   | final frame of a multi-frame message |
| `0x3` | `Bulk`   | complete message in one frame (= First & Last) |

Because `Bulk` shares bits with both `First` and `Last`, AACS deliberately uses
**masked equality**, not a bit test, to distinguish a true FIRST frame:

```cpp
if ((flags & FrameType::Bulk) == FrameType::First)   // AaCommunicator.cpp:421, 432
```

`(flags & 0x3) == 0x1` is true **only** for a pure First frame and is false for
both `Bulk` (`0x3`) and `Last` (`0x2`). This exact test gates emission of the
`total_len` field (§3). Inbound, the receiver instead masks the raw type:
`FrameType frameType = (FrameType)(flags & 0x3)` (`AaCommunicator.cpp:349`).

### Encryption bit

`encrypted = flags & 0x8` (`AaCommunicator.cpp:348`). Handshake-phase frames
(Version Request/Response, all `SslHandshake` frames) are sent `Plain`
(`:82, :289`); everything from `ServiceDiscoveryRequest` onward is `Encrypted`
(`:103, :267`). See §5 and `04-tls-auth.md`.

### Specific bit

`Specific` (`0x04`) marks a channel-specific message vs. a control-style
message *within* a channel. On the control channel it is forwarded as
`message.flags & MessageTypeFlags::Specific` (`AaCommunicator.cpp:167`). Channel
semantics are in `06-channel-lifecycle.md`; it does not affect framing.

## 3. The `total_len` rule

A FIRST frame — and **only** a FIRST frame — is followed by a 4-byte
big-endian `total_len` *before* its payload. Outbound, the writer reserves the
extra 4 bytes by bumping the payload offset:

```cpp
auto offset = 4;                                          // :420  base header
if ((flags & FrameType::Bulk) == FrameType::First)        // :421
  offset += 4;                                            // :422  reserve total_len
auto length = BIO_read(writeBio, encBuf + offset, ...);   // :424  payload after header(+total_len)
...
if ((flags & FrameType::Bulk) == FrameType::First) {      // :432
  encBuf[4] = (totalLength >> 24) & 0xff;                 // :433
  encBuf[5] = (totalLength >> 16) & 0xff;                 // :434
  encBuf[6] = (totalLength >>  8) & 0xff;                 // :435
  encBuf[7] = (totalLength >>  0) & 0xff;                 // :436
}
```

`totalLength` is captured **once, up front** as
`uint32_t totalLength = msg.content.size()` (`AaCommunicator.cpp:375`) — i.e.
the size of the **plaintext** message *before* fragmentation and *before* TLS
encryption. The per-frame `length` field, by contrast, is the size of the
**ciphertext** chunk for that frame (`length = BIO_read(...)`, `:424`,
`:430-431`). So for encrypted multi-frame messages `total_len` and the sum of
per-frame `length`s are **not** numerically equal; `total_len` describes the
decrypted whole, each `length` describes an encrypted slice.

`Bulk` (single-frame) and `Last`/intermediate frames use the bare 4-byte
header — no `total_len` (the `:421` / `:432` mask is false for them).

> aasdk cross-check (confirms the plaintext/ciphertext split): in
> `MessageOutStream::compoundFrame` (`MessageOutStream.cpp:103-127`) the frame
> is built `FrameSize(payloadSize, totalSize)` where `payloadSize` is the
> **post-encrypt** byte count returned by `cryptor_->encrypt` (`:112`) and
> `totalSize` is `message_->getPayload().size()` — the **pre-encrypt,
> pre-split** whole-message size (`:118`, `setFrameSize :122-127`). i.e.
> aasdk's `frameSize` ≙ AACS per-frame `length` (ciphertext), aasdk's
> `totalSize` ≙ AACS `total_len` (plaintext whole). Independent confirmation
> that `total_len` is the decrypted total and is **not** the sum of per-frame
> `length`s for encrypted messages.

## 4. Outbound fragmentation (`getMessage`, `AaCommunicator.cpp:364-450`)

`getMessage` pulls one queued `Message` and emits exactly one frame per call,
re-queuing the remainder with an advanced `msg.offset` until drained.

Frame size cap: `int maxSize = 2000;` (`AaCommunicator.cpp:372`). The comment
notes it "should work up to about 16k" but 2000 is used for hardware safety.
`maxSize` bounds the **plaintext slice** fed to `SSL_write` per frame, not the
resulting ciphertext frame size.

### Plaintext branch (`:439-449`)

If `flags` has the encryption bit clear, the whole `msg.content` is emitted in
one frame regardless of size: `[channel][flags][len:2 BE][payload]`, length =
`msg.content.size()` (`pushBackInt16`, `:444`). No fragmentation, no
`total_len`. Used for the handshake (Version, `SslHandshake`).

### Encrypted branch (`:377-438`)

Let `remaining = msg.content.size() - msg.offset`. The branch is chosen by
`remaining` vs. `maxSize` and the current frame-type bits:

| Condition | Branch | Flag transition | Queue action |
|-----------|--------|------------------|--------------|
| `remaining <= maxSize` **and** `flags & Bulk` | **full** (`:383-388`) | keep `Bulk` (`0x3`) | `pop_front()` — done |
| `remaining > maxSize` **and** `flags & Bulk`  | **first** (`:390-398`) | `flags &= ~Bulk; flags \|= First` ⇒ `0x1` | re-queue: `flags &= ~Bulk`, `offset += maxSize` |
| `remaining > maxSize` (Bulk already cleared) | **intermediate** (`:400-405`) | `flags &= ~Bulk` ⇒ `0x0` in type bits | re-queue: `flags &= ~Bulk`, `offset += maxSize` |
| else (`remaining <= maxSize`, Bulk cleared) | **last** (`:407-412`) | `flags \|= Last` ⇒ `0x2` | `pop_front()` — done |

Notes:

- `flags & FrameType::Bulk` is true if **either** bit 0 or bit 1 is set
  (`Bulk = 0x3`). On the original queued message the type bits are `Bulk`
  (`0x3`) — see callers using `... | FrameType::Bulk` (`:82, :103, :267`). So
  the *first* `getMessage` call on a message always tests true for "Bulk".
- The **first** branch clears the Bulk bits and sets `First` for the emitted
  frame's flags, and **also** rewrites the re-queued message's flags to
  `flags & ~Bulk` (`:396`) — clearing the type bits to `0x0`. Subsequent calls
  therefore fall into **intermediate** until `remaining <= maxSize`, at which
  point **last** sets `Last` (`0x2`).
- Encryption bit (`0x08`) and `Specific` bit (`0x04`) are preserved across all
  transitions (only the low 2 bits are rewritten).
- Per-frame `length` (`encBuf[2..3]`) is the ciphertext length from
  `BIO_read` (`:424, :430-431`), i.e. however many bytes TLS produced for that
  2000-byte plaintext slice — not necessarily 2000.

So the frame-type sequence for a fragmented message is:

```
First(0x1, +total_len)  →  intermediate(type=0x0)  →  …  →  Last(0x2)
```

Exactly one `First`, zero or more intermediates, exactly one `Last`. `Bulk`
(`0x3`) never appears in a fragmented message — it is the single-frame case
only.

## 5. Plaintext vs. ciphertext payload

| Phase | Enc bit | `payload` bytes | Reassembly target |
|-------|---------|-----------------|-------------------|
| Version negotiation, `SslHandshake` | `0` (`Plain`) | raw protocol bytes | plaintext message |
| Auth complete onward (service discovery, channels, ping, media) | `1` (`Encrypted`) | TLS records | decrypt → plaintext message |

Outbound encrypted frames: the plaintext slice goes through `SSL_write`, then
the ciphertext is read out of the memory `writeBio` into the frame body
(`AaCommunicator.cpp:414, :424`). Inbound encrypted frames: the frame body is
fed to `decryptMessage`, which `BIO_write`s it into `readBio` and `SSL_read`s
plaintext back (`:194-218`, called from `:359`). TLS setup, the SSL BIO
plumbing, and the handshake message flow are in `04-tls-auth.md`.

**Decrypted payload layout.** After decryption (or, for handshake frames, the
plaintext directly), the message body begins with a **2-byte big-endian
message id**:

```
 +----+----+--------------------------+
 | message_id : u16 BE |   protobuf / opaque body
 +----+----+--------------------------+
```

Read everywhere as `be16_to_cpu(((const __u16*)content.data())[0])`
(`AaCommunicator.cpp:153, :224`). Its namespace depends on `channel`: control
channel ids are `MessageType` (`enums.h:21-37`), feature channels use per-channel
enums e.g. `MediaMessageType` / `InputChannelMessageType` (`enums.h:39-54`). The
id is disambiguated by channel, then dispatched. Full id catalog and routing:
`03-control-channel.md` and `10-message-catalog.md`.

## 6. Inbound parsing & reassembly (`handleMessage`, `AaCommunicator.cpp:344-362`)

```cpp
int channel  = byteView[0];                              // :346
int flags    = byteView[1];                              // :347
bool encrypted = flags & 0x8;                            // :348
FrameType frameType = (FrameType)(flags & 0x3);          // :349
int lengthRaw = *(__u16*)(byteView + 2);                 // :350  raw LE copy, UNUSED
int length = be16_to_cpu(*(__u16*)(byteView + 2));       // :351
if (nbytes < 4 + length)                                 // :353
  throw runtime_error("nbytes<4+length");                // :354  length validation
std::copy(byteView + 4, byteView + 4 + length, ...msg);  // :355  payload = [4 .. 4+length)
message.content = encrypted ? decryptMessage(msg) : msg; // :359
handleMessageContent(message);                           // :360
return length + 4;                                       // :361  bytes consumed
```

Length validation: the call is rejected unless `nbytes >= 4 + length`
(`:353`). Note the bound is the bare `4` header — it is **not** widened to `8`
when the FIRST bit is set, so a fragmented inbound frame is not even
length-checked against its true `8 + length` extent. `handleMessage` is wired
as the inbound thread's `writeFun` (`startThread(ep2fd, readWraper, handleMessage)`,
`:510-511`). `dataPump` does one `read()` of up to `bufSize = 100*1024` bytes
(`:548, :556`) then loops `while (length > 0) { partLength = writeFun(buffer+start,
length); length -= partLength; start += partLength; }` (`:561-570`,
`dataPump`, `:547-577`). So `handleMessage`'s `length + 4` return (`:361`) is
exactly the cursor advance: several whole frames packed into one read are parsed
in sequence, but only on the assumption every return value lands on a frame
boundary.

**Reassembly asymmetry — read carefully.** AACS's inbound path:

- Computes `frameType` (`:349`) but **does not branch on it** — `First`,
  `Last`, `Bulk` are all handled identically.
- Treats the payload as starting at **offset 4** unconditionally (`:355`,
  `:361`). It does **not** detect a FIRST frame and does **not** skip the
  4-byte `total_len` field.

In other words AACS's receiver assumes inbound messages from the head unit are
**single-frame (`Bulk`)**, and there is no inbound chunk accumulator. Precise
failure mode on a genuinely fragmented inbound message: for the FIRST frame the
real layout is `[ch][fl][len:2][total_len:4][payload…]`, but `:355` copies
`[byteView+4 .. byteView+4+length)` — so the 4 `total_len` bytes become the
first 4 bytes of `msg`, the true payload is truncated by 4, then `:359` hands
that corrupted buffer to `decryptMessage` (TLS record desync) or, if plaintext,
to a message-id parse that reads `total_len`'s high half as the id. The
`return length + 4` (`:361`) is also short by 4 (it should be `length + 8`), so
the `dataPump` cursor lands 4 bytes before the next frame and every subsequent
frame in that read is mis-aligned. `Last`/intermediate frames carry no
`total_len`, but with no accumulator each is treated as a standalone message and
decrypted on its own, which fails for a multi-frame TLS record. AACS only
survives in practice because the head unit's control/handshake traffic fits in
one `Bulk` frame.

This is an asymmetry to preserve awareness of, not to copy: `smartcar`'s
receiver **must** implement real reassembly — on a frame with
`(flags & 0x3) == First`, read the 4-byte `total_len`, accumulate payloads from
the following intermediate/`Last` frames until the reassembled (post-decrypt)
buffer reaches `total_len`, then decrypt if needed and dispatch. `Bulk` frames
are processed immediately.

> aasdk cross-check (the correct receiver, contrast with AACS's gap):
> `MessageInStream` (`MessageInStream.cpp`) implements exactly the reassembly
> AACS lacks. It reads the 2-byte header, then reads `EXTENDED` (6-byte) frame
> size iff `FrameType::FIRST` else `SHORT` (2-byte) (`:97-98`), buffering
> partial messages **per channel** in `messageBuffer_` keyed by `channelId`
> (`:67-94, :158-161`). It resolves the message only on `BULK` or `LAST`
> (`:152-156`); a `FIRST`/`BULK` arriving over an existing buffered message
> restarts it (`:75-80`); a `MIDDLE`/`LAST` with no buffered message is
> rejected as `MESSENGER_INTERTWINED_CHANNELS` (`:84-90`). Decryption is
> per-frame and **appended** into the message's running payload
> (`cryptor_->decrypt(message_->getPayload(), buffer, frameSize_)`, `:134-147`)
> — i.e. each frame's ciphertext is decrypted and concatenated, the
> reassembled plaintext is the message. This both confirms the wire format and
> shows the channel-interleaving subtlety AACS's single-buffer model would also
> get wrong (frames for different channels can interleave; reassembly state
> must be keyed by channel, not global). Divergence to record: **AACS RX has no
> reassembly and assumes `Bulk`; aasdk RX does full per-channel reassembly.**
> Both agree on TX framing and on the `total_len`=plaintext-whole /
> per-frame-size=ciphertext-slice split.

## 7. Worked example — a fragmented encrypted message

Server sends an encrypted control-channel message whose plaintext is **4500**
bytes (2-byte message id + 4498-byte body). `maxSize = 2000`. Plaintext is cut
into 2000/2000/500-byte slices; each slice is `SSL_write`-encrypted and the
resulting ciphertext lengths are denoted `c0`, `c1`, `c2` (each ≈ slice size +
TLS overhead, exact value = the `length` field for that frame).

Queued message: `channel=0`, `flags = Encrypted | Bulk = 0x08 | 0x03 = 0x0B`,
`content.size() = 4500`, `offset = 0`. `totalLength` captured = **4500**
(`:375`).

**Call 1 — FIRST frame** (`remaining=4500 > 2000`, Bulk set ⇒ first branch
`:390-398`):

- emitted `flags = (0x0B & ~0x03) | First = 0x08 | 0x01 = 0x09`
- `(flags & 0x3) == First` ⇒ `total_len` emitted; payload offset = 8 (`:420-422`)
- header: `00 09 [c0 BE:2] [00 00 11 94]` (`0x1194 = 4500`) then `c0` ciphertext bytes
- re-queued: `flags = 0x0B & ~0x03 = 0x08`, `offset = 2000`

```
00 09 | hi(c0) lo(c0) | 00 00 11 94 | <c0 ciphertext bytes>
ch fl    length=c0       total_len=4500
```

**Call 2 — intermediate frame** (`remaining = 4500-2000 = 2500 > 2000`, Bulk
already clear ⇒ intermediate branch `:400-405`):

- `flags = 0x08 & ~0x03 = 0x08` (type bits `0x0`); no `total_len`; payload offset = 4
- header: `00 08 [c1 BE:2]` then `c1` ciphertext bytes
- re-queued: `flags = 0x08`, `offset = 4000`

```
00 08 | hi(c1) lo(c1) | <c1 ciphertext bytes>
```

**Call 3 — LAST frame** (`remaining = 4500-4000 = 500 <= 2000`, Bulk clear ⇒
last branch `:407-412`):

- `flags = 0x08 | Last = 0x08 | 0x02 = 0x0A`; no `total_len`; payload offset = 4
- header: `00 0A [c2 BE:2]` then `c2` ciphertext bytes
- `sendQueue.pop_front()` — message drained

```
00 0A | hi(c2) lo(c2) | <c2 ciphertext bytes>
```

Receiver: on Call-1's frame `(0x09 & 0x3) == 1` (FIRST) → read `total_len`
`= 4500`, start buffer. Append Call-2 (`0x08`, intermediate) and Call-3
(`0x0A`, `Last`) ciphertext. Decrypt the concatenated TLS stream; the
reassembled plaintext is 4500 bytes; verify it equals `total_len`; the first 2
bytes are the BE message id; dispatch by `channel`.

## See also

- `00-overview.md` — frame-anatomy preview & end-to-end sequence.
- `01-physical-transport.md` — how raw frame bytes are carried (USB/TCP).
- `03-control-channel.md` — message-id (`MessageType`) namespace & routing.
- `04-tls-auth.md` — the BIO/`SSL_*` machinery behind the encrypted payload.
- `06-channel-lifecycle.md` — the `Specific` flag in channel context.
- `10-message-catalog.md` — exhaustive message-id ↔ protobuf table.
