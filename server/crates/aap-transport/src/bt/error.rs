//! Errors surfaced by the AAW BT bootstrap.
//!
//! Kept in a sibling file (instead of the usual `lib.rs`-level error type) so
//! `BtError` doesn't leak into crates that don't compile the bt module
//! (macOS dev builds, stub builds without the aasdk submodule).

use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BtError {
    #[error("BlueZ session error: {0}")]
    Bluez(#[from] bluer::Error),

    #[error("BT/RFCOMM I/O: {0}")]
    Io(#[from] io::Error),

    #[error("AAW message encode: {0}")]
    Encode(#[from] prost::EncodeError),

    #[error("AAW message decode: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error("AAW protocol: {0}")]
    Framing(String),

    #[error("HU returned non-success AAW status: {0:?}")]
    HuStatus(crate::bt::proto::aaw::Status),

    #[error("timeout waiting for RFCOMM connect")]
    ConnectTimeout,

    #[error("timeout waiting for RFCOMM read")]
    ReadTimeout,

    #[error("peer closed RFCOMM stream mid-handshake")]
    PeerClosed,

    #[error("wpa_supplicant: {0}")]
    Wpa(String),

    #[error("Wi-Fi never associated/got an IP within timeout")]
    WifiTimeout,

    #[error("Wi-Fi join failed: {0}")]
    WifiJoin(String),

    #[error(
        "no paired AAW-capable device found within timeout — open the car's \
         Android Auto Wireless setup and pick `smartcar` to pair"
    )]
    NoAawPairedDevice,

    #[error("BlueZ agent registration: {0}")]
    Agent(String),

    #[error("system command `{cmd}` failed (exit {status}): {stderr}")]
    Subprocess {
        cmd: String,
        status: i32,
        stderr: String,
    },
}
