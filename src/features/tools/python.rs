//! `python_exec` tool (spec §9.3, §13.2): executes Python in one of two
//! modes ([`PythonMode`]):
//!
//! - **Wasmer** (default) — an isolated WASIX sandbox behind the `wasmer` sidecar
//!   (`shared::sandbox`): no access to the host filesystem, network via a flag, preinstalled
//!   packages. See docs/research/python-wasmer-sandbox.md.
//! - **Local** — the previous behavior: the system interpreter as a separate process with
//!   a timeout. No OS sandbox (the code runs on the user's machine).
//!
//! The master switch `tools.python_enabled` gates the tool as a whole (off
//! by default). The tool's id (`python_exec`) doesn't depend on the mode.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::entities::attachment::format_bytes;
use crate::entities::chat_file::{ChatFile, is_text_like, sanitize_name, sniff_image};
use crate::entities::profile::ToolId;
use crate::features::chat_files::{self, Stored};
use crate::shared::config::{MAX_TOOL_RESULT_IMAGES, PythonMode};
use crate::shared::i18n::Locale;
use crate::shared::sandbox::{
    OutputFile, OutputLimits, SandboxAvailability, SandboxInput, SandboxJob, SandboxOutput,
    SandboxRunner, SkipReason, SkippedOutput,
};

use super::{ChatEffect, Tool, ToolContext, ToolImage, ToolOutcome};

/// Execution timeout in local mode.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum size of captured output (characters) — protection against a flood.
const MAX_OUTPUT_CHARS: usize = 8000;
/// How much of a text-like output its entry quotes (docs/sandbox-file-exchange.md F5 (c)).
const TEXT_HEAD_BYTES: usize = 1024;

/// `python_exec` — executes the given Python code and returns stdout/stderr.
pub struct PythonExec {
    /// The execution mode (sandbox/local).
    mode: PythonMode,
    /// The interpreter path (Local; `None` → the system `python3`/`python`).
    python_path: Option<String>,
    /// The sandbox implementation (Wasmer).
    sandbox: Arc<dyn SandboxRunner>,
    /// Allow network access in the sandbox (Wasmer).
    net: bool,
    /// Execution timeout in the sandbox (Wasmer).
    wasm_timeout: Duration,
    /// Whether an image the code saved to `/w/out` is shown to the model
    /// (`tools.python_images`, docs/sandbox-file-exchange.md §11 S8). The description reads
    /// it too, so what the model is told and what it gets cannot disagree.
    images: bool,
}

impl PythonExec {
    pub fn new(
        mode: PythonMode,
        python_path: Option<String>,
        sandbox: Arc<dyn SandboxRunner>,
        net: bool,
        wasm_timeout: Duration,
    ) -> Self {
        Self {
            mode,
            python_path,
            sandbox,
            net,
            wasm_timeout,
            images: true,
        }
    }

    /// Sets whether the images the code saves are shown to the model (builder-style).
    pub fn with_images(mut self, images: bool) -> Self {
        self.images = images;
        self
    }

    /// The interpreter's name/path with a sensible platform default (Local).
    fn interpreter(&self) -> String {
        self.python_path.clone().unwrap_or_else(|| {
            if cfg!(windows) {
                "python".to_string()
            } else {
                "python3".to_string()
            }
        })
    }

