//! Prost-generated AAW protobuf types.
//!
//! Built by `aap-transport/build.rs` from the aasdk submodule at
//! `third_party/aasdk/protobuf/aap_protobuf/aaw/*.proto` plus the
//! `wifiprojection` enum imports.
//!
//! Wire format inside RFCOMM is a 4-byte big-endian header followed by the
//! protobuf body — see `aa-proxy-rs/src/bluetooth.rs` (`HEADER_LEN = 4`):
//!
//! ```text
//! [u16 BE payload length][u16 BE MessageId][payload bytes]
//! ```
//!
//! # Module shape
//!
//! prost generates one `.rs` per proto package and uses *absolute* paths for
//! cross-package references (e.g. `WifiInfoResponse` references
//! `::aap_protobuf::service::wifiprojection::message::WifiSecurityMode`). So
//! the include hierarchy here mirrors the proto package tree literally — and
//! we re-export the two leaf types the rest of the module wants under
//! shorter aliases.

#![allow(missing_docs)]
#![allow(clippy::all)]

pub mod aap_protobuf {
    pub mod aaw {
        include!(concat!(env!("OUT_DIR"), "/aap_protobuf.aaw.rs"));
    }
    pub mod service {
        pub mod wifiprojection {
            pub mod message {
                include!(concat!(
                    env!("OUT_DIR"),
                    "/aap_protobuf.service.wifiprojection.message.rs"
                ));
            }
        }
    }
}

// ── Short aliases used by the rest of the bt module ───────────────────────────

pub use aap_protobuf::aaw;
pub use aap_protobuf::service::wifiprojection::message as wifiprojection;
