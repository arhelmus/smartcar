# smartcar — P0 Bootstrap Spec

> Task for a single autonomous agent. One PR. After this lands, workstreams W1–W7 (see `docs/workstreams.md` you will create) can proceed in parallel without merge conflicts.

## Context

`smartcar` is a custom Android Auto **projection source** (the role normally played by a phone running Android Auto) written in Rust. It connects to `openauto` — the head unit emulator from opencardev — for local testing. Future siblings in the repo: iOS and Android client apps. Repo is a polyglot monorepo; the Rust workspace is scoped under `server/`.

P0 exists to **remove merge contention** for downstream parallel work. It must:
- Lay down the full directory tree (every leaf reachable).
- Declare the full Cargo workspace with every future crate pre-registered as a compiling stub.
- Implement **one** real crate, `aap-contracts`, which contains only the trait/type contracts that decouple the other crates from each other.
- Stub Docker, CI, scripts, and app folders so later workstreams only edit files, never add them to shared directories.

## Working directory

`/Users/arhelmus/smartcar`. Assume empty, or contains at most a `.git` from a prior `git init`. If `.git` is absent, run `git init -b main`.

## Definition of done

All must succeed, run from `/Users/arhelmus/smartcar`:

```
cd server && cargo check --workspace
cd server && cargo clippy --workspace --all-targets -- -D warnings
cd server && cargo fmt --all -- --check
cd server && cargo test --workspace
cd docker && docker compose config >/dev/null
git submodule status   # three entries, uninitialized state is fine
test -f .gitignore && test -f README.md
```

Final `git status` must be clean (everything committed).

---

## Step 1 — Directory tree

Create exactly this layout. Empty leaf directories get a `.gitkeep`.

```
smartcar/
├── .gitignore
├── .gitmodules
├── .github/
│   └── workflows/
│       └── ci.yml
├── README.md
├── apps/
│   ├── ios/
│   │   ├── .gitkeep
│   │   ├── README.md
│   │   └── third_party/.gitkeep
│   └── android/
│       ├── .gitkeep
│       ├── README.md
│       └── third_party/.gitkeep
├── docker/
│   ├── docker-compose.yml
│   ├── openauto.Dockerfile
│   └── entrypoint.sh
├── docs/
│   ├── architecture.md
│   └── workstreams.md
├── scripts/
│   ├── bootstrap.py
│   ├── run_emulator.py
│   ├── run_server.py
│   └── run_stack.py
└── server/
    ├── Cargo.toml
    ├── rust-toolchain.toml
    ├── rustfmt.toml
    ├── clippy.toml
    ├── certs/.gitkeep
    ├── proto/.gitkeep
    ├── third_party/.gitkeep
    └── crates/
        ├── aap-contracts/
        │   ├── Cargo.toml
        │   └── src/
        │       ├── channel.rs
        │       ├── frame.rs
        │       ├── lib.rs
        │       ├── message.rs
        │       ├── service.rs
        │       └── transport.rs
        ├── aap-proto/
        │   ├── Cargo.toml
        │   └── src/lib.rs
        ├── aap-transport/
        │   ├── Cargo.toml
        │   └── src/lib.rs
        ├── aap-core/
        │   ├── Cargo.toml
        │   └── src/lib.rs
        ├── aap-video/
        │   ├── Cargo.toml
        │   └── src/lib.rs
        └── smartcar-server/
            ├── Cargo.toml
            └── src/main.rs
```

---

## Step 2 — Submodules

Add but do **not** init. P0 should not require network for downstream agents to clone.

```
git submodule add https://github.com/opencardev/aasdk     server/third_party/aasdk
git submodule add https://github.com/opencardev/openauto  server/third_party/openauto
git submodule add https://github.com/opencardev/AAProto   server/third_party/AAProto
```

If any URL 404s, leave a `TODO` comment in `.gitmodules` next to the entry and continue — do not block P0 on this. Verify the org/repo path with the user only if all three fail.

---

## Step 3 — `.gitignore` (root)

```
target/
Cargo.lock          # workspace-level; we commit at server/ only
**/.DS_Store
.idea/
.vscode/
*.swp
docker/.env
server/certs/*.key
server/certs/*.crt
server/certs/*.pem
!server/certs/.gitkeep
```

Note: `server/Cargo.lock` **is** committed (binary crate present). Only ignore stray `Cargo.lock` at repo root if some agent creates one by mistake.

---

## Step 4 — `server/Cargo.toml` (workspace root)

