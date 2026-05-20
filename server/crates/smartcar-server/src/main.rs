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
//! Flutter is the **default** renderer.  `--testkit` explicitly opts in to
//! the synthetic producers (deterministic bringup / CI).  A Flutter init
//! failure is **fatal** — there is no implicit fallback — so missing engine
//! assets or runtime errors surface immediately rather than silently
//! degrading the user-visible UI to a test pattern.
//!
//! | Mode               | Video producer    | Audio producer       |
//! |--------------------|-------------------|----------------------|
//! | default (Flutter)  | Flutter embedder  | (silent for now)     |
//! | `--testkit`        | TestVideoProducer | looping WAV mixers   |

use std::sync::Arc;

use clap::{Parser, ValueEnum};
use tokio::net::TcpStream;

use aap_audio::{AudioService, MediaFmt, MixerSink, SpeechFmt, SystemFmt};
use aap_bridge::{run_bridge, BridgeTransport, ControlEvent, ControlRequest, DeviceInfo};
use aap_core::{Connection, ServiceRegistry};
use aap_input::{InputService, LogPointerSink, PointerSink};
use aap_testkit::{
    LoopingWavStream, TestVideoProducer, ASSET_KICK_IN, ASSET_SNARE_UNDER, ASSET_SYNTH_01,
};
use aap_transport::TcpTransport;
use aap_video::{
    advertise, video_frame_channel, video_start_gate, VideoService, VideoStartRx, SOFTWARE_CAPS,
};

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

/// Which transport carries the iOS-app bridge control plane.
#[derive(ValueEnum, Clone, Debug, PartialEq)]
enum BridgeChoice {
    /// TCP server — default; lets the iOS Simulator (no Bluetooth) connect
    /// to a `smartcar-server` on the same Mac.
    Tcp,
    /// BLE GATT server — Linux only. Used by the board's run/boot scripts.
    Ble,
    /// Disable the bridge.
    None,
}

#[derive(Parser, Debug)]
#[command(name = "smartcar-server", version)]
struct Args {
    /// Transport to use for the AA connection.
    #[arg(long, value_enum, default_value_t = TransportChoice::Tcp)]
    transport: TransportChoice,

    /// Head-unit address (TCP transport only).
    /// Native openauto (built via `scripts/run_openauto.py`) listens on 5278.
    #[arg(long, default_value = "127.0.0.1:5278")]
    target: String,

    /// Use the synthetic testkit producers instead of the Flutter renderer.
    ///
    /// Flutter is the default.  Pass this for bringup / CI validation when you
    /// want a deterministic colour-cycling video pattern and looping WAV audio
    /// with no Flutter engine dependency.
    #[arg(long, default_value_t = false)]
    testkit: bool,

    /// Transport for the iOS-app bridge.  Defaults to TCP so the Simulator
    /// can connect; the board's run/boot scripts pass `--bridge ble`.
    #[arg(long, value_enum, default_value_t = BridgeChoice::Tcp)]
    bridge: BridgeChoice,

    /// Listen address when `--bridge tcp`.
    #[arg(long, default_value = "127.0.0.1:4789")]
    bridge_addr: String,
}

/// Per-event flushing writer for `tracing-subscriber`.
///
/// `tracing-subscriber::fmt`'s default writer is `io::stdout()`, which on
/// Linux is a `LineWriter` over the stdout fd: it flushes on the `'\n'` at
/// the end of each event. *In normal operation* that's already per-line
/// durable. But when running on the board off USB-car-mode Vbus, a power
/// cut can come between the formatter's intra-event writes (timestamp →
/// level → target → message): the not-yet-newline pieces sit in the
/// LineWriter's small internal buffer and vanish with the process.
///
/// `FlushingStdout` forces a `flush()` after every `write` so each chunk
/// reaches the kernel pipe to journald immediately. The 0-vs-20-lines
/// python test on the board confirmed this is exactly the mechanism that
/// preserves data through a SIGKILL/Vbus-loss.
struct FlushingStdout;

