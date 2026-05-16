use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FLUTTER_ENGINE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=FLUTTER_ASSETS_DIR");
    println!("cargo:rerun-if-env-changed=FLUTTER_ICU_DATA");
    println!("cargo:rerun-if-env-changed=FLUTTER_BIN");

    // ── Flutter engine library ────────────────────────────────────────────────
    match std::env::var("FLUTTER_ENGINE_LIB_DIR") {
        Ok(dir) => {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=dylib=flutter_engine");
        }
        Err(_) => {
            println!(
                "cargo:warning=FLUTTER_ENGINE_LIB_DIR is not set. \
                 Building with --features flutter will fail at link time. \
                 Set it to the directory containing libflutter_engine.so:\n  \
                 https://storage.googleapis.com/flutter_infra_release/releases/\
                 stable/linux/flutter_linux_<VERSION>-stable.tar.xz"
            );
        }
    }

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
}

// ── Source watching ───────────────────────────────────────────────────────────

fn watch_flutter_sources(flutter_project: &Path) {
    for file in ["pubspec.yaml", "pubspec.lock"] {
        println!("cargo:rerun-if-changed={}", flutter_project.join(file).display());
    }

    let lib_dir = flutter_project.join("lib");
    // Watch the directory itself so Cargo re-runs when files are added/removed.
    println!("cargo:rerun-if-changed={}", lib_dir.display());

    // Also watch each file so Cargo re-runs when contents change.
    if let Ok(entries) = std::fs::read_dir(&lib_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "dart") {
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
