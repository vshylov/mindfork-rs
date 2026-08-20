//! **SPIKE (stage 0 probe, `docs/code-workspace.md`) — throwaway wiring.**
//!
//! Minimal `code_read` / `code_grep` / `code_edit` for the go/no-go measurement:
//! does a local model honour the exact-substring edit contract at all? Only the
//! *contract the model sees* is real here; everything around it is deliberately
//! cheap and is replaced in stage 1:
//!
//! - the workspace root is taken from `tools.fs_root` instead of
//!   `Chat.workspace`, so no `Chat`/`ToolContext` plumbing is touched;
//! - `code_grep` is a literal substring scan, not `grep-searcher` + regex;
//! - the path resolver is a local copy of `fs::FsRoot`'s idea rather than the
//!   shared one stage 1 hoists.
//!
//! What is **not** cheap, because it is exactly what the probe measures: the
//! line-numbered read format, the uniqueness rules of the edit, the errors the
//! model reads when a match misses, and EOL/BOM fidelity on write.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

pub const CODE_READ_ID: &str = "code_read";
pub const CODE_GREP_ID: &str = "code_grep";
pub const CODE_EDIT_ID: &str = "code_edit";

/// Default window of a read, in lines.
const DEFAULT_READ_LINES: usize = 400;
/// Ceiling on characters returned by one read.
const MAX_READ_CHARS: usize = 40_000;
/// Ceiling on grep matches returned.
const MAX_GREP_HITS: usize = 100;
/// Files larger than this are refused (bytes) — a probe-sized guard.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Directory names never walked by `code_grep`.
const SKIP_DIRS: [&str; 5] = [".git", "target", "node_modules", ".venv", "dist"];

/// The attached project's root. Unlike `fs::FsRoot`, having **no** root is not
/// "unrestricted" but "no workspace attached" — the tools refuse, because a
/// workspace tool without a workspace has nothing legitimate to reach.
#[derive(Clone)]
struct Workspace {
    root: Option<PathBuf>,
}

impl Workspace {
    fn new(root: Option<String>) -> Self {
        Self {
            root: root
                .filter(|s| !s.trim().is_empty())
                .map(|s| PathBuf::from(s.trim())),
        }
    }

    /// Canonical workspace root, or an error naming what the user must do.
    fn root(&self, loc: &crate::shared::i18n::Locale) -> Result<PathBuf> {
        let Some(root) = &self.root else {
            anyhow::bail!(loc.t("tool.code.err.no_root").to_string());
        };
        let canonical = root
            .canonicalize()
            .map_err(|e| anyhow::anyhow!(format!("{}: {e}", root.display())))?;
        Ok(strip_verbatim(&canonical))
    }

    /// Resolves a (relative or absolute) argument path inside the root.
    fn resolve(&self, raw: &str, loc: &crate::shared::i18n::Locale) -> Result<PathBuf> {
        let raw = raw.trim();
        if raw.is_empty() {
            anyhow::bail!(loc.t("tool.code.err.path_required").to_string());
        }
        let root = self.root(loc)?;
        let requested = PathBuf::from(raw);
        let candidate = if requested.is_absolute() {
            requested
        } else {
            root.join(&requested)
        };
        // Existing path → canonical form; a new file → canonical parent + name.
        let canonical = match candidate.canonicalize() {
            Ok(c) => strip_verbatim(&c),
            Err(_) => {
                let parent = candidate.parent().ok_or_else(|| {
                    anyhow::anyhow!(loc.t("tool.code.err.path_required").to_string())
                })?;
                let parent = parent
                    .canonicalize()
                    .map_err(|e| anyhow::anyhow!(format!("{}: {e}", parent.display())))?;
                let name = candidate.file_name().ok_or_else(|| {
                    anyhow::anyhow!(loc.t("tool.code.err.path_required").to_string())
                })?;
                strip_verbatim(&parent).join(name)
            }
        };
        if !canonical.starts_with(&root) {
            anyhow::bail!(loc.tf(
                "tool.code.err.outside",
                &[("root", &root.display().to_string())]
            ));
        }
        Ok(canonical)
    }
}