```toml
[workspace]
resolver = "2"
members = [
    "crates/aap-contracts",
    "crates/aap-proto",
    "crates/aap-transport",
    "crates/aap-core",
    "crates/aap-video",
    "crates/smartcar-server",
]

[workspace.package]
edition      = "2021"
license      = "MIT OR Apache-2.0"
rust-version = "1.83"
repository   = "https://github.com/arhelmus/smartcar"

[workspace.dependencies]
# Internal
aap-contracts = { path = "crates/aap-contracts" }
aap-proto     = { path = "crates/aap-proto" }
aap-transport = { path = "crates/aap-transport" }
aap-core      = { path = "crates/aap-core" }
aap-video     = { path = "crates/aap-video" }

# External — pinned majors, agents in W1–W6 may bump minor versions
async-trait        = "0.1"
bitflags           = "2"
bytes              = "1"
thiserror          = "1"
anyhow             = "1"
tokio              = { version = "1", features = ["full"] }
tracing            = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap               = { version = "4", features = ["derive"] }
prost              = "0.13"
prost-build        = "0.13"
openssl            = "0.10"

[profile.release]
lto      = "thin"
codegen-units = 1
strip    = "symbols"
```

---

## Step 5 — Tooling configs

`server/rust-toolchain.toml`:
```toml
[toolchain]
channel    = "1.83"
components = ["rustfmt", "clippy"]
profile    = "minimal"
```

`server/rustfmt.toml`:
```toml
edition             = "2021"
max_width           = 100
imports_granularity = "Crate"
group_imports       = "StdExternalCrate"
newline_style       = "Unix"
```

`server/clippy.toml`:
```toml
msrv = "1.83"
```

---

## Step 6 — `aap-contracts` crate (the only real code in P0)

This is the single load-bearing artifact of P0. Every parallel workstream depends on these types. Get the API surface right; implementation details are trivial.

### `server/crates/aap-contracts/Cargo.toml`

```toml
[package]
name        = "aap-contracts"
version     = "0.1.0"
edition.workspace      = true
license.workspace      = true
rust-version.workspace = true

[dependencies]
async-trait = { workspace = true }
bitflags    = { workspace = true }
bytes       = { workspace = true }
thiserror   = { workspace = true }
```

### `src/lib.rs`

```rust
//! Wire-protocol-agnostic contracts shared across `aap-*` crates.
//!
//! This crate is deliberately tiny. It exists so that `aap-transport`,
//! `aap-proto`, `aap-core`, and service crates (`aap-video`, etc.) can be
//! developed and tested in isolation by depending only on traits and POD types
//! defined here.
//!
//! No I/O. No protobuf. No async runtime assumptions beyond `Send`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod channel;
pub mod frame;
pub mod message;
pub mod service;
pub mod transport;

pub use channel::ChannelId;
pub use frame::{Frame, FrameFlags, FrameType};
pub use message::MessageType;
pub use service::{Service, ServiceDescriptor, ServiceError};
pub use transport::{Transport, TransportError};

/// Top-level error type usable by binaries that compose multiple layers.
#[derive(Debug, thiserror::Error)]
pub enum AapError {
    /// Transport-level failure (TCP, USB, TLS, framing).
    #[error("transport: {0}")]
    Transport(#[from] TransportError),

    /// Service-level failure (bad message, internal logic).
    #[error("service: {0}")]
    Service(#[from] ServiceError),

    /// Protocol violation that doesn't fit the layered errors above.
    #[error("protocol: {0}")]
    Protocol(String),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, AapError>;
```

### `src/channel.rs`

```rust
//! Android Auto channel identifiers.
//!
//! Values mirror `aasdk`'s `ChannelId` enum
//! (see `server/third_party/aasdk/.../ChannelId.proto` once the submodule is
//! initialized). Verify exact integer values after submodule init; the values
//! below match the canonical aasdk source at time of writing.

use std::fmt;

/// Channel identifier as transmitted on the wire (1 byte).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelId {
    /// Control channel — version, SSL, service discovery, channel mgmt.
    Control     = 0,
    Input       = 1,
    Sensor      = 2,
    Video       = 3,
    MediaAudio  = 4,
    SpeechAudio = 5,
    SystemAudio = 6,
    AvInput     = 7,
    Bluetooth   = 8,
    /// Sentinel.
    None        = 255,
}

impl ChannelId {
    /// Returns the raw byte value as sent on the wire.
    pub const fn as_u8(self) -> u8 { self as u8 }
}

impl TryFrom<u8> for ChannelId {
    type Error = u8;
    fn try_from(v: u8) -> Result<Self, u8> {
        Ok(match v {
            0   => Self::Control,
            1   => Self::Input,
            2   => Self::Sensor,
            3   => Self::Video,
            4   => Self::MediaAudio,
            5   => Self::SpeechAudio,
            6   => Self::SystemAudio,
            7   => Self::AvInput,
            8   => Self::Bluetooth,
            255 => Self::None,
            other => return Err(other),
        })
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        for v in [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 255] {
            let id = ChannelId::try_from(v).unwrap();
            assert_eq!(id.as_u8(), v);
        }
        assert!(ChannelId::try_from(99).is_err());
    }
}
```

