//! Build script for `aap-proto`.
//!
//! Compiles the Android Auto `.proto` files from `third_party/AAProto/`
//! using `prost-build` and writes the generated Rust modules into `$OUT_DIR`.
//!
//! Only the protos needed for the control channel and service-discovery flow
//! are compiled here (W1 scope). Additional channel protos can be added in
//! later work items.

use std::path::PathBuf;

fn main() {
    // Declare the custom cfg key so rustc's check-cfg lint accepts it without
    // an `unexpected_cfg` warning even when this build script is the one that
    // emits it conditionally.
    println!("cargo::rustc-check-cfg=cfg(aap_proto_stub)");

    // Resolve the proto directory relative to the workspace root.
    // CARGO_MANIFEST_DIR is `server/crates/aap-proto`; going up two levels
    // reaches `server/`, and then we descend into `third_party/AAProto`.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let proto_dir = manifest_dir.join("../../third_party/AAProto");

    // Bail gracefully if the submodule has not been initialized yet so that CI
    // environments without the submodule can still compile the crate in stub
    // mode (lib.rs handles the `cfg` flag below).
    if !proto_dir.exists() {
        println!(
            "cargo:warning=aap-proto: AAProto submodule not found at {}; using stub mode",
            proto_dir.display()
        );
        println!("cargo:rustc-cfg=aap_proto_stub");
        return;
    }

    // Tell Cargo to re-run this script whenever a proto file changes.
    println!("cargo:rerun-if-changed={}", proto_dir.display());

    // -----------------------------------------------------------------------
    // Full set of .proto files required for the control channel +
    // service-discovery handshake (dependencies listed in topological order).
    // -----------------------------------------------------------------------
    let proto_names: &[&str] = &[
        // ── Leaf enums / data types (no imports) ───────────────────────────
        "StatusEnum.proto",
        "ShutdownReasonEnum.proto",
        "AudioFocusTypeEnum.proto",
        "AudioFocusStateEnum.proto",
        "SensorTypeEnum.proto",
        "AVStreamTypeEnum.proto",
        "AudioTypeEnum.proto",
        "AudioConfigData.proto",
        "VideoResolutionEnum.proto",
        "VideoFPSEnum.proto",
        "TouchConfigData.proto",
        "BluetoothPairingMethodEnum.proto",
        "NavigationImageOptionsData.proto",
        "VendorExtensionChannelData.proto",
        "MediaChannelData.proto",
        // ── Level-1 data ───────────────────────────────────────────────────
        "SensorData.proto",
        "VideoConfigData.proto",
        // ── Level-2 channel descriptors ────────────────────────────────────
        "SensorChannelData.proto",
        "AVChannelData.proto",
        "InputChannelData.proto",
        "AVInputChannelData.proto",
        "BluetoothChannelData.proto",
        "NavigationChannelData.proto",
        // ── Level-3 composite descriptor ──────────────────────────────────
        "ChannelDescriptorData.proto",
        // ── Control-channel messages ───────────────────────────────────────
        "ControlMessageIdsEnum.proto",
        "ServiceDiscoveryRequestMessage.proto",
        "ServiceDiscoveryResponseMessage.proto",
        "ChannelOpenRequestMessage.proto",
        "ChannelOpenResponseMessage.proto",
        "PingRequestMessage.proto",
        "PingResponseMessage.proto",
        "NavigationFocusRequestMessage.proto",
        "NavigationFocusResponseMessage.proto",
        "ShutdownRequestMessage.proto",
        "ShutdownResponseMessage.proto",
        "AuthCompleteIndicationMessage.proto",
        "AudioFocusRequestMessage.proto",
        "AudioFocusResponseMessage.proto",
    ];

    // Map each bare filename to its full path; skip any that are missing so
    // the build remains resilient to submodule state changes.
    let proto_paths: Vec<PathBuf> = proto_names
        .iter()
        .map(|name| proto_dir.join(name))
        .filter(|p| {
            if !p.exists() {
                println!(
                    "cargo:warning=aap-proto: skipping missing proto file {}",
                    p.display()
                );
            }
            p.exists()
        })
        .collect();

    if proto_paths.is_empty() {
        println!("cargo:warning=aap-proto: no proto files found; using stub mode");
        println!("cargo:rustc-cfg=aap_proto_stub");
        return;
    }

    let mut config = prost_build::Config::new();

    match config.compile_protos(&proto_paths, std::slice::from_ref(&proto_dir)) {
        Ok(()) => {}
        Err(err) => {
            // Rather than hard-failing the build, emit a warning and fall back
            // to stub mode so the rest of the workspace remains compilable.
            println!(
                "cargo:warning=aap-proto: prost-build failed ({}); falling back to stub mode",
                err
            );
            println!("cargo:rustc-cfg=aap_proto_stub");
        }
    }
}
