//! Synchronous frame encoding and decoding (no I/O).
//!
//! Wire format (big-endian):
//!
//! ```text
//! +--------+--------+------------------+------------------+----------+
//! | chan 1 | flag 1 | payload_len   2  | total_size?   4  | payload  |
//! +--------+--------+------------------+------------------+----------+
//! ```
//!
//! `total_size` is present **only** when `FrameType::First` (FIRST set, LAST not set).

use bytes::{Buf, BufMut, BytesMut};

use aap_contracts::{
    frame::{Frame, FrameFlags, FrameType},
    ChannelId, TransportError,
};

/// Minimum header size: channel(1) + flags(1) + payload_len(2).
pub(crate) const MIN_HEADER: usize = 4;

/// Extra bytes for the `total_size` field present in `FrameType::First`.
pub(crate) const TOTAL_SIZE_FIELD: usize = 4;

/// Encode `frame` into `dst`.
///
/// The `total_size` argument is written only for `FrameType::First` frames and
/// must equal the total reassembled payload length across all fragments.
pub fn encode_frame(frame: &Frame, total_size: Option<u32>, dst: &mut BytesMut) {
    let frame_type = frame.flags.frame_type();
    let has_total_size = frame_type == FrameType::First;

    dst.put_u8(frame.channel.as_u8());
    dst.put_u8(frame.flags.bits());
    dst.put_u16(u16::try_from(frame.payload.len()).unwrap_or(u16::MAX));

    if has_total_size {
        dst.put_u32(total_size.unwrap_or(0));
    }

    dst.put_slice(&frame.payload);
}

/// Attempt to decode one wire frame from `src`.
///
/// Returns `Ok(None)` if there is not yet enough data.
/// Advances `src` past the consumed bytes on success.
/// The returned frame's `total_size` hint (present on `FrameType::First`) is
/// returned via the second element of the tuple.
pub fn decode_frame(src: &mut BytesMut) -> Result<Option<(Frame, Option<u32>)>, TransportError> {
    if src.len() < MIN_HEADER {
        return Ok(None);
    }

    let channel_byte = src[0];
    let flags_byte = src[1];
    let payload_len = u16::from_be_bytes([src[2], src[3]]) as usize;

    let flags = FrameFlags::from_bits(flags_byte).ok_or_else(|| {
        TransportError::InvalidFrame(format!("unknown flag bits: 0x{flags_byte:02x}"))
    })?;
    let frame_type = flags.frame_type();
    let has_total_size = frame_type == FrameType::First;

    let header_size = MIN_HEADER + if has_total_size { TOTAL_SIZE_FIELD } else { 0 };
    let needed = header_size + payload_len;
    if src.len() < needed {
        return Ok(None);
    }

    // Consume header bytes.
    src.advance(MIN_HEADER);

    let total_size = if has_total_size {
        let ts = u32::from_be_bytes([src[0], src[1], src[2], src[3]]);
        src.advance(TOTAL_SIZE_FIELD);
        Some(ts)
    } else {
        None
    };

    let payload = src.copy_to_bytes(payload_len);

    let channel = ChannelId::try_from(channel_byte)
        .map_err(|b| TransportError::InvalidFrame(format!("unknown channel id: {b}")))?;

    let frame = Frame {
        channel,
        flags,
        payload,
    };
    Ok(Some((frame, total_size)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aap_contracts::{
        frame::{Frame, FrameFlags},
        ChannelId,
    };
    use bytes::{Bytes, BytesMut};

    fn make_bulk(channel: ChannelId, payload: &[u8]) -> Frame {
        Frame {
            channel,
            flags: FrameFlags::FIRST | FrameFlags::LAST,
            payload: Bytes::copy_from_slice(payload),
        }
    }

    #[test]
    fn bulk_frame_roundtrip() {
        let original = make_bulk(ChannelId::Control, b"hello world");
        let mut buf = BytesMut::new();
        encode_frame(&original, None, &mut buf);

        // Bulk frame: no total_size field.
        // Header: 4 bytes, payload: 11 bytes → 15 total.
        assert_eq!(buf.len(), MIN_HEADER + original.payload.len());

        let result = decode_frame(&mut buf).unwrap().unwrap();
        let (decoded, total_size) = result;
        assert_eq!(decoded.channel, original.channel);
        assert_eq!(decoded.flags, original.flags);
        assert_eq!(decoded.payload, original.payload);
        assert!(total_size.is_none());
        assert!(buf.is_empty());
    }

    #[test]
    fn fragmented_sequence_roundtrip() {
        let payload_first = Bytes::copy_from_slice(b"first chunk");
        let payload_middle = Bytes::copy_from_slice(b"mid chunk");
        let payload_last = Bytes::copy_from_slice(b"last chunk");
        let total: u32 = (payload_first.len() + payload_middle.len() + payload_last.len()) as u32;

        let first_frame = Frame {
            channel: ChannelId::Video,
            flags: FrameFlags::FIRST,
            payload: payload_first.clone(),
        };
        let middle_frame = Frame {
            channel: ChannelId::Video,
            flags: FrameFlags::empty(),
            payload: payload_middle.clone(),
        };
        let last_frame = Frame {
            channel: ChannelId::Video,
            flags: FrameFlags::LAST,
            payload: payload_last.clone(),
        };

        let mut buf = BytesMut::new();
        encode_frame(&first_frame, Some(total), &mut buf);
        encode_frame(&middle_frame, None, &mut buf);
        encode_frame(&last_frame, None, &mut buf);

        // Decode first frame (has total_size).
        let (f, ts) = decode_frame(&mut buf).unwrap().unwrap();
        assert_eq!(f.flags.frame_type(), FrameType::First);
        assert_eq!(f.payload, payload_first);
        assert_eq!(ts, Some(total));

        // Decode middle frame.
        let (m, ts) = decode_frame(&mut buf).unwrap().unwrap();
        assert_eq!(m.flags.frame_type(), FrameType::Middle);
        assert_eq!(m.payload, payload_middle);
        assert!(ts.is_none());

        // Decode last frame.
        let (l, ts) = decode_frame(&mut buf).unwrap().unwrap();
        assert_eq!(l.flags.frame_type(), FrameType::Last);
        assert_eq!(l.payload, payload_last);
        assert!(ts.is_none());

        assert!(buf.is_empty());
    }

    #[test]
    fn partial_data_returns_none() {
        let original = make_bulk(ChannelId::Control, b"data");
        let mut full_buf = BytesMut::new();
        encode_frame(&original, None, &mut full_buf);

        // Only supply 3 bytes — not enough for even the header.
        let mut partial = BytesMut::from(&full_buf[..3]);
        let result = decode_frame(&mut partial).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn control_bulk_frame_roundtrip() {
        let frame = Frame::control_bulk(ChannelId::Control, Bytes::from_static(b"\x00\x01proto"));
        let mut buf = BytesMut::new();
        encode_frame(&frame, None, &mut buf);

        let (decoded, _) = decode_frame(&mut buf).unwrap().unwrap();
        assert!(decoded.flags.contains(FrameFlags::CONTROL));
        assert_eq!(decoded.payload, frame.payload);
    }
}
