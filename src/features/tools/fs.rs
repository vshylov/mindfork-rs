//! Local file-access tools (spec §9.3, §13.2): `fs_read`,
//! `fs_write`, `fs_list`. Under the global switch `tools.fs_enabled` (off
//! by default).
//!
//! They work only inside the folder `tools.fs_root` names: every path must lie
//! inside it (protection against escaping via `..`/absolute paths), and with the
//! row empty they refuse — a whole disk is one deliberate value away (`C:\`, `/`),
//! never the default (docs/research/safe-defaults.md D1). Whatever the root, the
//! app's own directories stay out of reach ([`super::reach`], D2).

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

/// The shared root of the file tools: the one folder they work in.
#[derive(Clone)]
struct FsRoot {
    /// The folder from `tools.fs_root` (`None` → the row is empty, and every call
    /// refuses).
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

    /// Resolves the path from the argument and checks it's inside the root.
    /// For existing paths, comparison uses the canonical form; for ones that don't
    /// yet exist (writing a new file), the parent directory is canonicalized.
    /// Error texts are in the turn's scaffold language (`ctx.loc`).
    fn resolve(&self, raw: &str, ctx: &ToolContext) -> Result<PathBuf> {
        let loc = ctx.loc;
        let raw = raw.trim();
        if raw.is_empty() {
            anyhow::bail!(loc.t("tool.fs.err.empty_path").to_string());
        }
        let requested = PathBuf::from(raw);
        let Some(root) = &self.root else {
            anyhow::bail!(loc.t("tool.fs.err.no_root").to_string());
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
                super::reach::refuse_dangling_link(&candidate, loc)?;
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
        super::reach::refuse_app_dirs(ctx, &canonical)?;
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
    /// A pure read of one file: nothing a sibling call could observe, nothing
    /// to clean up if dropped (docs/research/concurrent-tools.md §2.3).
    fn concurrent(&self) -> bool {
        true
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "read file"
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
        let path = self.fs.resolve(&arg_path(&args, ctx.loc)?, ctx)?;
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
        // In its own encoding, like every reader of a user's file
        // (docs/research/local-file-encoding.md F1a): a binary file is refused rather than
        // handed over as noise, and a file not in UTF-8 says what it was read as (F4b).
        let markup = crate::shared::text_decode::is_markup_path(&path);
        let Some(file) = crate::shared::text_decode::decode_file(&bytes, markup, ctx.file_hint)
        else {
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.fs_read.result.binary",
                &[("path", &path.display().to_string())],
            )));
        };
        let text = if file.encoding == encoding_rs::UTF_8 {
            file.text
        } else {
            let note = ctx.loc.tf(
                "tool.fs_read.decoded_as",
                &[("encoding", file.encoding.name())],
            );
            format!("{note}\n{}", file.text)
        };
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
    /// Creates or **overwrites** a file. `fs_read`/`fs_list` are read-only and
    /// stay unasked — asking about reads would be the noise that trains a user
    /// to stop reading the prompt.
    fn danger(&self) -> bool {
        true
    }
    fn ui_label(&self) -> &'static str {
        "write file"
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
        let path = self.fs.resolve(&arg_path(&args, ctx.loc)?, ctx)?;
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
///
/// The `flush` is **required**, not tidiness: `tokio::fs::File` buffers writes
/// and hands them to a background blocking task, and `Drop` cannot await — so
/// without it the bytes may not have reached the OS by the time the call
/// returns, and a read that follows sees the file as it was. It surfaced as an
/// intermittent CI failure on Linux (`append_adds_to_end`: "a" instead of "ab")
/// while passing on Windows, which is exactly how a lost race presents.
/// `tokio::fs::write` on the non-append path is a complete operation and needs
/// none of this.
async fn append_to(path: &Path, content: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(content.as_bytes()).await?;
    file.flush().await
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
    /// A directory listing: a read like `fs_read`'s.
    fn concurrent(&self) -> bool {
        true
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "list files"
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
        let path = self.fs.resolve(&arg_path(&args, ctx.loc)?, ctx)?;
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
        let root = Some(dir.path().to_string_lossy().to_string());
        let file = dir.path().join("note.txt");
        let path = file.to_string_lossy().to_string();

        FsWrite::new(root.clone())
            .invoke(&ctx, serde_json::json!({"path": path, "content": "привет"}))
            .await
            .unwrap();
        let out = FsRead::new(root)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert_eq!(out.result, "привет");
    }

    /// A windows-1251 note came back with `U+FFFD` for every letter, and a binary as noise
    /// (docs/research/local-file-encoding.md §1): the note reads in its own encoding and
    /// says which, the binary is refused.
    #[tokio::test]
    async fn read_decodes_a_legacy_file_and_refuses_a_binary() {
        let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
        ctx.file_hint = Some("ru");
        let dir = tempfile::tempdir().unwrap();
        let tool = FsRead::new(Some(dir.path().to_string_lossy().to_string()));
        let note = dir.path().join("report.txt");
        let prose = "Выручка за март составила сто двадцать тысяч, за апрель немного больше.";
        std::fs::write(&note, encoding_rs::WINDOWS_1251.encode(prose).0).unwrap();
        let out = tool
            .invoke(&ctx, serde_json::json!({"path": note.to_string_lossy()}))
            .await
            .unwrap()
            .result;
        assert!(out.contains(prose) && out.contains("windows-1251"), "{out}");
        let blob = dir.path().join("blob.bin");
        std::fs::write(&blob, [0x50, 0x4B, 0x03, 0x04, 0x00, 0x00]).unwrap();
        let out = tool
            .invoke(&ctx, serde_json::json!({"path": blob.to_string_lossy()}))
            .await
            .unwrap()
            .result;
        assert!(!out.contains('\0') && out.contains("blob.bin"), "{out}");
    }

    #[tokio::test]
    async fn append_adds_to_end() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        let root = Some(dir.path().to_string_lossy().to_string());
        let path = dir.path().join("log.txt").to_string_lossy().to_string();
        let w = FsWrite::new(root.clone());
        w.invoke(&ctx, serde_json::json!({"path": path, "content": "a"}))
            .await
            .unwrap();
        w.invoke(
            &ctx,
            serde_json::json!({"path": path, "content": "b", "append": true}),
        )
        .await
        .unwrap();
        let out = FsRead::new(root.clone())
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert_eq!(out.result, "ab");
        // Several appends in a row: each one has to be visible to the next read,
        // which is what `append_to`'s flush guarantees. Without it this raced and
        // failed intermittently on Linux while passing on Windows.
        for c in ["c", "d", "e"] {
            w.invoke(
                &ctx,
                serde_json::json!({"path": path, "content": c, "append": true}),
            )
            .await
            .unwrap();
        }
        let out = FsRead::new(root)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert_eq!(out.result, "abcde");
    }

