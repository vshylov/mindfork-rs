//! Code-workspace tools (spec §9.12,
//! [docs/history/code-workspace.md](../../../docs/history/code-workspace.md)): `code_list`,
//! `code_read`, `code_grep`, `code_edit`, `code_write` — listing, reading,
//! searching and changing the project the user attached to this chat with
//! `/project attach` — plus `code_build`, `code_run` and `code_test`, which run
//! the command lines the **user** typed into that project's three slots.
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
//! - **The model never composes a command.** The three command tools take no
//!   arguments at all (`{}`): the line comes from `/project build-cmd` and is
//!   run as written, so there is no argument to inject into and no slot the
//!   model can point somewhere else. It can *read* the line, which is what lets
//!   it tell the user their command is wrong — and that is the whole of its say.
//! - **The read format is a contract with the model.** Lines come back as
//!   `   12→text`, and stage 0 measured that both live model families strip
//!   those prefixes and reproduce the payload byte-for-byte when they edit
//!   (docs/history/code-workspace.md §7). Nothing here may change that shape casually.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::entities::workspace::CommandSlot;

use super::{Tool, ToolContext, ToolOutcome};

pub const CODE_LIST_ID: &str = "code_list";
pub const CODE_READ_ID: &str = "code_read";
pub const CODE_GREP_ID: &str = "code_grep";
pub const CODE_EDIT_ID: &str = "code_edit";
pub const CODE_WRITE_ID: &str = "code_write";
pub const CODE_BUILD_ID: &str = "code_build";
pub const CODE_RUN_ID: &str = "code_run";
pub const CODE_TEST_ID: &str = "code_test";

/// The workspace family, in one place, so the registry, the gate and the system
/// block cannot drift apart.
pub const WORKSPACE_TOOL_IDS: [&str; 8] = [
    CODE_LIST_ID,
    CODE_READ_ID,
    CODE_GREP_ID,
    CODE_EDIT_ID,
    CODE_WRITE_ID,
    CODE_BUILD_ID,
    CODE_RUN_ID,
    CODE_TEST_ID,
];

/// Every workspace tool, for the registry.
pub const ALL: [CodeTool; 8] = [
    CodeTool::List,
    CodeTool::Read,
    CodeTool::Grep,
    CodeTool::Edit,
    CodeTool::Write,
    CodeTool::Command(CommandSlot::Build),
    CodeTool::Command(CommandSlot::Run),
    CodeTool::Command(CommandSlot::Test),
];

/// Which of the project's command slots carry a line this turn.
///
/// A slot with nothing in it means its tool is **not offered at all** — a
/// disabled tool is never advertised (spec §9.4, S12), and a `code_test` that
/// can only answer "no command is configured" would spend a round teaching the
/// model something the system block already says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspaceCommands {
    pub build: bool,
    pub run: bool,
    pub test: bool,
}

impl WorkspaceCommands {
    /// The slots a project actually has lines for.
    pub fn of(ws: &crate::entities::workspace::Workspace) -> Self {
        Self {
            build: ws.command(CommandSlot::Build).is_some(),
            run: ws.command(CommandSlot::Run).is_some(),
            test: ws.command(CommandSlot::Test).is_some(),
        }
    }

    fn has(self, slot: CommandSlot) -> bool {
        match slot {
            CommandSlot::Build => self.build,
            CommandSlot::Run => self.run,
            CommandSlot::Test => self.test,
        }
    }
}

/// Whether `id` belongs to the workspace family (consulted by
/// [`super::effective_tool_ids`], which offers them only with a project attached).
pub fn is_workspace_tool(id: &str) -> bool {
    WORKSPACE_TOOL_IDS.contains(&id)
}

