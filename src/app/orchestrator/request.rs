//! Maps domain messages ([`Message`]) into the engine request format ([`ChatRequest`]).

use crate::entities::attachment::{AttachMode, Attachment, format_bytes};
use crate::entities::chat::Chat;
use crate::entities::message::{Message, MessageRole, ToolCallRecord};
use crate::entities::message_image::MessageImage;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiImage, ApiMessage, ApiToolCall, ChatRequest};
use crate::shared::config::{AttachmentSettings, CompactionSettings};
use crate::shared::i18n::Locale;

/// Converts a domain message into a message for the model. System messages
/// go through [`ChatRequest::system`] (here — `None`). Assistant messages with
/// tool calls and tool results are rebuilt for a correct history
/// (strict order validation by the server, contract §3.2).
fn message_to_api(message: &Message, loc: &Locale) -> Option<ApiMessage> {
    match message.role {
        MessageRole::System => None,
        MessageRole::User => {
            Some(ApiMessage::user(&message.text).with_images(images_to_api(&message.images, loc)))
        }
        MessageRole::Assistant => {
            if message.tool_calls.is_empty() {
                Some(ApiMessage::assistant(&message.text))
            } else {
                let calls = message.tool_calls.iter().map(record_to_api).collect();
                Some(ApiMessage::assistant_tool_calls(&message.text, calls))
            }
        }
        // A tool result can carry images too (spec §9.10): an MCP screenshot tool is the
        // first producer. Four of the five engines accept them inside the tool result
        // itself; Gemini refuses with a hard 400 and its builder moves them just after it
        // (docs/research/mcp-tool-images.md §2.2, fork F1).
        MessageRole::Tool => message.tool_call_id.as_ref().map(|id| {
            ApiMessage::tool(id, &message.text).with_images(images_to_api(&message.images, loc))
        }),
    }
}

/// Domain messages → wire messages. A **task notification** (a `System` row
/// carrying [`Message::notification`], spec §9.3.2,
/// docs/research/background-subagents.md §4.4) is user text to the model:
/// merged in front of the user message that follows it — one wire message,
/// the notification first, the user's words after, which every provider
/// accepts without relying on consecutive same-role turns — or standing alone
/// when it is the last user-side row (a turn the app started on it) or an
/// assistant row follows it.
fn api_messages(messages: &[Message], loc: &Locale) -> Vec<ApiMessage> {
    /// Between a notification and the user's words, and between two
    /// notifications delivered in one message.
    const SEP: &str = "\n\n";
    let mut out = Vec::with_capacity(messages.len());
    let mut pending: Vec<&str> = Vec::new();
    for m in messages {
        if m.is_notification() {
            pending.push(&m.text);
            continue;
        }
        if m.role == MessageRole::User && !pending.is_empty() {
            let mut parts = std::mem::take(&mut pending);
            parts.push(&m.text);
            out.push(ApiMessage::user(parts.join(SEP)).with_images(images_to_api(&m.images, loc)));
            continue;
        }
        if !pending.is_empty() {
            out.push(ApiMessage::user(std::mem::take(&mut pending).join(SEP)));
        }
        if let Some(api) = message_to_api(m, loc) {
            out.push(api);
        }
    }
    if !pending.is_empty() {
        out.push(ApiMessage::user(pending.join(SEP)));
    }
    out
}

/// Domain images → the request form, each carrying the label part emitted just before it
/// (spec §9.10, fork F5 of docs/research/multimodal-images.md).
///
/// The label is numbered from 1 and names the file, matching what `/image list` showed
/// the user — so "the chart in `plot.png`" means the same thing on both sides of the
/// conversation. It is scaffold the *model* reads, hence the profile language (axis A).
fn images_to_api(images: &[MessageImage], loc: &Locale) -> Vec<ApiImage> {
    images
        .iter()
        .enumerate()
        .map(|(i, image)| {
            ApiImage::new(
                image.mime.clone(),
                &image.data,
                Some(loc.tf(
                    "prompt.images.label",
                    &[("n", &(i + 1).to_string()), ("name", &image.name)],
                )),
            )
        })
        .collect()
}

