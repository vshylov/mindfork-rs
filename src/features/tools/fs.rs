//! Local file-access tools (spec §9.3, §13.2): `fs_read`,
//! `fs_write`, `fs_list`. Under the global switch `tools.fs_enabled` (off
//! by default — the tool can read/overwrite any file).
//!
//! An optional "sandbox" `tools.fs_root`: if set, all paths must lie
//! inside it (protection against escaping via `..`/absolute paths). If unset —
//! access to the whole file system (under the switch, like Python).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Tool names (gated by the `tools.fs_enabled` switch).
pub const FS_READ_ID: &str = "fs_read";
pub const FS_WRITE_ID: &str = "fs_write";
pub const FS_LIST_ID: &str = "fs_list";

/// Ceiling on the size of read/returned text (characters) — protects the context.
const MAX_READ_CHARS: usize = 50_000;
/// Ceiling on the number of entries in a directory listing.
const MAX_LIST_ENTRIES: usize = 500;

/// The shared "sandbox" for file tools: an optional restricting root.
#[derive(Clone)]
struct FsRoot {
    /// The canonical restricting directory (`None` → no restriction).
    root: Option<PathBuf>,
}

impl FsRoot {
    fn new(root: Option<String>) -> Self {
        Self {
            root: root
                .filter(|s| !s.trim().is_empty())
                .map(|s| PathBuf::from(s.trim())),
        }
    }

    /// Resolves the path from the argument and checks it's inside the sandbox (if set).
    /// For existing paths, comparison uses the canonical form; for ones that don't
    /// yet exist (writing a new file), the parent directory is canonicalized.
    /// `loc` — the scaffold language for error texts.
    fn resolve(&self, raw: &str, loc: &crate::shared::i18n::Locale) -> Result<PathBuf> {
        let raw = raw.trim();
        if raw.is_empty() {
            anyhow::bail!(loc.t("tool.fs.err.empty_path").to_string());
        }
        let requested = PathBuf::from(raw);
        let Some(root) = &self.root else {
            return Ok(requested);
        };
        let root = root.canonicalize().with_context(|| {
            loc.tf(
                "tool.fs.err.sandbox_unavailable",
                &[("path", &root.display().to_string())],
            )
        })?;
        // An absolute path is taken as-is, a relative one — from the sandbox root.
        let candidate = if requested.is_absolute() {
            requested
        } else {
            root.join(&requested)
        };
        // The canonical form of the path itself (if it exists), or its parent + name.
        let canonical = match candidate.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                let parent = candidate
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!(loc.t("tool.fs.err.no_parent").to_string()))?;
                let parent = parent.canonicalize().with_context(|| {
                    loc.tf(
                        "tool.fs.err.parent_unavailable",
                        &[("path", &parent.display().to_string())],
                    )
                })?;
                let name = candidate
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!(loc.t("tool.fs.err.no_filename").to_string()))?;
                parent.join(name)
            }
        };
        if !canonical.starts_with(&root) {
            anyhow::bail!(loc.tf(
                "tool.fs.err.outside_sandbox",
                &[("root", &root.display().to_string())]
            ));
        }
        Ok(canonical)
    }
}

/// Extracts the string argument `path`.
fn arg_path(args: &serde_json::Value, loc: &crate::shared::i18n::Locale) -> Result<String> {
    args.get("path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!(loc.t("tool.fs.err.path_field_empty").to_string()))
}

/// Truncates a string to `max` characters (on a character boundary) with a marker.
fn truncate_chars(s: &str, max: usize, loc: &crate::shared::i18n::Locale) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!(
        "{cut}\n{}",
        loc.tf("tool.fs.truncated_read", &[("max", &max.to_string())])
    )
}

/// `fs_read` — reads a text file and returns its content.
pub struct FsRead {
    fs: FsRoot,
}

impl FsRead {
    pub fn new(root: Option<String>) -> Self {
        Self {
            fs: FsRoot::new(root),
        }
    }
}

#[async_trait::async_trait]
impl Tool for FsRead {
    fn id(&self) -> ToolId {
        FS_READ_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "прочитать файл"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Fs)
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.fs_read.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": loc.t("tool.fs.param.file_path")}},
            "required": ["path"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let path = self.fs.resolve(&arg_path(&args, ctx.loc)?, ctx.loc)?;
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(err) => {
                return Ok(ToolOutcome::text(ctx.loc.tf(
                    "tool.fs_read.result.read_failed",
                    &[
                        ("path", &path.display().to_string()),
                        ("err", &err.to_string()),
                    ],
                )));
            }
        };
        // Read as UTF-8 (replacing invalid bytes) — nothing to read binary files with.
        let text = String::from_utf8_lossy(&bytes);
        Ok(ToolOutcome::text(truncate_chars(
            &text,
            MAX_READ_CHARS,
            ctx.loc,
        )))
    }
}

/// `fs_write` — writes (or appends) text to a file.
pub struct FsWrite {
    fs: FsRoot,
}

impl FsWrite {
    pub fn new(root: Option<String>) -> Self {
        Self {
            fs: FsRoot::new(root),
        }
    }
}