/// Whether the turn offers `id`, given what the chat has attached.
/// `None` — not a workspace tool at all, so the caller's other gates decide.
///
/// The rule lives here rather than in `effective_tool_ids` because it is the
/// family's own: a reader needs a project, and a command tool needs a project
/// **and** a line in its slot.
pub fn offered(id: &str, attached: bool, commands: WorkspaceCommands) -> Option<bool> {
    if !is_workspace_tool(id) {
        return None;
    }
    Some(match CodeTool::from_id(id) {
        Some(CodeTool::Command(slot)) => attached && commands.has(slot),
        _ => attached,
    })
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
    /// `code_build`/`code_run`/`code_test` — one variant, because the three
    /// differ **only** in which slot they read (design fork F3). Three
    /// variants would be three copies of one runner, which is the shape that
    /// already cost this file a duplication-gate failure once.
    Command(CommandSlot),
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
            Self::Command(CommandSlot::Build) => CODE_BUILD_ID,
            Self::Command(CommandSlot::Run) => CODE_RUN_ID,
            Self::Command(CommandSlot::Test) => CODE_TEST_ID,
        }
    }

    /// The tool this id names, if any.
    pub fn from_id(id: &str) -> Option<Self> {
        ALL.into_iter().find(|t| t.id() == id)
    }

    /// Short label for the profile's tool toggles.
    fn label(self) -> &'static str {
        match self {
            Self::List => "list project files",
            Self::Read => "read project file",
            Self::Grep => "search project",
            Self::Edit => "edit project file",
            Self::Write => "write project file",
            Self::Command(CommandSlot::Build) => "build the project",
            Self::Command(CommandSlot::Run) => "run the project",
            Self::Command(CommandSlot::Test) => "test the project",
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
            Self::Command(CommandSlot::Build) => "tool.code_build.desc",
            Self::Command(CommandSlot::Run) => "tool.code_run.desc",
            Self::Command(CommandSlot::Test) => "tool.code_test.desc",
        }
    }

    /// Bundle key of the one-line gloss the **system block** uses (axis A).
    ///
    /// Shorter than [`Self::description_key`], which the schema already carries:
    /// the block's job is to say which of the family this turn actually has, not
    /// to re-teach each one. Per tool rather than one sentence listing all of
    /// them, because a profile can switch any of them off individually, and a
    /// block naming an absent tool costs the model a turn of improvising with
    /// the wrong ones (spec §9.7 learned this).
    pub fn gloss_key(self) -> &'static str {
        match self {
            Self::List => "prompt.workspace.tool.list",
            Self::Read => "prompt.workspace.tool.read",
            Self::Grep => "prompt.workspace.tool.grep",
            Self::Edit => "prompt.workspace.tool.edit",
            Self::Write => "prompt.workspace.tool.write",
            Self::Command(CommandSlot::Build) => "prompt.workspace.tool.build",
            Self::Command(CommandSlot::Run) => "prompt.workspace.tool.run",
            Self::Command(CommandSlot::Test) => "prompt.workspace.tool.test",
        }
    }

    /// The slot this tool runs, if it is a command tool.
    pub fn slot(self) -> Option<CommandSlot> {
        match self {
            Self::Command(slot) => Some(slot),
            _ => None,
        }
    }

    /// Whether this tool changes the user's files — what
    /// `tools.confirm_dangerous` (spec §9.8) keys on. Reading the project is not
    /// asked about, exactly as `fs_read` is not.
    fn changes_files(self) -> bool {
        // A command tool is dangerous for a different reason than an edit is:
        // it does not write a file itself, it runs the project's own code
        // (`build.rs`, an npm script, the test suite). That is inherent to the
        // feature and consented to twice already — the user typed the line and
        // attached the directory — but it is exactly what someone who turns on
        // `tools.confirm_dangerous` wants to be asked about.
        matches!(self, Self::Edit | Self::Write | Self::Command(_))
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
            // No properties, deliberately: the schema is the guarantee that the
            // model cannot add an argument, a flag or a second command.
            Self::Command(_) => serde_json::json!({"type": "object", "properties": {}}),
        }
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        match self {
            Self::List => list(ctx, args).await,
            Self::Read => read(ctx, args).await,
            Self::Grep => grep(ctx, args).await,
            Self::Edit => edit(ctx, args).await,
            Self::Write => write(ctx, args).await,
            Self::Command(slot) => run_command(ctx, *slot).await,
        }
    }
}