/// Domain tool-call record → the request-body form (arguments as a JSON string).
fn record_to_api(rec: &ToolCallRecord) -> ApiToolCall {
    ApiToolCall {
        id: rec.id.clone(),
        name: rec.name.clone(),
        arguments: rec.arguments.to_string(),
        // A thought signature (Gemini 3) is preserved on the record — resent on
        // history replay, otherwise Gemini 3 returns a 400 on a historical functionCall.
        thought_signature: rec.thought_signature.clone(),
    }
}

/// Timestamp of the chat's last user message (for `ToolContext`).
pub(super) fn last_user_message_at(chat: &Chat) -> Option<chrono::DateTime<chrono::Utc>> {
    chat.messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::User)
        .map(|m| m.timestamp)
}

/// Everything the system prompt is assembled from besides the chat itself —
/// gathered so a new injection input does not lengthen [`build_request`]'s
/// signature again (the `ToolDeps`/`ToolParams` pattern,
/// docs/history/refactoring-solid.md §3).
pub(super) struct PromptContext<'a> {
    pub attachments: &'a AttachmentSettings,
    pub compaction: &'a CompactionSettings,
    /// Attached files that actually have a semantic index.
    pub indexed: &'a [uuid::Uuid],
    /// Whether this turn offers `history_read`/`history_search` — the summary
    /// block only names them when it does (see [`inject_compaction`]).
    pub history_tools: bool,
    /// The tools this turn actually offers. The workspace block names the
    /// `code_*` ones it finds here and no others — a bool would have been enough
    /// while the family was read-only, and stopped being enough the moment the
    /// block had to say whether the assistant can *change* the project and
    /// which commands it can run (see [`inject_workspace`]).
    pub offered_tools: &'a [crate::entities::profile::ToolId],
    /// The chat's files as one numbered list, for the block that tells the model what
    /// `python_exec` may copy into its input folder (docs/sandbox-file-exchange.md §12 T5).
    /// Empty unless the turn offers that tool — the **caller** decides, so the block can
    /// never name a tool the turn does not have (docs/lessons.md §4).
    pub files: &'a [crate::features::chat_inputs::ChatInput],
    /// That mode's input and output folders as the model must write them (§14 V2):
    /// `/w/in`/`/w/out` in the sandbox, `in`/`out` on the host.
    pub python_dirs: (&'static str, &'static str),
    /// Scaffold language (axis A): every injected block is read by the model.
    pub loc: &'a Locale,
}

/// The chat's **environment** — what the system prompt is assembled around
/// besides the persona and the messages: the attached files, the attached
/// project, the rolling summary. Borrowed from a [`Chat`] on an ordinary turn
/// ([`RequestEnv::of`]); kept apart from the chat because a sub-agent's request
/// is built from its **parent's** environment with a persona and messages of
/// its own (docs/research/subagent-chats.md §3.3) — the environment is the
/// turn's, the conversation is the loop's.
pub(super) struct RequestEnv<'a> {
    pub attachments: &'a [Attachment],
    pub workspace: Option<&'a crate::entities::workspace::Workspace>,
    /// The summary text and the index the verbatim messages start at, when a
    /// summary is in force (see [`Chat::compaction_view`]).
    pub compaction: Option<(&'a str, usize)>,
}

impl<'a> RequestEnv<'a> {
    /// The environment of `chat` itself, with compression honoured per the
    /// master switch (`config.compaction.enabled`).
    pub(super) fn of(chat: &'a Chat, compaction_enabled: bool) -> Self {
        Self {
            attachments: &chat.attachments,
            workspace: chat.workspace.as_ref(),
            compaction: chat.compaction_view(compaction_enabled),
        }
    }
}

