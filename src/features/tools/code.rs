//! Code-workspace tools (spec §9.12,
//! [docs/code-workspace.md](../../../docs/code-workspace.md)): `code_list`,
//! `code_read`, `code_grep`, `code_edit`, `code_write` — listing, reading,
//! searching and changing the project the user attached to this chat with
//! `/project attach`.
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
pub const CODE_EDIT_ID: &str = "code_edit";
pub const CODE_WRITE_ID: &str = "code_write";

/// The workspace family, in one place, so the registry, the gate and the system
/// block cannot drift apart.
pub const WORKSPACE_TOOL_IDS: [&str; 5] = [
    CODE_LIST_ID,
    CODE_READ_ID,
    CODE_GREP_ID,
    CODE_EDIT_ID,
    CODE_WRITE_ID,
];

/// Every workspace tool, for the registry.
pub const ALL: [CodeTool; 5] = [
    CodeTool::List,
    CodeTool::Read,
    CodeTool::Grep,
    CodeTool::Edit,
    CodeTool::Write,
];

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
    // An existing path canonicalizes. One that does not exist yet is judged by
    // its **deepest existing ancestor** plus the tail: `code_write` creates
    // missing directories, so the whole chain can be absent, and canonicalizing
    // only the immediate parent fails on the first new directory.
    //
    // The tail cannot smuggle an escape: a `..` component has no `file_name`,
    // so it ends the walk with an error rather than being re-attached — and
    // whatever comes out still has to pass the containment check below.
    let canonical = match candidate.canonicalize() {
        Ok(c) => strip_verbatim(&c),
        Err(_) => {
            let mut existing = candidate.as_path();
            let mut tail: Vec<std::ffi::OsString> = Vec::new();
            while !existing.exists() {
                let name = existing.file_name().ok_or_else(|| {
                    anyhow::anyhow!(loc.t("tool.code.err.path_required").to_string())
                })?;
                tail.push(name.to_owned());
                existing = existing.parent().ok_or_else(|| {
                    anyhow::anyhow!(loc.t("tool.code.err.path_required").to_string())
                })?;
            }
            let mut resolved = strip_verbatim(
                &existing
                    .canonicalize()
                    .map_err(|e| anyhow::anyhow!(format!("{}: {e}", existing.display())))?,
            );
            for part in tail.iter().rev() {
                resolved.push(part);
            }
            resolved
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

    /// Restores the file's original shape for writing back.
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

/// Which of the workspace tools this is.
///
/// One type with a discriminant rather than five unit structs with five
/// near-identical `impl Tool` blocks: everything the trait asks for is the same
/// across the family except an id, a label, a bundle key, a schema and the work
/// itself, so the five adapters were the same tokens with different literals —
/// the shape docs/lessons.md §2 records, and the shape a duplication gate reads
/// as one block copied five times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeTool {
    List,
    Read,
    Grep,
    Edit,
    Write,
}

impl CodeTool {
    /// The id the model calls it by (also what `WORKSPACE_TOOL_IDS` lists).
    pub fn id(self) -> &'static str {
        match self {
            Self::List => CODE_LIST_ID,
            Self::Read => CODE_READ_ID,
            Self::Grep => CODE_GREP_ID,
            Self::Edit => CODE_EDIT_ID,
            Self::Write => CODE_WRITE_ID,
        }
    }

    /// Short label for the profile's tool toggles.
    fn label(self) -> &'static str {
        match self {
            Self::List => "list project files",
            Self::Read => "read project file",
            Self::Grep => "search project",
            Self::Edit => "edit project file",
            Self::Write => "write project file",
        }
    }

    /// Bundle key of the description the model reads (axis A).
    fn description_key(self) -> &'static str {
        match self {
            Self::List => "tool.code_list.desc",
            Self::Read => "tool.code_read.desc",
            Self::Grep => "tool.code_grep.desc",
            Self::Edit => "tool.code_edit.desc",
            Self::Write => "tool.code_write.desc",
        }
    }

    /// Whether this tool changes the user's files — what
    /// `tools.confirm_dangerous` (spec §9.8) keys on. Reading the project is not
    /// asked about, exactly as `fs_read` is not.
    fn changes_files(self) -> bool {
        matches!(self, Self::Edit | Self::Write)
    }
}