    /// Local mode: the system interpreter as a separate process. Returns an already-
    /// formatted result text (success/error/timeout) in the language `loc`.
    async fn run_local(&self, code: &str, loc: &crate::shared::i18n::Locale) -> String {
        // The argument is passed directly (no shell) — no escaping issues.
        let mut cmd = tokio::process::Command::new(self.interpreter());
        cmd.arg("-c")
            .arg(code)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Output goes into a pipe, not the console, so Python on Windows picks
            // an encoding by locale (often cp1252) and fails on Cyrillic in `print`
            // (`UnicodeEncodeError`). We read the output as UTF-8, so we
            // also ask Python to write UTF-8. See docs/journal/milestones.md (M7).
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                return loc.tf(
                    "tool.python_exec.err.spawn",
                    &[("py", &self.interpreter()), ("err", &err.to_string())],
                );
            }
        };

        // On a timeout the future is dropped → the process is killed (kill_on_drop).
        match tokio::time::timeout(LOCAL_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(out)) => format_output_parts(
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
                out.status.success(),
                out.status.code(),
                loc,
            ),
            Ok(Err(err)) => loc.tf("tool.python_exec.err.exec", &[("err", &err.to_string())]),
            Err(_) => loc.tf(
                "tool.python_exec.err.timeout",
                &[("secs", &LOCAL_TIMEOUT.as_secs().to_string())],
            ),
        }
    }

    /// Wasmer sandbox mode: the console output, and what the code left in `/w/out` kept in
    /// the chat's folder (docs/sandbox-file-exchange.md §11 S5–S9). The result is in the
    /// profile's language (`ctx.loc`).
    async fn run_wasmer(&self, code: &str, handles: &[String], ctx: &ToolContext) -> ToolOutcome {
        let loc = ctx.loc;
        if let SandboxAvailability::Missing(why) = self.sandbox.availability(loc) {
            return ToolOutcome::text(
                loc.tf("tool.python_exec.err.sandbox_missing", &[("why", &why)]),
            );
        }
        let inputs = match self.stage(handles, ctx) {
            Ok(inputs) => inputs,
            Err(refusal) => return ToolOutcome::text(refusal),
        };
        let out = match self
            .sandbox
            .run(
                SandboxJob::new(code, self.net, self.wasm_timeout).with_inputs(&inputs),
                loc,
            )
            .await
        {
            Ok(out) => out,
            Err(e) => {
                return ToolOutcome::text(
                    loc.tf("tool.python_exec.err.sandbox", &[("e", &e.to_string())]),
                );
            }
        };
        let kept = self.keep_outputs(ctx, &out);
        let result = if out.timed_out {
            let timeout = loc.tf(
                "tool.python_exec.err.timeout",
                &[("secs", &self.wasm_timeout.as_secs().to_string())],
            );
            match kept.section {
                Some(section) => format!("{timeout}\n\n{section}"),
                None => timeout,
            }
        } else {
            let console = || {
                format_output_parts(
                    &out.stdout,
                    &out.stderr,
                    out.exit_code == Some(0),
                    out.exit_code,
                    loc,
                )
            };
            // A run that printed nothing and saved files opens with what it saved: the
            // "(empty output, success)" line would say nothing the section does not.
            let quiet = out.stdout.trim().is_empty()
                && out.stderr.trim().is_empty()
                && out.exit_code == Some(0);
            match kept.section {
                Some(section) if quiet => section,
                Some(section) => format!("{}\n\n{section}", console()),
                None => console(),
            }
        };
        ToolOutcome::with_effects(result, kept.effects).with_images(kept.images)
    }

    /// Resolves the files a call named against the chat's one numbered list
    /// (docs/sandbox-file-exchange.md §12 T2) and turns them into copies for `/w/in`.
    ///
    /// `Err` is the refusal the model gets **instead of a run**: an unknown handle, a name
    /// several files share, a listed file whose copy is gone. Nothing is staged and
    /// nothing is executed, so a script cannot answer confidently from three of the four
    /// files it asked for (§12 T7, lessons §4). A handle named twice is staged once.
    fn stage(&self, handles: &[String], ctx: &ToolContext) -> Result<Vec<SandboxInput>, String> {
        use crate::entities::attachment::Resolved;
        use crate::features::chat_inputs;

        if handles.is_empty() {
            return Ok(Vec::new());
        }
        let loc = ctx.loc;
        let dir = ctx.files_dir.clone().unwrap_or_default();
        let items = chat_inputs::items(
            &ctx.attachments,
            &ctx.files,
            &ctx.images.iter().collect::<Vec<_>>(),
            &dir,
        );
        let mut inputs = Vec::new();
        let mut taken: Vec<usize> = Vec::new();
        for handle in handles {
            let at = match chat_inputs::resolve(&items, handle) {
                Resolved::One(at) => at,
                Resolved::Shared(hits) => {
                    let candidates = hits
                        .iter()
                        .map(|&i| format!("\n• #{} {} — {}", i + 1, items[i].name, items[i].source))
                        .collect::<String>();
                    return Err(loc.tf(
                        "tool.python_exec.err.files_shared",
                        &[("handle", handle.trim()), ("candidates", &candidates)],
                    ));
                }
                Resolved::Nothing => {
                    return Err(loc.tf(
                        "tool.python_exec.err.files_unknown",
                        &[("handle", handle.trim()), ("files", &known_files(&items))],
                    ));
                }
            };
            if taken.contains(&at) {
                continue; // named twice — one copy, one name in `/w/in`
            }
            taken.push(at);
            inputs.push(self.input_for(&items[at], ctx)?);
        }
        Ok(inputs)
    }

    /// One resolved item as a copy for `/w/in`: a file from the chat's folder is copied
    /// without being read here, while an attachment's text and an image's pixels are bytes
    /// the context already holds (§12 T12).
    fn input_for(
        &self,
        item: &crate::features::chat_inputs::ChatInput,
        ctx: &ToolContext,
    ) -> Result<SandboxInput, String> {
        let loc = ctx.loc;
        if let Some(name) = &item.file {
            let Some(dir) = &ctx.files_dir else {
                return Err(loc.t("tool.python_exec.err.files_no_folder").to_string());
            };
            let path = dir.join(name);
            if !path.is_file() {
                return Err(loc.tf("tool.python_exec.err.files_missing", &[("name", name)]));
            }
            return Ok(SandboxInput::path(item.staged.clone(), path));
        }
        if let Some(at) = item.attachment {
            let text = ctx.attachments[at].text.clone();
            return Ok(SandboxInput::bytes(item.staged.clone(), text.into_bytes()));
        }
        let at = item
            .image
            .expect("an item is a file, an attachment or an image");
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&ctx.images[at].data)
            .map_err(|_| {
                loc.tf(
                    "tool.python_exec.err.files_missing",
                    &[("name", &item.name)],
                )
            })?;
        Ok(SandboxInput::bytes(item.staged.clone(), bytes))
    }

    /// Stores what the call left in `/w/out` in the chat's folder and says what became of
    /// each entry: the result's `files:` section (§11 S9), a listing effect per new file
    /// (S7), and the images the model is shown (S8). Nothing when `/w/out` held nothing.
    fn keep_outputs(&self, ctx: &ToolContext, out: &SandboxOutput) -> Kept {
        let loc = ctx.loc;
        let mut kept = Kept::default();
        if out.files.is_empty() && out.skipped.is_empty() {
            return kept;
        }
        let mut lines = Vec::new();
        match &ctx.files_dir {
            Some(dir) => {
                if !out.files.is_empty() {
                    lines.push(loc.tf(
                        "tool.python_exec.files.saved_in",
                        &[("dir", &dir.display().to_string())],
                    ));
                }
                let mut listed = ctx.files.to_vec();
                for file in &out.files {
                    lines.extend(self.keep_one(loc, dir, &mut listed, file, &mut kept));
                }
            }
            None => lines.push(loc.t("tool.python_exec.files.no_folder").to_string()),
        }
        lines.extend(out.skipped.iter().map(|s| skipped_line(loc, s)));
        kept.section = Some(format!("files:\n{}", lines.join("\n")));
        kept
    }

    /// Keeps one collected file under a sanitized, free name and returns its lines: the
    /// entry, then the head of a text-like file. `listed` grows by what was stored, so a
    /// second file of the same call versions against the first.
    fn keep_one(
        &self,
        loc: &Locale,
        dir: &std::path::Path,
        listed: &mut Vec<ChatFile>,
        file: &OutputFile,
        kept: &mut Kept,
    ) -> Vec<String> {
        let Some(name) = sanitize_name(&file.name) else {
            return vec![skipped_with(
                loc,
                &file.name,
                loc.t("tool.python_exec.files.reason.bad_name"),
            )];
        };
        let stored = match chat_files::store(dir, listed, &name, &file.bytes) {
            Ok(Stored::New(stored)) => stored,
            Ok(Stored::Unchanged(existing)) => {
                return vec![loc.tf(
                    "tool.python_exec.files.unchanged",
                    &[("name", &file.name), ("stored", &existing.name)],
                )];
            }
            Err(e) => {
                let reason = loc.tf(
                    "tool.python_exec.files.reason.write_failed",
                    &[("err", &e.to_string())],
                );
                return vec![skipped_with(loc, &file.name, &reason)];
            }
        };
        let size = format_bytes(file.bytes.len());
        let mut entry = if stored.name == file.name {
            loc.tf(
                "tool.python_exec.files.item",
                &[
                    ("name", &stored.name),
                    ("size", &size),
                    ("mime", &stored.mime),
                ],
            )
        } else {
            loc.tf(
                "tool.python_exec.files.renamed",
                &[
                    ("name", &file.name),
                    ("stored", &stored.name),
                    ("size", &size),
                    ("mime", &stored.mime),
                ],
            )
        };
        if let Some(mime) = sniff_image(&file.bytes) {
            if !self.images {
                entry.push_str(loc.t("tool.python_exec.files.not_shown_off"));
            } else if kept.images.len() >= MAX_TOOL_RESULT_IMAGES {
                entry.push_str(&loc.tf(
                    "tool.python_exec.files.not_shown_cap",
                    &[("max", &MAX_TOOL_RESULT_IMAGES.to_string())],
                ));
            } else {
                use base64::Engine as _;
                kept.images.push(ToolImage {
                    mime: mime.to_string(),
                    data: base64::engine::general_purpose::STANDARD.encode(&file.bytes),
                });
                entry.push_str(loc.t("tool.python_exec.files.shown"));
            }
        } else if stored.mime == "image/svg+xml" {
            entry.push_str(loc.t("tool.python_exec.files.svg"));
        }
        let mut lines = vec![entry];
        if is_text_like(&stored.mime) {
            lines.extend(text_head(&file.bytes, TEXT_HEAD_BYTES));
        }
        listed.push(stored.clone());
        kept.effects.push(ChatEffect::AddChatFile(Box::new(stored)));
        lines
    }
}

