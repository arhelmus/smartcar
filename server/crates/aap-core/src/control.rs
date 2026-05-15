//! Control-channel frame encoding and decoding helpers.
//!
//! Every control-channel payload starts with a big-endian u16 `message_id`
//! followed by optional protobuf bytes.  The helpers in this module encode and
//! decode that layout so [`crate::connection::Connection`] stays readable.

use bytes::Bytes;
use prost::Message;

use aap_contracts::{ChannelId, Frame, MessageType};

/// Encode a protobuf message into a complete control-channel [`Frame`].
///
/// The resulting payload is `[msg_id_hi, msg_id_lo, <proto bytes>]`.
pub fn encode_control<M: Message>(msg_type: MessageType, msg: &M) -> Frame {
    let proto_len = msg.encoded_len();
    let mut payload = Vec::with_capacity(2 + proto_len);

    let id = msg_type.as_u16();
    payload.push((id >> 8) as u8);
    payload.push((id & 0xFF) as u8);
    msg.encode(&mut payload)
        .expect("encoding into a Vec never fails");

    Frame::control_bulk(ChannelId::Control, Bytes::from(payload))
}

/// Parse the `message_id` from the first two bytes of a control-channel payload.
///
/// Returns `None` when the payload is shorter than two bytes.
pub fn parse_message_type(payload: &[u8]) -> Option<Result<MessageType, u16>> {
    if payload.len() < 2 {
        return None;
    }
    let id = u16::from_be_bytes([payload[0], payload[1]]);
    Some(MessageType::try_from(id).map_err(|_| id))
}

/// Extract the protobuf body from a control-channel payload (bytes after the
/// 2-byte message-id prefix).
pub fn proto_body(payload: &Bytes) -> Bytes {
    payload.slice(2..)
}

#[cfg(test)]
mod tests {
    use aap_contracts::{FrameFlags, MessageType};
    use prost::Message;

    use super::*;

    #[test]
    fn ping_response_first_two_bytes() {
        let resp = aap_proto::PingResponse { timestamp: 12345 };
        let frame = encode_control(MessageType::PingResponse, &resp);

        // The frame must be a control-bulk frame on channel 0.
        assert_eq!(frame.channel, ChannelId::Control);
        assert!(frame.flags.contains(FrameFlags::CONTROL));

        // First two bytes must be 0x00 0x0C (PingResponse = 0x000C).
        assert_eq!(frame.payload[0], 0x00);
        assert_eq!(frame.payload[1], 0x0C);

        // The rest must decode back correctly.
        let decoded =
            aap_proto::PingResponse::decode(&frame.payload[2..]).expect("decode must succeed");
        assert_eq!(decoded.timestamp, 12345);
    }

    #[test]
    fn parse_message_type_known() {
        let payload = [0x00, 0x0B, 0xAA, 0xBB]; // PingRequest = 0x000B
        let result = parse_message_type(&payload).expect("must parse");
        assert_eq!(result.unwrap(), MessageType::PingRequest);
    }

    #[test]
    fn parse_message_type_unknown() {
        let payload = [0xFF, 0xFF]; // unknown id
        let result = parse_message_type(&payload).expect("must return Some");
        assert!(result.is_err());
    }

    #[test]
    fn parse_message_type_too_short() {
        assert!(parse_message_type(&[0x00]).is_none());
        assert!(parse_message_type(&[]).is_none());
    }

    #[test]
    fn proto_body_slices_correctly() {
        let raw = Bytes::from_static(&[0x00, 0x0B, 0x01, 0x02, 0x03]);
        let body = proto_body(&raw);
        assert_eq!(&body[..], &[0x01, 0x02, 0x03]);
    }
}