### `src/frame.rs`

```rust
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
            (true, true)   => FrameType::Bulk,
            (true, false)  => FrameType::First,
            (false, true)  => FrameType::Last,
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
    pub flags:   FrameFlags,
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
        assert_eq!((FrameFlags::FIRST | FrameFlags::LAST).frame_type(), FrameType::Bulk);
        assert_eq!(FrameFlags::FIRST.frame_type(), FrameType::First);
        assert_eq!(FrameFlags::LAST.frame_type(), FrameType::Last);
        assert_eq!(FrameFlags::empty().frame_type(), FrameType::Middle);
    }
}
```

### `src/message.rs`

```rust
//! Control-channel message type ids.
//!
//! Values mirror `aasdk`'s `ControlMessageIdsEnum`. The agent implementing W1
//! (`aap-proto`) is responsible for keeping `aap-proto`'s generated types in
//! sync with these ids; treat this enum as the canonical source within Rust.
//!
//! For non-control channels, message ids are channel-specific and not enumerated
//! here — those crates define their own constants.

/// Control-channel message identifiers (u16 BE, occupies the first two bytes
/// of a frame payload when `FrameFlags::CONTROL` is set).
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    VersionRequest            = 0x0001,
    VersionResponse           = 0x0002,
    SslHandshake              = 0x0003,
    AuthComplete              = 0x0004,
    ServiceDiscoveryRequest   = 0x0005,
    ServiceDiscoveryResponse  = 0x0006,
    ChannelOpenRequest        = 0x0007,
    ChannelOpenResponse       = 0x0008,
    PingRequest               = 0x000B,
    PingResponse              = 0x000C,
    NavigationFocusRequest    = 0x000D,
    NavigationFocusResponse   = 0x000E,
    ShutdownRequest           = 0x000F,
    ShutdownResponse          = 0x0010,
    VoiceSessionRequest       = 0x0011,
    AudioFocusRequest         = 0x0012,
    AudioFocusResponse        = 0x0013,
}

impl MessageType {
    /// Raw on-the-wire u16.
    pub const fn as_u16(self) -> u16 { self as u16 }
}

impl TryFrom<u16> for MessageType {
    type Error = u16;
    fn try_from(v: u16) -> Result<Self, u16> {
        Ok(match v {
            0x0001 => Self::VersionRequest,
            0x0002 => Self::VersionResponse,
            0x0003 => Self::SslHandshake,
            0x0004 => Self::AuthComplete,
            0x0005 => Self::ServiceDiscoveryRequest,
            0x0006 => Self::ServiceDiscoveryResponse,
            0x0007 => Self::ChannelOpenRequest,
            0x0008 => Self::ChannelOpenResponse,
            0x000B => Self::PingRequest,
            0x000C => Self::PingResponse,
            0x000D => Self::NavigationFocusRequest,
            0x000E => Self::NavigationFocusResponse,
            0x000F => Self::ShutdownRequest,
            0x0010 => Self::ShutdownResponse,
            0x0011 => Self::VoiceSessionRequest,
            0x0012 => Self::AudioFocusRequest,
            0x0013 => Self::AudioFocusResponse,
            other  => return Err(other),
        })
    }
}
```

> **W1 reminder:** before merging, verify these values against `aasdk_proto/ControlMessageIdsEnum.proto` from the initialized submodule and patch any drift.

### `src/transport.rs`

