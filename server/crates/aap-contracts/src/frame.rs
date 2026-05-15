//! Wire frame: the unit of transport between projection source and head unit.
//!
//! Frame layout on the wire (per aasdk reverse-engineered spec):
//!
//! ```text
//!   +--------+--------+----------------+------------------+----------+
//!   | chan 1 | flag 1 | payload_len  2 | total_size?    4 | payload  |
//!   +--------+--------+----------------+------------------+----------+
//! ```
//!
//! - `total_size` (u32 BE) is present only when `FrameFlags::FIRST` is set on
//!   a multi-frame message (i.e. FIRST without LAST). Single-frame messages
//!   use `FIRST | LAST` (a.k.a. "bulk") and omit `total_size`.
//! - `payload` may be wrapped in TLS records when `FrameFlags::ENCRYPTED` is
//!   set. Plaintext frames are used only during version negotiation.
//!
//! Encoding/decoding lives in `aap-transport`. This crate only provides the
//! type.

use bytes::Bytes;

use crate::ChannelId;

bitflags::bitflags! {
    /// Frame flag byte (per aasdk).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameFlags: u8 {
        /// First fragment of a multi-frame message.
        const FIRST     = 0x01;
        /// Last fragment of a multi-frame message.
        const LAST      = 0x02;
        /// Frame body is a control-channel message (has a `message_id` prefix).
        const CONTROL   = 0x04;
        /// Frame body is wrapped in TLS.
        const ENCRYPTED = 0x08;
    }
}

/// Categorical fragmentation state derived from `FrameFlags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// `FIRST | LAST` — entire message in one frame.
    Bulk,
    /// `FIRST` only — first of many.
    First,
    /// Neither — interior.
    Middle,
    /// `LAST` only — final of many.
    Last,
}

impl FrameFlags {
    /// Convenience: classify by First/Last bits.
    pub fn frame_type(self) -> FrameType {
        let first = self.contains(Self::FIRST);
        let last = self.contains(Self::LAST);
        match (first, last) {
            (true, true) => FrameType::Bulk,
            (true, false) => FrameType::First,
            (false, true) => FrameType::Last,
            (false, false) => FrameType::Middle,
        }
    }
}

/// A single AA wire frame after decoding (or before encoding).
///
/// `payload` for a frame with `FrameFlags::CONTROL` set has the first two bytes
/// as the `MessageType` (big-endian u16) followed by the protobuf body.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Channel this frame belongs to.
    pub channel: ChannelId,
    /// Fragmentation + control + encryption flags.
    pub flags: FrameFlags,
    /// Decoded payload. For multi-frame messages, callers reassemble in
    /// `aap-transport` before yielding a single logical `Frame`.
    pub payload: Bytes,
}

impl Frame {
    /// Construct a single-frame ("bulk") control message frame.
    pub fn control_bulk(channel: ChannelId, payload: Bytes) -> Self {
        Self {
            channel,
            flags: FrameFlags::FIRST | FrameFlags::LAST | FrameFlags::CONTROL,
            payload,
        }
    }

    /// Construct a single-frame ("bulk") data message frame.
    pub fn data_bulk(channel: ChannelId, payload: Bytes) -> Self {
        Self {
            channel,
            flags: FrameFlags::FIRST | FrameFlags::LAST,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_type_classification() {
        assert_eq!(
            (FrameFlags::FIRST | FrameFlags::LAST).frame_type(),
            FrameType::Bulk
        );
        assert_eq!(FrameFlags::FIRST.frame_type(), FrameType::First);
        assert_eq!(FrameFlags::LAST.frame_type(), FrameType::Last);
        assert_eq!(FrameFlags::empty().frame_type(), FrameType::Middle);
    }
}
