//! Flutter Embedded renderer for the Android Auto video channel.
//!
//! This crate embeds a Flutter engine in-process via the C embedder API
//! (`flutter_embedder.h`).  Decoded H.264 NAL units from the AA video channel
//! are pushed into a Flutter `Texture(textureId: 0)` widget through the
//! engine's external-texture mechanism.
//!
//! # Build prerequisites
//!
//! * `FLUTTER_ENGINE_LIB_DIR` — directory containing `libflutter_engine.so`.
//! * `FLUTTER_ASSETS_DIR` (optional) — path to the compiled Flutter bundle;
//!   defaults to `../../flutter-ui/build/linux/x64/release/bundle` relative
//!   to this crate.
//!
//! # Runtime prerequisites
//!
//! Build the Flutter project before launching with `--flutter`:
//!
//! ```sh
//! cd server/flutter-ui
//! flutter build bundle          # debug (JIT)
//! # — or —
//! flutter build linux --release # release (AOT, needs libapp.so)
//! ```

mod decoder;
mod engine;
mod ffi;
mod sink;
mod texture;

pub use sink::FlutterSink;

/// Default asset bundle path baked in at compile time by `build.rs`.
///
/// Override `FLUTTER_ASSETS_DIR` at build time to change this.
pub const DEFAULT_ASSETS_DIR: &str = env!("FLUTTER_ASSETS_DIR");
