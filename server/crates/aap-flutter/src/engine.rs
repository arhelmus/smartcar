//! Flutter engine lifecycle: initialise, register textures, shut down.
//!
//! # Threading model
//!
//! `FlutterEngineRun` spawns the engine's own render / UI / IO threads
//! internally.  All public `FlutterEngine*` API calls are documented as
//! thread-safe, so [`FlutterEngineHandle`] is `Send + Sync`.
//!
//! The `surface_present_callback` is invoked on the engine's render thread.
//! In this P0 implementation it discards the composited frame (logs only).
//! A future step will blit to a KMS/DRM plane or `/dev/fb0`.

use std::ffi::CString;
use std::os::raw::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

use anyhow::Context as _;

use crate::ffi::{
    self, FlutterEngine, FlutterProjectArgs, FlutterRendererConfig, FlutterWindowMetricsEvent,
};
use crate::lib_loader::FlutterLib;
use crate::texture::{PixelStore, SharedPixelStore};

// ── Callback data ─────────────────────────────────────────────────────────────

/// Heap-allocated data passed as `user_data` to every embedder callback.
///
/// The pointer is set by `FlutterEngineHandle::launch` immediately after
/// `FlutterEngineRun` returns and before any external-texture signals are sent.
pub(crate) struct EngineCallbackData {
    /// Latest composited frame, written by [`surface_present_callback`] and
    /// read by the encoder task.
    pub store: SharedPixelStore,
    /// Raw engine handle; null until `FlutterEngineRun` has returned.
    pub engine: AtomicPtr<c_void>,
}

// Safety: AtomicPtr is Send; SharedPixelStore is Arc<RwLock<_>> which is Send.
unsafe impl Send for EngineCallbackData {}
unsafe impl Sync for EngineCallbackData {}

// ── Callbacks ─────────────────────────────────────────────────────────────────

/// Software renderer surface-present callback.
///
/// Called on the Flutter render thread each time Flutter finishes compositing
/// a frame.  The composited buffer is the platform-native 32-bit format
/// (`row_bytes` may exceed `width × 4` due to stride padding).  We copy it —
/// de-padded to a tightly packed `width × height × 4` buffer — into the shared
/// [`PixelStore`]; the encoder task reads the latest frame from there and
/// pushes H.264 to the Android Auto video channel.
unsafe extern "C" fn surface_present_callback(
    user_data: *mut c_void,
    allocation: *const c_void,
    row_bytes: usize,
    height: usize,
) -> bool {
    if user_data.is_null() || allocation.is_null() || row_bytes == 0 || height == 0 {
        return false;
    }
    let data = &*(user_data as *const EngineCallbackData);

    let width = row_bytes / 4;
    let src = std::slice::from_raw_parts(allocation as *const u8, row_bytes * height);

    // De-pad rows: copy width*4 bytes per row, skipping the row stride.
    let mut packed = vec![0u8; width * height * 4];
    for row in 0..height {
        let s = row * row_bytes;
        let d = row * width * 4;
        packed[d..d + width * 4].copy_from_slice(&src[s..s + width * 4]);
    }

    {
        let mut store = data.store.write();
        *store = PixelStore {
            rgba: packed,
            width: width as u32,
            height: height as u32,
        };
    }
    tracing::trace!(width, height, "flutter: composited frame captured");
    true
}

// ── Engine handle ─────────────────────────────────────────────────────────────

/// Newtype to impl `Send` for a raw Flutter engine pointer.
struct RawEngine(FlutterEngine);
// Safety: Flutter engine API functions are documented as thread-safe.
unsafe impl Send for RawEngine {}
unsafe impl Sync for RawEngine {}