#[async_trait::async_trait]
impl Tool for CodeTool {
    fn id(&self) -> ToolId {
        CodeTool::id(*self).into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        self.label()
    }
    fn danger(&self) -> bool {
        self.changes_files()
    }
    /// The whole family is exempt: see `Tool::counts_toward_round_limit` and
    /// spec §9.12.
    fn counts_toward_round_limit(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t(self.description_key()).into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        match self {
            Self::List => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": loc.t("tool.code.param.dir")},
                    "depth": {"type": "integer", "description": loc.t("tool.code.param.depth")}
                }
            }),
            Self::Read => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": loc.t("tool.code.param.path")},
                    "offset": {"type": "integer", "description": loc.t("tool.code.param.offset")},
                    "limit": {"type": "integer", "description": loc.t("tool.code.param.limit")}
                },
                "required": ["path"]
            }),
            Self::Grep => serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": loc.t("tool.code.param.pattern")},
                    "path": {"type": "string", "description": loc.t("tool.code.param.grep_dir")},
                    "glob": {"type": "string", "description": loc.t("tool.code.param.glob")}
                },
                "required": ["pattern"]
            }),
            Self::Edit => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": loc.t("tool.code.param.path")},
                    "old_string": {"type": "string", "description": loc.t("tool.code.param.old_string")},
                    "new_string": {"type": "string", "description": loc.t("tool.code.param.new_string")},
                    "replace_all": {"type": "boolean", "description": loc.t("tool.code.param.replace_all")}
                },
                "required": ["path", "old_string", "new_string"]
            }),
            Self::Write => serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": loc.t("tool.code.param.path")},
                    "content": {"type": "string", "description": loc.t("tool.code.param.content")}
                },
                "required": ["path", "content"]
            }),
        }
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        match self {
            Self::List => list(ctx, args).await,
            Self::Read => read(ctx, args).await,
            Self::Grep => grep(ctx, args).await,
            Self::Edit => edit(ctx, args).await,
            Self::Write => write(ctx, args).await,
        }
    }
}

