# 04 — TLS & Auth

How the session goes encrypted. The TLS handshake is tunnelled inside control
(channel `0`) messages, runs as a multi-round-trip memory-BIO pump, and ends
with an `AuthComplete` that makes the **server** kick off Service Discovery.

Prereq: version negotiation done (cf. `03-control-channel.md`). Frame layout
and the `Plain`/`Encrypted` encryption flag are in `02-framing.md`.

> Terminology trap (see `00-overview.md`): the projection source ("server")
> calls `SSL_accept()` — it holds **TLS server-state** — but the **head unit
> drives** the handshake as TLS client and presents the AA client cert. "Server"
> below always means the projection source.

## Roles at the TLS layer

| | Projection source ("server") | Head unit (cf. aasdk) |
|---|---|---|
| OpenSSL call | `SSL_accept` / `SSL_set_accept_state` | `SSL_do_handshake` / `SSL_set_connect_state` |
| Drives handshake | no — reacts | yes — sends first |
| Presents cert | the server cert/key (`android_auto.crt/.key`) | a client cert |
| Verifies peer cert | `SSL_VERIFY_PEER` + **no-op accept** (see below) | `SSL_VERIFY_NONE` in aasdk — no verification at all |

(Head-unit column cross-referenced against aasdk's `SSLWrapper`
(`server/third_party/aasdk/src/Transport/SSLWrapper.cpp`), the closest available
client-side reference — see "aasdk cross-check" below.)

## SSL context (one-time, at construction)

`initializeSslContext()` (`AAServer/src/AaCommunicator.cpp:302`), called from
the `AaCommunicator` ctor (`AaCommunicator.cpp:466`). Negotiated constraints:

- **Method**: `SSLv23_server_method()` (`:305`) — version-flexible server.
- **TLS 1.3 disabled**: `SSL_CTX_set_options(ctx, SSL_OP_NO_TLSv1_3)` (`:336`).
  Net effect: **TLS 1.2 is the ceiling**; 1.0/1.1 only if the peer insists and
  the linked OpenSSL still allows them. Treat **TLS 1.2** as the target.
- **ECDH auto**: `SSL_CTX_set_ecdh_auto(ctx, 1)` (`:312`) — OpenSSL picks the
  ECDHE curve automatically.
- **Server cert**: `SSL_CTX_use_certificate_file(ctx, "android_auto.crt", PEM)`
  (`:313`).
- **Server key**: `SSL_CTX_use_PrivateKey_file(ctx, "android_auto.key", PEM)`
  (`:317`).
- **DH params required**: `fopen("dhparam.pem")` (`:322`) →
  `PEM_read_DHparams` (`:324`) → `SSL_CTX_set_tmp_dh` (`:332`). A missing file
  (`:327`), an unreadable/invalid PEM (`:330`), or a failing `set_tmp_dh`
  (`:333`) each `throw` — a DH params file **must** be present in the process
  CWD for classic-DHE cipher suites. Note ordering: `SSL_CTX_set_options` is
  the **last** call in `initializeSslContext` (`:336`), after `set_verify`
  (`:335`); the variable is named `dh_2048` but the actual key size is whatever
  the shipped/generated `dhparam.pem` contains — not asserted here.
- **Peer cert verification**:
  `SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, &verifyCertificate)` (`:335`).
  `SSL_VERIFY_PEER` requests the head unit's client cert, **but**
  `verifyCertificate()` (`AaCommunicator.cpp:339`) unconditionally
  `return 1;` — see security note.

### Cert/key files

Shipped in `AACS/AAServer/ssl/`: `android_auto.crt` (1158 bytes) and
`android_auto.key` (1703 bytes) — both PEM. Loaded by basename from the
process CWD (constants `CRT_FILE`/`PRIVKEY_FILE` at `AaCommunicator.cpp:32`,
`:33`; `DHPARAM_FILE` at `:34`). **Provenance unknown**: the AACS repo ships
these files; no in-tree source or this project's `README.md` asserts where the
AA cert/key originate (the README's source-material table lists only
AACS/aasdk/AAProto). Do not claim an openauto/Google origin without evidence.
`dhparam.pem` is **not** shipped in `AACS/AAServer/ssl/` (only the two files
above) — it must be generated/supplied separately or `initializeSslContext`
throws (`:327`). (Key contents are not reproduced.)

## Handshake transport: `SslHandshake` (msg id `3`) over channel 0

The handshake bytes never hit the socket directly — they ride inside control
messages whose 2-byte BE message id is `SslHandshake`, framed `Plain` (the
encryption flag is **off** during the handshake itself; cf. `02-framing.md`).

Dispatch: `handleMessageContent` routes `MessageType::SslHandshake` to
`handleSslHandshake(payload+2, len-2)` (`AaCommunicator.cpp:235`) — i.e. the
TLS bytes are the message body *after* the 2-byte id.

### The memory-BIO pump

`initializeSsl()` (`AaCommunicator.cpp:292`), lazily on first handshake msg:

- `ssl = SSL_new(ctx)`
- `readBio = BIO_new(BIO_s_mem())` — inbound TLS bytes from the head unit
- `writeBio = BIO_new(BIO_s_mem())` — outbound TLS bytes to the head unit
- `SSL_set_accept_state(ssl)` + `SSL_set_bio(ssl, readBio, writeBio)`

No real socket is given to OpenSSL. Each inbound `SslHandshake` message is
pumped through `readBio`; whatever OpenSSL wants to send is drained from
`writeBio` and shipped back as the next `SslHandshake` message.

### One round trip — `handleSslHandshake` (`AaCommunicator.cpp:270`)

```
1. initializeSsl()                          // first call only          (:271)
2. BIO_write(readBio, buf, nbytes)          // feed inbound handshake    (:272)
3. ret = SSL_accept(ssl)                    // advance handshake         (:274)
4. if ret == -1 && err != SSL_ERROR_WANT_READ: throw   // (:275–:279)
5. msg = [ id=SslHandshake (BE u16) ]                                    (:282)
   char buffer[512]                                                      (:283–:284)
   while ((len = BIO_read(writeBio, buffer, 512)) != -1):  // drain ALL  (:286)
       msg += buffer[0..len]
6. sendMessage(ch 0, Plain|Bulk, msg)                                    (:289)
```

Drain-loop detail (`:286`–`:288`): the sentinel is `BIO_read(...) != -1`.
`BIO_read` on a memory BIO returns the number of bytes copied while data
remains and **`-1` once the BIO is drained** (mem BIOs are non-blocking and
report empty-as-EOF this way). The `!= -1` test is evaluated *before* the
`std::copy`, so the terminating iteration never copies a negative length. The
fixed 512-byte stack buffer means a single `SSL_accept` flight larger than
512 bytes (typical: ServerHello+Certificate is far larger) is reassembled
across **many** `BIO_read` iterations into one `msg` vector — i.e. one
`SslHandshake` response message can carry an arbitrarily large flight; the
512 is only the drain chunk size, not a wire limit.

`SSL_accept` returning `-1` with `SSL_ERROR_WANT_READ` is the **normal**,
expected state for every round before the last: it means "I produced my
flight, send it, then bring me the peer's reply." Each call to
`handleSslHandshake` does exactly one feed → advance → drain → send cycle, so
the full TLS 1.2 handshake spans **several** `SslHandshake` request/response
exchanges on channel 0 (ServerHello / cert / key exchange / Finished, etc.).
Any other `SSL_accept` error throws and aborts. On the final round
`SSL_accept` succeeds; the last drained `writeBio` flight is still sent as a
`Plain` `SslHandshake` message.

There is no explicit "handshake done" message from the server side here — the
head unit completes the handshake on its end and signals readiness with
`AuthComplete`.

## `AuthComplete` (msg id `4`) — end of auth

Sent by the head unit on channel 0 after the TLS handshake completes.
`handleMessageContent` (`AaCommunicator.cpp:237`):

```
} else if (messageType == MessageType::AuthComplete) {
    cout << "auth complete";
    sendServiceDiscoveryRequest();          // (:239)
}
```

No body is parsed — its arrival alone is the trigger. The **server**
immediately initiates Service Discovery: `sendServiceDiscoveryRequest()`
(`AaCommunicator.cpp:95`) builds a `ServiceDiscoveryRequest`
(`manufacturer="TAG"`, `model="AAServer"`), prefixes message id
`ServiceDiscoveryRequest`, and sends it on channel 0 with
`EncryptionType::Encrypted | FrameType::Bulk` (`:103`) — i.e. the **first
encrypted frame of the session**. This confirms the server is the active party
post-auth. Forward ref: `05-service-discovery.md`.

## Steady state: encrypt / decrypt of every frame

After auth, all payloads carry the `Encrypted` flag. The same `readBio` /
`writeBio` + `SSL_read` / `SSL_write` pair is reused for record crypto — no
new BIOs, no socket; OpenSSL only ever touches the two memory BIOs.

### Decrypt (inbound) — `decryptMessage` (`AaCommunicator.cpp:194`)

`handleMessage` (`:344`) reads the frame header, checks the encryption bit
(`flags & 0x8`, `:348`), and for encrypted frames calls
`decryptMessage(framePayload)` (`:359`):

```
ERR_clear_error()                                          // (:196)
n = BIO_write(readBio, encryptedMsg, size)  // push record  (:198)
n < 0  -> throw "BIO_write failed"                          // (:200–:202)
ret = SSL_read(ssl, plainBuf, 100*1024)     // pull plaintext (:205)
ret < 0  -> SSL_get_error / ERR_print / throw              // (:206–:216)
return plainBuf[0..ret]                                     // (:217)
```

Note the asymmetry vs the handshake pump: decrypt does **one** `SSL_read` for
up to 100 KiB (`plainBufSize = 100*1024`, `:203`), not a drain loop — one
encrypted frame is assumed to yield ≤ 100 KiB of plaintext in a single read.
A `0` return is treated as success (empty plaintext); only `< 0` throws.

The decrypted buffer is the message: its first 2 BE bytes are the message id
(control- or channel-namespaced; see `02`/`03`).

