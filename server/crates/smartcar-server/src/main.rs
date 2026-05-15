//! smartcar — Android Auto projection source binary.
//!
//! Connects to a head unit over TCP, performs the AA protocol handshake, and
//! dispatches data frames to the registered services.

use clap::Parser;
use tokio::net::TcpStream;

use aap_core::{Connection, ServiceRegistry};
use aap_transport::TcpTransport;
use aap_video::{VideoConfig, VideoService};

#[derive(Parser, Debug)]
#[command(name = "smartcar-server", version)]
struct Args {
    /// Headunit target, e.g. `127.0.0.1:5277`.
    #[arg(long, default_value = "127.0.0.1:5277")]
    target: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    tracing::info!(target = %args.target, "connecting to head unit");
    let stream = TcpStream::connect(&args.target).await?;
    tracing::info!("TCP connection established");

    let transport = TcpTransport::new(stream);

    let mut registry = ServiceRegistry::new();
    registry.register(VideoService::new(VideoConfig::default()));

    let conn = Connection::new(transport, registry);
    conn.run().await?;

    tracing::info!("connection closed cleanly");
    Ok(())
}