/// Windows canonicalization yields `\\?\C:\…`; that form leaks into results and
/// then comes back from the model as a path we would have to accept. Strip it
/// once, at the boundary.
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

/// Path as the model should see it: relative to the root, forward slashes.
fn display_rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// The EOL/BOM shape of a file, so an edit gives it back unchanged.
struct TextFile {
    /// Content with `\n` endings and no BOM — what matching runs against.
    text: String,
    crlf: bool,
    bom: bool,
}

impl TextFile {
    fn load(bytes: &[u8]) -> Option<Self> {
        if bytes.contains(&0) {
            return None; // binary
        }
        let (bom, rest) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            Some(rest) => (true, rest),
            None => (false, bytes),
        };
        let raw = String::from_utf8_lossy(rest).into_owned();
        let crlf = raw.contains("\r\n");
        Some(Self {
            text: raw.replace("\r\n", "\n"),
            crlf,
            bom,
        })
    }

    /// Restores the original shape for writing back.
    fn encode(&self, text: &str) -> Vec<u8> {
        let body = if self.crlf {
            text.replace('\n', "\r\n")
        } else {
            text.to_string()
        };
        let mut out = Vec::new();
        if self.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(body.as_bytes());
        out
    }
}

fn arg_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn arg_usize(args: &serde_json::Value, key: &str) -> Option<usize> {
    args.get(key).and_then(|v| {
        v.as_u64()
            .map(|n| n as usize)
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

/// Reads a file as text, mapping the two "cannot" cases to model-readable errors.
async fn read_text(path: &Path, loc: &crate::shared::i18n::Locale) -> Result<TextFile> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| anyhow::anyhow!(format!("{}: {e}", path.display())))?;
    if meta.len() > MAX_FILE_BYTES {
        anyhow::bail!(loc.tf(
            "tool.code.err.too_large",
            &[("max", &(MAX_FILE_BYTES / 1024).to_string())]
        ));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| anyhow::anyhow!(format!("{}: {e}", path.display())))?;
    TextFile::load(&bytes).ok_or_else(|| anyhow::anyhow!(loc.t("tool.code.err.binary").to_string()))
}

/// Renders `lines[from..to]` with 1-based numbers in the read format.
fn numbered(lines: &[&str], from: usize, to: usize) -> String {
    let mut out = String::new();
    for (i, line) in lines[from..to].iter().enumerate() {
        out.push_str(&format!("{:>6}\u{2192}{line}\n", from + i + 1));
    }
    out
}

/// `code_read` — line-numbered window over a file of the attached project.
pub struct CodeRead {
    ws: Workspace,
}

impl CodeRead {
    pub fn new(root: Option<String>) -> Self {
        Self {
            ws: Workspace::new(root),
        }
    }
}

#[async_trait::async_trait]
impl Tool for CodeRead {
    fn id(&self) -> ToolId {
        CODE_READ_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "read project file"
    }
    fn gate(&self) -> Option<super::meta::ToolGate> {
        Some(super::meta::ToolGate::Fs)
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.code_read.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": loc.t("tool.code.param.path")},
                "offset": {"type": "integer", "description": loc.t("tool.code.param.offset")},
                "limit": {"type": "integer", "description": loc.t("tool.code.param.limit")}
            },
            "required": ["path"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let root = self.ws.root(ctx.loc)?;
        let raw = arg_str(&args, "path")
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.path_required").to_string()))?;
        let path = self.ws.resolve(&raw, ctx.loc)?;
        let file = read_text(&path, ctx.loc).await?;
        let lines: Vec<&str> = file.text.lines().collect();
        let total = lines.len();
        let offset = arg_usize(&args, "offset").unwrap_or(1).max(1);
        let limit = arg_usize(&args, "limit")
            .unwrap_or(DEFAULT_READ_LINES)
            .max(1);
        let start = offset - 1;
        if start >= total && total > 0 {
            anyhow::bail!(
                ctx.loc
                    .tf("tool.code.err.bad_offset", &[("total", &total.to_string())])
            );
        }
        let end = (start + limit).min(total);
        let mut body = numbered(&lines, start, end);
        let clipped = body.chars().count() > MAX_READ_CHARS;
        if clipped {
            body = body.chars().take(MAX_READ_CHARS).collect();
        }
        let mut out = ctx.loc.tf(
            "tool.code.read.header",
            &[
                ("path", &display_rel(&path, &root)),
                ("from", &(start + 1).to_string()),
                ("to", &end.to_string()),
                ("total", &total.to_string()),
            ],
        );
        out.push('\n');
        out.push_str(&body);
        if end < total || clipped {
            out.push_str(
                &ctx.loc
                    .tf("tool.code.read.more", &[("next", &(end + 1).to_string())]),
            );
        }
        Ok(ToolOutcome::text(out))
    }
}

/// `code_grep` — literal, case-insensitive substring search over the project.
pub struct CodeGrep {
    ws: Workspace,
}

impl CodeGrep {
    pub fn new(root: Option<String>) -> Self {
        Self {
            ws: Workspace::new(root),
        }
    }
}

/// Collects text files under `dir` (depth-first, skipping `SKIP_DIRS`).
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                collect_files(&path, out);
            }
        } else {
            out.push(path);
        }
    }
}

