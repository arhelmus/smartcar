//! USB FunctionFS transport implementing [`Transport`].
//!
//! [`UsbTransport`] drives the two-persona AOAP handshake (see [`gadget`]) and
//! then implements [`Transport`] over the bulk EP1/EP2 endpoints.
//!
//! Frame codec, multi-frame reassembly, and TLS are structurally identical to
//! [`TcpTransport`]; only the byte-level I/O differs (file write/read on
//! FunctionFS endpoints vs. TCP socket).
//!
//! # Requirements
//! - Linux with `CONFIG_USB_CONFIGFS` + `CONFIG_USB_CONFIGFS_F_FS` enabled.
//! - A USB Device Controller (UDC) driver loaded for the board's OTG port.
//! - Must run as root (or with `CAP_SYS_ADMIN`) for configfs/mount access.

mod descriptors;
mod gadget;

use std::collections::HashMap;
use std::io::{Read as IoRead, Write as IoWrite};

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use openssl::ssl::{HandshakeError, Ssl};
use tracing::{debug, info};

use aap_contracts::{
    frame::{Frame, FrameFlags, FrameType},
    ChannelId, MessageType, Transport, TransportError,
};

use crate::codec::{decode_frame, encode_frame};
use crate::tls::{self, TlsSession};

/// Read chunk size for ep2 (host → board).
const READ_CHUNK: usize = 16_384;

/// Maximum decrypted payload we allocate in one shot (same as TcpTransport).
const MAX_PLAIN_BUF: usize = 65_536;

// ── Reassembly ────────────────────────────────────────────────────────────────

struct ReassemblyBuf {
    data: BytesMut,
    first_flags: FrameFlags,
    expected_total: Option<u32>,
}

// ── UsbTransport ──────────────────────────────────────────────────────────────

/// USB FunctionFS transport.
///
/// Call [`UsbTransport::connect`] to run the AOAP handshake and obtain a ready
/// transport.  The gadget is disabled when this struct is dropped.
pub struct UsbTransport {
    /// EP1 IN: board → host (outbound frames).
    ep1_tx: tokio::fs::File,
    /// EP2 OUT: host → board (inbound frames).
    ep2_rx: tokio::fs::File,
    read_buf: BytesMut,
    reassembly: HashMap<ChannelId, ReassemblyBuf>,
    tls: Option<TlsSession>,
    /// Keeps the accessory gadget alive until the transport is dropped.
    _gadget: gadget::GadgetHandle,
}

impl UsbTransport {
    /// Run the AOAP two-persona handshake and return a ready transport.
    ///
    /// Internally calls `spawn_blocking` for the synchronous gadget setup and
    /// AOAP ep0 negotiation.  Returns once the host has enumerated the
    /// accessory gadget and the bulk endpoints are open.
    pub async fn connect() -> Result<Self, TransportError> {
        info!("USB transport: starting AOAP handshake");

        let (guard, ep1_std, ep2_std) = tokio::task::spawn_blocking(gadget::run_handshake)
            .await
            .map_err(|e| TransportError::Io(std::io::Error::other(e)))?
            .map_err(TransportError::Io)?;

        let ep1_tx = tokio::fs::File::from_std(ep1_std);
        let ep2_rx = tokio::fs::File::from_std(ep2_std);

        info!("USB transport: bulk endpoints open — ready");
        Ok(Self {
            ep1_tx,
            ep2_rx,
            read_buf: BytesMut::with_capacity(READ_CHUNK),
            reassembly: HashMap::new(),
            tls: None,
            _gadget: guard,
        })
    }

    // ── Private I/O helpers ───────────────────────────────────────────────────

    async fn fill_buf(&mut self) -> Result<(), TransportError> {
        use tokio::io::AsyncReadExt;
        let n = self.ep2_rx.read_buf(&mut self.read_buf).await?;
        if n == 0 {
            return Err(TransportError::Closed);
        }
        debug!(bytes = n, "read bytes from ep2");
        Ok(())
    }

    async fn recv_frame_inner(&mut self) -> Result<Frame, TransportError> {
        loop {
            while let Some((wire_frame, total_size_hint)) = decode_frame(&mut self.read_buf)? {
                let frame_type = wire_frame.flags.frame_type();
                let channel = wire_frame.channel;

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
                        return Ok(Frame {
                            channel,
                            flags,
                            payload: buf.data.freeze(),
                        });
                    }
                }
            }

