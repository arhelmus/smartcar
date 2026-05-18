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
//! | Build / flag              | Video producer          | Audio producer      |
//! |---------------------------|-------------------------|---------------------|
//! | default (no flag)         | TestVideoProducer       | TestAudioProducer   |
//! | `--flutter` (+ feature)   | Flutter embedder (TODO) | Flutter method chan  |

use clap::{Parser, ValueEnum};
use tokio::net::TcpStream;

use aap_audio::{AudioService, AudioStream, AudioStreamConfig, MixerSink, ResampleStream};
use aap_contracts::ChannelId;
use aap_core::{Connection, ServiceRegistry};
use aap_testkit::{
    LoopingWavStream, TestVideoProducer, ASSET_KICK_IN, ASSET_SNARE_UNDER, ASSET_SYNTH_01,
};
use aap_transport::TcpTransport;
use aap_video::{video_frame_channel, VideoConfig, VideoService};

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

    /// Use the Flutter Embedded renderer instead of the testkit producers.
    ///
    /// Requires `--features flutter` at build time and a compiled Flutter
    /// bundle:  cd server/flutter-ui && flutter build bundle
    #[arg(long, default_value_t = false)]
    flutter: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    // ── Video frame channel ───────────────────────────────────────────────────
    // The sender goes to the frame producer; the receiver goes to Connection.
    let (frame_tx, frame_rx) = video_frame_channel();

    // ── Audio mixers ──────────────────────────────────────────────────────────
    // Use the canonical per-stream formats the head unit expects.
    let media_cfg = AudioStreamConfig::media_audio();
    let speech_cfg = AudioStreamConfig::speech_audio();
    let system_cfg = AudioStreamConfig::system_audio();

    let mut media_mixer = MixerSink::new(media_cfg.clone());
    let mut speech_mixer = MixerSink::new(speech_cfg.clone());
    let mut system_mixer = MixerSink::new(system_cfg.clone());

    if args.flutter {
        start_flutter_producers(frame_tx)?;
    } else {
        start_testkit_producers(
            frame_tx,
            &mut media_mixer,
            &mut speech_mixer,
            &mut system_mixer,
        )?;
    }

    // ── Transport + connection ────────────────────────────────────────────────
    let mut registry = ServiceRegistry::new();
    registry.register(VideoService::new(VideoConfig::default()));
    registry.register(AudioService::new(
        ChannelId::MediaAudio,
        media_cfg,
        Box::new(media_mixer),
    ));
    registry.register(AudioService::new(
        ChannelId::SpeechAudio,
        speech_cfg,
        Box::new(speech_mixer),
    ));
    registry.register(AudioService::new(
        ChannelId::SystemAudio,
        system_cfg,
        Box::new(system_mixer),
    ));

    match args.transport {
        TransportChoice::Tcp => {
            tracing::info!(target = %args.target, "TCP: connecting to head unit");
            let stream = TcpStream::connect(&args.target).await?;
            tracing::info!("TCP connection established");
            let transport = TcpTransport::new(stream);
            Connection::new(transport, registry, frame_rx).run().await?;
        }
        TransportChoice::Usb => {
            #[cfg(target_os = "linux")]
            {
                use aap_transport::UsbTransport;
                tracing::info!(
                    "USB: starting AOAP handshake — plug the USB cable into the head unit"
                );
                let transport = UsbTransport::connect().await?;
                Connection::new(transport, registry, frame_rx).run().await?;
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
    media_mixer: &mut MixerSink,
    speech_mixer: &mut MixerSink,
    system_mixer: &mut MixerSink,
) -> anyhow::Result<()> {
    // Video: colour-cycling H.264 test pattern at 30 fps.
    let video_producer = TestVideoProducer::new(30)?;
    tokio::task::spawn_blocking(move || video_producer.run(frame_tx));

    // Audio: pull-based looping WAV streams — no threads, no channels, no
    // timing drift.  The mixer tick drives sample generation synchronously.
    // The embedded assets are 44.1 kHz; each is resampled to its channel's
    // canonical rate via a chained ResampleStream before reaching the mixer.
    let media_cfg = media_mixer.config().clone();
    let speech_cfg = speech_mixer.config().clone();
    let system_cfg = system_mixer.config().clone();

    media_mixer.add_stream(looping_wav_resampled(ASSET_SYNTH_01, &media_cfg)?);
    speech_mixer.add_stream(looping_wav_resampled(ASSET_KICK_IN, &speech_cfg)?);
    system_mixer.add_stream(looping_wav_resampled(ASSET_SNARE_UNDER, &system_cfg)?);

    Ok(())
}

/// Sample rate of every embedded testkit WAV asset.
const ASSET_WAV_RATE: u32 = 44_100;

/// Build a looping WAV source resampled to `out_cfg`'s rate.
///
/// `LoopingWavStream` decodes the 44.1 kHz asset and adapts its channel
/// count to `out_cfg`; `ResampleStream` then converts 44.1 kHz → the
/// channel's canonical rate so the mixer sees a single uniform format.
fn looping_wav_resampled(
    bytes: &'static [u8],
    out_cfg: &AudioStreamConfig,
) -> anyhow::Result<Box<dyn AudioStream>> {
    let in_cfg = AudioStreamConfig {
        sample_rate: ASSET_WAV_RATE,
        bit_depth: 16,
        channel_count: out_cfg.channel_count,
        audio_type: out_cfg.audio_type,
    };
    let wav = LoopingWavStream::from_embedded_wav(bytes, in_cfg.clone())?;
    Ok(Box::new(ResampleStream::new(
        Box::new(wav),
        in_cfg,
        out_cfg.sample_rate,
    )))
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
