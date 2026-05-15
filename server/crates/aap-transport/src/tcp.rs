//! TCP transport implementing [`Transport`].
//!
//! [`TcpTransport`] wraps a [`tokio::net::TcpStream`], adds wire-frame
//! encoding/decoding, and multi-frame reassembly keyed by channel.

use std::collections::HashMap;

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, trace};

use aap_contracts::{
    frame::{Frame, FrameFlags, FrameType},
    ChannelId, Transport, TransportError,
};

use crate::codec::{decode_frame, encode_frame};

/// Read chunk size when pulling bytes from the TCP stream.
const READ_BUF_SIZE: usize = 4096;

/// Per-channel reassembly state for multi-frame messages.
struct ReassemblyBuf {
    /// Accumulated payload bytes.
    data: BytesMut,
    /// Flags from the first fragment (carried to the reassembled frame).
    first_flags: FrameFlags,
    /// Expected total size (from the `total_size` header field of the first frame).
    expected_total: Option<u32>,
}

/// TCP transport with frame codec and multi-frame reassembly.
pub struct TcpTransport {
    stream: TcpStream,
    /// Raw bytes received from the network, not yet fully decoded.
    read_buf: BytesMut,
    /// In-progress multi-frame reassembly, keyed by channel.
    reassembly: HashMap<ChannelId, ReassemblyBuf>,
}

impl TcpTransport {
    /// Wrap an existing [`TcpStream`] in the transport.
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            read_buf: BytesMut::with_capacity(READ_BUF_SIZE),
            reassembly: HashMap::new(),
        }
    }

    /// Read enough bytes from the socket to decode at least one frame.
    ///
    /// Returns `Err(TransportError::Closed)` when the peer has closed the connection.
    async fn fill_buf(&mut self) -> Result<(), TransportError> {
        let n = self.stream.read_buf(&mut self.read_buf).await?;
        if n == 0 {
            return Err(TransportError::Closed);
        }
        trace!(bytes = n, "read bytes from stream");
        Ok(())
    }
}

#[async_trait]
impl Transport for TcpTransport {
    async fn recv_frame(&mut self) -> Result<Frame, TransportError> {
        loop {
            // Try to decode a wire frame from whatever we already have buffered.
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
                    FrameType::Bulk => {
                        // Single-frame message — return immediately.
                        return Ok(wire_frame);
                    }

                    FrameType::First => {
                        // Start of a multi-frame message.
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

                        // Sanity-check against the total_size advertised in the First frame.
                        if let Some(expected) = buf.expected_total {
                            let actual = buf.data.len() as u32;
                            if actual != expected {
                                return Err(TransportError::InvalidFrame(format!(
                                    "reassembly size mismatch on {channel}: \
                                     expected {expected}, got {actual}"
                                )));
                            }
                        }

                        // Reassembled frame gets FIRST|LAST so callers see a Bulk frame.
                        let flags = (buf.first_flags & !(FrameFlags::FIRST))
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

            // Need more data from the network.
            self.fill_buf().await?;
        }
    }

    async fn send_frame(&mut self, frame: Frame) -> Result<(), TransportError> {
        let frame_type = frame.flags.frame_type();
        debug!(
            channel = ?frame.channel,
            ?frame_type,
            payload_bytes = frame.payload.len(),
            "sending frame"
        );

        // For bulk/first frames determine the total_size hint.
        // We send the entire payload in one wire frame here (no fragmentation).
        let total_size = if frame_type == FrameType::First {
            Some(frame.payload.len() as u32)
        } else {
            None
        };

        let mut buf = BytesMut::new();
        encode_frame(&frame, total_size, &mut buf);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    async fn upgrade_tls(&mut self) -> Result<(), TransportError> {
        // W2: implement TLS handshake using openssl SslStream over the TCP socket.
        // The full AA-flavoured TLS state machine (wrapping AA SSL handshake frames
        // on the control channel) will be wired up in a later work item.
        todo!("W2: TLS handshake not yet implemented")
    }
}
