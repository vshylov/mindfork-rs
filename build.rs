//! Build script: spellcheck dictionaries next to the binary + a Windows `.exe` icon
//! + the build stamp.
//!
//! **Dictionaries.** In portable mode the application reads data from a `data/` subdirectory
//! next to the executable (in dev — `target/<profile>/data/`, see shared/paths.rs),
//! so dictionaries are expected at `data/dictionaries/`. Dictionaries live in the project
//! root's `dictionaries/` (checked into the repo; the release workflow also packs them into
//! `data/dictionaries/`); this script copies them into the output directory so
//! `cargo run` sees spellcheck right away, with no manual copying. Missing dictionaries
//! aren't an error (spellcheck simply turns off).
//!
//! **Icon.** For the Windows target, an icon resource from
//! `assets/mindfork.ico` is embedded into the `.exe` — otherwise Explorer, the taskbar, and Alt+Tab show the
//! default icon. Shortcuts and the installer's `UninstallDisplayIcon`
//! (`packaging/windows/mindfork.iss`) pick it up from here for free too. See docs/branding.md §4.1.
//!
//! **Build stamp.** The moment of the build goes in as `MINDFORK_BUILD_EPOCH`
//! (Unix seconds, or `SOURCE_DATE_EPOCH` when set) and surfaces as the build-date
//! row of the "About" tab — in release builds only, because this script does not
//! re-run for a `src/` change. See `embed_build_stamp` and
//! `shared/credits.rs::build_date` (spec §11.7).
//!
//! **Syntax dump.** The vendored grammars in `syntaxes/` (see its `SOURCES.md`)
//! are added to syntect's bundled set and written into `OUT_DIR` as one
//! uncompressed dump, which `shared/markdown/code.rs` embeds. Assembling the
//! set costs ~130 ms; loading the dump costs ~0.55 ms, so it belongs here and
//! not in a `LazyLock` — and a grammar syntect cannot load fails the **build**
//! instead of silently disappearing at runtime (docs/history/vendored-syntaxes.md §2.3).

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    // Re-copy only when the source dictionaries change.
    println!("cargo:rerun-if-changed=dictionaries");

    embed_build_stamp();
    embed_windows_icon();
    build_syntax_dump();

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("dictionaries");
    if !src.is_dir() {
        return; // no dictionaries — nothing to copy
    }

    let Some(profile_dir) = profile_dir() else {
        println!("cargo:warning=could not determine the build directory for dictionaries");
        return;
    };
    // The portable data root = `<profile>/data/` (see shared/paths.rs).
    let dst = profile_dir.join("data").join("dictionaries");
    if let Err(err) = copy_dir(&src, &dst) {
        println!("cargo:warning=could not copy dictionaries: {err}");
    }
}

/// Compiles the moment of the build in as `MINDFORK_BUILD_EPOCH` (Unix seconds),
/// which `shared/credits.rs` turns into the build-date row of the "About" tab.
///
/// **Seconds, not a formatted date**: `chrono` is already a runtime dependency
/// and knows how to render a timestamp, so a build dependency (and a second
/// date implementation) would buy nothing. Formatting happens where the value
/// is displayed.
///
/// `SOURCE_DATE_EPOCH` wins when it is set — the cross-distribution convention
/// for reproducible builds (Debian, Nix, openSUSE): a package rebuilt from the
/// same source must produce the same bytes, and a wall clock in the binary is
/// exactly what breaks that. `rerun-if-env-changed` makes cargo notice when the
/// variable appears or changes.
///
/// **The value is only as fresh as the last run of this script**, and this
/// script declares `rerun-if-changed` paths, so cargo will not re-run it when
/// `src/` changes — a development binary would carry the date of whenever
/// `dictionaries/`, `assets/` or `syntaxes/` last moved. That is why the row
/// is shown for release builds alone (`credits::build_date`); forcing a re-run
/// on every build would rebuild the syntax dump each time and buy a row nobody
/// reads in a debug build.
fn embed_build_stamp() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let secs = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        });
    println!("cargo:rustc-env=MINDFORK_BUILD_EPOCH={secs}");
}

