//! Runtime [`libloading`] wrapper around `libflutter_engine.so`.
//!
//! Loading the engine via `dlopen` (instead of as a `DT_NEEDED` of the
//! executable) keeps the 96 MB `mmap` and the C++ static initialisers out of
//! the pre-`main()` phase.  On the board's USB-car-mode boot that's the
//! difference between the host seeing our gadget within milliseconds and the
//! host cutting Vbus before our binary even reaches `main()`.
//!
//! Every libflutter symbol the rest of the crate uses lives as a typed
//! function-pointer field on [`FlutterLib`].  Wrong argument types caught at
//! compile time the same as `extern "C"`; missing symbols caught at
//! [`FlutterLib::load`] time with a specific message naming the symbol.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_LAZY};

use crate::ffi::{
    FlutterEngine, FlutterEngineResult, FlutterPointerEvent, FlutterProjectArgs,
    FlutterRendererConfig, FlutterWindowMetricsEvent,
};

/// All libflutter entry points used by this crate, loaded via `dlopen`.
///
/// The [`Library`] is held privately and never accessed directly after
/// construction — the typed function-pointer fields below are valid for as
/// long as `FlutterLib` is alive, which is enforced by the field order
/// (`_lib` is the last field, dropped last).
pub struct FlutterLib {
    pub run: unsafe extern "C" fn(
        version: usize,
        config: *const FlutterRendererConfig,
        args: *const FlutterProjectArgs,
        user_data: *mut std::os::raw::c_void,
        engine_out: *mut FlutterEngine,
    ) -> FlutterEngineResult,
    pub shutdown: unsafe extern "C" fn(engine: FlutterEngine) -> FlutterEngineResult,
    pub send_pointer_event: unsafe extern "C" fn(
        engine: FlutterEngine,
        events: *const FlutterPointerEvent,
        events_count: usize,
    ) -> FlutterEngineResult,
    pub send_window_metrics_event: unsafe extern "C" fn(
        engine: FlutterEngine,
        event: *const FlutterWindowMetricsEvent,
    ) -> FlutterEngineResult,
    pub register_external_texture:
        unsafe extern "C" fn(engine: FlutterEngine, texture_id: i64) -> FlutterEngineResult,
    pub unregister_external_texture:
        unsafe extern "C" fn(engine: FlutterEngine, texture_id: i64) -> FlutterEngineResult,
    pub mark_external_texture_frame_available:
        unsafe extern "C" fn(engine: FlutterEngine, texture_id: i64) -> FlutterEngineResult,
    pub on_vsync: unsafe extern "C" fn(
        engine: FlutterEngine,
        baton: isize,
        frame_start_time_nanos: u64,
        frame_target_time_nanos: u64,
    ) -> FlutterEngineResult,

    /// AOT VM snapshot data (the `_kDartVmSnapshotData` symbol inside the
    /// engine library).  When linked statically the engine infers this from
    /// its own symbol table; under `dlopen` the inference path on macOS
    /// fails ("VM snapshot invalid and could not be inferred"), so we look
    /// the symbol up ourselves and pass it through `FlutterProjectArgs`.
    /// `None` on a pure-JIT engine that doesn't ship these symbols.
    pub vm_snapshot_data: Option<*const u8>,
    pub vm_snapshot_instructions: Option<*const u8>,
    pub isolate_snapshot_data: Option<*const u8>,
    pub isolate_snapshot_instructions: Option<*const u8>,

    /// Keeps the `dlopen`'d library alive as long as any fn pointer is in use.
    /// Last field → dropped last; the `dlclose` happens only after every
    /// other field of [`FlutterLib`] has been dropped.
    _lib: Library,
}

// Safety: the snapshot data pointers above point at read-only data inside the
// engine library, which `_lib` keeps mapped for the lifetime of `FlutterLib`.
// They are immutable and the engine API is thread-safe.
unsafe impl Send for FlutterLib {}
unsafe impl Sync for FlutterLib {}