/// `code_list` — the shape of the project, or of one directory in it.
async fn list(ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
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

/// `code_read` — a line-numbered window over a file of the attached project.
async fn read(ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
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

/// `code_grep` — regular-expression search over the project's text files.
async fn grep(ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
    let root = workspace_root(ctx)?;
    let pattern = arg_str(&args, "pattern")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.pattern_required").to_string()))?;
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

/// Journals the pre-image of `path` before it is changed, or explains why the
/// change must not happen.
///
/// The refusal is deliberate rather than best-effort: the user's control over
/// what the assistant does to their code is the changes screen and its revert
/// (design fork F1), and both rest on this file's bytes, which exist nowhere
/// else once it is overwritten. An unjournaled edit would quietly remove that
/// control.
async fn journal_before_write(
    ctx: &ToolContext,
    root: &Path,
    path: &Path,
    existing: Option<&[u8]>,
) -> Result<()> {
    let Some(dir) = &ctx.workspace_journal else {
        // No journal directory at all — a background turn, or a context built
        // without one. The edit tools are not offered there, so this is a
        // programming error rather than a user-facing state.
        anyhow::bail!(ctx.loc.t("tool.code.err.no_journal").to_string());
    };
    let journal = crate::features::workspace_journal::Journal::new(dir.clone());
    let rel = display_rel(path, root);
    let root = root.display().to_string();
    let bytes = existing.map(|b| b.to_vec());
    // Off the async runtime: this is a handful of small synchronous file
    // operations, and a blocking write inside the turn's task would stall it.
    let dir = dir.clone();
    tokio::task::spawn_blocking(move || {
        let journal = crate::features::workspace_journal::Journal::new(dir);
        journal.record(&root, &rel, bytes.as_deref())
    })
    .await?
    .map_err(|err| {
        anyhow::anyhow!(
            ctx.loc
                .tf("tool.code.err.journal_failed", &[("err", &err.to_string())])
        )
    })?;
    drop(journal);
    Ok(())
}

/// `code_edit` — exact-substring replacement. The contract stage 0 measured.
async fn edit(ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
    let root = workspace_root(ctx)?;
    let raw = arg_str(&args, "path")
        .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.path_required").to_string()))?;
    let path = resolve(&root, &raw, ctx.loc)?;
    let old = arg_str(&args, "old_string")
        .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.edit_args").to_string()))?;
    let new = arg_str(&args, "new_string")
        .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.edit_args").to_string()))?;
    if old.is_empty() {
        anyhow::bail!(ctx.loc.t("tool.code.err.edit_args").to_string());
    }
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| anyhow::anyhow!(format!("{}: {e}", path.display())))?;
    let file = TextFile::load(&bytes)
        .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.binary").to_string()))?;
    // The model writes `\n`; the file may be CRLF. Match on the normalized
    // text and give the file's own shape back on write.
    let old_n = old.replace("\r\n", "\n");
    let new_n = new.replace("\r\n", "\n");
    let count = file.text.matches(&old_n).count();
    let rel = display_rel(&path, &root);
    // The two refusals are the contract's working half: each says which of
    // the two happened and what to do next, because a message that only says
    // "no" costs the model its next round (docs/lessons.md §4). Nothing is
    // written in either case.
    if count == 0 {
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

    journal_before_write(ctx, &root, &path, Some(&bytes)).await?;
    let updated = if replace_all {
        file.text.replace(&old_n, &new_n)
    } else {
        file.text.replacen(&old_n, &new_n, 1)
    };
    tokio::fs::write(&path, file.encode(&updated))
        .await
        .map_err(|e| anyhow::anyhow!(format!("{}: {e}", path.display())))?;

    // Echo the neighbourhood of the change, numbered, so the model can
    // verify without spending a round on a second read.
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

/// `code_write` — create a file, or replace one whole.
async fn write(ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
    let root = workspace_root(ctx)?;
    let raw = arg_str(&args, "path")
        .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.path_required").to_string()))?;
    let path = resolve(&root, &raw, ctx.loc)?;
    let content = arg_str(&args, "content")
        .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.write_args").to_string()))?;

    let existing = tokio::fs::read(&path).await.ok();
    // An existing file keeps its own line endings and BOM: replacing a CRLF
    // file with `\n` text would make one edit look like a whole-file rewrite
    // in the diff, and in the user's own version control afterwards.
    let shaped = match existing.as_deref().and_then(TextFile::load) {
        Some(file) => file.encode(&content.replace("\r\n", "\n")),
        None => content.replace("\r\n", "\n").into_bytes(),
    };
    journal_before_write(ctx, &root, &path, existing.as_deref()).await?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow::anyhow!(format!("{}: {e}", parent.display())))?;
    }
    tokio::fs::write(&path, &shaped)
        .await
        .map_err(|e| anyhow::anyhow!(format!("{}: {e}", path.display())))?;
    let rel = display_rel(&path, &root);
    let key = if existing.is_some() {
        "tool.code.write.replaced"
    } else {
        "tool.code.write.created"
    };
    Ok(ToolOutcome::text(ctx.loc.tf(
        key,
        &[("path", &rel), ("n", &content.lines().count().to_string())],
    )))
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
        let out = CodeTool::Read
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
        let first = CodeTool::Read
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

        let second = CodeTool::Read
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
        let out = CodeTool::Grep
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
        let lower = CodeTool::Grep
            .invoke(&f.ctx, serde_json::json!({"pattern": "widget"}))
            .await
            .unwrap();
        assert!(lower.result.contains("a.rs:1"), "got: {}", lower.result);
        let upper = CodeTool::Grep
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
        let out = CodeTool::Grep
            .invoke(&f.ctx, serde_json::json!({"pattern": "fn ("}))
            .await
            .unwrap();
        assert!(out.result.contains("fn ("), "got: {}", out.result);
    }

    #[tokio::test]
    async fn grep_filters_by_glob() {
        let f = fixture(&[("src/a.rs", "target\n"), ("notes.md", "target\n")]);
        let out = CodeTool::Grep
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
        let no_hits = CodeTool::Grep
            .invoke(&f.ctx, serde_json::json!({"pattern": "absent"}))
            .await
            .unwrap();
        let no_files = CodeTool::Grep
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
        let grep = CodeTool::Grep
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
        let list = CodeTool::List
            .invoke(&f.ctx, serde_json::json!({"depth": 3}))
            .await
            .unwrap();
        assert!(list.result.contains("src/"), "got: {}", list.result);
        assert!(!list.result.contains("target/"), "got: {}", list.result);
    }

    #[tokio::test]
    async fn list_shows_the_tree_and_marks_directories() {
        let f = fixture(&[("src/a.rs", "x\n"), ("README.md", "y\n")]);
        let out = CodeTool::List
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
        let shallow = CodeTool::List
            .invoke(&f.ctx, serde_json::json!({"depth": 1}))
            .await
            .unwrap();
        assert!(!shallow.result.contains("x.rs"), "{}", shallow.result);
        let deep = CodeTool::List
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
                CodeTool::Read
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
            CodeTool::Read
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
            CodeTool::Read
                .invoke(&f.ctx, serde_json::json!({"path": "Cargo.toml"}))
                .await
                .is_err()
        );
        assert!(
            CodeTool::Grep
                .invoke(&f.ctx, serde_json::json!({"pattern": "fn"}))
                .await
                .is_err()
        );
        assert!(
            CodeTool::List
                .invoke(&f.ctx, serde_json::json!({}))
                .await
                .is_err()
        );
        // The refusal must name the route that works, not merely say "no".
        let err = CodeTool::List
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
            CodeTool::Read
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

    /// Like [`fixture`], with a journal directory, which the editing tools
    /// require — they refuse to change a file whose original they cannot record.
    fn editable(files: &[(&str, &str)]) -> (Fixture, tempfile::TempDir) {
        let mut f = fixture(files);
        let journal = tempfile::tempdir().unwrap();
        f.ctx.workspace_journal = Some(journal.path().join("chat"));
        (f, journal)
    }

    #[tokio::test]
    async fn edit_replaces_a_unique_fragment() {
        let (f, _j) = editable(&[("a.rs", "let x = 1;\nlet y = 2;\n")]);
        let out = CodeTool::Edit
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "a.rs", "old_string": "let y = 2;", "new_string": "let y = 3;"}),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("a.rs")).unwrap(),
            "let x = 1;\nlet y = 3;\n"
        );
        // The result echoes the neighbourhood, so the model can check its own
        // work without spending a round on a second read.
        assert!(
            out.result.contains("\u{2192}let y = 3;"),
            "got: {}",
            out.result
        );
    }

    /// The two refusals are the working half of the contract: each says which of
    /// the two happened, and **neither writes anything**.
    #[tokio::test]
    async fn edit_refuses_a_missing_or_ambiguous_fragment_without_writing() {
        let (f, _j) = editable(&[("a.rs", "dup\ndup\n")]);
        let miss = CodeTool::Edit
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "a.rs", "old_string": "absent", "new_string": "x"}),
            )
            .await
            .unwrap();
        let ambiguous = CodeTool::Edit
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "a.rs", "old_string": "dup", "new_string": "x"}),
            )
            .await
            .unwrap();
        assert_ne!(miss.result, ambiguous.result, "the two must be told apart");
        assert!(
            ambiguous.result.contains('2'),
            "the count is what makes it actionable: {}",
            ambiguous.result
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("a.rs")).unwrap(),
            "dup\ndup\n",
            "a refused edit must not touch the file"
        );
        // `replace_all` is the sanctioned way past the ambiguity.
        CodeTool::Edit
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "a.rs", "old_string": "dup", "new_string": "x", "replace_all": true}),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("a.rs")).unwrap(),
            "x\nx\n"
        );
    }

    /// A model emits `\n`; a Windows checkout is CRLF. Without normalization the
    /// match misses; without re-encoding, one edit rewrites every line of the
    /// file and the diff (and the user's own version control) says so.
    #[tokio::test]
    async fn edit_preserves_crlf_and_bom() {
        let (f, _j) = editable(&[]);
        let path = f.dir.path().join("a.rs");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"let x = 1;\r\nlet y = 2;\r\n");
        std::fs::write(&path, &bytes).unwrap();
        CodeTool::Edit
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "a.rs", "old_string": "let y = 2;", "new_string": "let y = 3;"}),
            )
            .await
            .unwrap();
        let mut want = vec![0xEF, 0xBB, 0xBF];
        want.extend_from_slice(b"let x = 1;\r\nlet y = 3;\r\n");
        assert_eq!(std::fs::read(&path).unwrap(), want);
    }

    /// The baseline is what the changes screen and its revert rest on, so it is
    /// recorded **before** the write, and only on the first touch.
    #[tokio::test]
    async fn an_edit_journals_the_original_once() {
        let (f, journal) = editable(&[("a.rs", "one\n")]);
        let j = crate::features::workspace_journal::Journal::new(journal.path().join("chat"));
        for (old, new) in [("one", "two"), ("two", "three")] {
            CodeTool::Edit
                .invoke(
                    &f.ctx,
                    serde_json::json!({"path": "a.rs", "old_string": old, "new_string": new}),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("a.rs")).unwrap(),
            "three\n"
        );
        assert_eq!(
            j.baseline_of("a.rs").as_deref(),
            Some(&b"one\n"[..]),
            "the baseline must be the file before the *first* edit"
        );
        assert_eq!(j.entries().len(), 1);
    }

    /// An unjournalable edit is refused rather than applied: the user's control
    /// over what the assistant does is the changes screen, and it rests on the
    /// baseline (design fork F1).
    #[tokio::test]
    async fn an_edit_that_cannot_be_journaled_does_not_happen() {
        let (mut f, journal) = editable(&[("a.rs", "one\n")]);
        // A file where the journal directory should be — the portable way to
        // make the journal unwritable.
        let blocked = journal.path().join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        f.ctx.workspace_journal = Some(blocked);
        let err = CodeTool::Edit
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "a.rs", "old_string": "one", "new_string": "two"}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("a.rs")).unwrap(),
            "one\n",
            "the file must be untouched: {err}"
        );
    }

    /// With no journal at all — a background turn — editing is impossible, and
    /// the message says so rather than failing obscurely.
    #[tokio::test]
    async fn without_a_journal_editing_refuses() {
        let f = fixture(&[("a.rs", "one\n")]);
        assert!(f.ctx.workspace_journal.is_none());
        assert!(
            CodeTool::Edit
                .invoke(
                    &f.ctx,
                    serde_json::json!({"path": "a.rs", "old_string": "one", "new_string": "two"}),
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn write_creates_a_file_with_its_parents_and_journals_it_as_new() {
        let (f, journal) = editable(&[]);
        let out = CodeTool::Write
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "src/deep/new.rs", "content": "fn main() {}\n"}),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("src/deep/new.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert!(
            out.result.contains("src/deep/new.rs"),
            "got: {}",
            out.result
        );
        let j = crate::features::workspace_journal::Journal::new(journal.path().join("chat"));
        let entries = j.entries();
        assert_eq!(entries.len(), 1);
        assert!(
            !entries[0].existed,
            "reverting a created file deletes it, so the entry must say it was new"
        );
    }

    /// Replacing a CRLF file whole keeps its endings: otherwise one write turns
    /// every line of the file into a change in the user's version control.
    #[tokio::test]
    async fn write_keeps_an_existing_file_s_line_endings() {
        let (f, _j) = editable(&[]);
        let path = f.dir.path().join("a.rs");
        std::fs::write(&path, b"old\r\n").unwrap();
        CodeTool::Write
            .invoke(
                &f.ctx,
                serde_json::json!({"path": "a.rs", "content": "new\nlines\n"}),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new\r\nlines\r\n");
    }

    /// A path whose directories do not exist yet is resolved against its
    /// deepest existing ancestor, so an escape must still be caught there — a
    /// `..` in the not-yet-existing tail is where a containment check written
    /// for existing paths would have a hole.
    #[tokio::test]
    async fn a_missing_chain_cannot_be_used_to_escape() {
        let (f, _j) = editable(&[]);
        for path in ["new_dir/../../escaped.rs", "a/b/c/../../../../escaped.rs"] {
            assert!(
                CodeTool::Write
                    .invoke(&f.ctx, serde_json::json!({"path": path, "content": "x"}))
                    .await
                    .is_err(),
                "must be refused: {path}"
            );
        }
        // The legitimate half of the same mechanism still works.
        assert!(
            CodeTool::Write
                .invoke(
                    &f.ctx,
                    serde_json::json!({"path": "deep/nested/ok.rs", "content": "x"})
                )
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn writing_outside_the_root_is_refused() {
        let (f, _j) = editable(&[]);
        assert!(
            CodeTool::Write
                .invoke(
                    &f.ctx,
                    serde_json::json!({"path": "../escaped.rs", "content": "x"}),
                )
                .await
                .is_err()
        );
    }

    /// The editing tools declare themselves dangerous, which is what
    /// `tools.confirm_dangerous` (spec §9.8) keys on; the reading ones do not,
    /// or the switch would ask about every listing.
    #[test]
    fn only_the_writing_tools_are_dangerous_and_none_spend_a_round() {
        assert!(CodeTool::Edit.danger() && CodeTool::Write.danger());
        assert!(!CodeTool::Read.danger() && !CodeTool::Grep.danger() && !CodeTool::List.danger());
        for exempt in [
            CodeTool::Edit.counts_toward_round_limit(),
            CodeTool::Write.counts_toward_round_limit(),
            CodeTool::Read.counts_toward_round_limit(),
            CodeTool::Grep.counts_toward_round_limit(),
            CodeTool::List.counts_toward_round_limit(),
        ] {
            assert!(!exempt, "the workspace family does not spend the budget");
        }
    }

    #[test]
    fn the_family_list_matches_the_predicate() {
        for id in WORKSPACE_TOOL_IDS {
            assert!(is_workspace_tool(id), "{id}");
        }
        assert!(!is_workspace_tool("fs_read"));
    }
}