/// The blocking half of [`list`]: the project-relative names under `base`,
/// sorted, and whether [`MAX_LIST_ENTRIES`] cut them short.
fn collect_entries(base: &Path, depth: usize) -> (Vec<String>, bool) {
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in walker(base, Some(depth), None).flatten() {
        if entry.depth() == 0 {
            continue; // the directory itself
        }
        if entries.len() >= MAX_LIST_ENTRIES {
            truncated = true;
            break;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let rel = display_rel(entry.path(), base);
        entries.push(if is_dir { format!("{rel}/") } else { rel });
    }
    entries.sort();
    (entries, truncated)
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
    let (entries, truncated) =
        tokio::task::spawn_blocking(move || collect_entries(&base, depth)).await?;

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

/// One walker entry as searchable text, or `None` when it is not something
/// `code_grep` reads: not a regular file, too large, unreadable, or binary.
fn searchable(entry: &ignore::DirEntry) -> Option<TextFile> {
    if !entry.file_type().is_some_and(|t| t.is_file()) {
        return None;
    }
    let path = entry.path();
    if path.metadata().ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    TextFile::load(&bytes)
}

/// One hit's text, capped at [`MAX_GREP_LINE`] **characters** — a minified file
/// has lines that would otherwise fill the whole answer with one of them.
fn clip_hit(text: &str) -> String {
    if text.chars().count() > MAX_GREP_LINE {
        text.chars().take(MAX_GREP_LINE).collect::<String>() + "…"
    } else {
        text.to_string()
    }
}

/// Appends one file's matching lines to `hits`, prefixed `<path>:<line>: `.
/// `true` when [`MAX_GREP_HITS`] was reached and the walk must stop.
fn scan_text(text: &str, re: &regex::Regex, prefix: &str, hits: &mut Vec<String>) -> bool {
    for (i, line) in text.lines().enumerate() {
        if !re.is_match(line) {
            continue;
        }
        if hits.len() >= MAX_GREP_HITS {
            return true;
        }
        let text = clip_hit(line.trim_end());
        hits.push(format!("{prefix}:{}: {text}", i + 1));
    }
    false
}

/// The blocking half of [`grep`]: walks `dir` and collects the hits.
/// Returns them with the truncation flag and the number of files actually read
/// — the last of which is what separates "nothing matched" from "there was
/// nothing to match against".
fn search_files(
    dir: &Path,
    overrides: Option<ignore::overrides::Override>,
    re: &regex::Regex,
    root: &Path,
) -> (Vec<String>, bool, usize) {
    let mut hits: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut files = 0usize;
    for entry in walker(dir, None, overrides).flatten() {
        let Some(file) = searchable(&entry) else {
            continue;
        };
        files += 1;
        let prefix = display_rel(entry.path(), root);
        if scan_text(&file.text, re, &prefix, &mut hits) {
            truncated = true;
            break;
        }
    }
    (hits, truncated, files)
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
    let overrides = match glob.as_deref().map(|g| glob_override(&root, g)).transpose() {
        Ok(ov) => ov,
        Err(err) => {
            // Only reachable with a glob given, so the `unwrap_or_default` never
            // fires — it is there because the compiler cannot see that from here.
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.code.grep.bad_glob",
                &[
                    ("glob", glob.as_deref().unwrap_or_default()),
                    ("err", &err.to_string()),
                ],
            )));
        }
    };

    let root_for_walk = root.clone();
    let (hits, truncated, files) =
        tokio::task::spawn_blocking(move || search_files(&dir, overrides, &re, &root_for_walk))
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
    let rel = display_rel(path, root);
    let root = root.display().to_string();
    let bytes = existing.map(<[u8]>::to_vec);
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

/// One command at a time across the whole application.
///
/// The agentic loop is sequential anyway, so this is defence in depth rather
/// than a queue — two builds of the same project would fight over the same
/// `target/` directory, and the second one is never what the user wanted. A
/// caller that cannot take the permit is told so and can try again, which is
/// the sandbox's rule (ADR 0005) applied to a heavier subprocess.
static COMMAND_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// How a command run ended — the three outcomes the result has to distinguish,
/// because the next move differs for each.
enum Ended {
    Exited(std::process::ExitStatus),
    TimedOut,
    Cancelled,
}