#[async_trait::async_trait]
impl Tool for PythonExec {
    fn id(&self) -> ToolId {
        super::PYTHON_EXEC_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::ExternalWorld
    }
    fn ui_label(&self) -> &'static str {
        "run Python"
    }
    /// Arbitrary code. The sandbox bounds what it can reach (ADR 0005), not
    /// what it does with the network or the mounted directory.
    fn danger(&self) -> bool {
        true
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Python)
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        match self.mode {
            PythonMode::Local => loc.t("tool.python_exec.desc.local").into(),
            PythonMode::Wasmer => {
                let net = if self.net {
                    loc.t("tool.python_exec.net.on")
                } else {
                    loc.t("tool.python_exec.net.off")
                };
                // What `/w/out` does, with the caps and the images sentence built from the
                // same values the run uses (docs/sandbox-file-exchange.md §11 S10).
                let limits = OutputLimits::DEFAULT;
                let images = if self.images {
                    loc.t("tool.python_exec.desc.images_on")
                } else {
                    loc.t("tool.python_exec.desc.images_off")
                };
                let files = loc.tf(
                    "tool.python_exec.desc.files",
                    &[
                        ("files", &limits.max_files.to_string()),
                        ("file", &format_bytes(limits.max_file_bytes as usize)),
                        ("total", &format_bytes(limits.max_total_bytes as usize)),
                        ("images", images),
                    ],
                );
                format!(
                    "{} {} {files}",
                    loc.tf("tool.python_exec.desc.wasmer", &[("net", net)]),
                    loc.t("tool.python_exec.desc.inputs"),
                )
            }
        }
    }
    /// `files` exists in Wasmer mode only: Local runs no job directory until stage 5's
    /// parity (F11), and an argument the mode cannot honour is worse than none
    /// (docs/sandbox-file-exchange.md §12 T10; ADR 0005 §3 records the divergence).
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        let mut properties = serde_json::json!({"code": {"type": "string"}});
        if matches!(self.mode, PythonMode::Wasmer) {
            properties["files"] = serde_json::json!({
                "type": "array",
                "items": {"type": "string"},
                "description": loc.t("tool.python_exec.param.files"),
            });
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": ["code"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let code = args
            .get("code")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.python_exec.err.code_empty")))?
            .to_string();

        // The chat's files this call wants in `/w/in` (§12 T2): names or `#N`, as the
        // chat's file list numbers them.
        let files: Vec<String> = args
            .get("files")
            .and_then(|v| v.as_array())
            .map(|named| {
                named
                    .iter()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .filter(|f| !f.trim().is_empty())
                    .collect()
            })
            .unwrap_or_default();

        Ok(match self.mode {
            PythonMode::Local => ToolOutcome::text(self.run_local(&code, ctx.loc).await),
            PythonMode::Wasmer => self.run_wasmer(&code, &files, ctx).await,
        })
    }
}

/// Formats the execution result (stdout/stderr/exit code) — a shared shape for
/// both modes, so the feed's presenter (`present::parse_console`) recognizes the
/// console by its labels.
///
/// The assembly itself lives in `present::format_console`, next to the parser
/// that reads it back; what belongs here is the **truncation**, the one part
/// that differs between the two producers of this shape: a script's output is
/// cut at the tail, while a build keeps its head *and* its tail
/// (docs/history/code-workspace.md, fork F12).
fn format_output_parts(
    stdout: &str,
    stderr: &str,
    success: bool,
    code: Option<i32>,
    loc: &crate::shared::i18n::Locale,
) -> String {
    super::present::format_console(
        None,
        &truncate(stdout, MAX_OUTPUT_CHARS, loc),
        &truncate(stderr, MAX_OUTPUT_CHARS, loc),
        success,
        code,
        loc,
    )
}

/// Truncates a string to `max` characters with a truncation note (in the language `loc`).
fn truncate(s: &str, max: usize, loc: &crate::shared::i18n::Locale) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}\n{}", loc.t("python.truncated"))
    }
}

/// What keeping a call's outputs produced.
#[derive(Default)]
struct Kept {
    /// The result's `files:` section; `None` when `/w/out` held nothing.
    section: Option<String>,
    effects: Vec<ChatEffect>,
    images: Vec<ToolImage>,
}

/// The `not kept` entry of an output the sandbox did not collect, with its reason — a
/// folder named as one, so the fix ("directly into /w/out") reads off the name.
fn skipped_line(loc: &Locale, skipped: &SkippedOutput) -> String {
    let limits = OutputLimits::DEFAULT;
    let reason = match skipped.reason {
        SkipReason::Directory => loc.t("tool.python_exec.files.reason.directory").to_string(),
        SkipReason::NotAFile => loc
            .t("tool.python_exec.files.reason.not_a_file")
            .to_string(),
        SkipReason::TooLarge => loc.tf(
            "tool.python_exec.files.reason.too_large",
            &[("max", &format_bytes(limits.max_file_bytes as usize))],
        ),
        SkipReason::TooMany => loc.tf(
            "tool.python_exec.files.reason.too_many",
            &[("max", &limits.max_files.to_string())],
        ),
        SkipReason::OverTotal => loc.tf(
            "tool.python_exec.files.reason.over_total",
            &[("max", &format_bytes(limits.max_total_bytes as usize))],
        ),
        SkipReason::Unreadable => loc
            .t("tool.python_exec.files.reason.unreadable")
            .to_string(),
        SkipReason::TimedOut => loc.t("tool.python_exec.files.reason.timed_out").to_string(),
    };
    let name = if skipped.reason == SkipReason::Directory {
        format!("{}/", skipped.name)
    } else {
        skipped.name.clone()
    };
    skipped_with(loc, &name, &reason)
}

/// What this chat *does* have, listed for a handle that reached nothing: `#N`, the name
/// and the size, so the model can name one of them instead of guessing again (lessons §4).
/// Data, not prose — the numbering is the same one `/file list` shows the user.
fn known_files(items: &[crate::features::chat_inputs::ChatInput]) -> String {
    if items.is_empty() {
        return String::new();
    }
    items
        .iter()
        .map(|i| {
            format!(
                "\n• #{} {} ({})",
                i.handle,
                i.name,
                format_bytes(i.bytes as usize)
            )
        })
        .collect()
}

/// One `not kept` entry.
fn skipped_with(loc: &Locale, name: &str, reason: &str) -> String {
    loc.tf(
        "tool.python_exec.files.skipped",
        &[("name", name), ("reason", reason)],
    )
}

