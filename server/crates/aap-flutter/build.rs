use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FLUTTER_ENGINE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=FLUTTER_ENGINE_URL");
    println!("cargo:rerun-if-env-changed=FLUTTER_ASSETS_DIR");
    println!("cargo:rerun-if-env-changed=FLUTTER_ICU_DATA");
    println!("cargo:rerun-if-env-changed=FLUTTER_BIN");

    // ── Flutter project path ──────────────────────────────────────────────────
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // crates/aap-flutter → crates/ → server/ → server/flutter-ui
    let flutter_project = Path::new(&manifest)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("flutter-ui");

    // Re-run whenever Dart source or pubspec files change.
    watch_flutter_sources(&flutter_project);

    // ── Build Flutter bundle (unless the caller has pinned FLUTTER_ASSETS_DIR) ─
    let flutter_bin = find_flutter();

    // ── Flutter engine library ────────────────────────────────────────────────
    // Resolution order:
    //   1. FLUTTER_ENGINE_LIB_DIR — explicit override (macOS dev, pinned CI).
    //   2. Auto-download — derive the engine commit SHA from the SDK's
    //      bin/internal/engine.version and pull a matching libflutter_engine.so
    //      for the build target (Linux x64 / arm64).  This keeps the engine and
    //      the `flutter build bundle` output version-locked by construction.
    // A failure here is non-fatal: the link step only needs the lib when the
    // crate is actually compiled (`--features flutter`); otherwise the binary
    // falls back to the testkit producers at runtime.
    resolve_engine_lib(flutter_bin.as_deref());
    if std::env::var("FLUTTER_ASSETS_DIR").is_err() {
        build_flutter_bundle(&flutter_project, flutter_bin.as_deref());
    }

    // ── Baked-in path constants ───────────────────────────────────────────────

    // FLUTTER_ASSETS_DIR — path to the flutter_assets/ directory itself.
    // FlutterProjectArgs.assets_path expects exactly this directory.
    let assets_dir = std::env::var("FLUTTER_ASSETS_DIR").unwrap_or_else(|_| {
        flutter_project
            .join("build/flutter_assets")
            .to_string_lossy()
            .into_owned()
    });
    println!("cargo:rustc-env=FLUTTER_ASSETS_DIR={assets_dir}");

    // FLUTTER_ICU_DATA — path to icudtl.dat, which ships alongside
    // libflutter_engine.so and is also cached in the Flutter SDK.
    let icu_data = find_icu_data(flutter_bin.as_deref());
    println!("cargo:rustc-env=FLUTTER_ICU_DATA={icu_data}");

    // ── Copy bundle next to the binary ────────────────────────────────────────
    // This makes the binary self-contained: it finds flutter_assets/ and
    // icudtl.dat relative to itself at runtime, regardless of where it was
    // built.
    copy_bundle_to_target(
        Path::new(&assets_dir),
        if icu_data.is_empty() {
            None
        } else {
            Some(Path::new(&icu_data))
        },
    );
}

// ── Bundle copy to target dir ─────────────────────────────────────────────────