            self.fill_buf().await?;
        }
    }

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

    // ── TLS helpers (same logic as TcpTransport, different I/O primitives) ────

    async fn recv_ssl_bytes(&mut self) -> Result<Bytes, TransportError> {
        let frame = self.recv_frame_inner().await?;
        if frame.payload.len() < 2 {
            return Err(TransportError::InvalidFrame(
                "SslHandshake payload too short".into(),
            ));
        }
        Ok(frame.payload.slice(2..))
    }

    async fn send_ssl_frame(&mut self, tls_bytes: Vec<u8>) -> Result<(), TransportError> {
        use tokio::io::AsyncWriteExt;
        if tls_bytes.is_empty() {
            return Ok(());
        }
        let mut payload = BytesMut::with_capacity(2 + tls_bytes.len());
        payload.put_u16(MessageType::SslHandshake.as_u16());
        payload.put_slice(&tls_bytes);

        let frame = Frame::control_bulk(ChannelId::Control, payload.freeze());
        let mut buf = BytesMut::new();
        encode_frame(&frame, None, &mut buf);
        self.ep1_tx.write_all(&buf).await?;
        Ok(())
    }
}

// ── Transport impl ────────────────────────────────────────────────────────────

#[async_trait]
impl Transport for UsbTransport {
    async fn recv_frame(&mut self) -> Result<Frame, TransportError> {
        let frame = self.recv_frame_inner().await?;
        if frame.flags.contains(FrameFlags::ENCRYPTED) {
            return self.decrypt_frame(frame);
        }
        Ok(frame)
    }

    async fn send_frame(&mut self, frame: Frame) -> Result<(), TransportError> {
        use tokio::io::AsyncWriteExt;

        let (payload, flags) = if let Some(tls) = self.tls.as_mut() {
            tls.write_all(&frame.payload)
                .map_err(|e| TransportError::Tls(format!("encrypt: {e}")))?;
            let ciphertext = Bytes::from(tls.get_mut().drain());
            (ciphertext, frame.flags | FrameFlags::ENCRYPTED)
        } else {
            (frame.payload, frame.flags)
        };

        let out = Frame {
            channel: frame.channel,
            flags,
            payload,
        };
        let total_size =
            (out.flags.frame_type() == FrameType::First).then_some(out.payload.len() as u32);

        let mut buf = BytesMut::new();
        encode_frame(&out, total_size, &mut buf);
        self.ep1_tx.write_all(&buf).await?;
        Ok(())
    }

    async fn upgrade_tls(&mut self) -> Result<(), TransportError> {
        info!("USB: starting TLS handshake (phone/server side)");

        let ctx = tls::build_ssl_server_context()?;
        let ssl = Ssl::new(&ctx).map_err(|e| TransportError::Tls(format!("Ssl::new: {e}")))?;

        let mut bio = tls::BioAdapter::new();
        let client_hello = self.recv_ssl_bytes().await?;
        bio.push(&client_hello);

        let mut mid = match ssl.accept(bio) {
            Ok(mut stream) => {
                let out = stream.get_mut().drain();
                self.send_ssl_frame(out).await?;
                self.tls = Some(stream);
                info!("USB: TLS handshake complete (1 round)");
                return Ok(());
            }
            Err(HandshakeError::WouldBlock(mut mid)) => {
                let out = mid.get_mut().drain();
                self.send_ssl_frame(out).await?;
                mid
            }
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

        loop {
            let incoming = self.recv_ssl_bytes().await?;
            mid.get_mut().push(&incoming);

            mid = match mid.handshake() {
                Ok(mut stream) => {
                    let out = stream.get_mut().drain();
                    self.send_ssl_frame(out).await?;
                    self.tls = Some(stream);
                    info!("USB: TLS handshake complete");
                    return Ok(());
                }
                Err(HandshakeError::WouldBlock(mut new_mid)) => {
                    let out = new_mid.get_mut().drain();
                    self.send_ssl_frame(out).await?;
                    new_mid
                }
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
