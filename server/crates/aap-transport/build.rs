//! Build script for `aap-transport`.
//!
//! Compiles the **AAW (Android Auto Wireless)** protobuf schemas vendored in
//! `third_party/aasdk/protobuf/aap_protobuf/`. These define the messages that
//! travel over the RFCOMM channel during the BT handshake (phone-side
//! WifiInfoRequest, etc. — see `src/bt/handshake.rs`).
//!
//! The aap-proto crate already generates the *wired* AA messages from
//! `third_party/AAProto/`; AAW lives in the aasdk tree under a different
//! package namespace (`aap_protobuf.aaw`), so it is generated locally here
//! rather than being added to aap-proto.
//!
//! Mirrors aap-proto's resilience pattern: if the aasdk submodule is missing,
//! emit `cargo:rustc-cfg=aap_transport_no_aaw` so the bt module is excluded
//! from the build instead of failing it.

use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(aap_transport_no_aaw)");

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let aasdk_pb_root = manifest_dir.join("../../third_party/aasdk/protobuf");
    let aaw_dir = aasdk_pb_root.join("aap_protobuf/aaw");
    let wp_dir = aasdk_pb_root.join("aap_protobuf/service/wifiprojection/message");

    if !aaw_dir.exists() || !wp_dir.exists() {
        println!(
            "cargo:warning=aap-transport: aasdk submodule absent ({} / {}); \
             AAW bt module will be excluded",
            aaw_dir.display(),
            wp_dir.display()
        );
        println!("cargo:rustc-cfg=aap_transport_no_aaw");
        return;
    }

    println!("cargo:rerun-if-changed={}", aaw_dir.display());
    println!("cargo:rerun-if-changed={}", wp_dir.display());

    // Files needed to fully define every aaw message, with their imports.
    // WifiSecurityMode / AccessPointType are imported by WifiInfoResponse.
    let proto_files: Vec<PathBuf> = [
        "aap_protobuf/aaw/MessageId.proto",
        "aap_protobuf/aaw/Status.proto",
        "aap_protobuf/aaw/WifiVersionRequest.proto",
        "aap_protobuf/aaw/WifiVersionResponse.proto",
        "aap_protobuf/aaw/WifiInfoRequest.proto",
        "aap_protobuf/aaw/WifiInfoResponse.proto",
        "aap_protobuf/aaw/WifiStartRequest.proto",
        "aap_protobuf/aaw/WifiStartResponse.proto",
        "aap_protobuf/aaw/WifiConnectionStatus.proto",
        "aap_protobuf/service/wifiprojection/message/WifiSecurityMode.proto",
        "aap_protobuf/service/wifiprojection/message/AccessPointType.proto",
    ]
    .iter()
    .map(|rel| aasdk_pb_root.join(rel))
    .collect();

    for p in &proto_files {
        if !p.exists() {
            println!(
                "cargo:warning=aap-transport: missing aaw proto {}; AAW disabled",
                p.display()
            );
            println!("cargo:rustc-cfg=aap_transport_no_aaw");
            return;
        }
    }

    let mut config = prost_build::Config::new();

    if let Err(err) = config.compile_protos(&proto_files, &[aasdk_pb_root]) {
        println!(
            "cargo:warning=aap-transport: prost-build failed ({err}); AAW disabled"
        );
        println!("cargo:rustc-cfg=aap_transport_no_aaw");
    }
}
