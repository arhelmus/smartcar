//! No-op build script.
//!
//! Earlier revisions emitted `rustc-link-arg=-Wl,-rpath,...` so the
//! statically-linked `libflutter_engine.so` resolved at runtime.  We now load
//! the engine via `dlopen` at runtime (see `aap-flutter::lib_loader`), so the
//! binary has no `DT_NEEDED` entry for libflutter and needs no rpath.
fn main() {}