#[async_trait::async_trait]
impl Tool for FsWrite {
    fn id(&self) -> ToolId {
        FS_WRITE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "записать файл"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Fs)
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.fs_write.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": loc.t("tool.fs.param.file_path")},
                "content": {"type": "string", "description": loc.t("tool.fs_write.param.content")},
                "append": {"type": "boolean", "description": loc.t("tool.fs_write.param.append")}
            },
            "required": ["path", "content"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let path = self.fs.resolve(&arg_path(&args, ctx.loc)?, ctx.loc)?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.fs_write.err.content")))?;
        let append = args
            .get("append")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let result = if append {
            append_to(&path, content).await
        } else {
            tokio::fs::write(&path, content.as_bytes()).await
        };
        let args = [
            ("path", path.display().to_string()),
            ("n", content.chars().count().to_string()),
        ];
        match result {
            Ok(()) => {
                let key = if append {
                    "tool.fs_write.result.appended"
                } else {
                    "tool.fs_write.result.written"
                };
                Ok(ToolOutcome::text(
                    ctx.loc.tf(key, &[("path", &args[0].1), ("n", &args[1].1)]),
                ))
            }
            Err(err) => Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.fs_write.result.write_failed",
                &[("path", &args[0].1), ("err", &err.to_string())],
            ))),
        }
    }
}

/// Appends to the end of a file (creates it if missing).
async fn append_to(path: &Path, content: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(content.as_bytes()).await
}

/// `fs_list` — lists a directory's content.
pub struct FsList {
    fs: FsRoot,
}

impl FsList {
    pub fn new(root: Option<String>) -> Self {
        Self {
            fs: FsRoot::new(root),
        }
    }
}

#[async_trait::async_trait]
impl Tool for FsList {
    fn id(&self) -> ToolId {
        FS_LIST_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "список файлов"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Fs)
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.fs_list.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": loc.t("tool.fs.param.dir_path")}},
            "required": ["path"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let path = self.fs.resolve(&arg_path(&args, ctx.loc)?, ctx.loc)?;
        let mut rd = match tokio::fs::read_dir(&path).await {
            Ok(rd) => rd,
            Err(err) => {
                return Ok(ToolOutcome::text(ctx.loc.tf(
                    "tool.fs_list.result.open_failed",
                    &[
                        ("path", &path.display().to_string()),
                        ("err", &err.to_string()),
                    ],
                )));
            }
        };
        let mut entries: Vec<String> = Vec::new();
        let mut truncated = false;
        while let Some(entry) = rd.next_entry().await? {
            if entries.len() >= MAX_LIST_ENTRIES {
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push(if is_dir { format!("{name}/") } else { name });
        }
        entries.sort();
        if entries.is_empty() {
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.fs_list.result.empty",
                &[("path", &path.display().to_string())],
            )));
        }
        let mut out = format!(
            "{}\n",
            ctx.loc.tf(
                "tool.fs_list.result.header",
                &[
                    ("path", &path.display().to_string()),
                    ("n", &entries.len().to_string())
                ]
            )
        );
        out.push_str(&entries.join("\n"));
        if truncated {
            out.push('\n');
            out.push_str(&ctx.loc.tf(
                "tool.fs_list.truncated",
                &[("max", &MAX_LIST_ENTRIES.to_string())],
            ));
        }
        Ok(ToolOutcome::text(out))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        let path = file.to_string_lossy().to_string();

        FsWrite::new(None)
            .invoke(&ctx, serde_json::json!({"path": path, "content": "привет"}))
            .await
            .unwrap();
        let out = FsRead::new(None)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert_eq!(out.result, "привет");
    }

    #[tokio::test]
    async fn append_adds_to_end() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt").to_string_lossy().to_string();
        let w = FsWrite::new(None);
        w.invoke(&ctx, serde_json::json!({"path": path, "content": "a"}))
            .await
            .unwrap();
        w.invoke(
            &ctx,
            serde_json::json!({"path": path, "content": "b", "append": true}),
        )
        .await
        .unwrap();
        let out = FsRead::new(None)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert_eq!(out.result, "ab");
    }

    #[tokio::test]
    async fn list_shows_entries() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let out = FsList::new(None)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert!(out.result.contains("a.txt"), "got: {}", out.result);
        assert!(out.result.contains("sub/"), "got: {}", out.result);
    }

    #[tokio::test]
    async fn read_missing_file_reports_error_not_panic() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = FsRead::new(None)
            .invoke(
                &ctx,
                serde_json::json!({"path": "definitely-not-a-real-file-xyz.txt"}),
            )
            .await
            .unwrap();
        assert!(
            out.result.contains("Не удалось прочитать"),
            "got: {}",
            out.result
        );
    }

    #[tokio::test]
    async fn sandbox_blocks_outside_paths() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("inside.txt"), "ok").unwrap();
        let fs_root = Some(root.path().to_string_lossy().to_string());

        // Inside the sandbox — readable.
        let inside = FsRead::new(fs_root.clone())
            .invoke(&ctx, serde_json::json!({"path": "inside.txt"}))
            .await
            .unwrap();
        assert_eq!(inside.result, "ok");

        // Escaping via `..` — rejected (a tool error).
        let escape = FsRead::new(fs_root)
            .invoke(&ctx, serde_json::json!({"path": "../../etc/passwd"}))
            .await;
        assert!(escape.is_err(), "escaping the sandbox must be rejected");
    }

    #[tokio::test]
    async fn sandbox_allows_writing_new_file_inside() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let root = tempfile::tempdir().unwrap();
        let fs_root = Some(root.path().to_string_lossy().to_string());
        let out = FsWrite::new(fs_root)
            .invoke(
                &ctx,
                serde_json::json!({"path": "new.txt", "content": "data"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Записано"), "got: {}", out.result);
        assert_eq!(
            std::fs::read_to_string(root.path().join("new.txt")).unwrap(),
            "data"
        );
    }

    #[tokio::test]
    async fn rejects_empty_path() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            FsRead::new(None)
                .invoke(&ctx, serde_json::json!({"path": "  "}))
                .await
                .is_err()
        );
    }
}
