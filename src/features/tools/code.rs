//! Code-workspace tools (spec §9.12,
//! [docs/code-workspace.md](../../../docs/code-workspace.md)): `code_list`,
//! `code_read`, `code_grep` — listing, reading and searching the project the
//! user attached to this chat with `/project attach`.
//!
//! Three properties shape everything here:
//!
//! - **The workspace is the gate.** The root comes from the turn snapshot
//!   (`ToolContext.workspace`), and with no project attached the tools are not
//!   offered to the model at all (`effective_tool_ids`). A call that arrives
//!   anyway — a background turn, a stale schema — refuses and says who attaches
//!   a project, rather than falling back to the whole disk the way `fs_read`
//!   does when `tools.fs_root` is unset. The inversion is deliberate: these
//!   tools *narrow* access to one directory the user pointed at.
//! - **Every path is confined by canonicalization**, so `..`, an absolute path
//!   elsewhere and a symlink pointing out are one check rather than three.
//! - **The read format is a contract with the model.** Lines come back as
//!   `   12→text`, and stage 0 measured that both live model families strip
//!   those prefixes and reproduce the payload byte-for-byte when they edit
//!   (docs/code-workspace.md §7). Nothing here may change that shape casually.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

pub const CODE_LIST_ID: &str = "code_list";
pub const CODE_READ_ID: &str = "code_read";
pub const CODE_GREP_ID: &str = "code_grep";

/// The workspace family, in one place, so the registry, the gate and the system
/// block cannot drift apart.
pub const WORKSPACE_TOOL_IDS: [&str; 3] = [CODE_LIST_ID, CODE_READ_ID, CODE_GREP_ID];

/// Whether `id` belongs to the workspace family (consulted by
/// [`super::effective_tool_ids`], which offers them only with a project attached).
pub fn is_workspace_tool(id: &str) -> bool {
    WORKSPACE_TOOL_IDS.contains(&id)
}

/// Default window of a read, in lines.
const DEFAULT_READ_LINES: usize = 400;
/// Ceiling on characters returned by one read.
const MAX_READ_CHARS: usize = 40_000;
/// Ceiling on entries in one listing.
const MAX_LIST_ENTRIES: usize = 400;
/// Default depth of a listing: deep enough to show a source tree's shape,
/// shallow enough that a large repository's root is not a wall of text.
const DEFAULT_LIST_DEPTH: usize = 2;
/// Ceiling on matches returned by one search.
const MAX_GREP_HITS: usize = 100;
/// Ceiling on the length of one returned match line.
const MAX_GREP_LINE: usize = 300;
/// Files larger than this are refused rather than read (bytes).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Resolves the turn's workspace root, or explains that there is none.
///
/// A `Tool` cannot take the root in its constructor: the registry is built once
/// per config, while a project is attached per chat. So every `invoke` starts
/// here.
fn workspace_root(ctx: &ToolContext) -> Result<PathBuf> {
    let Some(ws) = &ctx.workspace else {
        anyhow::bail!(ctx.loc.t("tool.code.err.no_root").to_string());
    };
    let root = PathBuf::from(&ws.root);
    let canonical = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!(format!("{}: {e}", ws.root)))?;
    Ok(strip_verbatim(&canonical))
}

/// Resolves an argument path inside `root` (relative to it, or absolute within).
fn resolve(root: &Path, raw: &str, loc: &crate::shared::i18n::Locale) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        anyhow::bail!(loc.t("tool.code.err.path_required").to_string());
    }
    let requested = PathBuf::from(raw);
    let candidate = if requested.is_absolute() {
        requested
    } else {
        root.join(&requested)
    };
    // An existing path canonicalizes; one that does not exist yet is judged by
    // its parent plus its name (the file `code_write` will create, stage 2).
    let canonical = match candidate.canonicalize() {
        Ok(c) => strip_verbatim(&c),
        Err(_) => {
            let parent = candidate
                .parent()
                .ok_or_else(|| anyhow::anyhow!(loc.t("tool.code.err.path_required").to_string()))?;
            let parent = parent
                .canonicalize()
                .map_err(|e| anyhow::anyhow!(format!("{}: {e}", parent.display())))?;
            let name = candidate
                .file_name()
                .ok_or_else(|| anyhow::anyhow!(loc.t("tool.code.err.path_required").to_string()))?;
            strip_verbatim(&parent).join(name)
        }
    };
    if !canonical.starts_with(root) {
        anyhow::bail!(loc.tf(
            "tool.code.err.outside",
            &[("root", &root.display().to_string())]
        ));
    }
    Ok(canonical)
}

