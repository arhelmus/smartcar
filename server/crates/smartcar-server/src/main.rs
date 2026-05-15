//! smartcar — Android Auto projection source binary.
//!
//! Connects to a head unit over TCP, performs the AA protocol handshake, and
//! dispatches data frames to the registered services.
//!
//! # Renderer selection
//!
//! | Build command                                    | `--flutter` flag | Renderer       |
//! |--------------------------------------------------|------------------|----------------|
//! | `cargo build`                                    | (unavailable)    | Null (default) |
//! | `cargo build --features flutter`                 | absent           | Null (default) |
//! | `cargo build --features flutter`                 | present          | Flutter        |
//!
//! When using the Flutter renderer the binary expects a compiled Flutter
//! bundle in the path baked by `FLUTTER_ASSETS_DIR` at build time (defaults to
//! `server/flutter-ui/build/linux/x64/release/bundle`).

use clap::Parser;
use tokio::net::TcpStream;

use aap_core::{Connection, ServiceRegistry};
use aap_transport::TcpTransport;
use aap_video::{VideoConfig, VideoService};

#[derive(Parser, Debug)]
#[command(name = "smartcar-server", version)]
struct Args {
    /// Head-unit target address, e.g. `127.0.0.1:5277`.
    #[arg(long, default_value = "127.0.0.1:5277")]
    target: String,

    /// Use the Flutter Embedded renderer instead of the null sink.
    ///
    /// Requires the binary to have been compiled with `--features flutter`
    /// and the Flutter project to be built beforehand:
    ///
    ///   cd server/flutter-ui && flutter build bundle
    #[arg(long, default_value_t = false)]
    flutter: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let video_service = build_video_service(args.flutter)?;

    tracing::info!(target = %args.target, "connecting to head unit");
    let stream = TcpStream::connect(&args.target).await?;
    tracing::info!("TCP connection established");

    let transport = TcpTransport::new(stream);

    let mut registry = ServiceRegistry::new();
    registry.register(video_service);

    let conn = Connection::new(transport, registry);
    conn.run().await?;

    tracing::info!("connection closed cleanly");
    Ok(())
}

fn build_video_service(want_flutter: bool) -> anyhow::Result<VideoService> {
    if want_flutter {
        return build_flutter_video_service();
    }
    Ok(VideoService::new(VideoConfig::default()))
}

#[cfg(feature = "flutter")]
fn build_flutter_video_service() -> anyhow::Result<VideoService> {
    use std::path::Path;
    use aap_flutter::{FlutterSink, DEFAULT_ASSETS_DIR};

    let assets = Path::new(DEFAULT_ASSETS_DIR);
    let icu = assets.join("icudtl.dat");

    tracing::info!(?assets, "starting Flutter engine");
    let sink = FlutterSink::new(assets, &icu)?;
    Ok(VideoService::with_sink(VideoConfig::default(), Box::new(sink)))
}

#[cfg(not(feature = "flutter"))]
fn build_flutter_video_service() -> anyhow::Result<VideoService> {
    anyhow::bail!(
        "--flutter requires the binary to be compiled with the `flutter` feature:\n  \
         FLUTTER_ENGINE_LIB_DIR=/path/to/engine cargo build --features flutter"
    )
}
