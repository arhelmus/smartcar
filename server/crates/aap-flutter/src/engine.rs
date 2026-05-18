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

use anyhow::Context as _;

use crate::ffi::{
    self, FlutterEngine, FlutterProjectArgs, FlutterRendererConfig, FlutterWindowMetricsEvent,
};
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
        let args = FlutterProjectArgs::new(assets_cstr.as_ptr(), icu_cstr.as_ptr());

        let mut raw: FlutterEngine = std::ptr::null_mut();
        let rc = unsafe {
            ffi::FlutterEngineRun(
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
                 and that FLUTTER_ENGINE_LIB_DIR points to a matching \
                 libflutter_engine.so."
            );
        }

        // Store the handle so the texture-dirty callbacks can use it.
        data.engine.store(raw, Ordering::Release);

        // Register the video-stream external texture (id = 0).
        let rc = unsafe { ffi::FlutterEngineRegisterExternalTexture(raw, 0) };
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
        let rc = unsafe { ffi::FlutterEngineSendWindowMetricsEvent(self.engine.0, &event) };
        if rc != ffi::kSuccess {
            anyhow::bail!("FlutterEngineSendWindowMetricsEvent failed (code {rc})");
        }
        tracing::info!(width, height, "Flutter window metrics sent");
        Ok(())
    }

    /// Signal to the engine that a new frame is ready for texture id `0`.
    ///
    /// Called from the Tokio task after each decoded video frame is written
    /// into the [`SharedPixelStore`].  Thread-safe; the engine API is
    /// documented to allow calls from any thread.
    pub fn mark_texture_dirty(&self) {
        let rc = unsafe { ffi::FlutterEngineMarkExternalTextureFrameAvailable(self.engine.0, 0) };
        if rc != ffi::kSuccess {
            tracing::warn!(
                code = rc,
                "FlutterEngineMarkExternalTextureFrameAvailable failed"
            );
        }
    }
}

impl Drop for FlutterEngineHandle {
    fn drop(&mut self) {
        // Shut down the engine before freeing _data; the engine must not call
        // our callbacks after its data has been freed.
        let rc = unsafe { ffi::FlutterEngineShutdown(self.engine.0) };
        if rc != ffi::kSuccess {
            tracing::warn!(code = rc, "FlutterEngineShutdown returned non-success");
        }
        tracing::info!("Flutter engine shut down");
    }
}
