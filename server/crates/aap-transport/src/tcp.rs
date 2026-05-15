//! TCP transport implementing [`Transport`].
//!
//! [`TcpTransport`] wraps a [`tokio::net::TcpStream`] and provides:
//!
//! - Wire-format frame encoding/decoding ([`codec`]).
//! - Multi-frame reassembly keyed by channel.
//! - TLS upgrade via [`Transport::upgrade_tls`], using OpenSSL with an
//!   in-memory BIO so TLS records can be exchanged as AA `SslHandshake` frames
//!   rather than over the raw TCP stream.
//! - Transparent encrypt/decrypt of frames once TLS is established.

use std::io::{Read as IoRead, Write as IoWrite};

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use openssl::ssl::{HandshakeError, Ssl};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, info, trace};

use aap_contracts::{
    frame::{Frame, FrameFlags, FrameType},
    ChannelId, MessageType, Transport, TransportError,
};

use crate::codec::{decode_frame, encode_frame};
use crate::tls::{self, TlsSession};

/// Read chunk size when pulling bytes from the TCP stream.
const READ_BUF_SIZE: usize = 4096;

/// Maximum decrypted payload size we allocate in one shot (64 KiB).
///
/// Android Auto frames are well within this limit in practice.
const MAX_PLAIN_BUF: usize = 65536;

// ── Reassembly ────────────────────────────────────────────────────────────────

/// Per-channel reassembly state for multi-frame messages.
struct ReassemblyBuf {
    data: BytesMut,
    first_flags: FrameFlags,
    expected_total: Option<u32>,
}

// ── TcpTransport ─────────────────────────────────────────────────────────────

/// TCP transport with frame codec, multi-frame reassembly, and TLS.
pub struct TcpTransport {
    stream: TcpStream,
    /// Raw bytes received from the network, not yet fully decoded.
    read_buf: BytesMut,
    /// In-progress multi-frame reassembly, keyed by channel.
    reassembly: std::collections::HashMap<ChannelId, ReassemblyBuf>,
    /// Active TLS session once [`upgrade_tls`] has completed.
    tls: Option<TlsSession>,
}

