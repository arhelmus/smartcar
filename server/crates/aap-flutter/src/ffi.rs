//! Raw FFI bindings for the subset of `flutter_embedder.h` used by this crate.
//!
//! Only the structs and entry points actually consumed by the embedder are
//! declared here.  Refer to the canonical header in the Flutter engine SDK for
//! the full surface:
//! <https://github.com/flutter/engine/blob/main/shell/platform/embedder/flutter_embedder.h>

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code
)]

use std::os::raw::{c_char, c_int, c_void};

// ── Opaque types ─────────────────────────────────────────────────────────────

/// Opaque handle returned by [`FlutterEngineRun`].
pub type FlutterEngine = *mut c_void;

/// Return type for Flutter engine API calls.
pub type FlutterEngineResult = c_int;

pub const kSuccess: FlutterEngineResult = 0;

/// Value that must be passed as `version` to [`FlutterEngineRun`].
pub const FLUTTER_ENGINE_VERSION: usize = 1;

// ── Renderer ──────────────────────────────────────────────────────────────────

/// `FlutterRendererType` discriminant for the software renderer.
pub const kSoftware: c_int = 1;

/// Callback invoked by the engine to present a fully-composited software frame.
///
/// `allocation` is a row-major ARGB8888 buffer of `row_bytes × height` bytes,
/// valid only for the duration of the call.  Returns `true` on success.
pub type SoftwareSurfacePresentCallback = unsafe extern "C" fn(
    user_data: *mut c_void,
    allocation: *const c_void,
    row_bytes: usize,
    height: usize,
) -> bool;

/// Mirrors `FlutterSoftwareRendererConfig` from the embedder header.
#[repr(C)]
pub struct FlutterSoftwareRendererConfig {
    /// Must equal `size_of::<FlutterSoftwareRendererConfig>()`.
    pub struct_size: usize,
    pub surface_present_callback: SoftwareSurfacePresentCallback,
}

/// `FlutterRendererConfig` — type discriminant + union (software variant).
///
/// The C struct is `{ FlutterRendererType type; union { ... }; }` with no
/// `struct_size` field.  The union must be at least as large as the largest
/// variant (OpenGL is the biggest at ~300 bytes on 64-bit).  512 bytes is
/// comfortably safe for all current variants.
#[repr(C)]
pub struct FlutterRendererConfig {
    /// `FlutterRendererType` enum value; `kSoftware = 1`.
    pub renderer_type: c_int,
    /// Implicit C alignment padding between the 4-byte enum and 8-byte-aligned union.
    pub _align_pad: [u8; 4],
    /// Raw union storage.  Populate via [`FlutterRendererConfig::software`].
    pub _union: [u8; 512],
}

impl FlutterRendererConfig {
    pub fn software(callback: SoftwareSurfacePresentCallback) -> Self {
        let mut cfg = Self {
            renderer_type: kSoftware,
            _align_pad: [0; 4],
            _union: [0; 512],
        };
        let sw = FlutterSoftwareRendererConfig {
            struct_size: std::mem::size_of::<FlutterSoftwareRendererConfig>(),
            surface_present_callback: callback,
        };
        // Safety: FlutterSoftwareRendererConfig (16 bytes) fits within _union (512 bytes).
        unsafe {
            std::ptr::write(
                cfg._union.as_mut_ptr() as *mut FlutterSoftwareRendererConfig,
                sw,
            );
        }
        cfg
    }
}

// ── Project args ──────────────────────────────────────────────────────────────

/// Minimal `FlutterProjectArgs`.
///
/// `struct_size` tells the engine how many bytes of this struct are valid.
/// Fields beyond `icu_data_path` are implicitly zero (null / disabled) because
/// the engine guards each field with a `struct_size` range check.
#[repr(C)]
pub struct FlutterProjectArgs {
    /// Must equal `size_of::<FlutterProjectArgs>()`.
    pub struct_size: usize,
    /// Path to the compiled Flutter asset bundle directory (`flutter_assets/`).
    pub assets_path: *const c_char,
    /// Path to `icudtl.dat` (ICU data file shipped with the Flutter engine).
    pub icu_data_path: *const c_char,
}

impl FlutterProjectArgs {
    pub fn new(assets_path: *const c_char, icu_data_path: *const c_char) -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>(),
            assets_path,
            icu_data_path,
        }
    }
}

// Safety: FlutterProjectArgs contains raw pointers to C strings whose
// lifetimes are managed by the caller (CString kept alive in EngineHandle).
unsafe impl Send for FlutterProjectArgs {}

// ── External textures ─────────────────────────────────────────────────────────

/// Optional release callback called after the engine has consumed a pixel buffer.
pub type PixelBufferReleaseCallback = unsafe extern "C" fn(user_data: *mut c_void);

/// Pixel-buffer descriptor for software external textures.
///
/// Returned by the texture frame callback; the engine copies `buffer` into its
/// own GPU texture, then calls `release_callback` if set.
#[repr(C)]
pub struct FlutterDesktopPixelBuffer {
    /// RGBA8888 pixel data, row-major, `width × height × 4` bytes.
    pub buffer: *const u8,
    pub width: usize,
    pub height: usize,
    pub release_callback: Option<PixelBufferReleaseCallback>,
    pub release_context: *mut c_void,
}

// ── Engine entry points ───────────────────────────────────────────────────────

unsafe extern "C" {
    /// Initialise and run the Flutter engine.
    ///
    /// `version` must equal [`FLUTTER_ENGINE_VERSION`].
    /// `user_data` is passed through to all embedder callbacks unchanged.
    pub fn FlutterEngineRun(
        version: usize,
        config: *const FlutterRendererConfig,
        args: *const FlutterProjectArgs,
        user_data: *mut c_void,
        engine_out: *mut FlutterEngine,
    ) -> FlutterEngineResult;

    /// Shut down the engine and release all resources.
    pub fn FlutterEngineShutdown(engine: FlutterEngine) -> FlutterEngineResult;

    /// Register an external texture with `texture_id`.
    ///
    /// The engine will call the `texture_frame_callback` (set in
    /// `FlutterProjectArgs`) to populate each frame.
    pub fn FlutterEngineRegisterExternalTexture(
        engine: FlutterEngine,
        texture_id: i64,
    ) -> FlutterEngineResult;

    /// Unregister a previously registered external texture.
    pub fn FlutterEngineUnregisterExternalTexture(
        engine: FlutterEngine,
        texture_id: i64,
    ) -> FlutterEngineResult;

    /// Signal that a new frame is ready for `texture_id`.
    ///
    /// The engine will call the texture frame callback on the next render cycle.
    pub fn FlutterEngineMarkExternalTextureFrameAvailable(
        engine: FlutterEngine,
        texture_id: i64,
    ) -> FlutterEngineResult;

    /// Notify the engine of a display vsync event.
    ///
    /// `baton` is the value delivered by the engine's vsync-request callback.
    pub fn FlutterEngineOnVsync(
        engine: FlutterEngine,
        baton: isize,
        frame_start_time_nanos: u64,
        frame_target_time_nanos: u64,
    ) -> FlutterEngineResult;
}
