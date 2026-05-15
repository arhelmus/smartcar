fn main() {
    println!("cargo:rerun-if-env-changed=FLUTTER_ENGINE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=FLUTTER_ASSETS_DIR");

    // Emit link directives only when FLUTTER_ENGINE_LIB_DIR is set.
    // `cargo check` succeeds without the library (no linking phase).
    // `cargo build --features flutter` without the var will fail at link time
    // with a clear linker error pointing back to this warning.
    match std::env::var("FLUTTER_ENGINE_LIB_DIR") {
        Ok(dir) => {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=dylib=flutter_engine");
        }
        Err(_) => {
            println!(
                "cargo:warning=FLUTTER_ENGINE_LIB_DIR is not set. \
                 Building with --features flutter will fail at link time. \
                 Set it to the directory containing libflutter_engine.so, e.g.:\n  \
                 export FLUTTER_ENGINE_LIB_DIR=/opt/flutter-engine/linux-x64-release\n  \
                 Download: https://storage.googleapis.com/flutter_infra_release/\
                 releases/stable/linux/flutter_linux_<VERSION>-stable.tar.xz"
            );
        }
    }

    // FLUTTER_ASSETS_DIR defaults to the standard flutter build output path
    // relative to this crate.  Override to relocate the bundle at build time.
    let assets_dir = std::env::var("FLUTTER_ASSETS_DIR").unwrap_or_else(|_| {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        format!("{manifest}/../../flutter-ui/build/linux/x64/release/bundle")
    });
    println!("cargo:rustc-env=FLUTTER_ASSETS_DIR={assets_dir}");
}