/// `code_build`/`code_run`/`code_test` — runs the line the **user** put in
/// `slot`, in the project root, with no shell involved.
///
/// One function for the three tools: they differ in which slot they read and in
/// nothing else (design fork F3), and three copies of this would be three copies
/// of the spawn, the timeout, the tree kill and the truncation.
async fn run_command(ctx: &ToolContext, slot: CommandSlot) -> Result<ToolOutcome> {
    let root = workspace_root(ctx)?;
    let ws = ctx
        .workspace
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.code.err.no_root").to_string()))?;
    // Not offered without a line (see `offered`), so reaching this means a stale
    // schema or a background turn — and it still has to say who sets the line.
    let Some(line) = ws.command(slot) else {
        anyhow::bail!(ctx.loc.tf(
            "tool.code.cmd.not_set",
            &[("slot", slot.key()), ("cmd", &slot.setter_command())]
        ));
    };
    // Checked here as well as when the line was set: a chat file is JSON on disk
    // and can be edited by hand, and "the check ran once, somewhere else" is how
    // a guard quietly stops guarding (docs/lessons.md §2).
    if let Some(ch) = crate::shared::cmdline::shell_syntax(line) {
        anyhow::bail!(ctx.loc.tf(
            "tool.code.cmd.shell",
            &[("char", &ch.to_string()), ("line", line)]
        ));
    }
    let argv = crate::shared::cmdline::split(line);
    let Some((program, args)) = argv.split_first() else {
        anyhow::bail!(ctx.loc.tf(
            "tool.code.cmd.not_set",
            &[("slot", slot.key()), ("cmd", &slot.setter_command())]
        ));
    };
    let Ok(_permit) = COMMAND_GATE.try_acquire() else {
        anyhow::bail!(ctx.loc.t("tool.code.cmd.busy").to_string());
    };

    let cfg = ctx.workspace_cfg;
    let timeout = std::time::Duration::from_secs(cfg.command_timeout_secs.max(1));
    let started = std::time::Instant::now();
    let (ended, stdout, stderr) =
        spawn_and_wait(program, args, &root, timeout, &ctx.cancel).await?;

    let secs = format!("{:.1}", started.elapsed().as_secs_f64());
    let status = match &ended {
        Ended::Exited(_) => ctx.loc.tf("tool.code.cmd.finished", &[("secs", &secs)]),
        Ended::TimedOut => ctx.loc.tf(
            "tool.code.cmd.timed_out",
            &[("secs", &cfg.command_timeout_secs.to_string())],
        ),
        Ended::Cancelled => ctx.loc.t("tool.code.cmd.cancelled").to_string(),
    };
    // The command line first, then how it ended: the model has to be able to
    // quote the line back when it tells the user the command itself is wrong,
    // which is the only influence over it this design gives the model at all.
    let header = format!("{line}\n{status}");
    let limit = cfg.output_limit_chars.max(200);
    // A command we killed has no exit code at all. The header already says it
    // timed out or was stopped, so the failure line is suppressed rather than
    // filled with a fabricated `-1` — which the model would otherwise quote back
    // to the user as the command's exit code.
    let (success, code) = match &ended {
        Ended::Exited(st) => (st.success(), st.code()),
        Ended::TimedOut | Ended::Cancelled => (true, None),
    };
    Ok(ToolOutcome::text(super::present::format_console(
        Some(&header),
        &clip(&strip_ansi(&stdout), limit, ctx.loc),
        &clip(&strip_ansi(&stderr), limit, ctx.loc),
        success,
        code,
        ctx.loc,
    )))
}

/// Spawns the command, drains both pipes concurrently, and ends the **tree** on
/// a timeout or on `Esc`.
///
/// Two things here are deliberate and easy to get wrong:
///
/// - stdout and stderr are drained by tasks of their own. Waiting on the process
///   while a pipe fills is the classic deadlock, and a compiler fills stderr.
/// - partial output is **kept** when the command is killed. The Python sandbox
///   discards it, which is right for a script whose value is its final answer,
///   and wrong for a build, whose first errors arrived in the first second.
async fn spawn_and_wait(
    program: &str,
    args: &[String],
    root: &Path,
    timeout: std::time::Duration,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(Ended, String, String)> {
    use tokio::io::AsyncReadExt;

    // Resolved the way a shell would, so one command line works on both
    // platforms: Rust does not complete a bare name from `PATHEXT` and
    // `cmd.exe` does, which is why `npm test` needs `npm.cmd` on Windows.
    let resolved = crate::shared::mcp::resolve_command(program);
    let mut cmd = match &resolved {
        Some(path) => tokio::process::Command::new(path),
        None => tokio::process::Command::new(program),
    };
    cmd.args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Colour is escape codes in a pipe, and this output is read by a model
        // and drawn by our own renderer. Ask for it to be off rather than only
        // stripping it afterwards.
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window (CREATE_NO_WINDOW): the TUI owns this terminal, and
        // a build popping up a console would repaint over it.
        cmd.creation_flags(0x0800_0000);
    }
    crate::shared::proc::prepare_group(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!(format!("{program}: {e}")))?;
    let mut guard = crate::shared::proc::TreeGuard::assign_group(&child);

    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let out_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf).await;
        buf
    });
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf).await;
        buf
    });

    let ended = tokio::select! {
        status = child.wait() => match status {
            Ok(st) => Ended::Exited(st),
            Err(e) => return Err(anyhow::anyhow!(format!("{program}: {e}"))),
        },
        _ = tokio::time::sleep(timeout) => Ended::TimedOut,
        _ = cancel.cancelled() => Ended::Cancelled,
    };
    if !matches!(ended, Ended::Exited(_)) {
        // The tree, not the process: `cargo` is a launcher, and killing it alone
        // leaves the `rustc` children it started compiling.
        guard.kill();
        let _ = child.start_kill();
    }
    let _ = child.wait().await;
    guard.disarm();
    // The pipes close with the processes holding them, so these finish now — and
    // they carry whatever was printed before the kill.
    let stdout = out_task.await.unwrap_or_default();
    let stderr = err_task.await.unwrap_or_default();
    Ok((
        ended,
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    ))
}