    #[tokio::test]
    async fn list_shows_entries() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let out = FsList::new(Some(path.clone()))
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await
            .unwrap();
        assert!(out.result.contains("a.txt"), "got: {}", out.result);
        assert!(out.result.contains("sub/"), "got: {}", out.result);
    }

    #[tokio::test]
    async fn read_missing_file_reports_error_not_panic() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let root = tempfile::tempdir().unwrap();
        let out = FsRead::new(Some(root.path().to_string_lossy().to_string()))
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

    // ---------- docs/research/safe-defaults.md D1, D2, N1 ----------

    /// With the root row empty every tool refuses — an existing file is not read,
    /// nothing is written — and the refusal is the one that names the setting.
    #[tokio::test]
    async fn an_empty_root_refuses_every_call() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "secret").unwrap();
        let path = file.to_string_lossy().to_string();
        let refusal = ctx.loc.t("tool.fs.err.no_root");

        let read = FsRead::new(None)
            .invoke(&ctx, serde_json::json!({"path": path}))
            .await;
        assert_eq!(read.unwrap_err().to_string(), refusal);
        let write = FsWrite::new(Some("   ".into()))
            .invoke(&ctx, serde_json::json!({"path": path, "content": "x"}))
            .await;
        assert_eq!(write.unwrap_err().to_string(), refusal);
        let list = FsList::new(None)
            .invoke(
                &ctx,
                serde_json::json!({"path": dir.path().to_string_lossy()}),
            )
            .await;
        assert_eq!(list.unwrap_err().to_string(), refusal);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "secret");
    }

    /// A root that contains the app's data root — a whole drive, a home folder —
    /// still does not reach it: `settings.json` is neither read nor overwritten,
    /// while a folder beside the data root is reachable as before.
    #[tokio::test]
    async fn the_data_root_is_out_of_reach_even_inside_the_root() {
        let (data, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let settings = data.path().join("settings.json");
        std::fs::write(&settings, "{}").unwrap();
        let beside = tempfile::tempdir_in(data.path().parent().unwrap()).unwrap();
        std::fs::write(beside.path().join("ok.txt"), "ok").unwrap();
        let root = Some(data.path().parent().unwrap().to_string_lossy().to_string());
        let refusal = ctx.loc.t("tool.fs.err.app_dir");

        let read = FsRead::new(root.clone())
            .invoke(
                &ctx,
                serde_json::json!({"path": settings.to_string_lossy()}),
            )
            .await;
        assert_eq!(read.unwrap_err().to_string(), refusal);
        let write = FsWrite::new(root.clone())
            .invoke(
                &ctx,
                serde_json::json!({"path": settings.to_string_lossy(), "content": "{\"mcp\":1}"}),
            )
            .await;
        assert_eq!(write.unwrap_err().to_string(), refusal);
        let fresh = FsWrite::new(root.clone())
            .invoke(
                &ctx,
                serde_json::json!({"path": data.path().join("new.json").to_string_lossy(), "content": "x"}),
            )
            .await;
        assert_eq!(fresh.unwrap_err().to_string(), refusal);
        let list = FsList::new(root.clone())
            .invoke(
                &ctx,
                serde_json::json!({"path": data.path().to_string_lossy()}),
            )
            .await;
        assert_eq!(list.unwrap_err().to_string(), refusal);
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{}");
        assert!(!data.path().join("new.json").exists());

        let ok = FsRead::new(root)
            .invoke(
                &ctx,
                serde_json::json!({"path": beside.path().join("ok.txt").to_string_lossy()}),
            )
            .await
            .unwrap();
        assert_eq!(ok.result, "ok");
    }

    /// A link inside the root whose target is missing reads as absent, so it used
    /// to pass the containment check by its own name while the write followed it
    /// out. It is refused now, and nothing appears where it points.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dangling_link_inside_the_root_is_not_written_through() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("escaped.txt");
        std::os::unix::fs::symlink(&target, root.path().join("notes.md")).unwrap();
        let fs_root = Some(root.path().to_string_lossy().to_string());

        let write = FsWrite::new(fs_root)
            .invoke(
                &ctx,
                serde_json::json!({"path": "notes.md", "content": "payload"}),
            )
            .await;
        assert_eq!(
            write.unwrap_err().to_string(),
            ctx.loc.t("tool.fs.err.dangling_link")
        );
        assert!(
            !target.exists(),
            "the write followed the link out of the root"
        );
    }
}