#[async_trait::async_trait]
impl Tool for CodeGrep {
    fn id(&self) -> ToolId {
        CODE_GREP_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "search project"
    }
    fn gate(&self) -> Option<super::meta::ToolGate> {
        Some(super::meta::ToolGate::Fs)
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.code_grep.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": loc.t("tool.code.param.pattern")}
            },
            "required": ["pattern"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let root = self.ws.root(ctx.loc)?;
        let pattern = arg_str(&args, "pattern")
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(ctx.loc.t("tool.code.err.pattern_required").to_string())
            })?;
        let needle = pattern.to_lowercase();
        let root_for_walk = root.clone();
        let files = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            collect_files(&root_for_walk, &mut files);
            files
        })
        .await?;

        let mut hits = Vec::new();
        let mut truncated = false;
        for path in files {
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let Some(file) = TextFile::load(&bytes) else {
                continue;
            };
            for (i, line) in file.text.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    if hits.len() >= MAX_GREP_HITS {
                        truncated = true;
                        break;
                    }
                    hits.push(format!(
                        "{}:{}: {}",
                        display_rel(&path, &root),
                        i + 1,
                        line.trim_end()
                    ));
                }
            }
            if truncated {
                break;
            }
        }
        if hits.is_empty() {
            return Ok(ToolOutcome::text(
                ctx.loc.tf("tool.code.grep.empty", &[("pattern", &pattern)]),
            ));
        }
        let mut out = ctx.loc.tf(
            "tool.code.grep.header",
            &[("pattern", &pattern), ("n", &hits.len().to_string())],
        );
        out.push('\n');
        out.push_str(&hits.join("\n"));
        if truncated {
            out.push('\n');
            out.push_str(&ctx.loc.tf(
                "tool.code.grep.truncated",
                &[("max", &MAX_GREP_HITS.to_string())],
            ));
        }
        Ok(ToolOutcome::text(out))
    }
}

/// `code_edit` — exact-substring replacement. The contract under measurement.
pub struct CodeEdit {
    ws: Workspace,
}

impl CodeEdit {
    pub fn new(root: Option<String>) -> Self {
        Self {
            ws: Workspace::new(root),
        }
    }
}

