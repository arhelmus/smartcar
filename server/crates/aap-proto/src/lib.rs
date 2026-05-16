//! Generated protobuf types for the Android Auto protocol.
//!
//! This crate wraps the prost-generated code from the `.proto` files in
//! `third_party/AAProto/` and re-exports the types needed by the rest of the
//! `smartcar` workspace.
//!
//! # Module structure
//!
//! The proto files use the package prefix `gb.xxy.trial.proto.*`, which prost
//! translates into a set of per-package `.rs` files.  We re-export them here
//! as a flat, workspace-friendly hierarchy:
//!
//! - [`ids`]   — `ControlMessage::Enum` (control-channel message-type ids)
//! - [`enums`] — shared enumerations (status, audio focus, sensor type, …)
//! - [`data`]  — structured data types (channel descriptors, configs, …)
//! - [`msgs`]  — concrete control-channel messages (ping, open, discovery, …)
//!
//! # Bridging with `aap-contracts`
//!
//! The helper functions in [`bridge`] convert between the `aap-contracts` POD
//! types (e.g. [`aap_contracts::MessageType`]) and the prost-generated enums
//! so callers do not have to reach into the generated code directly.

// When the AAProto submodule is absent or proto compilation fails, the build
// script emits `cargo:rustc-cfg=aap_proto_stub` and we compile an empty but
// valid crate so the rest of the workspace keeps building.
#[cfg(aap_proto_stub)]
compile_error!(
    "aap-proto is in stub mode: initialize the AAProto submodule with \
     `git submodule update --init server/third_party/AAProto`"
);

// ── Generated protobuf modules ────────────────────────────────────────────────

/// Control-channel message-type identifier enum (mirrors `ControlMessageIdsEnum.proto`).
pub mod ids {
    include!(concat!(env!("OUT_DIR"), "/gb.xxy.trial.proto.ids.rs"));
}

/// Shared enumerations: status codes, audio focus, sensor types, A/V types, …
pub mod enums {
    include!(concat!(env!("OUT_DIR"), "/gb.xxy.trial.proto.enums.rs"));
}

/// Structured data types: channel descriptors, A/V configs, …
pub mod data {
    // The data module references enums via `super::enums::*`, which works
    // because prost emits absolute paths (e.g. `super::enums::status::Enum`).
    include!(concat!(env!("OUT_DIR"), "/gb.xxy.trial.proto.data.rs"));
}

/// Concrete control-channel messages: ping, channel open, service discovery, …
pub mod msgs {
    // The messages module references data via `super::data::*` and enums via
    // `super::enums::*`, consistent with prost's `super::` path generation.
    include!(concat!(env!("OUT_DIR"), "/gb.xxy.trial.proto.messages.rs"));
}

// ── Convenience re-exports ────────────────────────────────────────────────────

pub use data::ChannelDescriptor;
pub use ids::control_message::Enum as ControlMessageId;
pub use msgs::{
    AudioFocusRequest, AudioFocusResponse, AuthCompleteIndication, ChannelOpenRequest,
    ChannelOpenResponse, NavigationFocusRequest, NavigationFocusResponse, PingRequest,
    PingResponse, ServiceDiscoveryRequest, ServiceDiscoveryResponse, ShutdownRequest,
    ShutdownResponse,
};

// ── Bridge helpers ─────────────────────────────────────────────────────────────

/// Conversion helpers between `aap-contracts` types and prost-generated types.
///
/// `aap-contracts` does **not** depend on `aap-proto`; the dependency arrow
/// points the other way.  These functions live here so that upper-layer crates
/// can convert without having to import generated code directly.
pub mod bridge {
    use aap_contracts::{ChannelId, MessageType};

    use crate::ids::control_message;

    /// Convert an [`aap_contracts::MessageType`] to the corresponding
    /// prost-generated [`control_message::Enum`] discriminant.
    ///
    /// Returns `None` for `MessageType` values that have no direct proto
    /// counterpart (currently none, but the guard keeps the API honest).
    pub fn message_type_to_proto(mt: MessageType) -> Option<control_message::Enum> {
        let v = mt.as_u16() as i32;
        control_message::Enum::try_from(v).ok()
    }

