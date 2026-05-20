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

/// Subset of the embedder's `FlutterProjectArgs` that we set.
///
/// `struct_size` tells the engine how many bytes of this struct are valid;
/// fields not present here are skipped by the engine's range check.
///
/// We declare the four AOT snapshot pointer fields because the engine's
/// "infer VM snapshot from settings" path fails when libflutter is loaded
/// via `dlopen` on macOS — passing the symbol addresses explicitly (looked
/// up from the engine library's own symbol table; see
/// [`FlutterLib`](crate::lib_loader::FlutterLib)) bypasses inference.
/// `command_line_argc/argv` and `platform_message_callback` sit between
/// `icu_data_path` and `vm_snapshot_data` in the real `FlutterProjectArgs`,
/// so we keep their slots (filled with zero/null) to land the snapshot
/// fields at the correct offset.
#[repr(C)]
pub struct FlutterProjectArgs {
    /// Must equal `size_of::<FlutterProjectArgs>()` so the embedder reads
    /// all the way through `isolate_snapshot_instructions_size`.
    pub struct_size: usize,
    /// Path to the compiled Flutter asset bundle directory (`flutter_assets/`).
    pub assets_path: *const c_char,
    /// Path to `icudtl.dat` (ICU data file shipped with the Flutter engine).
    pub icu_data_path: *const c_char,
    /// We don't pass extra args.
    pub command_line_argc: c_int,
    pub command_line_argv: *const *const c_char,
    /// We don't handle platform messages.
    pub platform_message_callback: *mut c_void,
    /// AOT VM snapshot data symbol pointer; NULL → engine falls back to
    /// settings inference (which is what fails under `dlopen` on macOS).
    pub vm_snapshot_data: *const u8,
    /// 0 is permitted when `vm_snapshot_data` is a symbol reference (per the
    /// embedder header).
    pub vm_snapshot_data_size: usize,
    pub vm_snapshot_instructions: *const u8,
    pub vm_snapshot_instructions_size: usize,
    pub isolate_snapshot_data: *const u8,
    pub isolate_snapshot_data_size: usize,
    pub isolate_snapshot_instructions: *const u8,
    pub isolate_snapshot_instructions_size: usize,
}

impl FlutterProjectArgs {
    pub fn new(assets_path: *const c_char, icu_data_path: *const c_char) -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>(),
            assets_path,
            icu_data_path,
            command_line_argc: 0,
            command_line_argv: std::ptr::null(),
            platform_message_callback: std::ptr::null_mut(),
            vm_snapshot_data: std::ptr::null(),
            vm_snapshot_data_size: 0,
            vm_snapshot_instructions: std::ptr::null(),
            vm_snapshot_instructions_size: 0,
            isolate_snapshot_data: std::ptr::null(),
            isolate_snapshot_data_size: 0,
            isolate_snapshot_instructions: std::ptr::null(),
            isolate_snapshot_instructions_size: 0,
        }
    }

    /// Fill the AOT snapshot fields from a [`FlutterLib`].
    ///
    /// `size = 0` is explicitly permitted when the pointer is a symbol
    /// reference (per the embedder header documentation on each size field).
    pub fn with_aot_snapshots(mut self, lib: &crate::lib_loader::FlutterLib) -> Self {
        if let Some(p) = lib.vm_snapshot_data {
            self.vm_snapshot_data = p;
        }
        if let Some(p) = lib.vm_snapshot_instructions {
            self.vm_snapshot_instructions = p;
        }
        if let Some(p) = lib.isolate_snapshot_data {
            self.isolate_snapshot_data = p;
        }
        if let Some(p) = lib.isolate_snapshot_instructions {
            self.isolate_snapshot_instructions = p;
        }
        self
    }
}

// Safety: FlutterProjectArgs contains raw pointers to C strings whose
// lifetimes are managed by the caller (CString kept alive in EngineHandle).
unsafe impl Send for FlutterProjectArgs {}

// ── Window metrics ────────────────────────────────────────────────────────────

/// Mirrors the leading fields of `FlutterWindowMetricsEvent`.
///
/// The engine guards every field past `struct_size` with a range check, so
/// declaring only the first four (the ones we set) is sufficient: width,
/// height and `pixel_ratio` fully define the render surface; later fields
/// (insets, display id, …) default to zero/disabled.
///
/// Without one of these events the engine has a 0×0 surface and never
/// composites a frame — the present callback is never called.
#[repr(C)]
pub struct FlutterWindowMetricsEvent {
    /// Must equal `size_of::<FlutterWindowMetricsEvent>()`.
    pub struct_size: usize,
    /// Physical width of the render surface, in pixels.
    pub width: usize,
    /// Physical height of the render surface, in pixels.
    pub height: usize,
    /// Device pixel ratio (logical → physical). Use `1.0` for projection.
    pub pixel_ratio: f64,
}

impl FlutterWindowMetricsEvent {
    pub fn new(width: usize, height: usize, pixel_ratio: f64) -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>(),
            width,
            height,
            pixel_ratio,
        }
    }
}

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

// ── Pointer events ────────────────────────────────────────────────────────────

/// `FlutterPointerPhase` discriminants (declaration order from the header).
pub const kCancel: c_int = 0;
pub const kUp: c_int = 1;
pub const kDown: c_int = 2;
pub const kMove: c_int = 3;
pub const kAdd: c_int = 4;
pub const kRemove: c_int = 5;
pub const kHover: c_int = 6;

/// `FlutterPointerSignalKind` — only `None` is used (no scroll/zoom).
pub const kFlutterPointerSignalKindNone: c_int = 0;

/// `FlutterPointerDeviceKind` discriminants.
pub const kFlutterPointerDeviceKindMouse: c_int = 1;
pub const kFlutterPointerDeviceKindTouch: c_int = 2;

/// Mirrors `FlutterPointerEvent` up to `buttons` (every field past
/// `struct_size` is range-checked by the engine; the trailing
/// `view_id`/`pan`/`scale`/`rotation` fields default to zero/disabled).
#[repr(C)]
pub struct FlutterPointerEvent {
    /// Must equal `size_of::<FlutterPointerEvent>()`.
    pub struct_size: usize,
    /// `FlutterPointerPhase` (`kDown` / `kMove` / `kUp` / …).
    pub phase: c_int,
    /// Event time in microseconds (monotonic; engine clock-agnostic).
    pub timestamp: usize,
    /// X coordinate in physical pixels.
    pub x: f64,
    /// Y coordinate in physical pixels.
    pub y: f64,
    /// Device id (distinguishes simultaneous pointers).
    pub device: i32,
    /// `FlutterPointerSignalKind`.
    pub signal_kind: c_int,
    pub scroll_delta_x: f64,
    pub scroll_delta_y: f64,
    /// `FlutterPointerDeviceKind` — `kFlutterPointerDeviceKindTouch`.
    pub device_kind: c_int,
    /// Pressed mouse buttons; 0 for touch.
    pub buttons: i64,
}

// Engine entry points are not declared here — they live as typed function-
// pointer fields on `FlutterLib` (see `lib_loader.rs`) and are resolved via
// `dlopen` at runtime so the 96 MB libflutter_engine.so is not in the
// binary's DT_NEEDED list.  See that module for the argument signatures.
