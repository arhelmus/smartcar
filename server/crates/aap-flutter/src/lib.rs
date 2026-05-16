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

/// Path to the `flutter_assets/` directory, baked in at compile time.
///
/// Populated automatically by `build.rs` from `flutter build bundle` output.
/// Override with `FLUTTER_ASSETS_DIR=<path>` at build time.
pub const DEFAULT_ASSETS_DIR: &str = env!("FLUTTER_ASSETS_DIR");

/// Path to `icudtl.dat`, baked in at compile time.
///
/// Resolved by `build.rs` in order: `FLUTTER_ICU_DATA` env var →
/// `$FLUTTER_ENGINE_LIB_DIR/icudtl.dat` → Flutter SDK artifact cache.
/// Empty string when none of the above were found at build time.
pub const DEFAULT_ICU_DATA: &str = env!("FLUTTER_ICU_DATA");

/// Resolve `(flutter_assets_dir, icudtl_dat)` paths at runtime.
///
/// Search order — first match wins:
///
/// 1. **Next to the binary** — `build.rs` copies `flutter_assets/` and
///    `icudtl.dat` into `target/<profile>/` alongside the binary, so a
///    deployment just needs those two items next to the executable.
/// 2. **Runtime env overrides** — `FLUTTER_ASSETS_DIR` / `FLUTTER_ICU_DATA`.
/// 3. **Compile-time defaults** — absolute paths baked in by `build.rs`.
pub fn resolve_flutter_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    // 1. Relative to the running binary — works for deployed/packaged builds.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let assets = dir.join("flutter_assets");
            if assets.exists() {
                let icu = dir.join("icudtl.dat");
                return (assets, icu);
            }
        }
    }

    // 2. Runtime env overrides.
    let assets = std::env::var("FLUTTER_ASSETS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(DEFAULT_ASSETS_DIR));

    let icu = std::env::var("FLUTTER_ICU_DATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(DEFAULT_ICU_DATA));

    (assets, icu)
}
