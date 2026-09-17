//! What a file or code tool may not reach, whatever root or project it was given
//! (docs/research/safe-defaults.md D2, N1): the app's own directories, and the
//! far side of a symbolic link whose target is missing.
//!
//! Shared by `fs_*` and the code workspace because both resolve a model-chosen
//! path the same way — canonical where the path exists, a canonical ancestor plus
//! the missing tail where it does not — and both would otherwise have to keep the
//! same two refusals in step.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::ToolContext;

/// Windows canonicalization yields `\\?\C:\…`. One resolver keeps that form and
/// the other strips it, so every comparison here strips both sides first.
pub(super) fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

/// The app's own directories in the form a resolved path is compared with:
/// canonical and without the verbatim prefix. One that cannot be canonicalized
/// does not exist, and so holds nothing to protect.
pub(super) fn app_dirs(ctx: &ToolContext) -> Vec<PathBuf> {
    ctx.storage
        .json()
        .app_dirs()
        .iter()
        .filter_map(|d| d.canonicalize().ok())
        .map(|d| strip_verbatim(&d))
        .collect()
}

/// Refuses a resolved path inside the data root or the binary's directory.
///
/// A root or a project may legitimately *contain* them — a whole drive, or this
/// very repository, whose `target/debug/data/` is the development data root — so
/// the refusal is by the path, not by the root.
pub(super) fn refuse_app_dirs(ctx: &ToolContext, path: &Path) -> Result<()> {
    let path = strip_verbatim(path);
    if app_dirs(ctx).iter().any(|dir| path.starts_with(dir)) {
        anyhow::bail!(ctx.loc.t("tool.fs.err.app_dir").to_string());
    }
    Ok(())
}

/// Refuses `path` when it is a symbolic link whose target does not exist.
///
/// Called on the part of a path that did not resolve. `exists()` follows links,
/// so such a link reads as absent, and the resolver would judge it by its parent
/// and its own name — which pass the containment check — while the write that
/// follows goes wherever the link points (docs/research/safe-defaults.md §2.1).
pub(super) fn refuse_dangling_link(path: &Path, loc: &crate::shared::i18n::Locale) -> Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        anyhow::bail!(loc.t("tool.fs.err.dangling_link").to_string());
    }
    Ok(())
}