/// Builds a generation request from the chat's current state with a set of tool
/// schemas. Attached files (`/file attach`) are injected into the system prompt
/// — see [`inject_attachments`]. The chat's own persona, messages and
/// environment; [`build_request_in`] is the same assembly over an environment
/// that is not the conversation's own.
pub(super) fn build_request(
    chat: &Chat,
    sampling: SamplingConfig,
    tools: Vec<crate::shared::api::ToolSchema>,
    cx: &PromptContext<'_>,
) -> ChatRequest {
    build_request_in(
        &chat.system_message,
        &chat.messages,
        &RequestEnv::of(chat, cx.compaction.enabled),
        sampling,
        tools,
        cx,
    )
}

/// Assembles a request from a persona, a conversation and an environment
/// given separately. The compacted-away prefix of `messages` is replaced by a
/// summary block; the messages themselves are untouched, so this is the only
/// place the two views diverge.
pub(super) fn build_request_in(
    system_message: &str,
    messages: &[Message],
    env: &RequestEnv<'_>,
    sampling: SamplingConfig,
    tools: Vec<crate::shared::api::ToolSchema>,
    cx: &PromptContext<'_>,
) -> ChatRequest {
    let system = if system_message.trim().is_empty() {
        None
    } else {
        Some(system_message.to_string())
    };
    let (summary, upto) = match env.compaction {
        Some((s, i)) => (Some(s), i),
        None => (None, 0),
    };
    let system = inject_compaction(system, summary, cx.history_tools, cx.loc);
    let system = inject_attachments(system, env.attachments, cx.attachments, cx.indexed, cx.loc);
    // After the attachments, whose handles it repeats and whose content it does not, and
    // before the project: the same volatility order the other blocks follow (spec §6.6).
    let system = inject_files(system, cx.files, cx.python_dirs, cx.loc);
    ChatRequest {
        continue_final: false,
        system: inject_workspace(system, env.workspace, cx.offered_tools, cx.loc),
        messages: api_messages(&messages[upto..], cx.loc),
        sampling,
        tools,
    }
}

/// Appends the attached-project block to the system prompt (spec §9.12).
///
/// Placement is by volatility, like its two siblings: a chat's project can be
/// swapped mid-conversation, where its attachments rarely change, so this block
/// sits after them and before the self-model the generation task appends last
/// (spec §6.6).
///
/// Two rules, both learned the hard way (docs/lessons.md §4):
///
/// - the block **names the tools this turn actually offers**, one by one, and
///   nothing else — a profile can have any of them switched off while a project
///   is attached, and a block promising an absent tool is how a model spends a
///   turn improvising with the wrong ones;
/// - it marks the root as **data, not instruction**, the way the attachment
///   block marks file content: a path is user-supplied text arriving in the
///   system prompt (spec §13.4).
///
/// The command lines are quoted **verbatim**. The model cannot change them, pass
/// arguments to them or compose new ones — the tools take no arguments at all —
/// but it can read them, which is what lets it tell the user their own command is
/// the thing that is wrong.
pub(super) fn inject_workspace(
    system: Option<String>,
    workspace: Option<&crate::entities::workspace::Workspace>,
    offered: &[crate::entities::profile::ToolId],
    loc: &Locale,
) -> Option<String> {
    use crate::features::tools::code::CodeTool;

    // No project — `system` passes through untouched, and a request from a chat
    // without one is byte-identical to what the app sent before this feature.
    let Some(ws) = workspace else {
        return system;
    };
    let has = |tool: CodeTool| offered.iter().any(|id| id.as_str() == tool.id());
    let available: Vec<CodeTool> = crate::features::tools::code::ALL
        .into_iter()
        .filter(|&t| has(t))
        .collect();
    let reach = if available.is_empty() {
        // Attached, but this profile cannot reach it. Saying so is the whole
        // point: otherwise the model reads the root as an invitation and tries
        // `fs_read`, or asks the user for something they already did.
        loc.t("prompt.workspace.no_tools").to_string()
    } else {
        let mut reach = loc.t("prompt.workspace.tools").to_string();
        for tool in &available {
            reach.push('\n');
            match tool.slot() {
                // A command tool's gloss carries the line it runs: the text is
                // the whole of what the model knows about it.
                Some(slot) => reach.push_str(&loc.tf(
                    tool.gloss_key(),
                    &[("line", ws.command(slot).unwrap_or_default())],
                )),
                None => reach.push_str(loc.t(tool.gloss_key())),
            }
        }
        reach.push_str("\n\n");
        reach.push_str(loc.t("prompt.workspace.rules"));
        reach
    };
    let block = loc.tf(
        "prompt.workspace.block",
        &[("root", &ws.root), ("name", ws.name()), ("reach", &reach)],
    );
    Some(match system {
        Some(s) if !s.trim().is_empty() => format!("{s}\n\n{block}"),
        _ => block,
    })
}