```rust
//! Abstract byte-and-frame transport.
//!
//! Implemented by `aap-transport` (TCP today, USB later). Trait is
//! object-safe via `async-trait` so the rest of the stack can hold
//! `Box<dyn Transport>` without committing to a concrete impl.

use async_trait::async_trait;
use thiserror::Error;

use crate::frame::Frame;

/// Errors at the transport layer.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Peer closed the connection.
    #[error("connection closed")]
    Closed,

    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// TLS error during handshake or stream operation.
    #[error("tls: {0}")]
    Tls(String),

    /// Malformed frame on the wire.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),

    /// `upgrade_tls` called in an invalid state.
    #[error("invalid state: {0}")]
    InvalidState(&'static str),
}

/// Abstract bidirectional frame transport.
///
/// Implementations are responsible for:
/// - Wire framing (channel/flags/len/payload).
/// - Multi-frame reassembly: callers see only complete logical messages.
/// - Optional TLS upgrade via [`Transport::upgrade_tls`], to be invoked exactly
///   once between version negotiation and service discovery. After upgrade,
///   the implementation transparently wraps/unwraps TLS for any frame whose
///   `flags` contain `ENCRYPTED`.
#[async_trait]
pub trait Transport: Send {
    /// Read the next complete logical frame.
    async fn recv_frame(&mut self) -> Result<Frame, TransportError>;

    /// Write a complete logical frame. Implementation may fragment.
    async fn send_frame(&mut self, frame: Frame) -> Result<(), TransportError>;

    /// Perform the inline TLS handshake using AA-flavoured SSL handshake
    /// frames. Must be called exactly once, after `VersionResponse` and
    /// before any service discovery.
    async fn upgrade_tls(&mut self) -> Result<(), TransportError>;
}
```

### `src/service.rs`

```rust
//! Per-channel service abstraction consumed by `aap-core`.

use async_trait::async_trait;
use bytes::Bytes;
use thiserror::Error;

use crate::{channel::ChannelId, frame::Frame};

/// Opaque service descriptor used to populate
/// `ServiceDiscoveryResponse.services[]` on the control channel.
///
/// The byte payload is the protobuf-encoded `Service` sub-message defined in
/// AAProto. `aap-contracts` deliberately does not depend on `aap-proto`, so
/// the bytes are produced by individual service crates (which depend on
/// `aap-proto`) and shuttled to the control plane opaquely.
#[derive(Debug, Clone)]
pub struct ServiceDescriptor {
    /// The channel this descriptor advertises.
    pub channel:          ChannelId,
    /// Encoded `Service` protobuf body, less the channel id field.
    pub descriptor_bytes: Bytes,
}

/// Errors raised by a [`Service`] while handling an inbound message.
#[derive(Debug, Error)]
pub enum ServiceError {
    /// Inbound `message_id` is not recognised by this service.
    #[error("unsupported message: 0x{0:04X}")]
    UnsupportedMessage(u16),

    /// Payload failed to decode against the expected schema.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// Catch-all for internal failures inside the service implementation.
    #[error("internal: {0}")]
    Internal(String),
}

/// Implemented by every per-channel service (Video, Input, Sensor, …).
///
/// `aap-core` owns a registry keyed by `ChannelId`, dispatches inbound frames
/// to `handle`, and writes returned frames back via `Transport`.
///
/// Services are stateful and per-connection. Construct a fresh instance for
/// each accepted connection.
#[async_trait]
pub trait Service: Send {
    /// The channel this service serves.
    fn channel(&self) -> ChannelId;

    /// Descriptor for service discovery.
    fn descriptor(&self) -> ServiceDescriptor;

    /// Handle one inbound message, returning zero or more outbound frames.
    ///
    /// `message_id` is the channel-specific u16 stripped from the head of the
    /// inbound payload; `payload` is the remainder (the protobuf body).
    async fn handle(
        &mut self,
        message_id: u16,
        payload:    Bytes,
    ) -> Result<Vec<Frame>, ServiceError>;
}
```

---

## Step 7 — Stub crates

Each stub is identical in shape: `Cargo.toml` declaring the crate against the workspace, and `src/lib.rs` (or `src/main.rs` for the binary) containing a single comment. Names below; fill in the obvious pattern.

### `aap-proto/Cargo.toml`

```toml
[package]
name        = "aap-proto"
version     = "0.1.0"
edition.workspace      = true
license.workspace      = true
rust-version.workspace = true

[dependencies]
aap-contracts = { workspace = true }
prost         = { workspace = true }
bytes         = { workspace = true }

[build-dependencies]
prost-build   = { workspace = true }
```

`src/lib.rs`:
```rust
//! Generated protobuf types and helpers. Populated by W1.
//!
//! Until W1 lands, this crate is intentionally empty so the workspace builds.
```

