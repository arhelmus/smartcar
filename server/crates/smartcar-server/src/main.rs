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
