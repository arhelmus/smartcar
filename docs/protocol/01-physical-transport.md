# 01 — Physical Transport

How bytes get on and off the wire below the frame layer. Actors, roles, and the
terminology trap ("server" = projection source = AOAP *accessory* = app-layer
TLS server) are defined in [`00-overview.md`](00-overview.md) — not repeated here.
Frame parsing/encryption is [`02-framing.md`](02-framing.md); this doc stops at
"a buffer of frame bytes goes out EP1 / comes in EP2".

Behavioural reference: AACS `AAServer`, which is **USB-only**. The TCP variant
exists in aasdk but not in AACS — see [§5](#5-tcp-variant).

---

## 1. Two USB personas

The server presents the gadget **twice**, with a USB re-enumeration in between.

| Phase | VID:PID | Strings | Functions | Source |
|-------|---------|---------|-----------|--------|
| Initial (mode-switch) | `12d1:107e` | mfr `TAG`, prod `AAServer`, serial `TAGAAS` | MassStorage + FunctionFS (`descriptors_default`) | `AAServer/src/ModeSwitcher.cpp:47` (vid/pid), `:48` (strings) |
| Accessory (operational) | `18d1:2d00` | mfr `TAG`, prod `AAServer`, serial `TAGAAS` | FunctionFS only (`descriptors_accessory`) | `AAServer/src/AaCommunicator.cpp:488-489` (vid/pid), `:490` (strings) |

`18d1:2d00` is the standard Google AOAP accessory id (`18d1` = Google,
`2d00` = accessory, no ADB). `12d1:107e` is the pre-switch identity AACS picks
to get the head unit to talk AOAP to it. Both literals are in the source as
hex `Gadget(lib, 0x.., 0x.., …)` constructor args at the lines above; the
`18d1:2d00` literal is on line 489 because the constructor call wraps across
`AaCommunicator.cpp:488-489`. The third `setStrings` argument is `sr("TAGAAS")`
(a USB serial-number string), the same on both personas.

The initial gadget also exposes a 4 MiB mass-storage LUN backed by a temp file
(LUN file written at `ModeSwitcher.cpp:50-54`, `MassStorageFunction` bound at
`:55-56`); this is incidental scaffolding for the composite gadget, not part of
the AA protocol.

---

## 2. AOAP mode switch (control requests 51 / 52 / 53)

Driven entirely from FunctionFS **ep0** on the *initial* `12d1:107e` gadget.
`ModeSwitcher::handleSwitchToAccessoryMode` opens
`<mountpoint>/ep0`, writes `descriptors_default` + strings
(`ModeSwitcher.cpp:68-69`), enables the gadget, then loops `read(ep0)` →
`handleSwitchMessage` (`ModeSwitcher.cpp:77-90`).

`handleSwitchMessage` (`ModeSwitcher.cpp:21-44`) reads
`usb_functionfs_event` records and only acts on `FUNCTIONFS_SETUP`, dispatching
on `setup.bRequest`:

| `bRequest` | dec | Meaning (AOAP) | Server action | Source |
|-----------|-----|----------------|---------------|--------|
| `0x33` | **51** | Get-Protocol (host asks accessory protocol version) | reply 2 bytes `0x02 0x00` on ep0 → AOAP protocol **2** (LE u16) | `ModeSwitcher.cpp:27-29` |
| `0x34` | **52** | Send-String (host pushes an identifying string, indexed by `wIndex`) | logs `wIndex`, otherwise no-op | `ModeSwitcher.cpp:30-34` |
| `0x35` | **53** | Start-Accessory | logs `"Got 53, exit"`, `return 0` (see loop note below) | `ModeSwitcher.cpp:35-37` |

**Loop-exit subtlety (corrected):** `handleSwitchMessage` returning `0` on
req 53 does **not** by itself break the ep0 read loop. In
`handleSwitchToAccessoryMode` (`:77-90`) the loop's only `break` is gated on
the variable `length`, which holds the return of the *read* call (`:79`), not
the return of `handleSwitchMessage`. The `checkError(handleSwitchMessage(...))`
result on `:86` is **discarded**. So after req 53 the loop iterates again and
blocks in `read(ep0)`; it exits only when that read fails (`length == -1` at
`:88-89`) — which is what happens once the head unit re-enumerates the bus and
the initial gadget's ep0 fd dies. The `return 0` is effectively just an early
exit from processing the remaining events in the current buffer.

Sequence: head unit issues **51**, server answers protocol `2`; head unit
issues zero-or-more **52** strings; head unit issues **53**; the head unit then
re-enumerates the bus, the ep0 read on the initial gadget fails, its loop
exits and the fd is closed (`:91-93`), and the caller brings up the
`18d1:2d00` accessory gadget. The control-request numbers and the `\002\000`
reply are quoted verbatim at the lines cited (`write(fd, "\002\000", 2)` at
`ModeSwitcher.cpp:28`).

**Not in the AACS source — stated plainly:** AACS *responds* to the AOAP
control transfers but never *parses* the 51/52/53 string payloads. Req 52 only
logs `setup.wIndex` and never reads the string body — note `setup.wLength` is
compared against `nbytes` but the payload is never copied or stored
(`ModeSwitcher.cpp:30-34`). The exact AOAP string-descriptor handshake (the
manufacturer/model/description/version/URI/serial string indices 0–5, AOAP
control request 52 per index) is **not represented** in this code path; do not
infer its contents or ordering from here.

The two-persona orchestration is in `AAServer/main.cpp:52-54`:
`handleSwitchToAccessoryMode(lib)` runs to completion (its ep0 loop blocks
until the initial gadget's fd dies on re-enumeration), then `AaCommunicator`
is constructed and `aac.setup(...)` brings up `18d1:2d00`. The initial
gadget's teardown is therefore **function-scope RAII**: the local
`Gadget`/`MassStorageFunction`/`FfsFunction`/`Configuration` objects in
`handleSwitchToAccessoryMode` destruct as that function returns — there is no
explicit AA-protocol teardown message, and the host-side re-enumeration
trigger (what makes the head unit drop and re-probe the bus after req 53) is
**not in this source**.

---

## 3. FunctionFS descriptors

Both personas write a FunctionFS descriptor blob **then** a strings blob to
ep0, in that order, as two separate `write()` calls (`descriptors.cpp:161-170`:
`write_descriptors_accessory` / `write_descriptors_default` each do
`write(fd,&descriptors,…)` then `write(fd,&strings,…)`).

**Descriptor blob layout** (one packed C struct, little-endian throughout —
all scalar fields are `cpu_to_le*`):

1. `struct usb_functionfs_descs_head_v2 header` —
   `magic = FUNCTIONFS_DESCRIPTORS_MAGIC_V2` (so this is FunctionFS **v2**),
   `length = sizeof(whole blob)`, `flags` (see below)
   (`descriptors.cpp:19-25` / `:100-107`).
2. `__le32 fs_count` then `__le32 hs_count` — descriptor counts for the
   full-speed and high-speed sets (present because the `flags` set both
   `FUNCTIONFS_HAS_FS_DESC | FUNCTIONFS_HAS_HS_DESC`). Accessory: `3` and `3`
   (`:26-27`); default: `1` and `1` (`:108-109`).
3. `fs_descs` then `hs_descs`, **identical** to each other: a packed
   `{ intf; sink; source }` for the accessory blob, or just `{ intf }` for the
   default blob. There is no SS (SuperSpeed) descriptor set and no OS / class
   descriptors.

**Strings blob layout** (`descriptors.cpp:140-159`): `struct
usb_functionfs_strings_head header` (`magic = FUNCTIONFS_STRINGS_MAGIC`,
`length`, `str_count = 1`, `lang_count = 1`) followed by one language block:
`__le16 code = 0x0409` (en-US) then the NUL-terminated literal
`"Android Accessory Interface"`. The same `strings` struct is shared verbatim
by both personas.

### `descriptors_accessory` — operational (`descriptors.cpp:18-90`)

One vendor-specific interface, **2 bulk endpoints**, identical FS and HS
descriptor sets (`fs_count = hs_count = 3`: intf + 2 ep):

| Field | Value | Source |
|-------|-------|--------|
| `bInterfaceClass` / `SubClass` | `USB_CLASS_VENDOR_SPEC` / `USB_SUBCLASS_VENDOR_SPEC` | `descriptors.cpp:35-36` (FS), `:67-68` (HS) |
| `bInterfaceProtocol` | `0x00` | `descriptors.cpp:37` (FS), `:68` (HS) |
| `iInterface` | `1` (→ string blob) | `descriptors.cpp:38` (FS), `:69` (HS) |
| `bNumEndpoints` | `2` | `descriptors.cpp:34` (FS), `:65` (HS) |
| **EP1** (sink) | `bEndpointAddress = 1 \| USB_DIR_IN` → **IN, ep #1**, `bmAttributes = USB_ENDPOINT_XFER_BULK`, `wMaxPacketSize = 512`, `bInterval = 0` | `descriptors.cpp:40-48` (FS, addr `:44`), `:71-79` (HS, addr `:75`) |
| **EP2** (source) | `bEndpointAddress = 2 \| USB_DIR_OUT` → **OUT, ep #2**, `bmAttributes = USB_ENDPOINT_XFER_BULK`, `wMaxPacketSize = 512`, `bInterval = 0` | `descriptors.cpp:49-57` (FS, addr `:53`), `:80-88` (HS, addr `:84`) |

Endpoint naming reflects the *gadget's* point of view:

- **EP1 / IN** — gadget → host. **Server transmits frames here** (server →
  head unit). The struct member is named `sink`.
- **EP2 / OUT** — host → gadget. **Server receives frames here** (head unit →
  server). The struct member is named `source`.

`wMaxPacketSize = 512` is the USB bulk max-packet for high-speed; it is *not*
the AA frame limit. The AA send path self-limits at `maxSize = 2000`
(`AaCommunicator.cpp:372`), fragmenting larger messages — see
[`02-framing.md`](02-framing.md).

### `descriptors_default` — mode-switch placeholder (`descriptors.cpp:99-136`)

Same vendor-spec interface but **`bNumEndpoints = 0`** in both the FS and HS
sets (`descriptors.cpp:116` and `:129`) — no data endpoints, only ep0
(`fs_count = hs_count = 1`, `:108-109`). The blob `flags` add
`FUNCTIONFS_ALL_CTRL_RECIP` on top of `HAS_FS_DESC | HAS_HS_DESC`
(`descriptors.cpp:105-106`) so the function receives control transfers for
*all* recipients (not just interface-directed) on ep0 — that is what lets
`handleSwitchMessage` observe the vendor-class 51/52/53 setup packets the host
issues during AOAP negotiation. The accessory blob does **not** set this flag
(`descriptors.cpp:23-24`).

### Strings (`descriptors.cpp:138-159`)

Single string, `str_count = 1`, lang code `0x0409` (en-US, `:156`):
`"Android Accessory Interface"` (the `STR1` `#define`, `descriptors.cpp:138`),
referenced as `iInterface = 1` by both descriptor sets. The identical `strings`
struct is appended after the descriptor blob by both
`write_descriptors_accessory` and `write_descriptors_default`
(`descriptors.cpp:161-170`).

---

## 4. The ep0 / ep1 / ep2 file model and thread wiring

Once on `18d1:2d00`, `AaCommunicator::setup` (`AaCommunicator.cpp:487-514`)
opens three FunctionFS files under the function mountpoint and gives each its
own pump thread via `startThread(fd, readFun, writeFun)`
(`AaCommunicator.cpp:506-511`). `dataPump` calls `readFun(fd,buf)` then loops
`writeFun(fd,buf,…)` over the bytes read — so for each endpoint the "write fn"
is whatever was passed third, *regardless of actual data direction*:

| File | fd | `startThread` readFun → writeFun | Wire direction | Role |
|------|----|----------------------------------|----------------|------|
| `ep0` | `ep0fd` | `readWraper` → `handleEp0Message` | control | USB control events only |
| `ep1` | `ep1fd` | `getMessage` → `write` | gadget→host (IN) | **outbound** frames to head unit |
| `ep2` | `ep2fd` | `readWraper` → `handleMessage` | host→gadget (OUT) | **inbound** frames from head unit |

Note `ep1`'s readFun is `getMessage` (a send-queue pull, not a wire read) and
its writeFun is the libc `write` — the only thread where the "read"/"write"
roles line up with the wire (`AaCommunicator.cpp:508-509`).

Descriptors+strings are written to `ep0fd` (`write_descriptors_accessory`,
`AaCommunicator.cpp:502`) *immediately after* `ep0` is opened and *before*
`ep1`/`ep2` are opened (`:503-504`), before any thread is started
(`:506-511`), and before `mainGadget->enable(udc)` (`AaCommunicator.cpp:513`).

`startThread` (`AaCommunicator.cpp:535-545`) spawns one `dataPump` thread per
endpoint; `dataPump` (`AaCommunicator.cpp:547-577`) is a generic
read-fn → write-fn loop over a 100 KiB buffer:

- **ep0 thread** — `readWraper`(read ep0) → `handleEp0Message`.
  `handleEp0Message` is the **writeFun**, not a separate echo step: it
  (`AaCommunicator.cpp:452-462`) iterates the `usb_functionfs_event` records
  the read produced, logs each `event->type`, throws `aa_runtime_error` on
  `FUNCTIONFS_SUSPEND`, and otherwise returns `nbytes` to mark the buffer
  consumed. It **never calls `write(ep0,…)`** and never handles setup requests
  — the operational gadget's `descriptors_accessory` doesn't set
  `FUNCTIONFS_ALL_CTRL_RECIP`, and the 51/52/53 negotiation already finished on
  the *initial* gadget (§2). This thread exists mainly to notice
  suspend/teardown and unblock the others.
- **ep1 thread (TX)** — read fn is `getMessage`, which **pulls from the
  internal send queue** (not from the fd) and serialises one frame into the
  buffer; the buffer is then `write()`-ten to ep1. `getMessage` does the
  TLS encrypt + framing + fragmentation; details in
  [`02-framing.md`](02-framing.md) / [`04-tls-auth.md`](04-tls-auth.md). The
  `select`-based `readWraper` is **not** used here.
- **ep2 thread (RX)** — `readWraper`(read ep2) → `handleMessage`, which parses
  one frame header, decrypts if needed, and dispatches
  (`AaCommunicator.cpp:344-362`). One read may carry multiple frames;
  `dataPump`'s inner loop re-invokes the write fn (`handleMessage`) on the
  remaining bytes until the buffer is drained
  (`AaCommunicator.cpp:561-570`, return value = bytes consumed).

`readWraper` (`AaCommunicator.cpp:516-533`) wraps `read()` with a 1 s
`select()` timeout, returning `0`/`EINTR` on timeout so the pump stays
responsive to shutdown (`checkTerminate`, SIGUSR1 in the dtor at
`AaCommunicator.cpp:594-597`).

Key asymmetry to remember: **ep2 is event-driven** (read blocks on the wire),
**ep1 is queue-driven** (`getMessage` blocks ≤1 s on `sendQueueNotEmpty`,
`AaCommunicator.cpp:364-369`). Producers anywhere in the app call
`sendMessage` (`AaCommunicator.cpp:61-73`) which just enqueues; the ep1 thread
does the actual wire write.

---

## 5. TCP variant

aasdk supports an AA transport over TCP (a head unit reachable at an IP:port
instead of USB), with the **identical frame layer above it** — same channel /
flags / length header, same TLS, same message ids. In aasdk this is structural:
a common `transport::Transport` base (`aasdk/Transport/Transport.hpp`,
`receive`/`send` + send/receive queues) with two concrete subclasses —
`USBTransport` over an `IAOAPDevice` (`aasdk/Transport/USBTransport.hpp`) and
`TCPTransport` over a boost-asio socket `TCPEndpoint`
(`aasdk/Transport/TCPTransport.hpp`, `aasdk/TCP/TCPEndpoint.hpp`). Both satisfy
the same `ITransport` interface (`receive(size,…)` / `send(data,…)` / `stop()`,
`aasdk/Transport/ITransport.hpp`); only the byte-moving leaf differs, so
everything above the transport is shared. **AACS does not implement TCP**:
there is no socket path in `AAServer`; everything is FunctionFS endpoints as
above. (The aasdk role is also reversed — aasdk is the *client/head-unit*
library, not the projection source — so it is a structural cross-reference for
the transport seam, not a behavioural reference for the server.)

For `smartcar`, the practical consequence: the transport seam is exactly the
ep1-out / ep2-in byte boundary. A TCP transport substitutes a socket for the
ep1/ep2 fds; nothing in [`02-framing.md`](02-framing.md) and above changes.
The `openauto` emulator used in dev is reached over TCP, so smartcar's
transport layer is the TCP variant even though the AACS reference is USB —
treat §4's ep1/ep2 split as "TX stream / RX stream" and the mapping holds.

---

## Quick reference

- Mode-switch gadget: `12d1:107e`, ep0-only (`bNumEndpoints = 0`,
  `FUNCTIONFS_ALL_CTRL_RECIP`), ctrl reqs **51** (reply `02 00` = AOAP proto
  2, LE), **52** (string, logged & ignored), **53** (logged; mode-switch
  actually ends when the host re-enumerates and the ep0 read fails — req 53's
  `return 0` does *not* itself break the loop).
- Operational gadget: `18d1:2d00`, FunctionFS, **EP1 IN** = server→HU (TX),
  **EP2 OUT** = HU→server (RX), bulk, `wMaxPacketSize 512`.
- Threads: ep2 = blocking read → `handleMessage`; ep1 = `getMessage`
  (send-queue) → `write`; ep0 = control events only.
- AA send-side fragmentation threshold `2000` B, not `512` — see
  [`02-framing.md`](02-framing.md).
- TCP transport: aasdk-only, frame layer unchanged; AACS is USB-only.
