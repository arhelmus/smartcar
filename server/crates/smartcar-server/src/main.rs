//! smartcar — Android Auto projection source binary.
//!
//! Connects to a head unit and runs the full AA protocol.
//!
//! # Transport selection (`--transport`)
//!
//! | Flag                | Transport     | When to use                         |
//! |---------------------|---------------|-------------------------------------|
//! | `--transport tcp`   | TCP (default) | Local dev: board → laptop openauto  |
//! | `--transport usb`   | USB FunctionFS| Board plugged into real car head unit|
//!
//! The USB transport is Linux-only.  It runs the AOAP two-persona handshake
//! before starting the AA protocol.
//!
//! # Frame producer selection
//!
//! Flutter is the **default** renderer.  `--testkit` forces the synthetic
//! producers instead.  A binary built without `--features flutter`, or one
//! where the Flutter engine fails to start, transparently falls back to the
//! testkit producers at runtime (a warning is logged).
//!
//! | Flag / build              | Video producer          | Audio producer      |
//! |---------------------------|-------------------------|---------------------|
//! | default                   | Flutter embedder        | (silent for now)    |
//! | `--testkit`               | TestVideoProducer       | looping WAV mixers  |
//! | default, no `flutter` feat| TestVideoProducer (fb)  | looping WAV mixers  |

use clap::{Parser, ValueEnum};
use tokio::net::TcpStream;

use aap_audio::{AudioService, MediaFmt, MixerSink, SpeechFmt, SystemFmt};
use aap_core::{Connection, ServiceRegistry};
use aap_testkit::{
    LoopingWavStream, TestVideoProducer, ASSET_KICK_IN, ASSET_SNARE_UNDER, ASSET_SYNTH_01,
};
use aap_transport::TcpTransport;
use aap_video::{video_frame_channel, video_start_gate, VideoConfig, VideoService, VideoStartRx};

/// Which byte-level transport to use for the AA connection.
#[derive(ValueEnum, Clone, Debug, PartialEq)]
enum TransportChoice {
    /// TCP socket — for local development against the openauto emulator.
    Tcp,
    /// USB FunctionFS gadget — for connecting to a real (or laptop) head unit.
    ///
    /// Runs the AOAP two-persona handshake automatically.
    /// Linux only; requires root / CAP_SYS_ADMIN.
    Usb,
}

#[derive(Parser, Debug)]
#[command(name = "smartcar-server", version)]
struct Args {
    /// Transport to use for the AA connection.
    #[arg(long, value_enum, default_value_t = TransportChoice::Tcp)]
    transport: TransportChoice,

    /// Head-unit address (TCP transport only).
    /// openauto listens on 5000 inside Docker, mapped to host 5001
    /// (5000 is taken by macOS AirPlay Receiver).
    #[arg(long, default_value = "127.0.0.1:5001")]
    target: String,

    /// Use the synthetic testkit producers instead of the Flutter renderer.
    ///
    /// Flutter is the default.  Pass this for bringup / CI validation when you
    /// want a deterministic colour-cycling video pattern and looping WAV audio
    /// with no Flutter engine dependency.
    #[arg(long, default_value_t = false)]
    testkit: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    // ── Video frame channel + focus gate ──────────────────────────────────────
    // The sender goes to the producer, the receiver to Connection. The start
    // gate keeps the producer idle until the head unit grants video focus, so
    // its first encoded frame (a fresh IDR) is the first frame on the wire.
    let (frame_tx, frame_rx) = video_frame_channel();
    let (video_start_tx, video_start_rx) = video_start_gate();

    // ── Audio mixers ──────────────────────────────────────────────────────────
    // Format is the type parameter; the canonical per-stream layout the head
    // unit expects is encoded in MediaFmt/SpeechFmt/SystemFmt.
    let mut media_mixer = MixerSink::<MediaFmt>::new();
    let mut speech_mixer = MixerSink::<SpeechFmt>::new();
    let mut system_mixer = MixerSink::<SystemFmt>::new();