/// Copy `flutter_assets/` (and optionally `icudtl.dat`) into
/// `target/<profile>/` so they sit next to the compiled binary.
///
/// `target/<profile>/` is derived from `OUT_DIR`:
///   `OUT_DIR` = `target/<profile>/build/<crate>/out`  →  ancestor 3 levels up.
fn copy_bundle_to_target(assets_dir: &Path, icu_data: Option<&Path>) {
    let out_dir = match std::env::var("OUT_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => return,
    };
    // target/<profile>/build/<crate>/out  →  target/<profile>/
    let Some(target_dir) = out_dir.ancestors().nth(3) else {
        println!("cargo:warning=Could not locate target dir from OUT_DIR; skipping bundle copy");
        return;
    };

    // flutter_assets/
    let dst_assets = target_dir.join("flutter_assets");
    if assets_dir.exists() {
        if let Err(e) = copy_dir_all(assets_dir, &dst_assets) {
            println!("cargo:warning=Failed to copy flutter_assets to target: {e}");
        }
    }

    // icudtl.dat
    if let Some(icu) = icu_data {
        if icu.exists() {
            let dst_icu = target_dir.join("icudtl.dat");
            if let Err(e) = std::fs::copy(icu, &dst_icu) {
                println!("cargo:warning=Failed to copy icudtl.dat to target: {e}");
            }
        }
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

// ── Source watching ───────────────────────────────────────────────────────────

fn watch_flutter_sources(flutter_project: &Path) {
    for file in ["pubspec.yaml", "pubspec.lock"] {
        println!(
            "cargo:rerun-if-changed={}",
            flutter_project.join(file).display()
        );
    }

    let lib_dir = flutter_project.join("lib");
    // Watch the directory itself so Cargo re-runs when files are added/removed.
    println!("cargo:rerun-if-changed={}", lib_dir.display());

    // Also watch each file so Cargo re-runs when contents change.
    if let Ok(entries) = std::fs::read_dir(&lib_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "dart") {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

// ── Flutter bundle build ──────────────────────────────────────────────────────

fn build_flutter_bundle(flutter_project: &Path, flutter_bin: Option<&Path>) {
    let Some(flutter) = flutter_bin else {
        println!(
            "cargo:warning=Flutter binary not found — skipping automatic bundle build. \
             Set FLUTTER_BIN or add flutter to PATH, then re-run cargo build."
        );
        return;
    };

    let profile = std::env::var("PROFILE").unwrap_or_default();
    let mut cmd = std::process::Command::new(flutter);
    cmd.current_dir(flutter_project).arg("build").arg("bundle");
    if profile == "release" {
        cmd.arg("--release");
    }

    println!("cargo:warning=Running `flutter build bundle` ({profile}) ...");
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("flutter build bundle failed (exit {s})"),
        Err(e) => panic!("failed to spawn flutter: {e}"),
    }
}

// ── Flutter binary discovery ──────────────────────────────────────────────────

fn find_flutter() -> Option<PathBuf> {
    // 1. Explicit override via env var.
    if let Ok(bin) = std::env::var("FLUTTER_BIN") {
        let p = PathBuf::from(&bin);
        if p.exists() {
            return Some(p);
        }
    }
    // 2. Search PATH.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("flutter");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    // 3. Known Homebrew locations (ARM and Intel Mac).
    for p in [
        "/opt/homebrew/share/flutter/bin/flutter",
        "/usr/local/share/flutter/bin/flutter",
    ] {
        if Path::new(p).exists() {
            return Some(p.into());
        }
    }
    None
}

// ── Engine library resolution ─────────────────────────────────────────────────

/// Resolve `libflutter_engine.so` and emit the link directives.
///
/// Non-fatal on failure: emits a `cargo:warning` and returns.  The link step
/// only consumes the lib when the crate is compiled with `--features flutter`;
/// a feature-less build (CI, plain `cargo build`) never reaches link and the
/// server falls back to the testkit producers at runtime.
fn resolve_engine_lib(flutter_bin: Option<&Path>) {
    // 1. Explicit override — directory containing libflutter_engine.so.
    if let Ok(dir) = std::env::var("FLUTTER_ENGINE_LIB_DIR") {
        emit_engine_link(Path::new(&dir));
        return;
    }

    // 2. Auto-download, version-locked to the SDK's engine commit.
    let Some(sha) = engine_sha(flutter_bin) else {
        println!(
            "cargo:warning=Could not read the Flutter engine commit from \
             <sdk>/bin/internal/engine.version (FLUTTER_BIN / PATH not a \
             Flutter SDK?). Set FLUTTER_ENGINE_LIB_DIR to a directory \
             containing libflutter_engine.so to build with --features flutter."
        );
        return;
    };

    let Some((url, member)) = engine_artifact_url(&sha) else {
        println!(
            "cargo:warning=No prebuilt Flutter engine mapping for this target \
             (engine auto-download supports Linux x64/arm64). Set \
             FLUTTER_ENGINE_LIB_DIR (or FLUTTER_ENGINE_URL) to build \
             --features flutter for this target."
        );
        return;
    };

    match ensure_engine_downloaded(&sha, &url, member) {
        Ok(dir) => emit_engine_link(&dir),
        Err(e) => println!(
            "cargo:warning=Flutter engine download failed ({e}). Set \
             FLUTTER_ENGINE_LIB_DIR to build --features flutter."
        ),
    }
}

/// Emit link-search + link-lib for the directory holding libflutter_engine.so,
/// and bake an rpath so a locally-run binary finds the lib without
/// `LD_LIBRARY_PATH` (deployed builds get `$ORIGIN` via the deploy script).
fn emit_engine_link(dir: &Path) {
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=dylib=flutter_engine");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
}

/// The engine commit SHA, read verbatim from `<sdk>/bin/internal/engine.version`.
/// The Google artifact bucket is keyed on exactly this value.
fn engine_sha(flutter_bin: Option<&Path>) -> Option<String> {
    let flutter = flutter_bin?;
    // <sdk>/bin/flutter → <sdk>
    let sdk = flutter.parent()?.parent()?;
    let version_file = sdk.join("bin/internal/engine.version");
    println!("cargo:rerun-if-changed={}", version_file.display());
    let sha = std::fs::read_to_string(&version_file).ok()?;
    let sha = sha.trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// Map the build target to a `(zip_url, member_to_extract)` pair.
///
/// `FLUTTER_ENGINE_URL` overrides the URL (the archive must still contain
/// `libflutter_engine.so`).  Only Linux x64/arm64 are auto-resolved; macOS
/// uses the framework and must go through `FLUTTER_ENGINE_LIB_DIR`.
fn engine_artifact_url(sha: &str) -> Option<(String, &'static str)> {
    const MEMBER: &str = "libflutter_engine.so";
    if let Ok(url) = std::env::var("FLUTTER_ENGINE_URL") {
        if !url.is_empty() {
            return Some((url, MEMBER));
        }
    }
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let platform = match (os.as_str(), arch.as_str()) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        _ => return None,
    };
    let base = std::env::var("FLUTTER_STORAGE_BASE_URL")
        .unwrap_or_else(|_| "https://storage.googleapis.com".to_string());
    let url =
        format!("{base}/flutter_infra_release/flutter/{sha}/{platform}/{platform}-embedder.zip");
    Some((url, MEMBER))
}

/// Download + unpack the engine into a SHA-stamped cache dir; return that dir.
///
/// Cached at `<target>/flutter-engine/<sha>/`: survives `cargo clean -p` and is
/// re-used across profiles, so the download is one-time per engine bump.
fn ensure_engine_downloaded(sha: &str, url: &str, member: &str) -> Result<PathBuf, String> {
    let cache_dir = engine_cache_dir(sha)?;
    let lib_path = cache_dir.join(member);
    if lib_path.exists() {
        return Ok(cache_dir);
    }
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("mkdir {cache_dir:?}: {e}"))?;

    let zip_path = cache_dir.join("engine.zip");
    println!("cargo:warning=Downloading Flutter engine {sha} → {url}");
    run(std::process::Command::new("curl").args([
        "-fSL",
        "--retry",
        "3",
        "-o",
        &zip_path.to_string_lossy(),
        url,
    ]))?;

    // -j: flatten paths so `member` lands directly in cache_dir.
    run(std::process::Command::new("unzip").args([
        "-o",
        "-j",
        &zip_path.to_string_lossy(),
        member,
        "-d",
        &cache_dir.to_string_lossy(),
    ]))?;
    let _ = std::fs::remove_file(&zip_path);

    if lib_path.exists() {
        Ok(cache_dir)
    } else {
        Err(format!("{member} not found in archive {url}"))
    }
}

/// `<target>/flutter-engine/<sha>/`, derived from `OUT_DIR`
/// (`target/<profile>/build/<crate>/out` → `target/`).
fn engine_cache_dir(sha: &str) -> Result<PathBuf, String> {
    let out_dir = std::env::var("OUT_DIR").map_err(|_| "OUT_DIR unset".to_string())?;
    let out_dir = PathBuf::from(out_dir);
    let target_root = out_dir
        .ancestors()
        .nth(4)
        .ok_or_else(|| format!("cannot derive target dir from OUT_DIR {out_dir:?}"))?;
    Ok(target_root.join("flutter-engine").join(sha))
}

fn run(cmd: &mut std::process::Command) -> Result<(), String> {
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{cmd:?} exited with {s}")),
        Err(e) => Err(format!("failed to spawn {cmd:?}: {e}")),
    }
}

// ── icudtl.dat discovery ──────────────────────────────────────────────────────

fn find_icu_data(flutter_bin: Option<&Path>) -> String {
    // 1. Explicit override.
    if let Ok(p) = std::env::var("FLUTTER_ICU_DATA") {
        if !p.is_empty() {
            return p;
        }
    }
    // 2. Next to libflutter_engine.so (production / CI path).
    if let Ok(lib_dir) = std::env::var("FLUTTER_ENGINE_LIB_DIR") {
        let icu = Path::new(&lib_dir).join("icudtl.dat");
        if icu.exists() {
            return icu.to_string_lossy().into_owned();
        }
    }
    // 3. Flutter SDK artifact cache (development path).
    //    flutter binary lives at <sdk>/bin/flutter; icudtl.dat is at
    //    <sdk>/bin/cache/artifacts/engine/<platform>/icudtl.dat.
    if let Some(flutter) = flutter_bin {
        if let Some(sdk) = flutter.parent().and_then(|p| p.parent()) {
            let platform = if cfg!(target_os = "macos") {
                "darwin-x64"
            } else {
                "linux-x64"
            };
            let icu = sdk
                .join("bin/cache/artifacts/engine")
                .join(platform)
                .join("icudtl.dat");
            if icu.exists() {
                return icu.to_string_lossy().into_owned();
            }
        }
    }
    // 4. Not found — engine will fail at runtime with a clear message.
    String::new()
}