### Encrypt (outbound) — in `getMessage` (`AaCommunicator.cpp:364`)

The ep1 writer drains the send queue. For messages flagged
`EncryptionType::Encrypted` (`:377`):

```
SSL_write(ssl, contentBegin, len)           // (:414) plaintext -> TLS record
length = BIO_read(writeBio, encBuf+offset, ...)   // (:424) ciphertext out
// then hand-build the frame header in front of it:
encBuf[0] = channel
encBuf[1] = flags
encBuf[2..3] = length (BE)                   // (:430–:431)
// First-of-many frame also writes 4-byte BE total_len at encBuf[4..7]
```

So the ciphertext from `writeBio` is placed at frame offset 4 (or 8 for a
multi-frame first frame), and the `[channel][flags][len BE]([total_len BE])`
header is written ahead of it. Fragmentation (`maxSize = 2000`,
`First`/`Last`/`Bulk` selection, `msg.offset` walk: `:383`–`:412`) happens
around — and the SSL_write/BIO_read pair feeds — this; record framing details
are in `02-framing.md`. Plaintext (un-`Encrypted`) messages skip TLS entirely
and are serialized directly (`:439`–`:449`).

## aasdk cross-check (client side / TLS-version negotiation)

aasdk is the only client-side reference in-tree
(`server/third_party/aasdk/src/Transport/SSLWrapper.cpp`). It models the head
unit's TLS role. Divergences from AACS, all source-confirmed:

- **Method / TLS version**: aasdk uses `TLS_client_method()` (or
  `TLSv1_2_client_method()` on OpenSSL < 1.1, `SSLWrapper.cpp:97`–`:103`) and
  **does not set `SSL_OP_NO_TLSv1_3`**. AACS uses `SSLv23_server_method()`
  **with** `SSL_OP_NO_TLSv1_3`. The negotiated version is the floor of the
  two: aasdk-client would accept TLS 1.3, but the AACS server caps at 1.2, so
  the session lands on **TLS 1.2**. A `smartcar` server must likewise cap at
  1.2 to interop with the real AA stack, which historically pins TLS 1.2.
- **Handshake driver**: aasdk `SSL_set_connect_state` + `SSL_do_handshake`
  (`:138`, `:143`); AACS `SSL_set_accept_state` + `SSL_accept`. Confirms the
  role split — head unit is TLS client, projection source is TLS server.
- **Peer verification**: aasdk `SSL_set_verify(ssl, SSL_VERIFY_NONE, nullptr)`
  (`:139`) — the client does **not** verify the server cert either. Combined
  with AACS's no-op `SSL_VERIFY_PEER` callback, **neither end authenticates
  the other** (see security note).
- **BIO sizing**: aasdk calls `BIO_set_write_buf_size(bio, maxBufferSize)` on
  both mem BIOs (`:131`–`:135`); AACS sizes neither BIO (default growable mem
  BIO). Functional parity, different buffering posture.
- **Cipher list**: **neither** AACS nor aasdk sets an explicit cipher list or
  `SSL_CTX_set_cipher_list` / `set_ciphersuites` — both rely on the linked
  OpenSSL's default suite selection (constrained by the loaded DH params and
  ECDH-auto on the AACS side). No cipher suite is asserted in this doc; that
  is correct, not an omission.

Unresolved: exact negotiated cipher suite is not pinned by either codebase and
cannot be derived from source alone (depends on the OpenSSL build + the head
unit's offered list at runtime). Not specified here — would require a capture.

## Security note

`SSL_CTX_set_verify` requests the peer cert with `SSL_VERIFY_PEER`, but the
verify callback `verifyCertificate` (`AaCommunicator.cpp:339`, body at `:341`)
always `return 1;` — **the head unit's certificate is not validated at all**
(no chain, no identity, no expiry check). Any peer cert is accepted.

The mirror also holds: on the client side, aasdk uses `SSL_VERIFY_NONE`
(`SSLWrapper.cpp:139`), so the head unit does not validate the projection
source's cert either. **Net: the channel is mutually unauthenticated** —
confidentiality against a passive eavesdropper only, with **no authentication
of either party**; it does not defend against an active MITM. This matches
AA's real-world model (the trust anchor is physical USB possession), but a
`smartcar` reimplementation should be explicit that this is a deliberate
no-op, not an oversight to "fix" silently. Note also `SSL_VERIFY_PEER` here
does *request* the client cert during the handshake (it is sent and parsed),
it is merely never checked — a `smartcar` port that switches to
`SSL_VERIFY_NONE` would change handshake-message content (no CertificateRequest
/ client Certificate), so keep `SSL_VERIFY_PEER` + accepting callback for
wire fidelity.

## Cross-references

- `02-framing.md` — frame header, the `Encrypted` flag, fragmentation/`total_len`.
- `03-control-channel.md` — channel-0 message-id namespace, version negotiation (the step before this).
- `05-service-discovery.md` — `ServiceDiscoveryRequest/Response`, what the server sends right after `AuthComplete`.
- aasdk `server/third_party/aasdk/src/Transport/SSLWrapper.cpp` — client-side TLS reference used for the cross-check above.