impl TcpTransport {
    /// Wrap an existing [`TcpStream`] in the transport.
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            read_buf: BytesMut::with_capacity(READ_BUF_SIZE),
            reassembly: std::collections::HashMap::new(),
            tls: None,
        }
    }

    // ── Private I/O helpers ───────────────────────────────────────────────────

    /// Pull more bytes from the socket into `self.read_buf`.
    async fn fill_buf(&mut self) -> Result<(), TransportError> {
        let n = self.stream.read_buf(&mut self.read_buf).await?;
        if n == 0 {
            return Err(TransportError::Closed);
        }
        trace!(bytes = n, "read bytes from stream");
        Ok(())
    }

    /// Read one fully-reassembled frame from the TCP stream **without**
    /// applying TLS decryption.
    ///
    /// Used both for plaintext frames (before TLS upgrade) and as the inner
    /// read step inside [`Self::recv_frame`] (which adds decryption on top).
    async fn recv_frame_inner(&mut self) -> Result<Frame, TransportError> {
        loop {
            while let Some((wire_frame, total_size_hint)) = decode_frame(&mut self.read_buf)? {
                let frame_type = wire_frame.flags.frame_type();
                let channel = wire_frame.channel;

                debug!(
                    ?channel,
                    ?frame_type,
                    payload_bytes = wire_frame.payload.len(),
                    "received wire frame"
                );

                match frame_type {
                    FrameType::Bulk => return Ok(wire_frame),

                    FrameType::First => {
                        let mut data = BytesMut::new();
                        data.put_slice(&wire_frame.payload);
                        self.reassembly.insert(
                            channel,
                            ReassemblyBuf {
                                data,
                                first_flags: wire_frame.flags,
                                expected_total: total_size_hint,
                            },
                        );
                    }

                    FrameType::Middle => {
                        let buf = self.reassembly.get_mut(&channel).ok_or_else(|| {
                            TransportError::InvalidFrame(format!(
                                "middle frame on channel {channel} with no open reassembly"
                            ))
                        })?;
                        buf.data.put_slice(&wire_frame.payload);
                    }

                    FrameType::Last => {
                        let mut buf = self.reassembly.remove(&channel).ok_or_else(|| {
                            TransportError::InvalidFrame(format!(
                                "last frame on channel {channel} with no open reassembly"
                            ))
                        })?;
                        buf.data.put_slice(&wire_frame.payload);

                        if let Some(expected) = buf.expected_total {
                            let actual = buf.data.len() as u32;
                            if actual != expected {
                                return Err(TransportError::InvalidFrame(format!(
                                    "reassembly size mismatch on {channel}: \
                                     expected {expected}, got {actual}"
                                )));
                            }
                        }

                        let flags = (buf.first_flags & !FrameFlags::FIRST)
                            | FrameFlags::FIRST
                            | FrameFlags::LAST;
                        let payload: Bytes = buf.data.freeze();
                        debug!(
                            ?channel,
                            payload_bytes = payload.len(),
                            "reassembled multi-frame message"
                        );
                        return Ok(Frame {
                            channel,
                            flags,
                            payload,
                        });
                    }
                }
            }

            self.fill_buf().await?;
        }
    }

    /// Decrypt an `ENCRYPTED` frame payload using the established TLS session.
    ///
    /// Returns the frame with the `ENCRYPTED` flag cleared and the payload
    /// replaced by the decrypted content.
    fn decrypt_frame(&mut self, frame: Frame) -> Result<Frame, TransportError> {
        let tls = self.tls.as_mut().ok_or_else(|| {
            TransportError::Tls("received ENCRYPTED frame before TLS upgrade".into())
        })?;

        tls.get_mut().push(frame.payload.as_ref());

        let mut plain = vec![0u8; MAX_PLAIN_BUF];
        let n = tls
            .read(&mut plain)
            .map_err(|e| TransportError::Tls(format!("decrypt: {e}")))?;

        Ok(Frame {
            channel: frame.channel,
            flags: frame.flags & !FrameFlags::ENCRYPTED,
            payload: Bytes::from(plain[..n].to_vec()),
        })
    }

    // ── TLS handshake helpers ─────────────────────────────────────────────────

    /// Read the next `SslHandshake` AA frame and return the raw TLS bytes
    /// (payload stripped of the 2-byte message-id prefix).
    async fn recv_ssl_bytes(&mut self) -> Result<Bytes, TransportError> {
        let frame = self.recv_frame_inner().await?;
        if frame.payload.len() < 2 {
            return Err(TransportError::InvalidFrame(
                "SslHandshake frame payload too short".into(),
            ));
        }
        // Strip the 2-byte message-id (0x00 0x03) to get raw TLS record bytes.
        Ok(frame.payload.slice(2..))
    }

    /// Wrap raw TLS bytes in an `SslHandshake` AA frame and send them.
    ///
    /// Called only during the handshake, before `self.tls` is set, so no
    /// encryption is applied.
    async fn send_ssl_frame(&mut self, tls_bytes: Vec<u8>) -> Result<(), TransportError> {
        if tls_bytes.is_empty() {
            return Ok(());
        }
        let mut payload = BytesMut::with_capacity(2 + tls_bytes.len());
        payload.put_u16(MessageType::SslHandshake.as_u16());
        payload.put_slice(&tls_bytes);

        let frame = Frame::control_bulk(ChannelId::Control, payload.freeze());
        // Write raw to TCP — self.tls is None at this point.
        let mut buf = BytesMut::new();
        encode_frame(&frame, None, &mut buf);
        self.stream.write_all(&buf).await?;
        Ok(())
    }
}

// ── Transport impl ────────────────────────────────────────────────────────────

#[async_trait]
impl Transport for TcpTransport {
    async fn recv_frame(&mut self) -> Result<Frame, TransportError> {
        let frame = self.recv_frame_inner().await?;
        if frame.flags.contains(FrameFlags::ENCRYPTED) {
            return self.decrypt_frame(frame);
        }
        Ok(frame)
    }