/// Prepends the rolling-summary block to the system prompt (spec §6.7).
///
/// Placement mirrors [`inject_attachments`] and is ordered by volatility: the
/// persona first (never changes), then this block (changes only on a
/// compaction), then attachments, then the self-model — which the generation
/// task appends last because it changes most often. Everything after a changed
/// block is re-prefilled, so the most stable content goes first (spec §6.6).
///
/// The header is in the **profile** language (axis A — the model reads it), and
/// it says two things deliberately: the block is DATA rather than instructions
/// (the prompt-injection rule attachments follow, spec §13), and **what is
/// possible next** — a block that describes a situation without saying that is
/// what sends a model improvising (four case studies in docs/lessons.md §4).
///
/// `tools` — whether this turn actually offers `history_read`/`history_search`.
/// A folded range normally implies them (sub-decision S12 gates both on the same
/// `compaction_view`), but a profile can have the two tools switched off, and
/// then naming them would point at a dead end — the very failure the sentence
/// exists to prevent. So the wording follows the turn's real tool set, exactly
/// as an attachment's entry only offers `attachment_search` when that file has
/// an index.
pub(super) fn inject_compaction(
    system: Option<String>,
    summary: Option<&str>,
    tools: bool,
    loc: &Locale,
) -> Option<String> {
    // No summary — `system` passes through untouched. (Not `summary?`: that
    // would return `None` and silently drop the persona, which is what the
    // pre-existing request tests caught.)
    let Some(summary) = summary else {
        return system;
    };
    let reach = if tools {
        "compaction.block.tools"
    } else {
        "compaction.block.no_tools"
    };
    let block = format!(
        "{}{}\n\n{}",
        loc.t("compaction.block.header"),
        loc.t(reach),
        summary.trim()
    );
    Some(match system {
        Some(s) if !s.trim().is_empty() => format!("{s}\n\n{block}"),
        _ => block,
    })
}