/// The first lines of a text-like output, quoted under its entry (F5 (c)): at most `max`
/// bytes, whole lines when the file holds more, each behind `  | ` — never mistaken for a
/// section label — and `  | …` when cut. Nothing for bytes holding a NUL: not text,
/// whatever the extension says.
fn text_head(bytes: &[u8], max: usize) -> Vec<String> {
    let cut = bytes.len() > max;
    let head = &bytes[..bytes.len().min(max)];
    if head.contains(&0) {
        return Vec::new();
    }
    let decoded = String::from_utf8_lossy(head);
    let text: &str = match decoded.rfind('\n') {
        Some(end) if cut => &decoded[..end],
        _ => &decoded,
    };
    let mut lines: Vec<String> = text
        .trim_end()
        .lines()
        .map(|l| format!("  | {l}"))
        .collect();
    if cut && !lines.is_empty() {
        lines.push("  | …".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{ctx_with_storage, ctx_with_storage_lang};
    use super::*;
    use crate::shared::i18n::Lang;
    use crate::shared::sandbox::{MockSandbox, SandboxOutput};
    use uuid::Uuid;

    /// Whether the string has no Cyrillic (Russian leaking on an en profile).
    fn no_cyrillic(s: &str) -> bool {
        !s.chars()
            .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё')
    }

    /// Reference locale (ru) for direct calls to output formatting.
    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// The tool in local mode with a given interpreter path.
    fn local(python_path: Option<String>) -> PythonExec {
        PythonExec::new(
            PythonMode::Local,
            python_path,
            Arc::new(MockSandbox::missing("не должно вызываться в Local")),
            false,
            Duration::from_secs(30),
        )
    }

    /// The tool in sandbox mode with a given mock runner.
    fn wasmer(sandbox: Arc<dyn SandboxRunner>, net: bool) -> PythonExec {
        PythonExec::new(
            PythonMode::Wasmer,
            None,
            sandbox,
            net,
            Duration::from_secs(30),
        )
    }

    #[tokio::test]
    async fn rejects_empty_code() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            local(None)
                .invoke(&ctx, serde_json::json!({"code": "   "}))
                .await
                .is_err()
        );
    }

    #[test]
    fn truncate_marks_cut() {
        let long = "a".repeat(MAX_OUTPUT_CHARS + 10);
        let out = truncate(&long, MAX_OUTPUT_CHARS, ru());
        assert!(out.contains("вывод обрезан"));
    }

    #[test]
    fn format_output_parts_shapes_console() {
        let s = format_output_parts("hi", "oops", false, Some(2), ru());
        assert!(s.contains("stdout:\nhi"));
        assert!(s.contains("stderr:\noops"));
        assert!(s.contains("код возврата: 2"));
        assert_eq!(
            format_output_parts("", "", true, Some(0), ru()),
            "(пустой вывод, успех)"
        );
    }

    #[tokio::test]
    async fn missing_interpreter_reports_error_not_panic() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let tool = local(Some("definitely-not-a-real-python-xyz".into()));
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(1)"}))
            .await
            .unwrap();
        assert!(out.result.contains("Не удалось запустить Python"));
    }

    #[tokio::test]
    async fn wasmer_mode_dispatches_to_sandbox_and_formats() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let sb = Arc::new(MockSandbox::ready(SandboxOutput {
            stdout: "42\n".into(),
            stderr: String::new(),
            exit_code: Some(0),
            timed_out: false,
            ..Default::default()
        }));
        let tool = wasmer(sb.clone(), true);
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(6*7)"}))
            .await
            .unwrap();
        assert!(out.result.contains("stdout:\n42"));
        // The runner is called exactly once, with the net flag.
        let calls = sb.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1, "the net flag must be forwarded to the runner");
        assert!(calls[0].0.contains("print(6*7)"));
    }

    #[tokio::test]
    async fn wasmer_mode_timeout_message() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let sb = Arc::new(MockSandbox::ready(SandboxOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            timed_out: true,
            ..Default::default()
        }));
        let out = wasmer(sb, false)
            .invoke(&ctx, serde_json::json!({"code": "while True: pass"}))
            .await
            .unwrap();
        assert!(out.result.contains("превысил лимит времени"));
    }

    #[tokio::test]
    async fn wasmer_mode_missing_sandbox_explains() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let tool = wasmer(Arc::new(MockSandbox::missing("нет бинаря")), true);
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(1)"}))
            .await
            .unwrap();
        assert!(out.result.contains("Песочница Python недоступна"));
        assert!(out.result.contains("нет бинаря"));
    }

    #[test]
    fn description_varies_by_mode_and_net() {
        assert!(
            local(None)
                .description(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru))
                .contains("локальный интерпретатор")
        );
        let sb: Arc<dyn SandboxRunner> = Arc::new(MockSandbox::missing("x"));
        assert!(
            wasmer(sb.clone(), true)
                .description(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru))
                .contains("есть доступ в сеть")
        );
        assert!(
            wasmer(sb, false)
                .description(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru))
                .contains("без доступа в сеть")
        );
    }

    /// Every call gets a fresh `JobDir` and the guest's `/tmp` dies with the
    /// process, so nothing survives between calls — and the description used to
    /// say only "no access to the machine's files". The model in the transcript
    /// that prompted this (docs/history/fetch-url-fidelity.md, P3) wrote a 16 MB
    /// download to `/tmp` and lost it: one wasted round plus 16 MB fetched twice.
    /// Naming `/tmp` is the load-bearing part — that is the path a model reaches
    /// for — so the claim is pinned in every built-in locale.
    #[test]
    fn the_sandbox_description_says_state_does_not_survive_a_call() {
        let sb: Arc<dyn SandboxRunner> = Arc::new(MockSandbox::missing("x"));
        for lang in Lang::ALL {
            let d = wasmer(sb.clone(), true).description(crate::shared::i18n::locale(*lang));
            assert!(d.contains("/tmp"), "{lang:?} does not name /tmp: {d}");
        }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot-really-pixels";

    /// A context whose chat keeps its files in a temp folder, on an en profile (the
    /// assertions read English).
    fn ctx_with_folder() -> (tempfile::TempDir, tempfile::TempDir, ToolContext) {
        let (dir, _storage, mut ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
        let folder = tempfile::tempdir().unwrap();
        ctx.files_dir = Some(folder.path().to_path_buf());
        (dir, folder, ctx)
    }

    fn out_file(name: &str, bytes: &[u8]) -> OutputFile {
        OutputFile {
            name: name.into(),
            bytes: bytes.to_vec(),
        }
    }

    fn stored_names(out: &ToolOutcome) -> Vec<String> {
        out.effects
            .iter()
            .filter_map(|e| match e {
                ChatEffect::AddChatFile(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect()
    }

    async fn run_mock(tool: PythonExec, ctx: &ToolContext) -> ToolOutcome {
        tool.invoke(ctx, serde_json::json!({"code": "print(1)"}))
            .await
            .unwrap()
    }

    fn saved(files: Vec<OutputFile>) -> Arc<MockSandbox> {
        Arc::new(MockSandbox::ready(SandboxOutput {
            exit_code: Some(0),
            files,
            ..Default::default()
        }))
    }

    #[tokio::test]
    async fn outputs_are_stored_listed_and_an_image_is_shown() {
        let (_d, folder, ctx) = ctx_with_folder();
        let sb = Arc::new(MockSandbox::ready(SandboxOutput {
            stdout: "done\n".into(),
            exit_code: Some(0),
            files: vec![
                out_file("chart.png", PNG),
                out_file("totals.csv", b"month,total\n2024-01,7\n"),
            ],
            skipped: vec![SkippedOutput {
                name: "charts".into(),
                reason: SkipReason::Directory,
            }],
            ..Default::default()
        }));
        let out = run_mock(wasmer(sb, false), &ctx).await;
        let r = &out.result;
        assert!(r.starts_with("stdout:\ndone"), "{r}");
        assert!(r.contains("\n\nfiles:\n"), "{r}");
        assert!(r.contains(&folder.path().display().to_string()), "{r}");
        assert!(r.contains("image/png — shown to you below"), "{r}");
        assert!(r.contains("  | month,total\n  | 2024-01,7"), "{r}");
        assert!(r.contains("- charts/ — not kept: a folder"), "{r}");
        assert_eq!(stored_names(&out), ["chart.png", "totals.csv"]);
        assert_eq!(out.images.len(), 1);
        assert_eq!(out.images[0].mime, "image/png");
        assert_eq!(std::fs::read(folder.path().join("chart.png")).unwrap(), PNG);
    }

    /// §10's finding, pinned: when an image is not shown the result says so in words, or
    /// the model describes a chart it has not seen.
    #[tokio::test]
    async fn with_images_off_the_file_is_kept_and_the_model_is_told_it_has_not_seen_it() {
        let (_d, folder, ctx) = ctx_with_folder();
        let tool = wasmer(saved(vec![out_file("chart.png", PNG)]), false).with_images(false);
        let out = run_mock(tool, &ctx).await;
        assert!(out.images.is_empty());
        assert!(
            out.result.contains("you have not seen it"),
            "{}",
            out.result
        );
        assert_eq!(stored_names(&out), ["chart.png"]);
        assert!(folder.path().join("chart.png").exists());
    }

    #[tokio::test]
    async fn at_most_four_images_are_shown_and_the_one_past_the_cap_says_so() {
        let (_d, _folder, ctx) = ctx_with_folder();
        let files = (1u8..=5)
            .map(|i| out_file(&format!("{i}.png"), &[PNG, &[i]].concat()))
            .collect();
        let out = run_mock(wasmer(saved(files), false), &ctx).await;
        assert_eq!(out.images.len(), MAX_TOOL_RESULT_IMAGES);
        assert_eq!(
            out.result
                .matches("at most 4 images are shown per call")
                .count(),
            1,
            "{}",
            out.result
        );
        assert_eq!(stored_names(&out).len(), 5);
    }

    #[tokio::test]
    async fn a_run_that_printed_nothing_but_saved_a_file_opens_with_the_section() {
        let (_d, _folder, ctx) = ctx_with_folder();
        let out = run_mock(wasmer(saved(vec![out_file("a.txt", b"hi")]), false), &ctx).await;
        assert!(out.result.starts_with("files:\n"), "{}", out.result);
        assert!(!out.result.contains("empty output"), "{}", out.result);
    }

    #[tokio::test]
    async fn a_taken_name_is_versioned_and_the_same_bytes_are_not_saved_or_shown_twice() {
        let (_d, folder, mut ctx) = ctx_with_folder();
        let older = ChatFile::new(
            "chart.png",
            crate::entities::chat_file::FileOrigin::Sandbox,
            b"older",
        );
        std::fs::write(folder.path().join("chart.png"), b"older").unwrap();
        ctx.files = Arc::from(vec![older.clone()]);
        let sb = saved(vec![out_file("chart.png", PNG)]);
        let out = run_mock(wasmer(sb.clone(), false), &ctx).await;
        assert!(
            out.result
                .contains("- chart.png → saved as chart (2).png — "),
            "{}",
            out.result
        );
        let Some(ChatEffect::AddChatFile(stored)) = out.effects.first() else {
            panic!("expected a listing: {:?}", out.effects);
        };
        // The turn's mirror, as the loop keeps it.
        ctx.files = Arc::from(vec![older, (**stored).clone()]);
        let again = run_mock(wasmer(sb, false), &ctx).await;
        assert!(again.effects.is_empty(), "{:?}", again.effects);
        assert!(again.images.is_empty());
        assert!(again.result.contains("unchanged"), "{}", again.result);
    }

    #[tokio::test]
    async fn a_timed_out_run_keeps_nothing_and_names_what_it_left() {
        let (_d, _folder, ctx) = ctx_with_folder();
        let sb = Arc::new(MockSandbox::ready(SandboxOutput {
            timed_out: true,
            skipped: vec![SkippedOutput {
                name: "half.png".into(),
                reason: SkipReason::TimedOut,
            }],
            ..Default::default()
        }));
        let out = run_mock(wasmer(sb, false), &ctx).await;
        assert!(
            out.result.contains("exceeded the time limit"),
            "{}",
            out.result
        );
        assert!(
            out.result
                .contains("- half.png — not kept: the call timed out"),
            "{}",
            out.result
        );
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn without_a_chat_folder_nothing_is_kept_and_the_result_says_so() {
        let (_d, _s, ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
        let out = run_mock(wasmer(saved(vec![out_file("a.png", PNG)]), false), &ctx).await;
        assert!(
            out.result.contains("Nothing saved to /w/out was kept"),
            "{}",
            out.result
        );
        assert!(out.effects.is_empty() && out.images.is_empty());
    }

    #[tokio::test]
    async fn a_reserved_name_is_saved_renamed_and_an_svg_is_only_saved() {
        let (_d, folder, ctx) = ctx_with_folder();
        let files = vec![
            out_file("CON.txt", b"x"),
            out_file("drawing.svg", b"<svg/>"),
        ];
        let out = run_mock(wasmer(saved(files), false), &ctx).await;
        assert!(
            out.result.contains("- CON.txt → saved as _CON.txt"),
            "{}",
            out.result
        );
        assert!(
            out.result.contains("an SVG is not shown to you"),
            "{}",
            out.result
        );
        assert!(out.images.is_empty());
        assert!(folder.path().join("_CON.txt").exists());
    }

    #[test]
    fn the_description_names_the_output_folder_its_caps_and_whether_images_are_shown() {
        let sb: Arc<dyn SandboxRunner> = Arc::new(MockSandbox::missing("x"));
        for lang in Lang::ALL {
            let loc = crate::shared::i18n::locale(*lang);
            let on = wasmer(sb.clone(), false).description(loc);
            let off = wasmer(sb.clone(), false)
                .with_images(false)
                .description(loc);
            for d in [&on, &off] {
                assert!(d.contains("/w/out"), "{lang:?}: {d}");
                assert!(d.contains("matplotlib"), "{lang:?}: {d}");
                assert!(
                    d.contains("25.0 MB") && d.contains("50.0 MB"),
                    "{lang:?}: {d}"
                );
            }
            assert!(on.contains("savefig('/w/out/"), "{lang:?}: {on}");
            assert!(!off.contains("savefig('/w/out/"), "{lang:?}: {off}");
        }
    }

    #[test]
    fn a_text_head_quotes_whole_lines_and_marks_a_cut() {
        assert_eq!(text_head(b"a,b\r\n1,2\n", 1024), ["  | a,b", "  | 1,2"]);
        let long = "row\n".repeat(400);
        assert_eq!(
            text_head(long.as_bytes(), 10),
            ["  | row", "  | row", "  | …"]
        );
        assert!(text_head(b"PK\x03\x04\0\0", 1024).is_empty());
    }

    /// Real execution in the sandbox (manual): requires an installed `wasmer`
    /// (env `MINDFORK_SANDBOX_WASMER` or a binary in `data/sandbox/`) and network for the
    /// first download of `python/python`. `cargo test -- --ignored`.
    ///
    /// Still **not** a skip when the sidecar is absent (the 2026-07-24 decision):
    /// it provisions one instead. That preserves the point of that decision —
    /// this smoke always really runs the sandbox — while removing the part that
    /// was only ever a nuisance, failing on a machine that simply never ran
    /// `mindfork sandbox setup`. A provisioning failure is still loud.
    #[tokio::test]
    #[ignore = "runs the real wasmer sidecar; provisions one (~250 MB) if absent"]
    async fn runs_real_python_in_sandbox() {
        use crate::shared::sandbox::WasmerSandbox;
        let (_guard, dir) = ensure_sandbox().await;
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let tool = PythonExec::new(
            PythonMode::Wasmer,
            None,
            Arc::new(WasmerSandbox::new(dir)),
            false,
            Duration::from_secs(120),
        );
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print('hello sandbox')"}))
            .await
            .unwrap();
        assert!(out.result.contains("hello sandbox"), "got: {}", out.result);
    }

    /// A usable sandbox directory for [`runs_real_python_in_sandbox`], provisioning
    /// one if the machine has none. Returns the temp-dir guard (dropped → deleted)
    /// and the directory to hand to [`WasmerSandbox::new`].
    ///
    /// Order, cheapest first:
    /// 1. `MINDFORK_SANDBOX_WASMER` — an explicit binary; `WasmerSandbox` finds it
    ///    on its own, so nothing to provision and nothing to clean up.
    /// 2. `MINDFORK_SANDBOX_DIR` — the user named a location, so it doubles as a
    ///    cache: provision into it if empty, and **keep** it for the next run.
    /// 3. The app's own `data/sandbox`, **read-only**: used when it already holds
    ///    a `wasmer`, never written to. Running a test must not leave 250 MB in
    ///    the working data directory as a side effect.
    /// 4. Otherwise download into a temp directory and delete it afterwards.
    ///
    /// Note the cost of (4): ~250 MB and a warmup compile, every run. Setting
    /// `MINDFORK_SANDBOX_DIR` (or running `mindfork sandbox setup` once) turns
    /// this smoke back into a few seconds.
    async fn ensure_sandbox() -> (Option<tempfile::TempDir>, Option<std::path::PathBuf>) {
        use crate::shared::sandbox::locate_wasmer;

        if std::env::var_os("MINDFORK_SANDBOX_WASMER").is_some_and(|v| !v.is_empty()) {
            return (None, None);
        }

        let named = std::env::var("MINDFORK_SANDBOX_DIR")
            .ok()
            .filter(|d| !d.is_empty())
            .map(std::path::PathBuf::from);
        if let Some(dir) = &named
            && locate_wasmer(dir).is_some()
        {
            return (None, Some(dir.clone()));
        }
        // Read-only candidates: use one if it is already provisioned, never
        // write to it. The second entry matters more than it looks — a test
        // binary lives in `target/<profile>/deps/`, so resolving from
        // `current_exe()` looks for `deps/data/sandbox` and misses the real
        // `target/<profile>/data/sandbox` the app itself uses. That is why this
        // smoke used to fail on a machine that *did* have a sandbox installed.
        if named.is_none() {
            let mut candidates = Vec::new();
            if let Ok(paths) = crate::shared::paths::Paths::resolve() {
                candidates.push(paths.sandbox_dir());
            }
            if let Ok(exe) = std::env::current_exe()
                && let Some(profile_dir) = exe.parent().and_then(|deps| deps.parent())
            {
                candidates.push(profile_dir.join("data").join("sandbox"));
            }
            if let Some(found) = candidates.into_iter().find(|d| locate_wasmer(d).is_some()) {
                return (None, Some(found));
            }
        }

        let (guard, dir) = match named {
            Some(dir) => (None, dir),
            None => {
                let tmp = tempfile::tempdir().unwrap();
                let dir = tmp.path().to_path_buf();
                (Some(tmp), dir)
            }
        };
        eprintln!("provisioning a sandbox into {} (~250 MB)…", dir.display());
        crate::features::sandbox_setup::setup(
            &dir,
            &crate::features::sandbox_setup::SetupOptions::default(),
            // English: this is developer-facing progress in a test log.
            crate::shared::i18n::locale(crate::shared::i18n::Lang::En),
            |line| eprintln!("  {line}"),
        )
        .await
        .expect("provisioning the sandbox for the smoke");
        (guard, Some(dir))
    }

    /// The provisioned sandbox directory (`mindfork sandbox setup`) from env
    /// `MINDFORK_SANDBOX_DIR` (holding wasmer-dist/python.webc/site-packages).
    /// **Announces the skip**: without it these smokes return early and still
    /// report `ok`, which reads as a real run in the summary — every other
    /// env-gated smoke prints a skip line, so these do too.
    fn sandbox_dir_from_env() -> Option<String> {
        let dir = std::env::var("MINDFORK_SANDBOX_DIR").ok();
        if dir.is_none() {
            eprintln!("skip: MINDFORK_SANDBOX_DIR not set");
        }
        dir
    }

    /// The tool over a **provisioned** sandbox. `None` — the env isn't set, the
    /// smoke is skipped.
    fn provisioned(net: bool, timeout_secs: u64) -> Option<PythonExec> {
        use crate::shared::sandbox::WasmerSandbox;
        let dir = sandbox_dir_from_env()?;
        Some(PythonExec::new(
            PythonMode::Wasmer,
            None,
            Arc::new(WasmerSandbox::new(Some(std::path::PathBuf::from(dir)))),
            net,
            Duration::from_secs(timeout_secs),
        ))
    }

    /// numpy (native `.so` via WASIX dynamic linking) in a provisioned sandbox.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn numpy_in_sandbox() {
        let Some(tool) = provisioned(false, 120) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let code = "import numpy as np; print('numpy', np.__version__); \
                    print('sum', int(np.arange(10).sum()))";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(out.result.contains("numpy 2."), "got: {}", out.result);
        assert!(out.result.contains("sum 45"), "got: {}", out.result);
    }

    /// pandas (a native wasix wheel + pure dependencies) in a provisioned sandbox.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn pandas_in_sandbox() {
        let Some(tool) = provisioned(false, 120) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let code = "import pandas as pd; \
                    df = pd.DataFrame({'a': [1, 2, 3], 'b': [4, 5, 6]}); \
                    print('pandas', pd.__version__); print('total', int(df.values.sum()))";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(out.result.contains("pandas 2."), "got: {}", out.result);
        assert!(out.result.contains("total 21"), "got: {}", out.result);
    }

    /// beautifulsoup4 (pure Python + soupsieve for CSS selectors) in a provisioned
    /// sandbox — parses offline, so no network access is needed.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn beautifulsoup_in_sandbox() {
        let Some(tool) = provisioned(false, 120) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        // `select` exercises soupsieve, the dependency most likely to be missing.
        let code = "import bs4\n\
                    from bs4 import BeautifulSoup\n\
                    html = '<html><body><p class=\"x\">hi</p><p>bye</p></body></html>'\n\
                    soup = BeautifulSoup(html, 'html.parser')\n\
                    print('bs4', bs4.__version__)\n\
                    print('text', soup.p.get_text())\n\
                    print('select', len(soup.select('p.x')))";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(out.result.contains("bs4 4."), "got: {}", out.result);
        assert!(out.result.contains("text hi"), "got: {}", out.result);
        assert!(out.result.contains("select 1"), "got: {}", out.result);
    }

    /// Runs `code` in a provisioned sandbox without network and returns the tool's
    /// text; `None` — the env isn't set, the smoke is skipped.
    async fn run_provisioned(code: &str) -> Option<String> {
        let tool = provisioned(false, 120)?;
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        Some(out.result)
    }

    /// One use of every later addition to the starter set. Each line prints what the
    /// package *computed*, not its version: a wheel that imports but cannot work (a
    /// native module that traps) has to fail here, not in a user's chat.
    const STARTER_SET_SCRIPT: &str = r#"
import io, feedparser, mpmath, networkx, openpyxl, pypdf, regex, sympy, yaml
import pandas as pd
from bs4 import BeautifulSoup
from lxml import etree
from PIL import Image
x = sympy.symbols('x')
print('sympy', sympy.solve(x**2 - 4, x))
mpmath.mp.dps = 30
print('mpmath', str(mpmath.pi)[:12])
print('networkx', networkx.shortest_path(networkx.path_graph(4), 0, 3))
print('regex', regex.findall(r'\p{Cyrillic}+', 'abc Привет'))
print('yaml', yaml.safe_load('a: [1, 2]'), yaml.__with_libyaml__)
print('lxml', etree.fromstring('<a><b>7</b></a>').xpath('//b/text()'))
print('bs4-lxml', BeautifulSoup('<p>x<b>y</p>', 'lxml').get_text())
print(pd.DataFrame({'a': [1]}).to_markdown())
feed = feedparser.parse('<rss version="2.0"><channel><title>T</title><item><title>i</title></item></channel></rss>')
print('feedparser', feed.feed.title, len(feed.entries))
book = openpyxl.Workbook()
book.active.append(['q', 5])
xlsx = io.BytesIO()
book.save(xlsx)
xlsx.seek(0)
print('openpyxl', pd.read_excel(xlsx, header=None).iloc[0, 1])
writer = pypdf.PdfWriter()
writer.add_blank_page(width=100, height=100)
pdf = io.BytesIO()
writer.write(pdf)
pdf.seek(0)
print('pypdf', len(pypdf.PdfReader(pdf).pages))
png = io.BytesIO()
Image.new('RGB', (8, 8)).save(png, 'PNG')
print('pillow', png.getvalue()[:4] == b'\x89PNG')
"#;

    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn starter_set_packages_work_in_sandbox() {
        let Some(out) = run_provisioned(STARTER_SET_SCRIPT).await else {
            return;
        };
        for marker in [
            "sympy [-2, 2]",
            "mpmath 3.1415926535",
            "networkx [0, 1, 2, 3]",
            "regex ['Привет']",
            "yaml {'a': [1, 2]} True",
            "lxml ['7']",
            "bs4-lxml xy",
            "|  0 |   1 |",
            "feedparser T 1",
            "openpyxl 5",
            "pypdf 1",
            "pillow True",
        ] {
            assert!(out.contains(marker), "missing {marker:?} in: {out}");
        }
    }

    /// matplotlib draws a PNG with Cyrillic text in its title and legend — the path
    /// the wrapper's matplotlib shim exists for: without it the import fails on the
    /// missing `HOME`, and raster text traps in FreeType's autohinter.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn matplotlib_renders_text_in_sandbox() {
        let code = "import io\n\
                    import matplotlib.pyplot as plt\n\
                    fig, ax = plt.subplots()\n\
                    ax.plot([1, 2, 3], [3, 1, 2], label='ряд')\n\
                    ax.set_title('Проверка кириллицы')\n\
                    ax.legend()\n\
                    png = io.BytesIO()\n\
                    fig.savefig(png, format='png')\n\
                    print('png', png.getvalue()[:4] == b'\\x89PNG', len(png.getvalue()) > 1000)";
        let Some(out) = run_provisioned(code).await else {
            return;
        };
        assert!(out.contains("png True True"), "got: {out}");
    }

    /// What the guest writes to `site-packages` does not reach the next call — the
    /// defect this guards: mounted as a host directory, a `sitecustomize.py` one call
    /// wrote there ran inside the next. The host file is removed before asserting, so a
    /// regression cannot leave it behind to run inside every smoke after this one.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn site_packages_writes_do_not_survive_a_call() {
        let Some(dir) = sandbox_dir_from_env() else {
            return;
        };
        let inject =
            "open('/sp/sitecustomize.py', 'w').write('print(\"INJECTED\")')\nprint('wrote')";
        let first = run_provisioned(inject).await.unwrap();
        let host = std::path::Path::new(&dir)
            .join("site-packages")
            .join("sitecustomize.py");
        let reached_host = host.exists();
        if reached_host {
            let _ = std::fs::remove_file(&host);
        }
        let second = run_provisioned("print('clean')").await.unwrap();
        assert!(
            first.contains("wrote"),
            "the write itself must succeed: {first}"
        );
        assert!(!reached_host, "the write reached the host's site-packages");
        assert!(
            second.contains("clean") && !second.contains("INJECTED"),
            "got: {second}"
        );
    }

    /// Outputs from a real sandbox (docs/sandbox-file-exchange.md §8, stage 2): a
    /// matplotlib chart and a CSV saved to `/w/out` are kept and the chart is shown,
    /// although the script then exits with 3; a folder in `/w/out` is named, not walked;
    /// and the violations are attempted — a link to the job script and a file written
    /// beside `/w/out` must not come back. Under wasmer 7.2.0 a guest link never reaches
    /// the host directory at all (measured: `os.symlink` succeeds and the guest reads
    /// through it, while the host `out/` stays empty); where one does, it must be named as
    /// not kept, never read.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn outputs_are_kept_from_a_real_sandbox() {
        let Some(tool) = provisioned(false, 120) else {
            return;
        };
        let (_d, folder, ctx) = ctx_with_folder();
        let code = "import os\n\
                    import matplotlib.pyplot as plt\n\
                    plt.bar(['a', 'b'], [3, 5])\n\
                    plt.savefig('/w/out/chart.png')\n\
                    open('/w/out/totals.csv', 'w').write('k,v\\na,3\\nb,5\\n')\n\
                    os.makedirs('/w/out/nested', exist_ok=True)\n\
                    open('/w/out/nested/inner.txt', 'w').write('x')\n\
                    open('/w/beside.txt', 'w').write('not collected')\n\
                    try:\n\
                    \x20   os.symlink('/w/job.py', '/w/out/link.py')\n\
                    \x20   print('link made')\n\
                    except OSError as e:\n\
                    \x20   print('no link', e)\n\
                    raise SystemExit(3)";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        let r = &out.result;
        eprintln!("{r}");
        assert_eq!(stored_names(&out), ["chart.png", "totals.csv"], "{r}");
        assert_eq!(out.images.len(), 1, "{r}");
        assert!(r.contains("exit code: 3"), "{r}");
        assert!(r.contains("- nested/ — not kept"), "{r}");
        assert!(!r.contains("beside.txt"), "{r}");
        // Whether or not the guest's link reached the host, nothing it names is kept.
        let reached = r.contains("- link.py");
        eprintln!("the guest's link reached the host directory: {reached}");
        if reached {
            assert!(r.contains("- link.py — not kept"), "{r}");
        }
        assert!(!folder.path().join("link.py").exists());
        let chart = std::fs::read(folder.path().join("chart.png")).unwrap();
        assert!(chart.starts_with(b"\x89PNG"), "not a PNG");
    }

    /// A timed-out call keeps none of its outputs — they may be half-written — and says
    /// which ones it left.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn a_timed_out_call_keeps_none_of_its_outputs() {
        let Some(tool) = provisioned(false, 8) else {
            return;
        };
        let (_d, folder, ctx) = ctx_with_folder();
        let code = "open('/w/out/early.txt', 'w').write('x')\nwhile True: pass";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        let r = &out.result;
        assert!(out.effects.is_empty(), "{r}");
        assert!(
            r.contains("- early.txt — not kept: the call timed out"),
            "{r}"
        );
        assert!(!folder.path().join("early.txt").exists());
    }

    /// requests over HTTPS with network access enabled.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox + network (MINDFORK_SANDBOX_DIR)"]
    async fn requests_in_sandbox_with_net() {
        let Some(tool) = provisioned(true, 120) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let code = "import requests; r = requests.get('https://example.com', timeout=20); \
                    print('status', r.status_code)";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(out.result.contains("status 200"), "got: {}", out.result);
    }

    /// With no network access the request must fail (no sockets in the sandbox) — not 200.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn requests_blocked_without_net() {
        let Some(tool) = provisioned(false, 60) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let code = "import requests\n\
                    try:\n\
                    \x20   r = requests.get('https://example.com', timeout=10)\n\
                    \x20   print('status', r.status_code)\n\
                    except Exception as e:\n\
                    \x20   print('blocked')";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(!out.result.contains("status 200"), "got: {}", out.result);
    }

    /// Cyrillic in `print` must not fail (the WASIX guest is UTF-8).
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn cyrillic_print_in_sandbox() {
        let Some(tool) = provisioned(false, 60) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print('Привет, мир')"}))
            .await
            .unwrap();
        assert!(out.result.contains("Привет, мир"), "got: {}", out.result);
    }

    /// An infinite loop is interrupted by the timeout (killing the wasmer process).
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn timeout_kills_sandbox() {
        let Some(tool) = provisioned(false, 3) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "while True: pass"}))
            .await
            .unwrap();
        assert!(
            out.result.contains("превысил лимит времени"),
            "got: {}",
            out.result
        );
    }

    /// The tool over a provisioned sandbox with a memory limit (Windows).
    #[cfg(windows)]
    fn provisioned_capped(memory_mb: u64) -> Option<PythonExec> {
        use crate::shared::sandbox::WasmerSandbox;
        let dir = sandbox_dir_from_env()?;
        Some(PythonExec::new(
            PythonMode::Wasmer,
            None,
            Arc::new(
                WasmerSandbox::new(Some(std::path::PathBuf::from(dir)))
                    .with_memory_limit(Some(memory_mb)),
            ),
            false,
            Duration::from_secs(60),
        ))
    }

    /// A memory limit (Windows Job Object) keeps a runaway script from eating the host's
    /// memory: a large allocation under a low limit fails (the process is killed).
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn memory_cap_stops_runaway() {
        let Some(tool) = provisioned_capped(1024) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        // A 3 GB allocation under a 1 GB limit must fail — either a graceful
        // MemoryError, a fatal V8 crash, or a non-zero exit code.
        let code = "b = bytearray(3 * 1024 * 1024 * 1024)\nprint(len(b))";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        let r = &out.result;
        assert!(
            r.contains("MemoryError") || r.contains("Fatal") || r.contains("код возврата"),
            "expected the allocation to fail under the limit, got: {r}"
        );
        // And 3 GiB definitely weren't allocated (the byte count didn't show up in stdout).
        assert!(!r.contains("3221225472"), "got: {r}");
    }

    /// A reasonable limit (2 GB) doesn't get in the way of light work.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn memory_cap_allows_normal_work() {
        let Some(tool) = provisioned_capped(2048) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(sum(range(1000)))"}))
            .await
            .unwrap();
        assert!(out.result.contains("499500"), "got: {}", out.result);
    }

    /// On an en profile, sandbox unavailability is explained **in English** (axis A):
    /// the `python_exec` wrapper + the nested reason from `sandbox.rs` — both English, with no
    /// Russian leaking. Not `#[ignore]` (doesn't need a real `wasmer` — the Missing path).
    #[tokio::test]
    async fn en_sandbox_missing_is_localized() {
        use crate::shared::sandbox::WasmerSandbox;
        if std::env::var_os("MINDFORK_SANDBOX_WASMER").is_some() {
            return; // the environment supplies a binary — the Missing path won't reproduce
        }
        let empty = tempfile::tempdir().unwrap();
        let tool = PythonExec::new(
            PythonMode::Wasmer,
            None,
            Arc::new(WasmerSandbox::new(Some(empty.path().to_path_buf()))),
            false,
            Duration::from_secs(30),
        );
        let (_d, _s, ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(1)"}))
            .await
            .unwrap();
        let r = &out.result;
        assert!(r.contains("The Python sandbox is unavailable"), "{r}");
        assert!(r.contains("`wasmer` binary not found"), "{r}");
        assert!(no_cyrillic(r), "cyrillic leaked on en-profile: {r}");
    }

    /// On an en profile, the output and the **exit-code label** are English (axis A). A real
    /// (provisioned) sandbox: `print` + a non-zero `sys.exit`.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn en_sandbox_output_and_exit_label_localized() {
        let Some(tool) = provisioned(false, 60) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
        let code = "print('hello'); import sys; sys.exit(3)";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        let r = &out.result;
        assert!(r.contains("hello"), "{r}");
        assert!(r.contains("exit code:"), "the en exit-code label: {r}");
        assert!(no_cyrillic(r), "cyrillic leaked on en-profile: {r}");
    }

    /// On an en profile the timeout message is English (axis A). A real sandbox.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn en_sandbox_timeout_localized() {
        let Some(tool) = provisioned(false, 3) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "while True: pass"}))
            .await
            .unwrap();
        let r = &out.result;
        assert!(r.contains("exceeded the time limit"), "{r}");
        assert!(no_cyrillic(r), "cyrillic leaked on en-profile: {r}");
    }

    /// Real local execution (manual, if Python is installed).
    #[tokio::test]
    #[ignore = "requires a Python interpreter on PATH"]
    async fn runs_real_python_local() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = local(None)
            .invoke(&ctx, serde_json::json!({"code": "print('hello')"}))
            .await
            .unwrap();
        assert!(out.result.contains("hello"), "got: {}", out.result);
    }

    /// Cyrillic in `print` must not fail with `UnicodeEncodeError` (Windows cp1252).
    #[tokio::test]
    #[ignore = "requires a Python interpreter on PATH"]
    async fn prints_cyrillic_without_encoding_error() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = local(None)
            .invoke(&ctx, serde_json::json!({"code": "print('Привет, мир')"}))
            .await
            .unwrap();
        assert!(out.result.contains("Привет, мир"), "got: {}", out.result);
        assert!(
            !out.result.contains("UnicodeEncodeError"),
            "got: {}",
            out.result
        );
    }
}