    // Flutter is the default; --testkit forces the synthetic producers.
    // If the Flutter renderer can't start (not compiled in, engine init
    // failure), fall back to the testkit producers so the connection still
    // comes up.
    let mut run_testkit = args.testkit;
    if !run_testkit {
        match start_flutter_producers(frame_tx.clone()) {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Flutter renderer unavailable — falling back to testkit producers"
                );
                run_testkit = true;
            }
        }
    }
    if run_testkit {
        start_testkit_producers(
            frame_tx,
            video_start_rx,
            &mut media_mixer,
            &mut speech_mixer,
            &mut system_mixer,
        )?;
    }

    // ── Transport + connection ────────────────────────────────────────────────
    let mut registry = ServiceRegistry::new();
    registry.register(VideoService::new(VideoConfig::default()));
    registry.register(AudioService::new(Box::new(media_mixer)));
    registry.register(AudioService::new(Box::new(speech_mixer)));
    registry.register(AudioService::new(Box::new(system_mixer)));

    match args.transport {
        TransportChoice::Tcp => {
            tracing::info!(target = %args.target, "TCP: connecting to head unit");
            let stream = TcpStream::connect(&args.target).await?;
            tracing::info!("TCP connection established");
            let transport = TcpTransport::new(stream);
            Connection::new(transport, registry, frame_rx, video_start_tx)
                .run()
                .await?;
        }
        TransportChoice::Usb => {
            #[cfg(target_os = "linux")]
            {
                use aap_transport::UsbTransport;
                tracing::info!(
                    "USB: starting AOAP handshake — plug the USB cable into the head unit"
                );
                let transport = UsbTransport::connect().await?;
                Connection::new(transport, registry, frame_rx, video_start_tx)
                    .run()
                    .await?;
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!(
                    "--transport usb is only supported on Linux \
                     (FunctionFS gadget requires the Linux USB gadget stack)"
                );
            }
        }
    }

    tracing::info!("connection closed cleanly");
    Ok(())
}

// ── Testkit producers ─────────────────────────────────────────────────────────

fn start_testkit_producers(
    frame_tx: aap_video::VideoFrameSender,
    video_start_rx: VideoStartRx,
    media_mixer: &mut MixerSink<MediaFmt>,
    speech_mixer: &mut MixerSink<SpeechFmt>,
    system_mixer: &mut MixerSink<SystemFmt>,
) -> anyhow::Result<()> {
    // Video: colour-cycling H.264 test pattern at 30 fps. The producer stays
    // idle until Connection signals video focus via the start gate.
    let video_producer = TestVideoProducer::new(30)?;
    tokio::task::spawn_blocking(move || video_producer.run(frame_tx, video_start_rx));

    // Audio: pull-based looping WAV streams — no threads, no channels, no
    // timing drift.  Each WAV is decoded and run once through the Normalizer
    // boundary into its channel's format inside LoopingWavStream, so the
    // mixer only ever sees uniform, type-correct samples.
    media_mixer.add_stream(Box::new(LoopingWavStream::<MediaFmt>::from_embedded_wav(
        ASSET_SYNTH_01,
    )?));
    speech_mixer.add_stream(Box::new(LoopingWavStream::<SpeechFmt>::from_embedded_wav(
        ASSET_KICK_IN,
    )?));
    system_mixer.add_stream(Box::new(LoopingWavStream::<SystemFmt>::from_embedded_wav(
        ASSET_SNARE_UNDER,
    )?));

    Ok(())
}

// ── Flutter producers ─────────────────────────────────────────────────────────

fn start_flutter_producers(_frame_tx: aap_video::VideoFrameSender) -> anyhow::Result<()> {
    build_flutter_engine(_frame_tx)
}

#[cfg(feature = "flutter")]
fn build_flutter_engine(_frame_tx: aap_video::VideoFrameSender) -> anyhow::Result<()> {
    // TODO(M2): wire FlutterEngine present callback → frame_tx,
    //           and method channel handler → audio source handles.
    anyhow::bail!("Flutter producer not yet implemented (M2)")
}

#[cfg(not(feature = "flutter"))]
fn build_flutter_engine(_frame_tx: aap_video::VideoFrameSender) -> anyhow::Result<()> {
    anyhow::bail!(
        "--flutter requires the binary to be compiled with the `flutter` feature:\n  \
         FLUTTER_ENGINE_LIB_DIR=/path/to/engine cargo build --features flutter"
    )
}
