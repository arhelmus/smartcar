//! RFCOMM client + framed codec for AAW handshake messages.
//!
//! Frame layout (matches aa-proxy-rs / aawgd):
//!
//! ```text
//! ┌──────────────┬──────────────┬────────────────┐
//! │ u16 BE  len  │ u16 BE id    │  protobuf body │
//! └──────────────┴──────────────┴────────────────┘
//!  bytes 0..2     2..4           4..4+len
//! ```
//!
//! `len` is the length of the protobuf body only — the 4-byte header is *not*
//! counted. The header constant in `aa-proxy-rs` is `HEADER_LEN = 4`.

use std::time::Duration;

use bluer::{
    rfcomm::{SocketAddr, Stream},
    Address,
};
use bytes::{Buf, BufMut, BytesMut};
use prost::Message as ProstMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, trace, warn};

use super::error::BtError;
use super::proto::aaw::MessageId;

/// AAW SDP profile UUID, used for service discovery on the head unit and
/// (when we eventually scan for a paired AAW peer) for filtering.
///
/// Source: `aa-proxy-rs/src/bluetooth.rs`:
/// `AAWG_PROFILE_UUID = 0x4de17a0052cb11e6bdf40800200c9a66`.
///
/// Currently unused — v1 hardcodes channel 8 and doesn't run an SDP search.
/// Kept here so the follow-up that adds SDP-based channel discovery has a
/// single source of truth.
#[allow(dead_code)]
pub const AAWG_PROFILE_UUID: uuid::Uuid = uuid::uuid!("4de17a00-52cb-11e6-bdf4-0800200c9a66");

/// Conventional RFCOMM channel for the AAW service. Most head units register
/// the AAWG SDP record on channel 8; we hard-code it here for v1 instead of
/// doing an SDP service search. If the car uses a different channel the open
/// will fail and we'll log it — a follow-up will add SDP-based discovery.
pub const AAWG_DEFAULT_RFCOMM_CHANNEL: u8 = 8;

/// Header is `[u16 length][u16 message_id]` — 4 bytes total.
const HEADER_LEN: usize = 4;

/// A framed RFCOMM session over an open `bluer::rfcomm::Stream`.
pub struct Framed {
    stream: Stream,
    read_buf: BytesMut,
}

impl Framed {
    /// Connect outbound to `addr:channel` over RFCOMM and return a framed
    /// session. Times out after `connect_timeout` so a stuck pair-but-no-service
    /// HU doesn't hang the whole transport bring-up.
    pub async fn connect(
        addr: Address,
        channel: u8,
        connect_timeout: Duration,
    ) -> Result<Self, BtError> {
        let sa = SocketAddr::new(addr, channel);
        debug!(?sa, ?connect_timeout, "rfcomm: connecting");
        let stream = tokio::time::timeout(connect_timeout, Stream::connect(sa))
            .await
            .map_err(|_| BtError::ConnectTimeout)?
            .map_err(BtError::Io)?;
        Ok(Self {
            stream,
            read_buf: BytesMut::with_capacity(1024),
        })
    }

    /// Send one framed AAW message.
    pub async fn send<M: ProstMessage>(&mut self, id: MessageId, msg: &M) -> Result<(), BtError> {
        let mut body = Vec::with_capacity(msg.encoded_len());
        msg.encode(&mut body).map_err(BtError::Encode)?;
        if body.len() > u16::MAX as usize {
            return Err(BtError::Framing(format!(
                "AAW message {id:?} body too large: {} bytes",
                body.len()
            )));
        }

        let mut frame = BytesMut::with_capacity(HEADER_LEN + body.len());
        frame.put_u16(body.len() as u16);
        frame.put_u16(id as u16);
        frame.extend_from_slice(&body);

        trace!(?id, body_bytes = body.len(), "rfcomm: tx frame");
        self.stream.write_all(&frame).await.map_err(BtError::Io)?;
        self.stream.flush().await.map_err(BtError::Io)?;
        Ok(())
    }

    /// Receive one framed AAW message. Returns the message id and the
    /// undecoded body bytes — callers decode into the concrete type.
    pub async fn recv(&mut self, timeout: Duration) -> Result<(MessageId, BytesMut), BtError> {
        // Read until we have HEADER_LEN bytes.
        while self.read_buf.len() < HEADER_LEN {
            self.fill(timeout).await?;
        }
        let len = u16::from_be_bytes([self.read_buf[0], self.read_buf[1]]) as usize;
        let id_raw = u16::from_be_bytes([self.read_buf[2], self.read_buf[3]]) as i32;

        while self.read_buf.len() < HEADER_LEN + len {
            self.fill(timeout).await?;
        }

        self.read_buf.advance(HEADER_LEN);
        let body = self.read_buf.split_to(len);

        let id = MessageId::try_from(id_raw)
            .map_err(|_| BtError::Framing(format!("unknown AAW message id {id_raw}")))?;

        trace!(?id, body_bytes = body.len(), "rfcomm: rx frame");
        Ok((id, body))
    }

    async fn fill(&mut self, timeout: Duration) -> Result<(), BtError> {
        let n = tokio::time::timeout(timeout, self.stream.read_buf(&mut self.read_buf))
            .await
            .map_err(|_| BtError::ReadTimeout)?
            .map_err(BtError::Io)?;
        if n == 0 {
            warn!("rfcomm: peer closed during read");
            return Err(BtError::PeerClosed);
        }
        Ok(())
    }
}