/// Embeds the icon into the Windows `.exe` (the `IDI_ICON1` resource) — the Windows-host variant.
///
/// A double gate is unavoidable: `winresource` is declared under
/// `[target.'cfg(windows)'.build-dependencies]`, and for **build** dependencies `cfg`
/// is evaluated by the **host** (the build script runs on it) — meaning on a Linux host
/// the crate is absent and referencing it won't compile. Hence `#[cfg(windows)]` by host
/// (is the crate present) plus a `CARGO_CFG_TARGET_OS` check by target (is the icon needed).
///
/// A failure deliberately **doesn't fail the build**, going into `cargo:warning` instead: `winresource` on
/// the MSVC target calls `rc.exe` from the Windows SDK, and on a machine without the SDK the app should
/// still build — the icon is cosmetic.
#[cfg(windows)]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=assets/mindfork.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return; // a Windows host, but a different target — the resource doesn't apply
    }
    let icon = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/mindfork.ico");
    if !icon.is_file() {
        println!(
            "cargo:warning=icon not found, .exe will ship without it: {}",
            icon.display()
        );
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_string_lossy().as_ref());
    if let Err(err) = res.compile() {
        println!("cargo:warning=could not embed the icon into .exe: {err}");
    }
}

/// The non-Windows-host variant: `winresource` is unavailable (see above).
///
/// The release workflow builds Windows on a windows runner, so the regular path isn't
/// affected. We only warn on a Linux → Windows cross-build, so as not to silently
/// ship a `.exe` with no icon.
#[cfg(not(windows))]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=assets/mindfork.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!(
            "cargo:warning=cross-compiling for Windows from a non-Windows host: \
             icon not embedded in .exe (winresource is only available on a Windows host)"
        );
    }
}

/// Assembles syntect's bundled syntaxes plus everything in `syntaxes/` into one
/// uncompressed dump at `$OUT_DIR/syntaxes.packdump`.
///
/// **A grammar that fails to load fails the build**, deliberately: syntect
/// reads only `.sublime-syntax` and does not support `extends:`, so an upstream
/// that migrates to sublime-syntax v2 would otherwise drop a language silently
/// (docs/history/vendored-syntaxes.md §2.1). The `true` in `load_from_str` is
/// `lines_include_newline`, matching `load_defaults_newlines` — the renderer
/// feeds lines with their trailing `\n` (`LinesWithEndings`).
fn build_syntax_dump() {
    println!("cargo:rerun-if-changed=syntaxes");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = manifest.join("syntaxes");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("syntaxes.packdump");

    let mut builder = syntect::parsing::SyntaxSet::load_defaults_newlines().into_builder();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "sublime-syntax"))
        .collect();
    // Sorted so the dump is reproducible: `read_dir` order is filesystem-defined.
    files.sort();
    assert!(
        !files.is_empty(),
        "no .sublime-syntax files in {} — run `python tools/fetch_syntaxes.py`",
        dir.display()
    );
    for path in &files {
        let src = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let def = syntect::parsing::SyntaxDefinition::load_from_str(&src, true, None)
            .unwrap_or_else(|e| panic!("{} does not load: {e}", path.display()));
        builder.add(def);
    }
    syntect::dumps::dump_to_uncompressed_file(&builder.build(), &out)
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", out.display()));
}

/// The directory with the binary (`target/<profile>/`), derived from `OUT_DIR`:
/// `…/target/<profile>/build/<crate>-<hash>/out` → 3 levels up.
fn profile_dir() -> Option<PathBuf> {
    let out = PathBuf::from(env::var("OUT_DIR").ok()?);
    out.ancestors().nth(3).map(Path::to_path_buf)
}

/// Copies regular files from `src` into `dst` (no recursion, no hidden files).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        // skip hidden entries (e.g. .gitkeep) and subdirectories
        if name.to_string_lossy().starts_with('.') || !path.is_file() {
            continue;
        }
        fs::copy(&path, dst.join(&name))?;
    }
    Ok(())
}