/// Owns the running Flutter engine and its callback data.
///
/// Shutting down the engine (via [`Drop`]) must happen before the
/// `_data` allocation is freed, which is guaranteed by Rust's drop order:
/// the `Drop::drop` body runs first, then struct fields drop in
/// declaration order.
pub struct FlutterEngineHandle {
    engine: RawEngine,
    /// Holds the `dlopen`'d libflutter_engine.so and the typed fn pointers we
    /// dispatch through.  Last in declaration order so the engine is shut
    /// down via `FlutterEngineShutdown` *before* the library is unloaded.
    lib: Arc<FlutterLib>,
    // Keep CStrings alive — the engine may reference the paths after init.
    _assets_cstr: CString,
    _icu_cstr: CString,
    // Dropped after engine shutdown; callback data must outlive the engine.
    _data: Box<EngineCallbackData>,
}

impl FlutterEngineHandle {
    /// Launch the Flutter engine.
    ///
    /// `assets_dir` must be the `flutter_assets/` directory produced by
    /// `flutter build bundle` — this path is passed directly to
    /// `FlutterProjectArgs.assets_path`.
    /// `icu_data` must point to `icudtl.dat` (ships with
    /// `libflutter_engine.so` and is also cached in the Flutter SDK).
    ///
    /// After this call the engine is running and an external texture with
    /// id `0` is registered for the video stream.
    pub fn launch(
        lib: Arc<FlutterLib>,
        assets_dir: &Path,
        icu_data: &Path,
        store: SharedPixelStore,
    ) -> anyhow::Result<Self> {
        let assets_cstr = CString::new(
            assets_dir
                .to_str()
                .context("assets_dir is not valid UTF-8")?,
        )?;
        let icu_cstr = CString::new(
            icu_data
                .to_str()
                .context("icu_data path is not valid UTF-8")?,
        )?;

        let data = Box::new(EngineCallbackData {
            store,
            engine: AtomicPtr::new(std::ptr::null_mut()),
        });
        let data_ptr = data.as_ref() as *const EngineCallbackData as *mut c_void;

        let renderer = FlutterRendererConfig::software(surface_present_callback);
        let args = FlutterProjectArgs::new(assets_cstr.as_ptr(), icu_cstr.as_ptr())
            .with_aot_snapshots(&lib);

        let mut raw: FlutterEngine = std::ptr::null_mut();
        let rc = unsafe {
            (lib.run)(
                ffi::FLUTTER_ENGINE_VERSION,
                &renderer,
                &args,
                data_ptr,
                &mut raw,
            )
        };

        if rc != ffi::kSuccess {
            anyhow::bail!(
                "FlutterEngineRun failed (code {rc}).\n\
                 Ensure the Flutter project is built:\n  \
                 cd server/flutter-ui && flutter build bundle\n\
                 and that libflutter_engine.so is next to the binary."
            );
        }

        // Store the handle so the texture-dirty callbacks can use it.
        data.engine.store(raw, Ordering::Release);

        // Register the video-stream external texture (id = 0).
        let rc = unsafe { (lib.register_external_texture)(raw, 0) };
        if rc != ffi::kSuccess {
            tracing::warn!(
                code = rc,
                "FlutterEngineRegisterExternalTexture returned non-success; \
                 the Texture(textureId: 0) widget will show nothing"
            );
        }

        tracing::info!(?assets_dir, "Flutter engine running");
        Ok(Self {
            engine: RawEngine(raw),
            lib,
            _assets_cstr: assets_cstr,
            _icu_cstr: icu_cstr,
            _data: data,
        })
    }

    /// Tell the engine the render-surface size.
    ///
    /// Must be called once after [`launch`](Self::launch); until it receives
    /// non-zero window metrics the engine keeps a 0×0 surface and never
    /// composites a frame (the present callback is never invoked).
    pub fn send_window_metrics(
        &self,
        width: u32,
        height: u32,
        pixel_ratio: f64,
    ) -> anyhow::Result<()> {
        let event = FlutterWindowMetricsEvent::new(width as usize, height as usize, pixel_ratio);
        let rc = unsafe { (self.lib.send_window_metrics_event)(self.engine.0, &event) };
        if rc != ffi::kSuccess {
            anyhow::bail!("FlutterEngineSendWindowMetricsEvent failed (code {rc})");
        }
        tracing::info!(width, height, "Flutter window metrics sent");
        Ok(())
    }

