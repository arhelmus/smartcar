//! Sets the binary's rpath so the Flutter engine resolves at runtime.
//!
//! `aap-flutter`'s build script publishes the resolved engine directory as
//! `cargo:enginedir=…` metadata (via its `links = "flutter_engine"` key),
//! which Cargo exposes here as `DEP_FLUTTER_ENGINE_ENGINEDIR`.  The rpath has
//! to be emitted from *this* (binary) crate's build script — a library build
//! script's `rustc-link-arg` does not reach the final binary link.
//!
//! Only present when built with `--features flutter`; otherwise the dep (and
//! its metadata) doesn't exist and this is a no-op.

fn main() {
    let Ok(engine_dir) = std::env::var("DEP_FLUTTER_ENGINE_ENGINEDIR") else {
        return;
    };

    // Linux: `$ORIGIN` finds libflutter_engine.so rsynced next to the deployed
    // binary; the absolute cache dir makes a local `cargo run` work with no
    // LD_LIBRARY_PATH. macOS (dev host only): the cache dir is what dyld needs
    // to resolve the framework's `@rpath/FlutterEmbedder.framework/...` id.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,{engine_dir}");
}