/// Removes ANSI escape sequences from captured output.
///
/// `NO_COLOR` is a convention, not a guarantee: a tool that ignores it would
/// otherwise put raw escapes into the model's context and into a feed that draws
/// them as literal text.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                skip_csi(&mut chars);
            }
            Some(']') => {
                chars.next();
                skip_osc(&mut chars);
            }
            // A two-character escape (or a trailing ESC): drop what follows it.
            _ => {
                chars.next();
            }
        }
    }
    out
}

/// Consumes a CSI sequence's body: parameters and intermediates, then the one
/// final byte in `@`..=`~` that ends it.
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('@'..='~').contains(&c) {
            break;
        }
    }
}

/// Consumes an OSC string's body: it runs to BEL or to ST (`ESC \`). A terminal
/// title is the common case.
fn skip_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == '\u{7}' {
            break;
        }
        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
            chars.next();
            break;
        }
    }
}

/// Caps one stream, keeping the **head and the tail** (design fork F12).
///
/// A compiler puts its first errors at the top and its summary at the bottom,
/// and those are the two things worth reading; a tail-only cut — which is what
/// `python_exec` does, correctly, for a script — would throw away the errors and
/// keep the count of them.
fn clip(s: &str, max: usize, loc: &crate::shared::i18n::Locale) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let head_len = max / 2;
    let tail_len = max - head_len;
    let head: String = s.chars().take(head_len).collect();
    let tail: String = s.chars().skip(total - tail_len).collect();
    let note = loc.tf(
        "tool.code.cmd.truncated",
        &[("n", &(total - max).to_string())],
    );
    format!("{head}\n{note}\n{tail}")
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

    // ---- the command slots (spec §9.12) ----

    /// The command tests run one at a time.
    ///
    /// Not a fixture nicety: [`COMMAND_GATE`] is process-wide by design — one
    /// project command at a time, whichever chat asked — so two tests spawning
    /// commands in parallel make each other fail with the "busy" refusal, and
    /// which one loses depends on the scheduler. Taking this first makes them
    /// queue instead, and the gate's own behaviour is asserted deliberately by
    /// `a_second_command_is_refused_while_one_runs`.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A fixture whose project carries `line` in `slot`.
    fn with_command(slot: CommandSlot, line: &str) -> Fixture {
        let mut f = fixture(&[("a.rs", "fn main() {}\n")]);
        let ws = f.ctx.workspace.as_mut().expect("the fixture attaches one");
        ws.set_command(slot, Some(line.to_string()));
        f
    }

    /// The tool is not offered without a line, so a call that arrives anyway is
    /// a stale schema — and it still has to name **who** can set the line and
    /// with which command, or the model tries another tool instead of telling
    /// the user (docs/lessons.md §4).
    #[tokio::test]
    async fn a_slot_with_no_line_names_the_command_that_fills_it() {
        let f = fixture(&[]);
        for slot in CommandSlot::ALL {
            let err = CodeTool::Command(slot)
                .invoke(&f.ctx, serde_json::json!({}))
                .await
                .expect_err("no line means no run");
            let msg = err.to_string();
            assert!(
                msg.contains(&format!("/project {}-cmd", slot.key())),
                "{slot:?}: {msg}"
            );
        }
    }

    /// The shell check runs at execution time too, not only when the line was
    /// typed: a chat file is JSON on disk and can be edited by hand, and a guard
    /// that only lives at one end of the path is one refactor from being gone
    /// (docs/lessons.md §2). The refusal names the character and the way out.
    #[tokio::test]
    async fn a_pipeline_that_reached_the_tool_is_refused_by_name() {
        let f = with_command(CommandSlot::Build, "cargo build 2>&1 | tee log.txt");
        let err = CodeTool::Command(CommandSlot::Build)
            .invoke(&f.ctx, serde_json::json!({}))
            .await
            .expect_err("a pipeline cannot run without a shell");
        let msg = err.to_string();
        assert!(msg.contains('|') || msg.contains('>'), "{msg}");
        assert!(
            msg.contains("cargo build 2>&1 | tee log.txt"),
            "the line the user typed must be quoted back: {msg}"
        );
        // And nothing was spawned: the file the pipeline would have written
        // must not exist.
        assert!(!f.dir.path().join("log.txt").exists());
    }

    /// The ordinary path: output, exit code and the command line all reach the
    /// model, in the shape `present::parse_console` reads back.
    #[tokio::test]
    async fn a_command_reports_its_output_and_its_exit_code() {
        let _serial = SERIAL.lock().await;
        let Some(py) = crate::shared::proc::test_python() else {
            println!("SKIP: no python interpreter for the command fixture");
            return;
        };
        let f = with_command(
            CommandSlot::Test,
            &format!("{py} -c \"import sys; print('out'); sys.stderr.write('err'); sys.exit(3)\""),
        );
        let out = CodeTool::Command(CommandSlot::Test)
            .invoke(&f.ctx, serde_json::json!({}))
            .await
            .unwrap()
            .result;
        assert!(out.contains("command:"), "{out}");
        assert!(out.contains("out"), "stdout is missing: {out}");
        assert!(out.contains("err"), "stderr is missing: {out}");
        assert!(out.contains('3'), "the exit code is missing: {out}");
        // The whole point of the shape: the feed renders it as a console rather
        // than as flat text.
        let console = super::super::present::present(
            CODE_TEST_ID,
            "{}",
            &out,
            super::super::present::ArgDetail::Compact,
        );
        assert!(
            console
                .result
                .iter()
                .any(|b| matches!(b, super::super::present::ToolBlock::Console(_))),
            "the result must render as a console: {console:?}"
        );
    }

    /// The command runs **in the project**, not wherever the application was
    /// started from. Everything a build command does — finding a manifest,
    /// writing `target/` — depends on it.
    #[tokio::test]
    async fn a_command_runs_in_the_project_root() {
        let _serial = SERIAL.lock().await;
        let Some(py) = crate::shared::proc::test_python() else {
            println!("SKIP: no python interpreter for the command fixture");
            return;
        };
        let f = with_command(
            CommandSlot::Run,
            &format!("{py} -c \"import pathlib; pathlib.Path('here.txt').write_text('x')\""),
        );
        CodeTool::Command(CommandSlot::Run)
            .invoke(&f.ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(
            f.dir.path().join("here.txt").is_file(),
            "the command's working directory was not the project root"
        );
    }

    /// A command that outruns the limit is stopped — and what it printed first
    /// is **kept**. This is the deliberate inverse of the Python sandbox, which
    /// discards partial output: a build's first errors arrive in its first
    /// second, and throwing them away because the build was slow wastes the
    /// whole wait (docs/history/code-workspace.md §3.3).
    #[tokio::test]
    async fn a_timed_out_command_keeps_what_it_printed() {
        let _serial = SERIAL.lock().await;
        let Some(py) = crate::shared::proc::test_python() else {
            println!("SKIP: no python interpreter for the command fixture");
            return;
        };
        let mut f = with_command(
            CommandSlot::Build,
            &format!(
                "{py} -c \"import sys,time; print('early'); sys.stdout.flush(); time.sleep(60)\""
            ),
        );
        f.ctx.workspace_cfg.command_timeout_secs = 1;
        let out = CodeTool::Command(CommandSlot::Build)
            .invoke(&f.ctx, serde_json::json!({}))
            .await
            .unwrap()
            .result;
        assert!(out.contains("early"), "partial output was discarded: {out}");
        // And it says it timed out rather than presenting the fragment as the
        // whole answer.
        let timed_out = f.ctx.loc.tf("tool.code.cmd.timed_out", &[("secs", "1")]);
        assert!(out.contains(&timed_out), "{out}");
        // A killed command has no exit code, so none is reported. `-1` here
        // would be a number we invented, and the model would pass it on to the
        // user as the command's own.
        assert!(
            !out.contains("-1"),
            "a fabricated exit code for a killed command: {out}"
        );
    }

    /// One command at a time, across the whole application. Two builds of one
    /// project would fight over the same `target/`, and the second was never
    /// what the user wanted — so the refusal is the answer, and it says to wait
    /// rather than leaving the model to guess (docs/lessons.md §4).
    #[tokio::test]
    async fn a_second_command_is_refused_while_one_runs() {
        let Some(py) = crate::shared::proc::test_python() else {
            println!("SKIP: no python interpreter for the command fixture");
            return;
        };
        let _serial = SERIAL.lock().await;
        let slow = with_command(
            CommandSlot::Run,
            &format!("{py} -c \"import time; time.sleep(30)\""),
        );
        let mut first = slow;
        first.ctx.workspace_cfg.command_timeout_secs = 1;
        let ctx = first.ctx.clone();
        let running = tokio::spawn(async move {
            CodeTool::Command(CommandSlot::Run)
                .invoke(&ctx, serde_json::json!({}))
                .await
        });
        // Wait until the permit is actually taken, rather than racing the spawn.
        for _ in 0..100 {
            if COMMAND_GATE.available_permits() == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let second = with_command(CommandSlot::Build, &format!("{py} -c \"print(1)\""));
        let err = CodeTool::Command(CommandSlot::Build)
            .invoke(&second.ctx, serde_json::json!({}))
            .await
            .expect_err("the second command must be refused, not queued");
        assert!(
            err.to_string() == second.ctx.loc.t("tool.code.cmd.busy"),
            "the refusal must name the reason: {err}"
        );
        let _ = running.await;
    }

    /// Truncation keeps the head **and** the tail (design fork F12): a compiler
    /// puts its first errors at the top and its summary at the bottom, and a
    /// tail-only cut — right for a script — would keep the count of errors and
    /// throw away the errors.
    #[test]
    fn clipping_keeps_both_ends() {
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let body: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        let clipped = clip(&body, 300, loc);
        assert!(clipped.contains("line 1\n"), "the head is gone: {clipped}");
        assert!(clipped.contains("line 200"), "the tail is gone: {clipped}");
        assert!(
            !clipped.contains("line 100\n"),
            "the middle should have gone instead: {clipped}"
        );
        // Short output passes through untouched — the common case must not grow
        // a note about nothing.
        assert_eq!(clip("short", 300, loc), "short");
    }

    /// `NO_COLOR` is a convention, not a guarantee. Escapes that survive it
    /// would otherwise reach the model's context and be drawn as literal text.
    #[test]
    fn ansi_escapes_are_stripped() {
        assert_eq!(strip_ansi("\u{1b}[31merror\u{1b}[0m: x"), "error: x");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}ok"), "ok");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    /// The escape shapes the common case does not reach: an OSC string closed by
    /// ST (`ESC \`) rather than BEL, a two-character escape, and a sequence cut
    /// off by the output cap — each of which used to be an inline branch of
    /// [`strip_ansi`] and is now its own function.
    #[test]
    fn ansi_stripping_covers_the_less_common_terminators() {
        // OSC closed by ST — what a terminal that follows ECMA-48 emits.
        assert_eq!(strip_ansi("\u{1b}]0;title\u{1b}\\ok"), "ok");
        // A two-character escape (here RIS) takes its second character with it.
        assert_eq!(strip_ansi("a\u{1b}cb"), "ab");
        // Unterminated: a stream clipped mid-sequence must not put the tail back
        // into the model's context.
        assert_eq!(strip_ansi("a\u{1b}[31"), "a");
        assert_eq!(strip_ansi("a\u{1b}]0;title"), "a");
        assert_eq!(strip_ansi("a\u{1b}"), "a");
    }

    /// One matching line is capped by **characters**, so a minified file cannot
    /// spend the whole answer on one of them — and a line under the cap is
    /// returned untouched, note included.
    #[test]
    fn a_grep_hit_is_clipped_by_characters() {
        assert_eq!(clip_hit("short"), "short");
        let long = "п".repeat(MAX_GREP_LINE + 50);
        let clipped = clip_hit(&long);
        assert_eq!(clipped.chars().count(), MAX_GREP_LINE + 1, "the … is extra");
        assert!(clipped.ends_with('…'));
    }
}
