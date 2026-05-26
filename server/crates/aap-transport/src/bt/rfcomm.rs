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

use std::fmt::Write as _;

use bluer::rfcomm::Stream;
use bytes::{Buf, BufMut, BytesMut};
use prost::Message as ProstMessage;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, warn};

use super::error::BtError;
use super::proto::aaw::MessageId;

/// AAW SDP profile UUID — `4de17a00-52cb-11e6-bdf4-0800200c9a66`.
///
/// Source: `aa-proxy-rs/src/bluetooth.rs:42`. Used here both to register
/// the local Profile in `pair::open_adapter` (so cars filtering their pair
/// list by SDP UUID see us) and as the argument to `device.connect_profile(
/// &AAWG_PROFILE_UUID)` (so BlueZ resolves the channel from its on-disk
/// SDP cache and opens the RFCOMM connection without us touching it).
pub const AAWG_PROFILE_UUID: uuid::Uuid = uuid::uuid!("4de17a00-52cb-11e6-bdf4-0800200c9a66");

/// Header is `[u16 length][u16 message_id]` — 4 bytes total.
const HEADER_LEN: usize = 4;

/// A framed RFCOMM session over an open `bluer::rfcomm::Stream`.
pub struct Framed {
    stream: Stream,
    read_buf: BytesMut,
}

impl Framed {
    /// Wrap an already-open RFCOMM `Stream` with the AAW frame codec.
    ///
    /// The Stream comes from BlueZ's `Profile.NewConnection` callback (see
    /// `pair::open_adapter` and `pair::connect_aawg_profile`) — either an
    /// inbound car-initiated connection or our own outbound one. Either
    /// way the AAW byte stream is identical from here on.
    pub fn from_stream(stream: Stream) -> Self {
        Self {
            stream,
            read_buf: BytesMut::with_capacity(1024),
        }
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

        // Log the parsed fields AND the raw 4-byte header bytes alongside
        // a short prefix of the body — when something looks off mid-handshake
        // (unknown id, suspicious length, off-by-one) the raw bytes are
        // exactly what's needed and they're cheap to record.
        debug!(
            ?id,
            body_bytes = body.len(),
            header = %fmt_hex(&frame[..HEADER_LEN]),
            body_prefix = %fmt_hex_prefix(&body, 32),
            "rfcomm: tx frame"
        );
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
        // Snapshot the raw header now, so even an unknown-id error message
        // can include it verbatim.
        let raw_header = [
            self.read_buf[0],
            self.read_buf[1],
            self.read_buf[2],
            self.read_buf[3],
        ];
        let len = u16::from_be_bytes([raw_header[0], raw_header[1]]) as usize;
        let id_raw = u16::from_be_bytes([raw_header[2], raw_header[3]]) as i32;

        while self.read_buf.len() < HEADER_LEN + len {
            self.fill(timeout).await?;
        }

        self.read_buf.advance(HEADER_LEN);
        let body = self.read_buf.split_to(len);

        let id = MessageId::try_from(id_raw).map_err(|_| {
            BtError::Framing(format!(
                "unknown AAW message id {id_raw} (raw header {})",
                fmt_hex(&raw_header)
            ))
        })?;

        debug!(
            ?id,
            body_bytes = body.len(),
            header = %fmt_hex(&raw_header),
            body_prefix = %fmt_hex_prefix(&body, 32),
            "rfcomm: rx frame"
        );
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

/// Format a byte slice as space-separated hex pairs (`00 11 22 33`).
fn fmt_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        write!(&mut s, "{b:02x}").unwrap();
    }
    s
}

/// Format the first `max` bytes of `bytes` as hex, suffixed with `…` if
/// truncated. Use for body previews — keeps the log line bounded while
/// surfacing enough leading bytes to spot a protobuf type byte or a
/// printable SSID prefix.
fn fmt_hex_prefix(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        fmt_hex(bytes)
    } else {
        format!("{} … (+{} more)", fmt_hex(&bytes[..max]), bytes.len() - max)
    }
}

#[cfg(test)]
mod tests {
    use super::{fmt_hex, fmt_hex_prefix};

    #[test]
    fn hex_format() {
        assert_eq!(fmt_hex(&[]), "");
        assert_eq!(fmt_hex(&[0]), "00");
        assert_eq!(fmt_hex(&[0xde, 0xad, 0xbe, 0xef]), "de ad be ef");
    }

    #[test]
    fn hex_prefix_truncates() {
        let buf = [0x01_u8; 40];
        let out = fmt_hex_prefix(&buf, 8);
        assert!(out.starts_with("01 01 01 01 01 01 01 01 … "));
        assert!(out.ends_with("(+32 more)"));
    }

    #[test]
    fn hex_prefix_no_truncation() {
        let buf = [0x42, 0x43];
        assert_eq!(fmt_hex_prefix(&buf, 8), "42 43");
    }
}