    async fn send_frame(&mut self, frame: Frame) -> Result<(), TransportError> {
        let frame_type = frame.flags.frame_type();
        debug!(
            channel = ?frame.channel,
            ?frame_type,
            payload_bytes = frame.payload.len(),
            "sending frame"
        );

        // Encrypt if TLS is active; extract result before borrowing self.stream.
        let (payload_to_send, flags_to_use) = if let Some(tls) = self.tls.as_mut() {
            tls.write_all(&frame.payload)
                .map_err(|e| TransportError::Tls(format!("encrypt: {e}")))?;
            let encrypted = tls.get_mut().drain();
            (Bytes::from(encrypted), frame.flags | FrameFlags::ENCRYPTED)
        } else {
            (frame.payload, frame.flags)
        };
        // TLS borrow is released here; self.stream can be borrowed below.

        let send_frame = Frame {
            channel: frame.channel,
            flags: flags_to_use,
            payload: payload_to_send,
        };

        let total_size = if send_frame.flags.frame_type() == FrameType::First {
            Some(send_frame.payload.len() as u32)
        } else {
            None
        };

        let mut buf = BytesMut::new();
        encode_frame(&send_frame, total_size, &mut buf);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Perform the Android Auto TLS handshake.
    ///
    /// This method owns the entire `SslHandshake` frame exchange:
    ///
    /// 1. Reads the head unit's first `SslHandshake` frame (TLS ClientHello).
    /// 2. Drives the OpenSSL server-side handshake state machine using an
    ///    in-memory [`tls::BioAdapter`].
    /// 3. Exchanges further `SslHandshake` frames until the handshake is
    ///    complete, then stores the session in `self.tls`.
    ///
    /// After this returns `Ok(())`, all subsequent [`Self::send_frame`] /
    /// [`Self::recv_frame`] calls transparently encrypt/decrypt.
    async fn upgrade_tls(&mut self) -> Result<(), TransportError> {
        info!("starting TLS handshake");

        let (pkey, cert) = tls::load_or_generate_cert()?;
        let ctx = tls::build_ssl_context(&pkey, &cert)?;
        let ssl = Ssl::new(&ctx).map_err(|e| TransportError::Tls(format!("Ssl::new: {e}")))?;

        let mut adapter = tls::BioAdapter::new();

        // Read the first SslHandshake frame (TLS ClientHello) and feed it in.
        let first_bytes = self.recv_ssl_bytes().await?;
        adapter.push(&first_bytes);

        // ── Initial accept ────────────────────────────────────────────────────

        let mut mid = match ssl.accept(adapter) {
            Ok(mut stream) => {
                // Handshake completed in one shot (unusual but possible).
                let out = stream.get_mut().drain();
                self.send_ssl_frame(out).await?;
                self.tls = Some(stream);
                info!("TLS handshake complete (1 round)");
                return Ok(());
            }
            Err(HandshakeError::WouldBlock(mid)) => mid,
            Err(HandshakeError::Failure(mid)) => {
                return Err(TransportError::Tls(format!(
                    "TLS accept failure: {}",
                    mid.into_error()
                )));
            }
            Err(HandshakeError::SetupFailure(e)) => {
                return Err(TransportError::Tls(format!("TLS setup failure: {e}")));
            }
        };

        // ── Handshake loop ────────────────────────────────────────────────────
        //
        // Each iteration:
        //  1. Drain any output OpenSSL generated (e.g. ServerHello + Cert).
        //  2. Send it to the head unit as an SslHandshake frame.
        //  3. Receive the head unit's next SslHandshake frame.
        //  4. Feed it into OpenSSL.
        //  5. Continue the handshake; loop on WouldBlock, finish on Ok.

        loop {
            let outgoing = mid.get_mut().drain();
            self.send_ssl_frame(outgoing).await?;

            let incoming = self.recv_ssl_bytes().await?;
            mid.get_mut().push(&incoming);

            mid = match mid.handshake() {
                Ok(mut stream) => {
                    // Send any final bytes (server ChangeCipherSpec + Finished).
                    let out = stream.get_mut().drain();
                    self.send_ssl_frame(out).await?;
                    self.tls = Some(stream);
                    info!("TLS handshake complete");
                    return Ok(());
                }
                Err(HandshakeError::WouldBlock(new_mid)) => new_mid,
                Err(HandshakeError::Failure(mid)) => {
                    return Err(TransportError::Tls(format!(
                        "TLS handshake failure: {}",
                        mid.into_error()
                    )));
                }
                Err(HandshakeError::SetupFailure(e)) => {
                    return Err(TransportError::Tls(format!(
                        "TLS setup failure mid-handshake: {e}"
                    )));
                }
            };
        }
    }
}