/// Windows canonicalization yields `\\?\C:\…`; that form would travel into the
/// system prompt and every tool result, and come back from the model as a path
/// we would then have to accept. Strip it once, at the boundary.
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

/// Path as the model should see it: relative to the root, forward slashes. The
/// model sends these back verbatim, so there is one spelling in and out.
fn display_rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// The EOL/BOM shape of a file, so an edit can hand it back unchanged.
///
/// Stage 1 only reads, but the normalization belongs here rather than in the
/// future editor: reading and matching have to agree on one text. A model emits
/// `\n`, a Windows checkout is CRLF, and a fragment quoted back from a read must
/// match the file it came from.
pub(crate) struct TextFile {
    /// Content with `\n` endings and no BOM — what reading and matching use.
    pub text: String,
    pub crlf: bool,
    pub bom: bool,
}

impl TextFile {
    pub fn load(bytes: &[u8]) -> Option<Self> {
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

    /// Restores the file's original shape for writing back (stage 2's editor).
    #[allow(dead_code)] // The writer lands with `code_edit`; stage 1 needs the detection above.
    pub fn encode(&self, text: &str) -> Vec<u8> {
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

/// Builds the project walker: `.gitignore` honoured (nested, plus the repo's
/// exclude file), hidden entries and `.git/` skipped, symlinks not followed.
///
/// Not following links matters twice: a link out of the project would escape the
/// root check every *path* argument passes, and a link back into it is an
/// endless walk.
fn walker(
    dir: &Path,
    max_depth: Option<usize>,
    overrides: Option<ignore::overrides::Override>,
) -> ignore::Walk {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .parents(true)
        // Without this,  is consulted **only inside a git checkout**
        // — measured, an attached directory that is not a repository has its
        // ignore file silently disregarded, so  comes back in every
        // listing and search. A user who wrote the file meant it either way.
        .require_git(false)
        .follow_links(false)
        .max_depth(max_depth);
    if let Some(ov) = overrides {
        builder.overrides(ov);
    }
    builder.build()
}

/// `code_list` — the shape of the project, or of one directory in it.
pub struct CodeList;

#[async_trait::async_trait]
impl Tool for CodeList {
    fn id(&self) -> ToolId {
        CODE_LIST_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "list project files"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.code_list.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": loc.t("tool.code.param.dir")},
                "depth": {"type": "integer", "description": loc.t("tool.code.param.depth")}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let root = workspace_root(ctx)?;
        let dir = match arg_str(&args, "path").filter(|s| !s.trim().is_empty()) {
            Some(raw) => resolve(&root, &raw, ctx.loc)?,
            None => root.clone(),
        };
        let depth = arg_usize(&args, "depth")
            .unwrap_or(DEFAULT_LIST_DEPTH)
            .clamp(1, 16);
        let base = dir.clone();
        let (entries, truncated) = tokio::task::spawn_blocking(move || {
            let mut entries = Vec::new();
            let mut truncated = false;
            for entry in walker(&base, Some(depth), None).flatten() {
                if entry.depth() == 0 {
                    continue; // the directory itself
                }
                if entries.len() >= MAX_LIST_ENTRIES {
                    truncated = true;
                    break;
                }
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                let rel = display_rel(entry.path(), &base);
                entries.push(if is_dir { format!("{rel}/") } else { rel });
            }
            entries.sort();
            (entries, truncated)
        })
        .await?;

        let shown = display_rel(&dir, &root);
        let shown = if shown.is_empty() {
            ".".to_string()
        } else {
            shown
        };
        if entries.is_empty() {
            return Ok(ToolOutcome::text(
                ctx.loc.tf("tool.code.list.empty", &[("path", &shown)]),
            ));
        }
        let mut out = ctx.loc.tf(
            "tool.code.list.header",
            &[("path", &shown), ("n", &entries.len().to_string())],
        );
        out.push('\n');
        out.push_str(&entries.join("\n"));
        if truncated {
            out.push('\n');
            out.push_str(&ctx.loc.tf(
                "tool.code.list.truncated",
                &[("max", &MAX_LIST_ENTRIES.to_string())],
            ));
        }
        Ok(ToolOutcome::text(out))
    }
}

/// `code_read` — a line-numbered window over a file of the attached project.
pub struct CodeRead;

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
        let root = workspace_root(ctx)?;
        let raw = arg_str(&args, "path")
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.path_required").to_string()))?;
        let path = resolve(&root, &raw, ctx.loc)?;
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

/// `code_grep` — regular-expression search over the project's text files.
pub struct CodeGrep;

/// Compiles the optional `glob` argument into a walker override.
///
/// The override is the walker's own filter, so a glob costs nothing extra and
/// composes with `.gitignore` instead of fighting it. Matching is against the
/// **project-relative** path, so `src/**/*.rs` means what it looks like rather
/// than depending on where the application happens to be running.
fn glob_override(root: &Path, glob: &str) -> Result<ignore::overrides::Override, ignore::Error> {
    let mut builder = ignore::overrides::OverrideBuilder::new(root);
    builder.add(glob)?;
    builder.build()
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
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.code_grep.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": loc.t("tool.code.param.pattern")},
                "path": {"type": "string", "description": loc.t("tool.code.param.grep_dir")},
                "glob": {"type": "string", "description": loc.t("tool.code.param.glob")}
            },
            "required": ["pattern"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let root = workspace_root(ctx)?;
        let pattern = arg_str(&args, "pattern")
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(ctx.loc.t("tool.code.err.pattern_required").to_string())
            })?;
        let dir = match arg_str(&args, "path").filter(|s| !s.trim().is_empty()) {
            Some(raw) => resolve(&root, &raw, ctx.loc)?,
            None => root.clone(),
        };
        // Smart case, as a developer's grep does it: an all-lowercase pattern is
        // case-insensitive, one carrying a capital is taken as written.
        let smart_case = !pattern.chars().any(char::is_uppercase);
        // A broken pattern is the model's mistake, not a crash — and it has to
        // come back as an answer naming what broke, or the next round retries
        // the same expression.
        let re = match regex::RegexBuilder::new(&pattern)
            .case_insensitive(smart_case)
            .build()
        {
            Ok(re) => re,
            Err(err) => {
                return Ok(ToolOutcome::text(ctx.loc.tf(
                    "tool.code.grep.bad_pattern",
                    &[("pattern", &pattern), ("err", &err.to_string())],
                )));
            }
        };
        let glob = arg_str(&args, "glob").filter(|s| !s.trim().is_empty());
        let overrides = match &glob {
            Some(g) => match glob_override(&root, g) {
                Ok(ov) => Some(ov),
                Err(err) => {
                    return Ok(ToolOutcome::text(ctx.loc.tf(
                        "tool.code.grep.bad_glob",
                        &[("glob", g), ("err", &err.to_string())],
                    )));
                }
            },
            None => None,
        };

        let root_for_walk = root.clone();
        let (hits, truncated, files) = tokio::task::spawn_blocking(move || {
            let mut hits: Vec<String> = Vec::new();
            let mut truncated = false;
            let mut files = 0usize;
            for entry in walker(&dir, None, overrides).flatten() {
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let path = entry.path();
                let Ok(meta) = path.metadata() else { continue };
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                let Ok(bytes) = std::fs::read(path) else {
                    continue;
                };
                let Some(file) = TextFile::load(&bytes) else {
                    continue; // binary
                };
                files += 1;
                for (i, line) in file.text.lines().enumerate() {
                    if !re.is_match(line) {
                        continue;
                    }
                    if hits.len() >= MAX_GREP_HITS {
                        truncated = true;
                        break;
                    }
                    let text = line.trim_end();
                    let text: String = if text.chars().count() > MAX_GREP_LINE {
                        text.chars().take(MAX_GREP_LINE).collect::<String>() + "…"
                    } else {
                        text.to_string()
                    };
                    hits.push(format!(
                        "{}:{}: {text}",
                        display_rel(path, &root_for_walk),
                        i + 1
                    ));
                }
                if truncated {
                    break;
                }
            }
            (hits, truncated, files)
        })
        .await?;

        if hits.is_empty() {
            // "Nothing matched" and "there was nothing to match against" call for
            // different next moves, so they are different answers (lessons §4).
            let key = if files == 0 {
                "tool.code.grep.nothing_searched"
            } else {
                "tool.code.grep.empty"
            };
            return Ok(ToolOutcome::text(ctx.loc.tf(
                key,
                &[
                    ("pattern", &pattern),
                    ("glob", glob.as_deref().unwrap_or("*")),
                ],
            )));
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

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use crate::entities::workspace::Workspace;
    use uuid::Uuid;

    /// Fixture: a project directory holding `files`, plus a `ToolContext` whose
    /// turn snapshot has it attached. Every test starts this way, so it is a
    /// fixture rather than a test opening (docs/lessons.md §2).
    struct Fixture {
        _data: tempfile::TempDir,
        dir: tempfile::TempDir,
        ctx: ToolContext,
    }

    fn fixture(files: &[(&str, &str)]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let (data, _storage, mut ctx) = ctx_with_storage(Uuid::new_v4());
        ctx.workspace = Some(Workspace::new(dir.path().to_string_lossy().into_owned()));
        Fixture {
            _data: data,
            dir,
            ctx,
        }
    }

    /// Like [`fixture`], with no project attached.
    fn detached() -> Fixture {
        let f = fixture(&[]);
        Fixture {
            ctx: ToolContext {
                workspace: None,
                ..f.ctx
            },
            ..f
        }
    }

    #[tokio::test]
    async fn read_numbers_lines_and_reports_total() {
        let f = fixture(&[("a.rs", "one\ntwo\nthree\n")]);
        let out = CodeRead
            .invoke(&f.ctx, serde_json::json!({"path": "a.rs"}))
            .await
            .unwrap();
        assert!(out.result.contains("\u{2192}two"), "got: {}", out.result);
        assert!(
            out.result.contains('3'),
            "the total must be stated: {}",
            out.result
        );
    }

    /// The window half of the contract: `offset` picks up where the previous read
    /// stopped, the result says where to continue, and the numbers shown are the
    /// file's own — a fragment quoted back from a later window must not name the
    /// wrong line.
    #[tokio::test]
    async fn read_windows_a_long_file_and_says_where_to_continue() {
        let body: String = (1..=50).map(|i| format!("line {i}\n")).collect();
        let f = fixture(&[("big.rs", body.as_str())]);
        let first = CodeRead
            .invoke(&f.ctx, serde_json::json!({"path": "big.rs", "limit": 10}))
            .await
            .unwrap();
        assert!(first.result.contains("\u{2192}line 10"), "{}", first.result);
        assert!(
            !first.result.contains("\u{2192}line 11"),
            "{}",
            first.result
        );
        assert!(first.result.contains("offset=11"), "{}", first.result);

        let second = CodeRead
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "big.rs", "offset": 11, "limit": 10}),
            )
            .await
            .unwrap();
        assert!(
            second.result.contains("    11\u{2192}line 11"),
            "{}",
            second.result
        );
    }

    #[tokio::test]
    async fn grep_locates_matches_and_takes_a_regex() {
        let f = fixture(&[
            ("src/a.rs", "fn mean() {}\nfn median() {}\n"),
            ("src/b.rs", "// nothing here\n"),
        ]);
        let out = CodeGrep
            .invoke(&f.ctx, serde_json::json!({"pattern": r"fn me(an|dian)"}))
            .await
            .unwrap();
        assert!(out.result.contains("src/a.rs:1"), "got: {}", out.result);
        assert!(out.result.contains("src/a.rs:2"), "got: {}", out.result);
        assert!(!out.result.contains("b.rs"), "got: {}", out.result);
    }

    #[tokio::test]
    async fn grep_is_case_insensitive_until_the_pattern_has_a_capital() {
        let f = fixture(&[("a.rs", "struct Widget;\n")]);
        let lower = CodeGrep
            .invoke(&f.ctx, serde_json::json!({"pattern": "widget"}))
            .await
            .unwrap();
        assert!(lower.result.contains("a.rs:1"), "got: {}", lower.result);
        let upper = CodeGrep
            .invoke(&f.ctx, serde_json::json!({"pattern": "WIDGET"}))
            .await
            .unwrap();
        assert!(!upper.result.contains("a.rs:1"), "got: {}", upper.result);
    }

    /// A broken pattern or glob is an answer the model can act on, not an error
    /// that ends the round with nothing to do next.
    #[tokio::test]
    async fn grep_names_a_broken_pattern() {
        let f = fixture(&[("a.rs", "x\n")]);
        let out = CodeGrep
            .invoke(&f.ctx, serde_json::json!({"pattern": "fn ("}))
            .await
            .unwrap();
        assert!(out.result.contains("fn ("), "got: {}", out.result);
    }

    #[tokio::test]
    async fn grep_filters_by_glob() {
        let f = fixture(&[("src/a.rs", "target\n"), ("notes.md", "target\n")]);
        let out = CodeGrep
            .invoke(
                &f.ctx,
                serde_json::json!({"pattern": "target", "glob": "**/*.rs"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("src/a.rs"), "got: {}", out.result);
        assert!(!out.result.contains("notes.md"), "got: {}", out.result);
    }

    /// "No hits" and "nothing was searched" call for different next moves, so a
    /// glob that matches no file at all must not read as "the project does not
    /// contain this".
    #[tokio::test]
    async fn an_empty_search_says_which_kind_of_empty_it_was() {
        let f = fixture(&[("a.rs", "needle\n")]);
        let no_hits = CodeGrep
            .invoke(&f.ctx, serde_json::json!({"pattern": "absent"}))
            .await
            .unwrap();
        let no_files = CodeGrep
            .invoke(
                &f.ctx,
                serde_json::json!({"pattern": "needle", "glob": "**/*.py"}),
            )
            .await
            .unwrap();
        assert_ne!(
            no_hits.result, no_files.result,
            "the two empties must be told apart"
        );
        assert!(no_files.result.contains("*.py"), "{}", no_files.result);
    }

    /// The reason `ignore` is a dependency: the project's own rules decide what
    /// exists, and on a Rust checkout that is most of the bytes on disk.
    #[tokio::test]
    async fn gitignored_and_hidden_entries_are_invisible() {
        let f = fixture(&[
            (".gitignore", "target/\nsecret.txt\n"),
            ("src/a.rs", "needle\n"),
            ("target/build.rs", "needle\n"),
            ("secret.txt", "needle\n"),
            (".hidden/x.rs", "needle\n"),
        ]);
        let grep = CodeGrep
            .invoke(&f.ctx, serde_json::json!({"pattern": "needle"}))
            .await
            .unwrap();
        assert!(grep.result.contains("src/a.rs"), "got: {}", grep.result);
        for hidden in ["target/", "secret.txt", ".hidden"] {
            assert!(
                !grep.result.contains(hidden),
                "{hidden} must not be searched: {}",
                grep.result
            );
        }
        let list = CodeList
            .invoke(&f.ctx, serde_json::json!({"depth": 3}))
            .await
            .unwrap();
        assert!(list.result.contains("src/"), "got: {}", list.result);
        assert!(!list.result.contains("target/"), "got: {}", list.result);
    }

    #[tokio::test]
    async fn list_shows_the_tree_and_marks_directories() {
        let f = fixture(&[("src/a.rs", "x\n"), ("README.md", "y\n")]);
        let out = CodeList
            .invoke(&f.ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("src/"), "got: {}", out.result);
        assert!(out.result.contains("src/a.rs"), "got: {}", out.result);
        assert!(out.result.contains("README.md"), "got: {}", out.result);
    }

    /// Depth is a real bound, not decoration: a deep file must be absent at
    /// depth 1 and present when asked for.
    #[tokio::test]
    async fn list_respects_depth() {
        let f = fixture(&[("src/deep/x.rs", "x\n")]);
        let shallow = CodeList
            .invoke(&f.ctx, serde_json::json!({"depth": 1}))
            .await
            .unwrap();
        assert!(!shallow.result.contains("x.rs"), "{}", shallow.result);
        let deep = CodeList
            .invoke(&f.ctx, serde_json::json!({"depth": 3}))
            .await
            .unwrap();
        assert!(deep.result.contains("src/deep/x.rs"), "{}", deep.result);
    }

    #[tokio::test]
    async fn paths_outside_the_root_are_refused() {
        let f = fixture(&[("a.rs", "x\n")]);
        for path in ["../../secrets.txt", ".."] {
            assert!(
                CodeRead
                    .invoke(&f.ctx, serde_json::json!({"path": path}))
                    .await
                    .is_err(),
                "escaping the workspace must be refused: {path}"
            );
        }
        // An absolute path elsewhere is the same escape in another spelling.
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("out.txt"), "x").unwrap();
        assert!(
            CodeRead
                .invoke(
                    &f.ctx,
                    serde_json::json!({"path": elsewhere.path().join("out.txt").to_string_lossy()})
                )
                .await
                .is_err()
        );
    }

    /// No workspace is a refusal, not "the whole disk" — the one place this
    /// family deliberately differs from `fs_read`.
    #[tokio::test]
    async fn without_a_workspace_every_tool_refuses() {
        let f = detached();
        assert!(
            CodeRead
                .invoke(&f.ctx, serde_json::json!({"path": "Cargo.toml"}))
                .await
                .is_err()
        );
        assert!(
            CodeGrep
                .invoke(&f.ctx, serde_json::json!({"pattern": "fn"}))
                .await
                .is_err()
        );
        assert!(
            CodeList
                .invoke(&f.ctx, serde_json::json!({}))
                .await
                .is_err()
        );
        // The refusal must name the route that works, not merely say "no".
        let err = CodeList
            .invoke(&f.ctx, serde_json::json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("/project"), "got: {err}");
        let _ = f.dir;
    }

    #[tokio::test]
    async fn a_binary_file_is_refused_rather_than_mangled() {
        let f = fixture(&[]);
        std::fs::write(f.dir.path().join("a.bin"), [0x00, 0x01, 0x02]).unwrap();
        assert!(
            CodeRead
                .invoke(&f.ctx, serde_json::json!({"path": "a.bin"}))
                .await
                .is_err()
        );
    }

    /// A model emits `\n`; a Windows checkout is CRLF. Reading normalizes, or a
    /// fragment quoted from a read cannot match the file it came from — the
    /// contract stage 2 rests on.
    #[test]
    fn crlf_and_bom_are_normalized_for_reading_and_restored_for_writing() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"let x = 1;\r\nlet y = 2;\r\n");
        let file = TextFile::load(&bytes).unwrap();
        assert_eq!(file.text, "let x = 1;\nlet y = 2;\n");
        assert!(file.crlf && file.bom);
        assert_eq!(file.encode(&file.text), bytes);
    }

    #[test]
    fn the_family_list_matches_the_predicate() {
        for id in WORKSPACE_TOOL_IDS {
            assert!(is_workspace_tool(id), "{id}");
        }
        assert!(!is_workspace_tool("fs_read"));
    }
}