impl FlutterLib {
    /// `dlopen` libflutter_engine.so and resolve every symbol we use.
    ///
    /// This is the slow step (the 96 MB `mmap`, the C++ static initialisers,
    /// any disk I/O the engine does on load).  Call it **after** the USB
    /// gadget is on the bus so the host's empty-port timeout has already
    /// stopped.
    ///
    /// # Safety
    ///
    /// `path` must point at a real Flutter engine library matching the
    /// `engine.version` pinned in this crate.  Loading it runs the engine's
    /// C++ static initialisers in this process and resolves a set of
    /// `unsafe extern "C"` function pointers whose argument types must match
    /// the engine ABI for that revision — neither is checked at compile
    /// time. Passing an arbitrary `.so`/`.dylib` is undefined behaviour.
    pub unsafe fn load(path: &Path) -> anyhow::Result<Arc<Self>> {
        // RTLD_GLOBAL exposes the engine's symbols (including Dart VM
        // snapshot data symbols) to `dlsym(RTLD_DEFAULT, ...)` — Dart's
        // internal "find my own snapshot" path uses that lookup, so the
        // default RTLD_LOCAL leaves it returning "VM snapshot invalid".
        // RTLD_LAZY matches what `-framework` linking effectively gives us.
        let lib = unsafe { Library::open(Some(path), RTLD_LAZY | RTLD_GLOBAL) }
            .with_context(|| format!("dlopen {}", path.display()))?;

        // Each lookup wrapped so a missing symbol surfaces with its name.
        // `*lib.get(...)` deref → the typed fn pointer the engine exports.
        macro_rules! sym {
            ($name:literal) => {{
                let bytes: &[u8] = concat!($name, "\0").as_bytes();
                let raw = unsafe { lib.get(bytes) }
                    .with_context(|| format!("dlsym {} in {}", $name, path.display()))?;
                *raw
            }};
        }

        // AOT snapshot data — look up the four `_kDart*` data symbols that
        // the engine binary embeds. `Library::get` returns a typed pointer;
        // we deref the `Symbol` to the raw `*const u8`. Missing → JIT-only
        // engine; `None` and the engine will use settings inference.
        macro_rules! data_sym {
            ($name:literal) => {{
                let bytes: &[u8] = concat!($name, "\0").as_bytes();
                unsafe { lib.get::<*const u8>(bytes) }
                    .ok()
                    .map(|s| *s as *const u8)
            }};
        }
        let vm_snapshot_data = data_sym!("kDartVmSnapshotData");
        let vm_snapshot_instructions = data_sym!("kDartVmSnapshotInstructions");
        let isolate_snapshot_data = data_sym!("kDartIsolateSnapshotData");
        let isolate_snapshot_instructions = data_sym!("kDartIsolateSnapshotInstructions");

        let me = FlutterLib {
            run: sym!("FlutterEngineRun"),
            shutdown: sym!("FlutterEngineShutdown"),
            send_pointer_event: sym!("FlutterEngineSendPointerEvent"),
            send_window_metrics_event: sym!("FlutterEngineSendWindowMetricsEvent"),
            register_external_texture: sym!("FlutterEngineRegisterExternalTexture"),
            unregister_external_texture: sym!("FlutterEngineUnregisterExternalTexture"),
            mark_external_texture_frame_available: sym!(
                "FlutterEngineMarkExternalTextureFrameAvailable"
            ),
            on_vsync: sym!("FlutterEngineOnVsync"),
            vm_snapshot_data,
            vm_snapshot_instructions,
            isolate_snapshot_data,
            isolate_snapshot_instructions,
            _lib: lib,
        };
        tracing::info!(
            path = %path.display(),
            aot = me.vm_snapshot_data.is_some(),
            "libflutter_engine.so loaded"
        );
        Ok(Arc::new(me))
    }
}
