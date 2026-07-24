//! Build script: spellcheck dictionaries next to the binary + a Windows `.exe` icon.
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
//! `artwork/mindfork.ico` is embedded into the `.exe` — otherwise Explorer, the taskbar, and Alt+Tab show the
//! default icon. Shortcuts and the installer's `UninstallDisplayIcon`
//! (`packaging/windows/mindfork.iss`) pick it up from here for free too. See docs/branding.md §4.1.

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    // Re-copy only when the source dictionaries change.
    println!("cargo:rerun-if-changed=dictionaries");

    embed_windows_icon();

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("dictionaries");
    if !src.is_dir() {
        return; // no dictionaries — nothing to copy
    }

    let Some(profile_dir) = profile_dir() else {
        println!("cargo:warning=не удалось определить каталог сборки для словарей");
        return;
    };
    // The portable data root = `<profile>/data/` (see shared/paths.rs).
    let dst = profile_dir.join("data").join("dictionaries");
    if let Err(err) = copy_dir(&src, &dst) {
        println!("cargo:warning=не удалось скопировать словари: {err}");
    }
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
    println!("cargo:rerun-if-changed=artwork/mindfork.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return; // a Windows host, but a different target — the resource doesn't apply
    }
    let icon = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("artwork/mindfork.ico");
    if !icon.is_file() {
        println!(
            "cargo:warning=иконка не найдена, .exe будет без неё: {}",
            icon.display()
        );
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_string_lossy().as_ref());
    if let Err(err) = res.compile() {
        println!("cargo:warning=не удалось вшить иконку в .exe: {err}");
    }
}

/// The non-Windows-host variant: `winresource` is unavailable (see above).
///
/// The release workflow builds Windows on a windows runner, so the regular path isn't
/// affected. We only warn on a Linux → Windows cross-build, so as not to silently
/// ship a `.exe` with no icon.
#[cfg(not(windows))]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=artwork/mindfork.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!(
            "cargo:warning=кросс-сборка под Windows с не-Windows хоста: \
             иконка в .exe не вшита (winresource доступен только на Windows-хосте)"
        );
    }
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