#[async_trait::async_trait]
impl Tool for CodeEdit {
    fn id(&self) -> ToolId {
        CODE_EDIT_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Files
    }
    fn danger(&self) -> bool {
        true
    }
    fn ui_label(&self) -> &'static str {
        "edit project file"
    }
    fn gate(&self) -> Option<super::meta::ToolGate> {
        Some(super::meta::ToolGate::Fs)
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.code_edit.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": loc.t("tool.code.param.path")},
                "old_string": {"type": "string", "description": loc.t("tool.code.param.old_string")},
                "new_string": {"type": "string", "description": loc.t("tool.code.param.new_string")},
                "replace_all": {"type": "boolean", "description": loc.t("tool.code.param.replace_all")}
            },
            "required": ["path", "old_string", "new_string"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let root = self.ws.root(ctx.loc)?;
        let raw = arg_str(&args, "path")
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.path_required").to_string()))?;
        let path = self.ws.resolve(&raw, ctx.loc)?;
        let old = arg_str(&args, "old_string")
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.edit_args").to_string()))?;
        let new = arg_str(&args, "new_string")
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.edit_args").to_string()))?;
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if old.is_empty() {
            anyhow::bail!(ctx.loc.t("tool.code.err.edit_args").to_string());
        }
        let file = read_text(&path, ctx.loc).await?;
        // The model writes `\n`; the file may be CRLF. Match on the normalized
        // text and give the file's own shape back on write.
        let old_n = old.replace("\r\n", "\n");
        let new_n = new.replace("\r\n", "\n");
        let count = file.text.matches(&old_n).count();
        let rel = display_rel(&path, &root);
        if count == 0 {
            // A miss must say what to do next, or the model retries the same string.
            return Ok(ToolOutcome::text(
                ctx.loc.tf("tool.code.edit.not_found", &[("path", &rel)]),
            ));
        }
        if count > 1 && !replace_all {
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.code.edit.ambiguous",
                &[("n", &count.to_string()), ("path", &rel)],
            )));
        }
        let updated = if replace_all {
            file.text.replace(&old_n, &new_n)
        } else {
            file.text.replacen(&old_n, &new_n, 1)
        };
        tokio::fs::write(&path, file.encode(&updated))
            .await
            .map_err(|e| anyhow::anyhow!(format!("{}: {e}", path.display())))?;

        // Echo the neighbourhood of the change, numbered, so the model can verify
        // without spending a second round on a read.
        let at = updated
            .find(&new_n)
            .map(|byte| updated[..byte].matches('\n').count())
            .unwrap_or(0);
        let lines: Vec<&str> = updated.lines().collect();
        let from = at.saturating_sub(3);
        let to = (at + new_n.lines().count() + 3).min(lines.len());
        let applied = if replace_all { count } else { 1 };
        let mut out = ctx.loc.tf(
            "tool.code.edit.ok",
            &[("path", &rel), ("n", &applied.to_string())],
        );
        out.push('\n');
        out.push_str(&numbered(&lines, from, to));
        Ok(ToolOutcome::text(out))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    /// Builds a workspace with `files` and returns its tempdir plus the root string.
    fn workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, Option<String>) {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, body).unwrap();
        }
        let root = dir.path().to_string_lossy().to_string();
        (dir, Some(root))
    }

    #[tokio::test]
    async fn read_numbers_lines_and_reports_total() {
        let (_d, root) = workspace(&[("a.rs", "one\ntwo\nthree\n")]);
        let (_dd, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = CodeRead::new(root)
            .invoke(&ctx, serde_json::json!({"path": "a.rs"}))
            .await
            .unwrap();
        assert!(out.result.contains("\u{2192}two"), "got: {}", out.result);
        assert!(
            out.result.contains('3'),
            "total must be stated: {}",
            out.result
        );
    }

    #[tokio::test]
    async fn edit_replaces_unique_fragment() {
        let (d, root) = workspace(&[("a.rs", "let x = 1;\nlet y = 2;\n")]);
        let (_dd, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        CodeEdit::new(root)
            .invoke(
                &ctx,
                serde_json::json!({"path": "a.rs", "old_string": "let y = 2;", "new_string": "let y = 3;"}),
            )
            .await
            .unwrap();
        let after = std::fs::read_to_string(d.path().join("a.rs")).unwrap();
        assert_eq!(after, "let x = 1;\nlet y = 3;\n");
    }

    /// The two refusals are the whole point of the contract: a miss and an
    /// ambiguous match must leave the file untouched and say which it was.
    #[tokio::test]
    async fn edit_refuses_missing_and_ambiguous() {
        let (d, root) = workspace(&[("a.rs", "dup\ndup\n")]);
        let (_dd, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let tool = CodeEdit::new(root);
        let miss = tool
            .invoke(
                &ctx,
                serde_json::json!({"path": "a.rs", "old_string": "absent", "new_string": "x"}),
            )
            .await
            .unwrap();
        let ambiguous = tool
            .invoke(
                &ctx,
                serde_json::json!({"path": "a.rs", "old_string": "dup", "new_string": "x"}),
            )
            .await
            .unwrap();
        assert_ne!(miss.result, ambiguous.result, "the two must be told apart");
        assert!(
            ambiguous.result.contains('2'),
            "the count is what makes the message actionable: {}",
            ambiguous.result
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
            "dup\ndup\n",
            "a refused edit must not touch the file"
        );
        // `replace_all` is the sanctioned way past the ambiguity.
        tool.invoke(
            &ctx,
            serde_json::json!({"path": "a.rs", "old_string": "dup", "new_string": "x", "replace_all": true}),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
            "x\nx\n"
        );
    }

    /// A model emits `\n`; a Windows checkout is CRLF. Without normalization the
    /// match misses, and without re-encoding one edit rewrites every line.
    #[tokio::test]
    async fn edit_preserves_crlf_and_bom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"let x = 1;\r\nlet y = 2;\r\n");
        std::fs::write(&path, &bytes).unwrap();
        let (_dd, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        CodeEdit::new(Some(dir.path().to_string_lossy().to_string()))
            .invoke(
                &ctx,
                serde_json::json!({"path": "a.rs", "old_string": "let y = 2;", "new_string": "let y = 3;"}),
            )
            .await
            .unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(
            after,
            {
                let mut want = vec![0xEF, 0xBB, 0xBF];
                want.extend_from_slice(b"let x = 1;\r\nlet y = 3;\r\n");
                want
            },
            "CRLF and BOM must survive an edit"
        );
    }

    #[tokio::test]
    async fn grep_finds_and_locates() {
        let (_d, root) = workspace(&[("src/a.rs", "fn mean() {}\n"), ("src/b.rs", "// nothing\n")]);
        let (_dd, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = CodeGrep::new(root)
            .invoke(&ctx, serde_json::json!({"pattern": "mean"}))
            .await
            .unwrap();
        assert!(out.result.contains("src/a.rs:1"), "got: {}", out.result);
        assert!(!out.result.contains("b.rs"), "got: {}", out.result);
    }

    #[tokio::test]
    async fn paths_outside_the_root_are_refused() {
        let (_d, root) = workspace(&[("a.rs", "x\n")]);
        let (_dd, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            CodeRead::new(root)
                .invoke(&ctx, serde_json::json!({"path": "../../secrets.txt"}))
                .await
                .is_err()
        );
    }

    /// No workspace is a refusal, not "the whole disk" — the one place this
    /// family deliberately differs from `fs_read`.
    #[tokio::test]
    async fn without_a_root_the_tools_refuse() {
        let (_dd, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            CodeRead::new(None)
                .invoke(&ctx, serde_json::json!({"path": "Cargo.toml"}))
                .await
                .is_err()
        );
    }
}