### `aap-transport/Cargo.toml`

```toml
[package]
name        = "aap-transport"
version     = "0.1.0"
edition.workspace      = true
license.workspace      = true
rust-version.workspace = true

[dependencies]
aap-contracts = { workspace = true }
async-trait   = { workspace = true }
bytes         = { workspace = true }
thiserror     = { workspace = true }
tokio         = { workspace = true }
tracing       = { workspace = true }
openssl       = { workspace = true }
```

`src/lib.rs`:
```rust
//! TCP/USB transport with TLS upgrade. Populated by W2.
```

### `aap-core/Cargo.toml`

```toml
[package]
name        = "aap-core"
version     = "0.1.0"
edition.workspace      = true
license.workspace      = true
rust-version.workspace = true

[dependencies]
aap-contracts = { workspace = true }
aap-proto     = { workspace = true }
aap-transport = { workspace = true }
async-trait   = { workspace = true }
bytes         = { workspace = true }
thiserror     = { workspace = true }
tokio         = { workspace = true }
tracing       = { workspace = true }
```

`src/lib.rs`:
```rust
//! Connection state machine and service registry. Populated by W5.
```

### `aap-video/Cargo.toml`

```toml
[package]
name        = "aap-video"
version     = "0.1.0"
edition.workspace      = true
license.workspace      = true
rust-version.workspace = true

[dependencies]
aap-contracts = { workspace = true }
aap-proto     = { workspace = true }
async-trait   = { workspace = true }
bytes         = { workspace = true }
thiserror     = { workspace = true }
tokio         = { workspace = true }
tracing       = { workspace = true }
```

`src/lib.rs`:
```rust
//! Video projection service. Populated by W6.
```

### `smartcar-server/Cargo.toml`

```toml
[package]
name        = "smartcar-server"
version     = "0.1.0"
edition.workspace      = true
license.workspace      = true
rust-version.workspace = true

[dependencies]
aap-contracts      = { workspace = true }
aap-core           = { workspace = true }
aap-transport      = { workspace = true }
aap-video          = { workspace = true }
anyhow             = { workspace = true }
clap               = { workspace = true }
tokio              = { workspace = true }
tracing            = { workspace = true }
tracing-subscriber = { workspace = true }
```

`src/main.rs`:
```rust
//! smartcar — Android Auto projection source. Real entrypoint added by W5.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "smartcar-server", version)]
struct Args {
    /// Headunit target, e.g. `127.0.0.1:5277`.
    #[arg(long, default_value = "127.0.0.1:5277")]
    target: String,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    tracing::info!(target = %args.target, "smartcar-server stub — no-op");
    Ok(())
}
```

---

## Step 8 — CI

`.github/workflows/ci.yml`:

```yaml
name: ci
on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: "-D warnings"

jobs:
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.83
        with: { components: rustfmt }
      - run: cargo fmt --all -- --check
        working-directory: server

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.83
        with: { components: clippy }
      - uses: Swatinem/rust-cache@v2
        with: { workspaces: "server -> target" }
      - run: cargo clippy --workspace --all-targets -- -D warnings
        working-directory: server

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.83
      - uses: Swatinem/rust-cache@v2
        with: { workspaces: "server -> target" }
      - run: cargo test --workspace
        working-directory: server

  compose-config:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: docker compose -f docker/docker-compose.yml config >/dev/null
```

---

## Step 9 — Docker stubs

`docker/openauto.Dockerfile`:
```dockerfile
# Populated by W3. Stub keeps `docker compose config` valid.
FROM scratch
LABEL stage="p0-stub"
```

`docker/entrypoint.sh`:
```sh
#!/usr/bin/env sh
# Populated by W3.
echo "openauto entrypoint stub" >&2
exit 1
```
Make it executable (`chmod +x`).

`docker/docker-compose.yml`:
```yaml
services:
  openauto:
    build:
      context: ..
      dockerfile: docker/openauto.Dockerfile
    image: smartcar/openauto:dev
    network_mode: host
    environment:
      - DISPLAY=${DISPLAY:-:0}
    volumes:
      - /tmp/.X11-unix:/tmp/.X11-unix:rw
```

> Note: no `version:` key — Compose v2 ignores it. `docker compose config` must validate.

---

## Step 10 — Python script stubs

Identical shape; populated by W7. Use stdlib only.