impl std::io::Write for FlushingStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut h = std::io::stdout().lock();
        let n = h.write(buf)?;
        h.flush()?;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().lock().flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for FlushingStdout {
    type Writer = FlushingStdout;
    fn make_writer(&'a self) -> Self::Writer {
        FlushingStdout
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(FlushingStdout)
        .init();

    // First log line of the binary. With per-event flushing installed above,
    // even a Vbus cut immediately after this returns leaves "smartcar-server
    // starting" on disk — concrete proof the binary executed past `main()`'s
    // tracing init. If the next car attempt's journal has *nothing* from this
    // unit even with the flushing writer, the failure is upstream of `main()`
    // (binary not exec'd, or killed before tracing_subscriber initialized).
    tracing::info!(pid = std::process::id(), "smartcar-server: starting");

    let args = Args::parse();

    // ── iOS-app bridge ────────────────────────────────────────────────────────
    // Independent of the AA transport: a control plane the iOS app talks to
    // for out-of-band commands + signalling about A2DP / PAN state. Two
    // transports behind the same protobuf surface:
    //   --bridge tcp  → TCP server (default; Simulator-friendly, no Bluetooth)
    //   --bridge ble  → BLE GATT server (Linux only; the board's mode)
    //   --bridge none → disabled
    // Commands are logged for now; future wiring will route them to Flutter,
    // audio session, and the PAN watcher.
    let bridge_transport =
        match args.bridge {
            BridgeChoice::Tcp => BridgeTransport::Tcp(args.bridge_addr.parse().map_err(|e| {
                anyhow::anyhow!("invalid --bridge-addr '{}': {}", args.bridge_addr, e)
            })?),
            BridgeChoice::Ble => BridgeTransport::Ble,
            BridgeChoice::None => BridgeTransport::None,
        };
    spawn_bridge(bridge_transport);

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

    // Producer selection: Flutter by default, testkit only when explicitly
    // requested. A Flutter init failure propagates — no silent fallback —
    // so a broken engine deployment fails loud instead of pretending to
    // work with a test pattern.
    let pointer_sink: Arc<dyn PointerSink> = if args.testkit {
        start_testkit_producers(
            frame_tx,
            video_start_rx,
            &mut media_mixer,
            &mut speech_mixer,
            &mut system_mixer,
        )?;
        // No real UI to receive touches; head-unit input is logged.
        Arc::new(LogPointerSink)
    } else {
        // The Flutter embedder doubles as the pointer sink (touches drive its UI).
        start_flutter_producers(frame_tx, video_start_rx)?
    };

    // ── Transport + connection ────────────────────────────────────────────────
    // Build the negotiable config menu once; the descriptor (VideoService) and
    // the index resolver (Connection) must use the *same* list. Software-path
    // caps for now; the board GPU path will widen these.
    let advertised = advertise(&SOFTWARE_CAPS);

    let mut registry = ServiceRegistry::new();
    registry.register(VideoService::new(advertised.clone()));
    registry.register(AudioService::new(Box::new(media_mixer)));
    registry.register(AudioService::new(Box::new(speech_mixer)));
    registry.register(AudioService::new(Box::new(system_mixer)));
    registry.register(InputService::new(pointer_sink));

    match args.transport {
        TransportChoice::Tcp => {
            tracing::info!(target = %args.target, "TCP: connecting to head unit");
            let stream = TcpStream::connect(&args.target).await?;
            tracing::info!("TCP connection established");
            let transport = TcpTransport::new(stream);
            Connection::new(transport, registry, frame_rx, video_start_tx, advertised)
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
                Connection::new(transport, registry, frame_rx, video_start_tx, advertised)
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

// ── iOS-app bridge ────────────────────────────────────────────────────────────

/// Spawn the iOS-app bridge plus a command-drain logger.
///
/// The bridge owns its mpsc/broadcast channels for the lifetime of the
/// process. The drain task logs every decoded `ControlRequest` until future
/// consumers (Flutter, audio session, PAN watcher) take it over. The
/// broadcast sender is moved into the bridge task; event producers will get
/// their own clone when wired in.
fn spawn_bridge(transport: BridgeTransport) {
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<ControlRequest>(64);
    let (evt_tx, _) = tokio::sync::broadcast::channel::<ControlEvent>(64);

    tokio::spawn(async move {
        while let Some(req) = cmd_rx.recv().await {
            tracing::info!(?req, "bridge: command");
        }
    });

    tokio::spawn(async move {
        let info = DeviceInfo {
            name: "Smartcar".into(),
            firmware_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: 1,
        };
        if let Err(e) = run_bridge(transport, info, cmd_tx, evt_tx).await {
            tracing::warn!(error = %e, "bridge: stopped with error");
        }
    });
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

/// Start the Flutter renderer: launch the engine, point it at the AA video
/// resolution, and spawn the encode loop that streams composited frames.
///
/// Returns the pointer sink so head-unit touches can drive the Flutter UI.
/// Failure here is fatal: there is no fallback, and a broken engine deployment
/// must surface immediately rather than silently degrade to a test pattern.
/// Audio is silent for now — only the video path is wired.
fn start_flutter_producers(
    frame_tx: aap_video::VideoFrameSender,
    video_start_rx: VideoStartRx,
) -> anyhow::Result<Arc<dyn PointerSink>> {
    use aap_flutter::{
        new_store, resolve_flutter_paths, FlutterEngineHandle, FlutterVideoProducer,
    };

    // Launch the engine now, but defer window-metrics/encoder sizing to the
    // producer: the resolution isn't known until the head unit's
    // AVChannelSetupResponse arrives, which the producer receives via the
    // focus gate.
    let (assets, icu) = resolve_flutter_paths();
    let store = new_store();
    let engine = FlutterEngineHandle::launch(&assets, &icu, store.clone())?;

    // Grab a thread-safe pointer handle before the engine is moved into the
    // producer thread; it feeds head-unit touches to the UI.
    let pointer: Arc<dyn PointerSink> = Arc::new(engine.pointer_input());
    tokio::task::spawn_blocking(move || {
        // `engine` is moved into the producer so it outlives the encode loop
        // and is shut down cleanly when the loop returns.
        FlutterVideoProducer::new().run(store, frame_tx, video_start_rx, engine);
    });
    Ok(pointer)
}
