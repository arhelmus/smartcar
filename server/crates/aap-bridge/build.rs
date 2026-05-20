//! Build script for `aap-bridge`.
//!
//! Compiles `proto/control.proto` (the iPhone-bridge control plane) into
//! `$OUT_DIR` for `include!` from `src/lib.rs`.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let proto = manifest_dir.join("proto/control.proto");
    println!("cargo:rerun-if-changed={}", proto.display());

    prost_build::Config::new()
        .compile_protos(&[&proto], &[manifest_dir.join("proto")])
        .expect("compile aap-bridge control.proto");
}