`scripts/bootstrap.py`:
```python
#!/usr/bin/env python3
"""bootstrap.py — init submodules, build docker images, copy AA test certs.

Populated by W7.
"""
import sys


def main() -> int:
    print("bootstrap stub — not implemented", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
```

Same pattern for `run_emulator.py`, `run_server.py`, `run_stack.py`. Make each executable.

---

## Step 11 — App placeholders

`apps/ios/README.md`:
```markdown
# smartcar iOS

Placeholder. Xcode project lands in a later milestone. Native deps go under
`third_party/` (SPM/CocoaPods).
```

`apps/android/README.md`:
```markdown
# smartcar Android

Placeholder. Gradle project lands in a later milestone. Native deps go under
`third_party/`.
```

---

## Step 12 — Docs

`docs/architecture.md`:
```markdown
# smartcar architecture

`smartcar` plays the **projection source** role of the Android Auto protocol —
the side normally implemented by an Android phone running the Android Auto app.
It connects to a **head unit** which, for local development, is the
`openauto` emulator from opencardev.

Wire stack (bottom-up):

| Layer        | Crate           | Responsibility                                    |
|--------------|-----------------|---------------------------------------------------|
| Transport    | `aap-transport` | TCP/USB framing, multi-frame reassembly, TLS      |
| Codec        | `aap-proto`     | protobuf generated from AAProto                   |
| Control      | `aap-core`      | version, SSL, service discovery, channel mgmt     |
| Services     | `aap-video`, …  | per-channel logic (video sink, input source, …)   |
| Composition  | `smartcar-server` | bin: CLI, wiring, lifecycle                     |

All inter-crate coupling goes through `aap-contracts` (traits + POD types).
```

`docs/workstreams.md`:
```markdown
# Parallel workstreams (post-P0)

| ID | Crate / area         | Depends on | Can start when         |
|----|----------------------|------------|------------------------|
| W1 | `aap-proto`          | P0         | P0 merged              |
| W2 | `aap-transport`      | P0         | P0 merged              |
| W3 | Docker (`docker/`)   | P0         | P0 merged              |
| W4 | CI + repo hygiene    | P0         | P0 merged              |
| W5 | `aap-core` + bin     | W1, W2     | W1+W2 merged           |
| W6 | `aap-video`          | W5         | W5 merged              |
| W7 | `scripts/`           | W3 + W5    | W3+W5 merged           |

Each workstream owns its directory exclusively. Shared files
(`server/Cargo.toml`, `.github/workflows/ci.yml`, `docker/docker-compose.yml`)
are pre-populated in P0; later edits are append-only to minimise contention.
```

---

## Step 13 — Root README

`README.md`:
```markdown
# smartcar

Custom Android Auto projection source written in Rust. Connects to `openauto`
(head unit emulator) for local development; to a real car head unit in
production.

## Layout

- `server/` — Rust workspace (the projection source).
- `apps/ios`, `apps/android` — future client apps (placeholders).
- `docker/` — openauto emulator container + compose.
- `scripts/` — Python orchestration (stdlib only).
- `docs/` — architecture and workstream tracking.

## Quickstart

```sh
git submodule update --init --recursive
cd server && cargo check --workspace
```

See `docs/architecture.md`.
```

---

## Step 14 — Validate, then commit

```sh
cd server
cargo fmt --all
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd ..
docker compose -f docker/docker-compose.yml config >/dev/null
```

All must exit 0. If `cargo clippy` complains about anything in `aap-contracts`, fix it; do not `#[allow(...)]` to silence.

Then:
```sh
git add -A
git commit -m "P0: bootstrap monorepo scaffolding + aap-contracts"
```

If the repo is not yet on a remote, leave `origin` unset; user will wire it up.

---

## Out of scope (do not do)

- Any logic in `aap-proto`, `aap-transport`, `aap-core`, `aap-video`, `smartcar-server` beyond the stubs above. Those are W1, W2, W5, W6.
- Real Dockerfile contents. That is W3.
- Real Python scripts. That is W7.
- Initializing submodules (`git submodule update --init`). They must be **added** but left uninitialized; downstream agents fetch on demand.
- Adding any `#[allow(...)]` attributes to make warnings go away.
- Touching `apps/ios` or `apps/android` beyond the README placeholders.

## Hand-offs

After P0 lands, the W1 agent's first task is to `git submodule update --init server/third_party/AAProto` and verify the `ChannelId` and `MessageType` integer values in `aap-contracts` match the protobufs. Drift is fixed in their PR, not yours.
