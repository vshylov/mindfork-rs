//! Build script: spellcheck dictionaries next to the binary + a Windows `.exe` icon
//! + the build stamp.
//!
//! **Dictionaries.** In portable mode the application reads data from a `data/` subdirectory
//! next to the executable (in dev — `target/<profile>/data/`, see shared/paths.rs),
//! so dictionaries are expected at `data/dictionaries/`. Dictionaries live in the project
//! root's `dictionaries/` (checked into the repo; the release workflow also packs them into
//! `data/dictionaries/`); this script copies them into the output directory so
//! `cargo run` sees spellcheck right away, with no manual copying. Missing dictionaries
//! aren't an error (spellcheck simply turns off). The **destination** is an input of this
//! script as well as its output (`copy_dictionaries`) — without that, a `data/dictionaries`
//! deleted by hand never comes back.
//!
//! **Icon and version info.** For the Windows target, an icon resource from
//! `assets/mindfork.ico` is embedded into the `.exe` — otherwise Explorer, the taskbar, and Alt+Tab show the
//! default icon. Shortcuts and the installer's `UninstallDisplayIcon`
//! (`packaging/windows/mindfork.iss`) pick it up from here for free too. See docs/branding.md §4.1.
//! The same resource carries the VERSIONINFO strings — product name, description,
//! company, copyright — which Windows shows in the UAC dialog, Task Manager and
//! Explorer, and which code signing pins (docs/research/code-signing.md §6.2).
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

/// The name the product presents under, in the `.exe`'s VERSIONINFO block.
///
/// The brand, not the package id: `AppName` in the installer, the command, the
/// shortcut and every user-facing surface say `mindfork`, while `mindfork-rs`
/// stays the name of files and directories (docs/research/binary-rename.md).
/// `winresource` would otherwise take `package.name` and stamp the wrong one.
///
/// Gated by host, like everything else the resource needs: on a Linux host the
/// crate that would read it is not even a dependency (see [`embed_windows_icon`]).
#[cfg(windows)]
const PRODUCT_NAME: &str = "mindfork";
/// `CompanyName` — the same string as the installer's `AppPublisher`.
#[cfg(windows)]
const PUBLISHER: &str = "Vladimir Shylov";

fn main() {
    embed_build_stamp();
    embed_windows_icon();
    build_syntax_dump();
    copy_dictionaries();
}

/// Copies the repository's `dictionaries/` into the portable data root next to
/// the binary (`target/<profile>/data/dictionaries/`, see shared/paths.rs).
///
/// **Both `rerun-if-changed` sides are load-bearing.** The source, so that an
/// edited dictionary reaches the build. The destination, because cargo re-runs a
/// build script only for the paths it declares — and a declared path that does
/// not exist counts as changed. Without the destination among them, a
/// `data/dictionaries` that is *deleted* never comes back: wiping `data/` is the
/// ordinary way to put the app back to a fresh install, nothing under `src/`
/// re-runs this script, and so `cargo run` / `cargo run -r` keep launching a
/// build with spellcheck silently off until `dictionaries/`, `assets/` or
/// `syntaxes/` happen to move for their own reasons. Measured on 1.96.0: after
/// `rm -rf target/release/data` a no-op `cargo check --release` did not re-create
/// it, and on a probe crate with the same declaration shape neither did a build
/// after a `src/` edit or a `Cargo.toml` touch (docs/journal/release.md).
///
/// The destination is declared only once there is something to copy: a declared
/// path that never gets created would re-run this script on **every** build.
fn copy_dictionaries() {
    println!("cargo:rerun-if-changed=dictionaries");

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
    println!("cargo:rerun-if-changed={}", dst.display());

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
    // The rest of the VERSIONINFO block. `winresource` fills it from Cargo
    // metadata unless told otherwise, which spells the product `mindfork-rs`
    // (`package.name`) and leaves company, copyright and original file name
    // empty — measured on the built artifact, not assumed. Two consumers make
    // that more than cosmetic: Windows shows `FileDescription` as the program
    // name in the UAC dialog and Task Manager, and code signing pins these
    // strings through a file metadata restriction, so they must agree with the
    // installer's `VersionInfo*` (docs/research/code-signing.md §6.2).
    // `FileVersion`/`ProductVersion` are deliberately left to Cargo: the
    // manifest is already the single source of truth for the version.
    let copyright = copyright_from_license();
    for (key, value) in [
        ("ProductName", PRODUCT_NAME),
        ("FileDescription", PRODUCT_NAME),
        ("CompanyName", PUBLISHER),
        ("LegalCopyright", copyright.as_str()),
        ("OriginalFilename", "mindfork.exe"),
    ] {
        res.set(key, value);
    }
    if let Err(err) = res.compile() {
        println!("cargo:warning=could not embed the icon into .exe: {err}");
    }
}

/// The copyright line, read out of `LICENSE` rather than written down twice.
///
/// The year is the part that drifts, and a stale one in a signed binary is the
/// kind of detail nobody notices until it is in front of a reviewer. The
/// installer's `VersionInfoCopyright` cannot read a file, so it keeps its own
/// copy — pinned to this same line by `credits::tests` instead.
#[cfg(windows)]
fn copyright_from_license() -> String {
    println!("cargo:rerun-if-changed=LICENSE");
    let path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("LICENSE");
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| {
            text.lines()
                .map(str::trim)
                .find(|line| line.starts_with("Copyright (c)"))
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            println!("cargo:warning=no copyright line in LICENSE; .exe will ship without one");
            String::new()
        })
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
///
/// **The destination's timestamps are kept still**, because `dst` is a
/// `rerun-if-changed` input of this script (see [`copy_dictionaries`]): a file
/// already there is left alone ([`is_up_to_date`]), and one that is copied is
/// given its source's modification time. A copy that stamped "now" on every run
/// would leave the destination looking newer than the fingerprint, so cargo
/// would re-run this script on every build — and each re-run re-stamps
/// `MINDFORK_BUILD_EPOCH` ([`embed_build_stamp`]), which relinks the whole
/// binary (in release: LTO, one codegen unit). Recreating the directory still
/// costs one such extra run, which is the case where the work is wanted anyway.
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
        let target = dst.join(&name);
        if is_up_to_date(&path, &target) {
            continue;
        }
        fs::copy(&path, &target)?;
        stamp_mtime(&path, &target);
    }
    Ok(())
}

/// Is `dst` already the copy of `src` this script would make? Same length and
/// not older — the exact pair [`copy_dir`] leaves behind. Enough to notice an
/// edited dictionary: a rewrite that happens to preserve the byte count still
/// moves the modification time.
fn is_up_to_date(src: &Path, dst: &Path) -> bool {
    let (Ok(src), Ok(dst)) = (src.metadata(), dst.metadata()) else {
        return false;
    };
    src.len() == dst.len()
        && match (src.modified(), dst.modified()) {
            (Ok(src), Ok(dst)) => dst >= src,
            _ => false,
        }
}

/// Gives `dst` the modification time of `src` (see [`copy_dir`] for why).
///
/// Best effort: a filesystem that refuses the update costs an extra build-script
/// run, not a broken build, so a failure is deliberately ignored rather than
/// turned into a `cargo:warning` nobody can act on.
fn stamp_mtime(src: &Path, dst: &Path) {
    let Ok(mtime) = src.metadata().and_then(|m| m.modified()) else {
        return;
    };
    if let Ok(file) = fs::File::options().write(true).open(dst) {
        let _ = file.set_modified(mtime);
    }
}