/// Appends the pinned block of attached files to the system prompt
/// (docs/file-attachments.md §4.3). A pure function — testable with no engine.
///
/// Placement: **`system`**, so the block sits at the front of the prefix and the
/// whole conversation after it stays prefix-cached (spec §6.6); it is
/// re-prefilled only when the attachment set changes — the same trade-off
/// already accepted for the self-model injection. The header is in the
/// **profile** language (axis A): the model reads it.
///
/// An inline attachment contributes its full text; a by-reference one
/// contributes metadata plus the head excerpt. Returns `system` unchanged when
/// nothing is attached.
///
/// `indexed` — attachments that actually have a semantic index (from the DB, see
/// [`Db::attachment_indexed_ids`](crate::shared::storage::db::Db::attachment_indexed_ids)).
/// A by-reference entry only points the model at `attachment_search` for those:
/// promising search over a file that has none would send it down a dead end.
pub(super) fn inject_attachments(
    system: Option<String>,
    attachments: &[Attachment],
    cfg: &AttachmentSettings,
    indexed: &[uuid::Uuid],
    loc: &Locale,
) -> Option<String> {
    if attachments.is_empty() {
        return system;
    }
    let mut block = String::from(loc.t("prompt.attachments.header"));
    for a in attachments {
        // The fence must not occur inside the content, otherwise a file could
        // "close" its own section and the rest would read as instructions.
        let width = fence_width(&a.text);
        let open = "<".repeat(width);
        let close = ">".repeat(width);
        let size = format_bytes(a.bytes);
        // A by-reference entry must state **how to read the rest**: without that
        // the model improvises with the wrong tools (observed on a live run —
        // fs_read into the sandbox, then web_search) and ends up asking the user
        // for the impossible. See docs/file-attachments.md §4.4.
        let (head, body, tail) = match a.mode {
            AttachMode::Inline => (
                loc.tf(
                    "prompt.attachments.begin_full",
                    &[
                        ("open", &open),
                        ("name", &a.name),
                        ("size", &size),
                        ("close", &close),
                    ],
                ),
                a.text.as_str(),
                loc.tf(
                    "prompt.attachments.end",
                    &[("open", &open), ("name", &a.name), ("close", &close)],
                ),
            ),
            AttachMode::ByReference => {
                let pages = a.page_count(cfg.page_tokens).to_string();
                // With an index there are two ways in — search says *where* to
                // look, page reading guarantees *everything* can be read; without
                // one, only the pages.
                let tail = if indexed.contains(&a.id) {
                    "prompt.attachments.end_excerpt_search"
                } else {
                    "prompt.attachments.end_excerpt"
                };
                (
                    loc.tf(
                        "prompt.attachments.begin_excerpt",
                        &[
                            ("open", &open),
                            ("name", &a.name),
                            ("size", &size),
                            ("tokens", &a.est_tokens.to_string()),
                            ("pages", &pages),
                            ("close", &close),
                        ],
                    ),
                    a.excerpt(cfg.excerpt_tokens),
                    loc.tf(
                        tail,
                        &[
                            ("open", &open),
                            ("name", &a.name),
                            ("pages", &pages),
                            ("close", &close),
                        ],
                    ),
                )
            }
        };
        block.push_str("\n\n");
        block.push_str(&head);
        block.push('\n');
        block.push_str(body);
        block.push('\n');
        block.push_str(&tail);
    }
    Some(match system {
        Some(s) if !s.is_empty() => format!("{s}\n\n{block}"),
        _ => block,
    })
}

/// Appends the **chat files** block: what this turn's `python_exec` may copy into `/w/in`,
/// numbered exactly as `/file list` numbers it for the user
/// (docs/sandbox-file-exchange.md §12 T5).
///
/// Which items reach it is the caller's decision, not this function's: `build_request` is
/// given a list **only** for a turn that offers `python_exec` in its Wasmer mode. So the
/// block cannot name a tool the turn does not have — this project's most-repeated defect
/// (docs/lessons.md §4) — and the converse holds too: a file the block does not name goes
/// unused whatever the tool's schema says, which is why the handles live in the prompt
/// rather than only in the argument's description.
///
/// Names, sizes and types only, never content: that is what makes it cheap enough to carry
/// on every turn, and it changes only when the chat's set of files does (spec §6.6). The
/// header marks it as a list of files rather than instructions, as the attachment block
/// marks file content (spec §13.4).
pub(super) fn inject_files(
    system: Option<String>,
    inputs: &[crate::features::chat_inputs::ChatInput],
    dirs: (&str, &str),
    loc: &Locale,
) -> Option<String> {
    if inputs.is_empty() {
        return system;
    }
    let (in_dir, out_dir) = dirs;
    let mut block = loc.tf("prompt.files.header", &[("in", in_dir), ("out", out_dir)]);
    for item in inputs {
        block.push_str(&loc.tf(
            "prompt.files.item",
            &[
                ("i", &item.handle.to_string()),
                ("in", in_dir),
                ("name", &item.name),
                ("staged", &item.staged),
                ("size", &format_bytes(item.bytes as usize)),
                ("mime", &item.mime),
            ],
        ));
    }
    Some(match system {
        Some(s) if !s.is_empty() => format!("{s}\n\n{block}"),
        _ => block,
    })
}

/// How many angle brackets the section fence needs so that it doesn't occur in
/// the file's own content (a file quoting `>>>` must not be able to close its
/// own section). Starts at 3 and widens until the closing marker is absent.
fn fence_width(text: &str) -> usize {
    let mut width = 3;
    // A pathological file (a long run of '>') can't push this far: each step
    // requires the text to contain a strictly longer run.
    while text.contains(&">".repeat(width)) {
        width += 1;
    }
    width
}