    /// Convert a prost-generated [`control_message::Enum`] back to a
    /// [`MessageType`].
    ///
    /// Returns `None` if the proto value has no `MessageType` counterpart
    /// (e.g. `ControlMessage::None`).
    pub fn proto_to_message_type(e: control_message::Enum) -> Option<MessageType> {
        let v = e as u16;
        MessageType::try_from(v).ok()
    }

    /// Convert an [`aap_contracts::ChannelId`] to its wire integer value.
    ///
    /// The proto layer uses plain `uint32` for channel ids (see
    /// `ChannelDescriptorData.proto`); this helper provides a typed bridge.
    pub fn channel_id_to_u32(ch: ChannelId) -> u32 {
        ch.as_u8() as u32
    }

    /// Try to convert a raw `uint32` channel id (as carried in
    /// `ChannelDescriptor.channel_id`) back to a typed [`ChannelId`].
    ///
    /// Returns `Err(raw_value)` for unknown ids.
    pub fn u32_to_channel_id(raw: u32) -> Result<ChannelId, u32> {
        let byte = u8::try_from(raw).map_err(|_| raw)?;
        ChannelId::try_from(byte).map_err(|_| raw)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use aap_contracts::{ChannelId, MessageType};

    use super::bridge::{
        channel_id_to_u32, message_type_to_proto, proto_to_message_type, u32_to_channel_id,
    };
    use super::ids::control_message;

    #[test]
    fn control_message_id_roundtrip() {
        // Every MessageType must survive MessageType → proto → MessageType.
        let cases = [
            MessageType::VersionRequest,
            MessageType::VersionResponse,
            MessageType::SslHandshake,
            MessageType::AuthComplete,
            MessageType::ServiceDiscoveryRequest,
            MessageType::ServiceDiscoveryResponse,
            MessageType::ChannelOpenRequest,
            MessageType::ChannelOpenResponse,
            MessageType::PingRequest,
            MessageType::PingResponse,
            MessageType::NavigationFocusRequest,
            MessageType::NavigationFocusResponse,
            MessageType::ShutdownRequest,
            MessageType::ShutdownResponse,
            MessageType::VoiceSessionRequest,
            MessageType::AudioFocusRequest,
            MessageType::AudioFocusResponse,
        ];
        for mt in cases {
            let proto = message_type_to_proto(mt).expect("no proto enum for MessageType");
            let back = proto_to_message_type(proto).expect("no MessageType for proto enum");
            assert_eq!(mt, back, "roundtrip failed for {:?}", mt);
        }
    }

    #[test]
    fn proto_none_has_no_message_type() {
        assert!(proto_to_message_type(control_message::Enum::None).is_none());
    }

    #[test]
    fn channel_id_roundtrip() {
        let cases = [
            ChannelId::Control,
            ChannelId::Sensor,
            ChannelId::MediaSink,
            ChannelId::Video,
            ChannelId::MediaAudio,
            ChannelId::SpeechAudio,
            ChannelId::SystemAudio,
            ChannelId::TelephonyAudio,
            ChannelId::InputSource,
            ChannelId::Microphone,
            ChannelId::Bluetooth,
            ChannelId::None,
        ];
        for ch in cases {
            let raw = channel_id_to_u32(ch);
            let back = u32_to_channel_id(raw).expect("roundtrip failed");
            assert_eq!(ch, back);
        }
    }

    #[test]
    fn unknown_channel_id_returns_err() {
        assert!(u32_to_channel_id(99).is_err());
        assert!(u32_to_channel_id(256).is_err());
    }

    #[test]
    fn ping_message_encodes() {
        use prost::Message;
        let req = super::PingRequest { timestamp: 42 };
        let mut buf = Vec::new();
        req.encode(&mut buf).unwrap();
        let decoded = super::PingRequest::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded.timestamp, 42);
    }

    #[test]
    fn service_discovery_request_encodes() {
        use prost::Message;
        let req = super::ServiceDiscoveryRequest {
            device_name: "Smartcar".into(),
            device_brand: "Rust".into(),
        };
        let mut buf = Vec::new();
        req.encode(&mut buf).unwrap();
        let decoded = super::ServiceDiscoveryRequest::decode(buf.as_slice()).unwrap();
        assert_eq!(decoded.device_name, "Smartcar");
        assert_eq!(decoded.device_brand, "Rust");
    }
}