    /// A cloneable, thread-safe handle for injecting pointer events.
    ///
    /// The embedder pointer API is documented thread-safe, so this can be
    /// held by the input service (running on the connection task) while the
    /// engine itself is owned by the producer thread.
    pub fn pointer_input(&self) -> FlutterPointerInput {
        FlutterPointerInput {
            engine: self.engine.0,
            lib: Arc::clone(&self.lib),
        }
    }

    /// Signal to the engine that a new frame is ready for texture id `0`.
    ///
    /// Called from the Tokio task after each decoded video frame is written
    /// into the [`SharedPixelStore`].  Thread-safe; the engine API is
    /// documented to allow calls from any thread.
    pub fn mark_texture_dirty(&self) {
        let rc = unsafe { (self.lib.mark_external_texture_frame_available)(self.engine.0, 0) };
        if rc != ffi::kSuccess {
            tracing::warn!(
                code = rc,
                "FlutterEngineMarkExternalTextureFrameAvailable failed"
            );
        }
    }
}

// ── Pointer input ─────────────────────────────────────────────────────────────

/// Thread-safe handle for feeding head-unit touches into the engine.
///
/// Holds the raw engine pointer (valid for the engine's lifetime; the engine
/// is owned by the producer thread and shut down only when the connection
/// ends). The embedder pointer API is documented thread-safe.
#[derive(Clone)]
pub struct FlutterPointerInput {
    engine: FlutterEngine,
    /// Holds the engine library alive for the lifetime of every clone of
    /// this handle — the input service may outlive the producer thread.
    lib: Arc<FlutterLib>,
}

// Safety: FlutterEngineSendPointerEvent is thread-safe per the embedder docs.
unsafe impl Send for FlutterPointerInput {}
unsafe impl Sync for FlutterPointerInput {}

impl FlutterPointerInput {
    /// Inject one pointer event. `phase` is a `ffi::k{Down,Move,Up}` value.
    fn send(&self, phase: std::os::raw::c_int, x: f64, y: f64, timestamp_us: u64) {
        let ev = ffi::FlutterPointerEvent {
            struct_size: std::mem::size_of::<ffi::FlutterPointerEvent>(),
            phase,
            timestamp: timestamp_us as usize,
            x,
            y,
            device: 0,
            signal_kind: ffi::kFlutterPointerSignalKindNone,
            scroll_delta_x: 0.0,
            scroll_delta_y: 0.0,
            device_kind: ffi::kFlutterPointerDeviceKindTouch,
            buttons: 0,
        };
        let rc = unsafe { (self.lib.send_pointer_event)(self.engine, &ev, 1) };
        if rc != ffi::kSuccess {
            tracing::warn!(code = rc, "FlutterEngineSendPointerEvent failed");
        }
    }
}

impl aap_input::PointerSink for FlutterPointerInput {
    fn pointer(&self, phase: aap_input::PointerPhase, x: f64, y: f64, timestamp_us: u64) {
        let p = match phase {
            aap_input::PointerPhase::Down => ffi::kDown,
            aap_input::PointerPhase::Move => ffi::kMove,
            aap_input::PointerPhase::Up => ffi::kUp,
        };
        self.send(p, x, y, timestamp_us);
    }
}

impl Drop for FlutterEngineHandle {
    fn drop(&mut self) {
        // Shut down the engine before freeing _data; the engine must not call
        // our callbacks after its data has been freed.
        let rc = unsafe { (self.lib.shutdown)(self.engine.0) };
        if rc != ffi::kSuccess {
            tracing::warn!(code = rc, "FlutterEngineShutdown returned non-success");
        }
        tracing::info!("Flutter engine shut down");
    }
}
