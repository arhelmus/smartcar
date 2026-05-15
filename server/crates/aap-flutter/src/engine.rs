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
use std::path::Path;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::os::raw::c_void;

use anyhow::Context as _;

use crate::ffi::{self, FlutterEngine, FlutterRendererConfig, FlutterProjectArgs};
use crate::texture::SharedPixelStore;

// ── Callback data ─────────────────────────────────────────────────────────────

/// Heap-allocated data passed as `user_data` to every embedder callback.
///
/// The pointer is set by `FlutterEngineHandle::launch` immediately after
/// `FlutterEngineRun` returns and before any external-texture signals are sent.
pub(crate) struct EngineCallbackData {
    /// Pixel store read by the texture frame callback (P1: wire up OpenGL texture).
    #[allow(dead_code)]
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
/// a frame.  In P0 the composited ARGB buffer is discarded.  A future step
/// will write it to `/dev/fb0` or a KMS primary plane for actual display.
unsafe extern "C" fn surface_present_callback(
    _user_data: *mut c_void,
    _allocation: *const c_void,
    row_bytes: usize,
    height: usize,
) -> bool {
    tracing::trace!(row_bytes, height, "flutter: software frame composed (discarded in P0)");
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
    /// `assets_dir` must point to the compiled Flutter asset bundle
    /// (`flutter_assets/` parent produced by `flutter build bundle`).
    /// `icu_data` must point to `icudtl.dat` (bundled with
    /// `libflutter_engine.so`).
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
        let icu_cstr =
            CString::new(icu_data.to_str().context("icu_data path is not valid UTF-8")?)?;

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

    /// Signal to the engine that a new frame is ready for texture id `0`.
    ///
    /// Called from the Tokio task after each decoded video frame is written
    /// into the [`SharedPixelStore`].  Thread-safe; the engine API is
    /// documented to allow calls from any thread.
    pub fn mark_texture_dirty(&self) {
        let rc = unsafe {
            ffi::FlutterEngineMarkExternalTextureFrameAvailable(self.engine.0, 0)
        };
        if rc != ffi::kSuccess {
            tracing::warn!(code = rc, "FlutterEngineMarkExternalTextureFrameAvailable failed");
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
