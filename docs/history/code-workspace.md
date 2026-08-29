# Code workspace — a project directory attached to a chat

Track design plan (stages, scope, forks). Genre per AGENTS.md §1; once the track
is done this file moves to `docs/history/`.

**Status:** design accepted by the user on 2026-08-20 (forks F1–F4 below).
**Stage 0 (the MVP probe) is done and is a GO on both model families** —
results, and what they do not settle, in §7. **Stage 1 (attach + the read-only
tools) is done** — `feat/code-workspace-core`; what it changed against the plan
is in §7.5. **Stage 2 (editing + the journal) is done** —
`feat/code-workspace-edit`, §7.6. **Stage 3 (the command slots) is done** —
`feat/code-workspace-commands`, §7.7. **Stage 4 (the changes screen) is done** —
`feat/code-workspace-changes`, §7.8. **Stage 5 (the semantic index) was measured
and rejected** — fork F4's go/no-go came back no, and the stage does not ship;
the measurement, and what would change the answer, are in §7.9. **The track is
complete.** Where the shipped code differs from what this plan describes —
audited afterwards, in §7.10.

The request, in one paragraph: the assistant should be able to work on a code
project the way modern coding agents (Claude Code, aider) do — the user attaches
a project directory to a chat, the model gets a small set of modern tools to
explore, read, search and *edit* that directory, plus fixed user-authored
build/run/test commands it can execute but never compose, and the user gets a
dedicated screen showing what the assistant changed, as a git-style diff, with
per-file revert. Search must exist in a plain (lexical) form and, optionally, as
an embedding index. The tool-round limit must not throttle these tools.

## 1. What already exists (survey, 2026-08-20)

The survey reshaped the task the way docs/lessons.md §3 predicts: most of the
machinery is already in the codebase, and the design's job is to compose it.

- **`fs_read`/`fs_write`/`fs_list`** (`src/features/tools/fs.rs`, spec §9.3) are
  the primitive ancestors: whole-file read capped at 50 k chars, whole-file
  write/append, flat listing; global `tools.fs_enabled` gate (off) and an
  optional global `tools.fs_root` sandbox. No line ranges, no point edits, no
  search, no per-chat scope. The new tools are a **separate family**, not an
  extension — `fs_*` stays untouched (fork F8).
- **The tool contract fits as-is** (spec §9.2, architecture §8): `Tool` with
  `group`/`gate`/`danger`/`enabled_by_default`, localized descriptions, result +
  effects. The dynamic-gate precedent is the history pair — `history_read` /
  `history_search` are offered **only while a folded range exists**
  (`effective_tool_ids`). Workspace tools are offered only while a workspace is
  attached; no new global master switch (fork F* below: attaching *is* the
  consent, mirroring `attachment_read`'s "narrows access to what the user
  explicitly attached" rationale).
- **Per-edit confirmation already exists** (spec §9.8): `tools.confirm_dangerous`
  parks any `danger()` tool call in a popup (`Enter`/`A`-for-turn/`Esc`,
  declining does not cancel the turn). The edit tools only need to declare
  `danger() = true`.
- **The round limit has a clean seam**: `max_tool_rounds` (default 8) is
  enforced at a single check in `generation.rs::tool_round`; exemption is a
  trait predicate plus a conditional round increment (§4.4).
- **Subprocess precedent exists but has a hole.** The Python sandbox
  (ADR 0005) and the MCP client give the template — a one-at-a-time semaphore
  gate, `kill_on_drop`, per-call timeout, `select!` against `ctx.cancel` — but
  **nothing in the repo kills a process tree**. Python runs in-process inside
  wasmer, so killing the child suffices there; `cargo build` spawns `rustc`
  grandchildren that survive it. Build/run/test need a Job Object on Windows
  (the `windows-sys` `Win32_System_JobObjects` feature is already a dependency)
  and a process group + `killpg` on Unix. Also: the sandbox *discards* partial
  output on timeout; a build tool must do the opposite.
- **Console presentation is free**: `python_exec`'s result shape is parsed back
  by `present.rs::parse_console` into stdout/stderr/exit-code blocks; the
  command tools reuse the shape.
- **The embedding stack is reusable** (ADR 0002, chunking params, vec0,
  background indexing with progress events). The code index is **derived data**
  and therefore belongs in `cache.db` (disposable by construction, excluded from
  backups), not in `data.db` next to RAG (fork F11).
- **UI plumbing is mapped**: a new screen is ~12 known touch points (the search
  screen is the newest template; the settings screen is the two-pane
  list+detail template). `F4` is free in the app and unclaimed by both measured
  hosts (VS Code's `commandsToSkipShell`, browser tabs). `/project`, `/changes`
  are free command names. Commands that take free-form paths get their own
  parser module (`features/image_command.rs` pattern), not a registry row.
- **No diff/walker crates yet**: `similar`, `ignore`, `grep-regex` +
  `grep-searcher` are new dependencies (all pure Rust, ripgrep's own family for
  the latter three).
- **Local-model feasibility data point** (docs/journal/tools.md, MCP live
  smoke): a local Gemma handled an MCP filesystem server's 14 tools (~7 KiB of
  schemas) and called `read_text_file` with correct JSON on the first round.
  The ~9 tools this track adds are within that envelope.

## 2. Decided forks (user, 2026-08-20)

| # | Fork | Decision |
|---|---|---|
| F1 | How edits are confirmed | **Auto-apply + changes screen + per-file revert.** Per-call popups come from the existing `tools.confirm_dangerous` (edit/write/build/run/test declare `danger() = true`). No new mandatory popup. |
| F2 | Diff baseline | **Snapshot at first touch**: before the assistant's first mutation of a file in this chat, the original is journaled; the diff is "everything since the assistant started", and revert restores the journaled original. No git dependency; a git-vs-HEAD mode is roadmap material, not v1. |
| F3 | Command slots | **Three: build / run / test.** Same rules for all: the command line is typed by the user, the model sees its text but can never pass arguments or compose commands. |
| F4 | Semantic index | **Last stage, with its own go/no-go**: a live measurement against `code_grep` on real questions; if it does not beat grep for the target local models, the rejection is recorded here and the stage does not ship. |

## 3. Design

### 3.1. Concept and commands

A chat owns at most one **workspace**: an attached project directory plus up to
three command slots. Everything is per-chat state (additive `Chat` fields, ADR
0006 §8 — no schema bump):

```rust
// entities/chat.rs
#[serde(default, skip_serializing_if = "Option::is_none")]
pub workspace: Option<Workspace>,

pub struct Workspace {
    pub root: String,               // canonicalized at attach, display form (no \\?\)
    pub build_cmd: Option<String>,
    pub run_cmd: Option<String>,
    pub test_cmd: Option<String>,
}
```

Commands (own parser module `features/project_command.rs`, modeled on
`image_command.rs`; `/project` and `/changes` are free names):

- `/project attach <dir>` — validate the directory exists, canonicalize, store.
  Re-attach to a different root resets the change journal (F13).
- `/project detach` — remove the workspace (journal kept on disk until the next
  attach; the changes screen empties).
- `/project status` — root, the three command lines (or "not set"), index
  state, changed-file count.
- `/project build-cmd [line]` / `run-cmd [line]` / `test-cmd [line]` — with an
  argument: set; without: show the current line.
- `/project clear build|run|test` — unset a slot (an explicit subcommand, not a
  magic empty argument — a command line could legitimately be any string).
- `/changes` — open the changes screen (same handler as the `F4` chord, the
  command-only-control rule).

Every refusal names the route that works (docs/lessons.md §4): attaching over a
missing directory says so; `/changes` with no workspace and no journal says
what to attach; a command tool with no slot configured is simply **not
offered** (S12: a disabled tool is never advertised), and the system block
says how the user sets it.

Mutation path: `chat_mut` + `mark_dirty`, **without** touching `modified_at`
(the `draft`/`feed_view` class — attaching a project must not bump the chat up
the list).

### 3.2. The tool family

Group `ToolGroup::Workspace`, ids `code_*` (fork F5). All are offered only
while the turn's snapshot has a workspace (`TurnInfo.workspace`, consumed by
`effective_tool_ids` the way `history_available` is). All results are plain
text; `code_read`/`code_grep` must stay **out of** `present.rs`'s
`PROSE_RESULT_TOOLS` (verbatim file content — the `rag_search` lesson).

| Tool | Args | Behavior |
|---|---|---|
| `code_list` | `{ path?, depth?, glob? }` | Directory tree from `path` (default root), `ignore`-walked (respects `.gitignore`, skips `.git/`), depth default 2, entry cap with an explicit truncation note. `glob` filters names. |
| `code_read` | `{ path, offset?, limit? }` | Line-numbered text (`cat -n` shape), default window ~400 lines, char-capped; reports total line count and how to continue (`offset`). Refuses binaries (NUL probe) and files over a hard size cap, naming the cap. |
| `code_edit` | `{ path, old_string, new_string, replace_all? }` | Exact-substring replacement. 0 matches → error suggesting a re-read; >1 without `replace_all` → error naming the count. Matching is done on newline-normalized text; the file is written back with its **original EOL style and BOM** (the CRLF trap, §3.5). Result echoes ±3 lines around the change with line numbers, so the model verifies without a second read. `danger() = true`. |
| `code_write` | `{ path, content }` | Create (with parents) or fully overwrite. `danger() = true`. |
| `code_grep` | `{ pattern, path?, glob?, max_results? }` | Regex content search (`grep-regex`/`grep-searcher`, smart-case), `.gitignore`-aware, binary-skipping; `path:line: text` grouped by file, result cap with a note. |
| `code_search` | `{ query, top_k? }` | Semantic search over the optional index (stage 5 only). Distinguishes "index disabled / still building / no hits", each answer naming `code_grep` as the route that always works. |
| `code_build` / `code_run` / `code_test` | `{}` | Execute the corresponding user-authored command line (§3.3). Offered only when the slot is set. `danger() = true`. |

Every path argument goes through one resolver: canonicalize against the root,
refuse anything that escapes (`..`, absolute paths outside, symlinks out —
canonicalization settles all three). This is `fs.rs::FsRoot::resolve`'s job
already; the struct is hoisted to a shared home rather than copied (the
duplication gate, docs/lessons.md §2). Windows canonical paths are stripped of
the `\\?\` prefix before display or inclusion in results.

Read-before-edit is **taught, not enforced** (the tool description instructs a
read first; enforcement would need per-turn read-set tracking — deferred until
a live run shows blind edits actually happening).

### 3.3. Command execution (`code_build`/`code_run`/`code_test`)

- The stored line is parsed with the shell-quote splitter the MCP editor
  already uses, and the program is resolved like MCP's `resolve_command`
  (PATHEXT completion on Windows) — **no shell** (fork F6): predictable
  quoting, no `cmd /c` re-parsing, and the one thing a shell would add
  (pipes/redirects) is out of scope by design. A user who needs a pipeline
  wraps it in a script and names the script.
- `cwd` = workspace root. Environment inherited, plus `NO_COLOR=1` /
  `CLICOLOR=0`; residual ANSI escapes are stripped from captured output.
- stdout and stderr are read concurrently into buffers (avoids pipe deadlock);
  timeout `workspace.command_timeout_secs` (default 300). **On timeout the
  partial output is kept** and the result says "timed out after N s" — the
  inverse of the sandbox's discard, deliberately.
- **Process-tree kill**: on Windows the child is assigned to a Job Object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` right after spawn (the ADR 0005 /
  MCP-manager precedent; the handle is not held across `await`); on Unix the
  child gets `process_group(0)` and cancellation/timeout kills the group.
  Cancellation via `ctx.cancel` (`Esc` works mid-build).
- One command at a time: the three tools share a semaphore gate
  (`try_acquire` → localized "busy" error), the sandbox pattern.
- Output caps: `workspace.output_limit_chars` per stream (default 10 000),
  truncation keeps **head + tail** with an omission marker (fork F12: compilers
  put first errors at the head and summaries at the tail).
- Result format mirrors `python_exec::format_output_parts` — exit code,
  duration, stdout/stderr sections — so `present.rs` renders a console block
  with zero new widget code.

### 3.4. The agentic loop: round-limit exemption

`Tool::counts_toward_round_limit()` (default `true`, `false` for the whole
`code_*` family). The loop increments `self.round` only when the round executed
at least one counting call; the exhaustion branch (`request.tools.clear()` + one
final text round) is unchanged. So a turn that is 40 rounds of
read/edit/build still has its full budget of "other" tool rounds, which is
exactly the user's requirement.

Backstop: `workspace.max_rounds` (default **0 = off**, fork F7 — the user's
stated intent is "no limit"; `Esc`, the per-call timeout and the one-at-a-time
gate remain the safety net). When set and reached, the same exhaustion path
runs, with a message naming the setting.

### 3.5. Edits, the journal, and the changes screen

**Journal** (per chat, on disk under the app's data root —
`Paths::workspace_dir(chat_id)`): before the first mutation of a path in this
chat, the tool writes the pre-image (`baseline/<hash-of-rel-path>`) and a
`manifest.json` row `{ rel_path, existed, baseline_hash, first_touched_at }`.
The manifest also records the root it belongs to; attaching a different root
starts a fresh manifest (F13). New files journal as `existed: false`. This is
deliberately **not** in `Chat` (a chat file must not grow by megabytes of
source snapshots) and **not** in `cache.db` (baselines are *not* recomputable —
they are the one thing that must survive).

**EOL/BOM fidelity**: files are read as bytes; a BOM and the dominant EOL style
are detected and re-applied on write, while matching runs on `\n`-normalized
text (models emit `\n`; Windows repos are CRLF — without this every edit turns
into a whole-file diff).

**Changes screen** (`ActiveScreen::Changes`, new; entered from the chat only,
so `Esc → Chat` and no `Back`-stack extension):

- Left pane: journaled files as `path  +a −b` (added files marked; deletion
  does not exist in v1 — no tool can delete).
- Right pane: unified diff baseline ↔ current file, computed lazily for the
  selected file with `similar`, colored by palette roles (`success`/`error`
  for +/−, accent hunk headers). Intra-line emphasis is a later nicety, not v1.
- Keys: `↑/↓` select file, `Tab` focus panes, `PgUp/PgDn` scroll the diff,
  `r` — revert the selected file behind a confirm popup (restore baseline, or
  delete a file that did not exist; the journal row is dropped), `Esc` back,
  `F10`/`Ctrl+Q` quit. Opened by `F4` (fork F10; free in the app, absent from
  both measured host skip-lists) and `/changes`.
- Data flow: `ChatIntent::OpenChanges` → `AppCommand` → the orchestrator loads
  the journal + current file contents off the runtime (`spawn_blocking`) →
  `AppEvent::Changes(snapshot)` → the screen. Revert is
  `AppCommand::RevertWorkspaceFile` and a refreshed snapshot. Oversized files
  render as "too large to display" rather than freezing the frame.

### 3.6. The system-prompt block

`request::inject_workspace`, next to the attachments block (spec §9.7 shape):
the root, the three command lines verbatim (or "not set" + the `/project`
subcommand that sets them), whether the semantic index is available, and two
lines of tool guidance (paths are relative to the root; read before editing;
run the test command after edits when one exists). The model sees the command
*text*, so it can advise the user how to fix a broken command — while the only
thing it can do itself is run the slot as-is. The block is stable across turns
unless the workspace config changes (prefix-cache friendly, axis A language).

### 3.7. Semantic index (stage 5) — **measured and rejected**

> **This section describes a design that did not ship.** Fork F4 made the stage
> conditional on a live measurement, the measurement came back no, and §7.9
> records it. The text is kept as written so the rejected design is legible to
> whoever revisits the question — not as a description of the application.


- Off by default: `workspace.semantic_index` toggle. Requires the ADR 0002
  embedder; absent embedder → indexing is skipped with a clear note and
  `code_search` closes the door (grep named as the route).
- Chunking: line windows (~40 lines, ~10 overlap, char-capped) over
  `ignore`-walked text files under the size cap. No tree-sitter (§5).
- Storage: `cache.db` (fork F11) — new `workspace_chunks` table + a vec0 table
  partitioned by root hash, plus the embedder fingerprint; a model change or
  dimension mismatch drops the tables and re-indexes lazily. Disposable by
  construction, excluded from backups, no interaction with `data.db`'s
  `rag_dim`/generation machinery.
- Build: background task on attach (progress in the RAG banner slot),
  incremental by file hash; files touched by `code_edit`/`code_write` are
  re-indexed after the turn; `/project reindex` forces a rebuild.
- Go/no-go: a fixed question set over a real repository, gemma-4-31b and
  qwen-3.6-27b, `code_search`+`code_grep` vs `code_grep` alone; ship only if
  the index measurably improves answers or reduces rounds.

### 3.8. Settings

A "Workspace" group in the **Tools** section (fork F9 — same home as the
python/web/fs groups; a ninth section is not warranted by four fields):

| Key | Type | Default |
|---|---|---|
| `workspace.command_timeout_secs` | int | 300 |
| `workspace.output_limit_chars` | int | 10 000 (per stream) |
| `workspace.max_rounds` | int | 0 (off) |
| `workspace.semantic_index` | toggle | off |

`WorkspaceSettings` on `AppConfig`, `#[serde(default)]` — additive, no bump.
Field registration in both `fields()` and `visit_field_sets` (the search/hint
gate), locale keys in both bundles.

### 3.9. Security posture

- The model never chooses a program: the three slots run user-typed lines
  verbatim, argument-free by schema (`{}`).
- Every path is confined to the attached root by canonicalization; the
  workspace family gives no access outside it (unlike `fs_*`, which stays a
  separate, globally-gated capability).
- Running build/run/test executes project code (`build.rs`, npm scripts) —
  inherent to the feature and consented to twice: the user typed the command
  and attached the directory; `tools.confirm_dangerous` adds per-call consent
  for those who want it.
- The tools are absent from the prompt until a workspace is attached, so a
  chat without one is byte-identical to today's requests.

## 4. Open forks (recommendation first; to confirm before the affected stage)

| # | Fork | Options | Recommendation |
|---|---|---|---|
| F5 | Tool id family | `code_*` / `project_*` / `ws_*` | `code_*` — matches the `X_read`/`X_search` convention, short, no collisions (`fs_*`, `file_*` avoided per survey) |
| F6 | Command execution | argv split + `resolve_command` / `sh -c`+`cmd /c` | **Decided (stage 3): argv split** — predictable, reuses MCP machinery; pipelines are out of scope by design, and the refusal says so (§4.1) |
| F7 | `workspace.max_rounds` default | 0 (off) / 200 | **Superseded (stage 3): a setting of its own, default 500, `0` = no limit** (§4.1) |
| F8 | `fs_*` relationship | untouched / hidden while a workspace is attached | untouched — orthogonal global capability, off by default anyway; revisit only if live runs show the model confusing the families |
| F9 | Settings home | group in Tools / own section | group in Tools |
| F10 | Hotkey | `F4` / `Ctrl+S` | `F4` (clean in VS Code and browsers; `Ctrl+S` collides with save reflexes) |
| F11 | Index storage | `cache.db` / `data.db` | `cache.db` — derived data, disposable, out of backups |
| F12 | Output truncation | head+tail / tail only | head+tail — first compiler errors live at the head, summaries at the tail |
| F13 | Journal on re-attach | reset for a new root / keep per-root history | reset — the screen shows the current workspace; multi-root history is complexity nobody asked for |

### 4.1. Decided before stage 3 (user, 2026-08-20)

Four decisions the stage needed, taken after reading the code rather than from
the plan's guesses. The first two were the user's call; the rest follow from what
the survey found already built.

- **The round ceiling becomes its own setting** (supersedes F7).
  `workspace.max_rounds` — an editable field, **default 500**, and **`0` means no
  limit**. Stage 2's `WORKSPACE_ROUND_CEILING = 200` was a constant written
  because "exempt" and "unbounded" are different promises; the user's answer is
  that both of the options built on it read badly — a number that cannot be
  raised is a wall with no door, and a number that silently means "the built-in
  default" is a field lying about its own value. So the constant becomes the
  field's default and the field says what it does, including the way out. Turning
  the limit off is a legitimate choice for a long refactor and it is the user's to
  make; `Esc`, the per-command timeout and the one-at-a-time gate stay the safety
  net underneath it. The exhaustion message names **which** of the two limits
  ended the turn and the setting that raises it — until this stage it always
  quoted `max_tool_rounds`, even when the workspace ceiling was what fired.
- **Settings live in `WorkspaceSettings` on `AppConfig`** (keys `workspace.*`), as
  §3.8 wrote, not flat in `ToolSettings` next to `python_*`/`fs_*`. That struct
  already carries sixteen fields, stage 5 adds a fifth workspace one, and the key
  names are the ones spec and this plan already quote.
- **The live smoke builds with `cargo`, the unit tests with `rustc` and scripts.**
  Stages 0 and 2 compiled their fixtures with bare `rustc` — no manifest, no
  network — and that is still right for a fast unit test. But `rustc` has no
  grandchildren, and the grandchild is the whole reason this stage exists: only
  `cargo build --offline` (a dependency-free fixture crate) puts a real
  `cargo → rustc` tree behind the kill, and only a real compiler's output is what
  the model has to read to fix the break.
- **Both halves of the runner already exist in the repository, in the wrong
  place.** The Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is
  `shared/mcp.rs::JobGuard`, written for `cmd /c npx → node`; the shell-style argv
  splitter is `screens/settings/helpers.rs::parse_args`, and `features` cannot
  import `screens` at all (FSD). Both are hoisted rather than copied — the stage-1
  `FsRoot` precedent, and the duplication gate's own rule (docs/lessons.md §2).
  What is genuinely new is the **unix** half, which MCP never needed:
  `process_group(0)` at spawn plus `killpg` on timeout or `Esc`.

## 5. Deliberately not doing (and why)

- **An arbitrary shell tool** — the whole point of the three fixed slots is
  that the model cannot compose commands. (User's own framing; agreed.)
- **Arguments on build/run/test** — same reason; a test-filter argument is the
  first thing to want and the first injection vector. The test slot itself
  (F3) covers the main loop.
- **tree-sitter / LSP** — heavy dependencies for marginal gain at this model
  class; line-window chunking and grep are the 90% solution.
- **git integration in v1** (diff vs HEAD, commits, blame) — the snapshot
  journal answers "what did the assistant do" more precisely, works in
  non-repos, and powers revert. A git mode is a possible later stage, not a
  foundation.
- **Indexing code into `data.db`/the RAG base** — derived data in a backed-up
  store, entangled with `rag_dim`/generations; `cache.db` exists for exactly
  this shape.
- **Filesystem watching (`notify`) / auto-reindex on external edits** — v1
  reindexes what the assistant touches and on demand; watchers are a lifecycle
  problem (and a Windows-reliability problem) with no v1 payoff.
- **Fuzzy edit matching** — whitespace-insensitive or "closest match" edits
  silently corrupt files; exact match with a good error is the safe contract.
- **Deleting files** — no tool can delete in v1; revert of a created file is
  the one deletion that exists, and it is user-initiated with a confirm.

## 6. Stages

Each stage is its own branch/PR (AGENTS.md §2); the track starts with a probe.

- **Stage 0 — MVP probe** (`spike/code-workspace-probe`, throwaway code) — **done, GO**.
  Minimal `code_read`/`code_grep`/`code_edit` wired to a fixture workspace; a
  live `#[ignore]` smoke: "this tiny crate fails to build; find and fix the
  bug" against gemma-4-31b and qwen-3.6-27b (the two live-gate families).
  **Go/no-go: ≥3/5 runs per family reach a correct edit** (exact-match format
  honored, right fix) without human help. No-go → re-design the edit contract
  (line-anchored edits / whole-file rewrite) before any tier ships.
- **Stage 1 — attach + read-only tools** (`feat/code-workspace-core`) — **done**:
  `/project attach|detach|status`, `Chat.workspace`, `TurnInfo` snapshot +
  dynamic gating, the system block, `code_list`/`code_read`/`code_grep`, the
  `FsRoot` hoist, settings group, i18n, docs. Live smoke: navigate a real repo
  and answer a code question.
- **Stage 2 — edits + journal** (`feat/code-workspace-edit`) — **done**: `code_edit` /
  `code_write`, EOL/BOM fidelity, the baseline journal, `danger()` wiring,
  round-limit exemption + backstop. Live smoke: the stage-0 scenario, now on
  production code.
- **Stage 3 — command slots** (`feat/code-workspace-commands`) — **done**:
  `code_build`/`code_run`/`code_test`, `/project *-cmd|clear`, process-tree
  kill on both OSes, timeout/truncation settings, console presentation. Live
  smoke: break a fixture crate, let the model build → fix → build green.
  Plus platform tests that a grandchild process dies on timeout. §7.7.
- **Stage 4 — changes screen** (`feat/code-workspace-changes`) — **done**: the
  screen, `similar` diffs, revert with confirm, `F4` + `/changes`. Pure UI —
  unit tests over an explicit journal fixture; no live run required (stated per
  AGENTS.md §3). §7.8.
- **Stage 5 — semantic index** (`spike/code-search-probe`) — **no-go, does not
  ship**. The probe was built, the measurement run, and the verdict recorded in
  §7.9. Nothing of it is in the application: no `cache.db` tables, no background
  indexing, no `code_search`, no `/project reindex`.

Test discipline per stage: unit tests alongside (path-escape negatives, edit
uniqueness/CRLF/BOM round-trips, truncation shapes, gitignore walking, journal
revert round-trip, additive-field round-trips copied from
`renamed_manually_is_additive_and_round_trips`), and the live smokes above via
the standard env-gated `#[ignore]` route. New-code duplication: the search/read
schema helpers (`search_parameters`/`paged_read_parameters`) are reused, and
the three command tools share one implementation parameterized by slot.

## 7. Stage 0 — probe results

**Verdict: GO on both families** (2026-08-20): gemma-4-31b **5/5** and
qwen-3.6-27b **5/5** on each of the two arms, against a bar of 3 of 5. The
criterion is per family precisely because docs/lessons.md §9 records the two
families flaking for *different* reasons; here neither flaked at all.

Branch `spike/code-workspace-probe` (deleted 2026-08-29; its two commits are in
`main`'s history): throwaway `code_read`/`code_grep`/`code_edit`
(`src/features/tools/code.rs`, root taken from `tools.fs_root`, all three off by
default) plus two live smokes in `src/app/orchestrator/tests/live.rs`.

### 7.1. What was measured

Both arms use the same ground truth, and it is not a string comparison against
the source: the fixture is compiled with `rustc` (no cargo, no manifest, no
network) and **run**, so a "fix" that deletes the arithmetic cannot pass. Each
arm checks its own precondition first — arm A asserts the fixture really does
not compile, arm B that it compiles and prints the *wrong* number — and both
assert `code_edit` was actually called, without which the smoke would pass on a
model that merely explains the fix in prose (docs/lessons.md §9).

- **Arm A — the compile error a user pastes.** `f64 / usize` in a file
  `main.rs` never names; the prompt is the `cargo build` output, as a user would
  paste it. **5/5 on both families**.
- **Arm B — the fragment is **not** in the prompt.** A median that builds and
  prints 6 instead of 5, with no code quoted in the message, so `old_string` can
  only come from what `code_read` returned. The obvious one-line fragment
  (`        values[mid]`) occurs **twice** in the file by construction, so a
  naive edit is refused as ambiguous. **5/5 on both families**.

Arm B exists because arm A cannot settle the question on its own: with the
failing line quoted in the prompt, a model can assemble `old_string` from the
message rather than from the file — the smoke would then measure copying, not
the contract (docs/lessons.md §2, "a test worded so it can be satisfied without
doing the thing it checks").

### 7.2. What the runs actually looked like

The intended workflow emerged without being scripted: locate → read → one
edit. On gemma the shape was identical in all ten runs — arm A ran `code_read`
then `code_edit` (2 calls), arm B ran `code_grep("median")`, `code_read`,
sometimes a second read of `main.rs`, then `code_edit` (3–4 calls).
**Exactly one `code_edit` per run across all twenty runs of both families** —
no run ever needed a second attempt, and no refusal ever fired.

The families differed in the *route*, not the outcome, and in the direction
the journal already predicts for a reasoning model: qwen wandered more before
committing (2–8 tool calls on arm A against gemma's steady 2, up to 6 on arm B)
and one arm-B run took 74 s against gemma's ~20 s. None of that reached the
contract — whatever route it took, the edit it finally sent was exact and
single. That is the useful shape of this result: the thing being measured was
insensitive to the difference that usually separates these two families.

The strongest single datum is arm B's argument. The model reproduced a
**five-line** fragment byte-for-byte, indentation included, from a read that had
line numbers prefixed to every line:

```
old_string: "    if values.len() % 2 == 0 {\n        values[mid]\n    } else {\n        values[mid]\n    }"
```

That is three separate things working at once: the `   12→` prefixes were
stripped rather than copied, the leading whitespace of each line survived, and
the model **widened the fragment past the duplicate on its own** instead of
sending the ambiguous one line. The fix itself was minimal and correct in every
run (`values.len() as f64` in arm A, `(values[mid - 1] + values[mid]) / 2.0` in
arm B).

The qwen arm was measured on a **rented** endpoint, since the live stack loads
one chat model at a time: `python tools/e2e_hf.py run --chat-model qwen-3.6-27b
--no-embed --no-alt-embed --no-mmproj --filter code_edit_probe --command "python
tools/probe_runs.py"`. The three `--no-*` flags cut the run to the single
endpoint the probe actually needs, and `tools/probe_runs.py` loops both arms
inside that one deployment — ten model turns for one deploy, ~5.5 minutes of
L40S in total. The endpoint was deleted by the gate and the deletion confirmed
independently with `e2e_hf.py list`, which is the one property that run has to
get right.

### 7.3. What this does *not* settle

- **The refusal paths never fired live.** Because the model never missed,
  `tool.code.edit.not_found` and `tool.code.edit.ambiguous` are covered by unit
  tests only. Those messages are load-bearing by design (§3.2: a miss must say
  what to do next), so **stage 2 keeps a smoke that provokes a miss** and asserts
  the model recovers from the message rather than giving up.
- **Two families, one quantization each** (gemma-4-31b `q4_0` on the local
  stack, qwen-3.6-27b `Q4_K_M` on a rented endpoint). These are the two the live
  gate runs, so the coverage matches the project's own bar — but nothing here
  speaks for the cloud providers, which is where the edit contract will next
  meet a model whose tool-calling differs.
- **One fixture size.** Both fixtures are a few files of a few lines. Nothing
  here measures the contract on a 2000-line file where the read window matters,
  which is a stage-1 concern (the `offset`/`limit` half of `code_read`).

### 7.4. Side findings for stage 1

- **Adding tools to the catalog drifts the committed demo dumps.** Three new
  catalog entries moved a displayed count from 48 to 51 and reddened
  `committed_dumps_match_the_code`. Stage 1 must regenerate: `cargo test
  dump_demo_frames -- --ignored`, then `python tools/screenshots.py`, and commit
  both (docs/lessons.md §1 on the font the regenerator needs).
- **The contract is testable without any `Chat`/`ToolContext` plumbing.** The
  spike reached a real model, a real profile and a real agentic loop with the
  root threaded through an existing config field. Stage 1's `Chat.workspace` work
  is therefore about *scope and lifecycle*, not about making the tools reachable
  — the two can be reviewed separately.
- **`enabled_by_default = false` kept the blast radius at zero**: no existing
  profile gained a tool, no default-tool test moved, and the probe enabled the
  three explicitly. Stage 1 should keep that and let the workspace's presence be
  the gate, as designed.

### 7.5. Stage 1 — what it changed against this plan

Three decisions differ from what §3 and §6 wrote down, each because building it
answered a question the plan had guessed at.

- **The settings section moved to stage 3.** §3.8 lists four fields; all four
  belong to stages that do not exist yet (command timeout and output truncation
  to stage 3, the round backstop with them, the semantic index to stage 5).
  Shipping a section with no field would be UI describing behaviour the binary
  does not have.
- **`grep-searcher`/`grep-regex` were not taken.** §1 assumed the ripgrep search
  crates alongside `ignore`. Matching line by line over files the walker has
  already opened is a dozen lines, and the throughput the crates buy has no
  consumer in a TUI searching one project — so the dependency list is `ignore`
  (gitignore semantics, genuinely hard to replicate) plus `regex` (already in the
  graph transitively, pinned to the lock version).
- **`effective_tool_ids` became `ToolGates`.** Not a refactor for its own sake:
  the workspace flag made it an eight-argument function with six `bool`s, which
  `clippy::too_many_arguments` refuses. The named struct also removed the
  wrong-position hazard at thirteen call sites.

And one thing the plan did **not** anticipate, found by a fixture: `ignore`
honours `.gitignore` **only inside a git checkout** unless told otherwise. An
attached directory that is not a repository would have had its ignore file
silently disregarded — on a Rust checkout, `target/` in every listing and search.
`require_git(false)` is the fix, and the test that caught it is a plain temp
directory with a `.gitignore` in it.

The live run also rewrote a test, in the shape docs/lessons.md §9 already
records: asked for a fact that is only in the project, the model answered
correctly from `code_list` + `code_grep` and never opened the file, because a
grep hit carries the whole line. Demanding `code_read` there would have pinned
the model to the worse route, so the smoke became two turns — one asserting the
outcome, one asking for the file's own shape, which only a read can give.

### 7.6. Stage 2 — the refusal paths, measured

The stage did what §6 asked, with one addition and one thing it could not
deliver.

**The addition — a ceiling on the exemption.** §3.4 made the `code_*` family
exempt from `max_tool_rounds` and put the backstop behind a setting defaulting to
off. Writing the test showed why that is not enough on its own: a model repeating
one exempt call leaves a turn that never ends, and a repeated tool call is a
*measured* failure mode of local models. So `WORKSPACE_ROUND_CEILING = 200` bounds
it in code — far above any real fix, and it ends the turn the way the ordinary
limit does. The configurable number still belongs to stage 3.

**What could not be delivered: a live miss.** §7.3 recorded that the refusal
paths (`not_found`, `ambiguous`) had never fired live, and asked this stage for a
smoke that provokes one. Three fixtures tried:

1. the user quotes the line to change, with a space missing from their quote;
2. the obvious fragment occurs twice in the file, with the request naming which
   one to change;
3. the stage-0 compile-error scenario.

**Zero misses in seven live runs.** The model normalizes an approximate quote to
what the file actually says, and includes the constant's name so its fragment is
unique — both because it reads the file first, and a read tells it the truth. A
miss appears reachable only by taking `code_read` away, which would also take
away the recovery the refusal message asks for, making the smoke measure a dead
end rather than the contract.

So the refusal messages are **insurance rather than a hot path**. They keep their
unit coverage — including the property that a refused edit writes nothing — and
the live smokes assert the *outcome* (an approximate quote must not cost the user
their change) while **reporting** the miss count, so the rate stays visible
across runs instead of being assumed. If a future model family does miss, the
count in the smoke's output is where it will show up first.

### 7.7. Stage 3 — what it changed against this plan

The stage delivered §3.3 as written, including the two things §1 flagged as
missing from the repository. Four decisions differ from what §3 guessed at, and
one part of §3.3 turned out to be already built in the wrong place.

- **Nothing had to be written from scratch for the process-tree kill.** §1 said
  "nothing in the repo kills a process tree", which was true of the *unix* half
  only: `shared/mcp.rs::JobGuard` had been doing exactly the Windows job
  (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) since the MCP host shipped, for exactly
  this reason — `cmd /c npx` → `node`. The stage hoisted it into
  `shared/proc.rs` and added `process_group(0)` + `killpg`. The same was true of
  the argv splitter §3.3 planned to reuse "the shell-quote splitter the MCP
  editor already uses": it lives in `screens/settings/helpers.rs`, and `features`
  cannot import `screens` at all, so it had to move down a layer
  (`shared/cmdline.rs`) rather than be reused where it stood.
- **The unix process group is opt-in, not part of the hoist.** `TreeGuard::
  assign` keeps MCP's behaviour byte-for-byte (a Windows job, nothing on unix);
  only `assign_group` remembers a pgid. Giving an MCP server its own process
  group changes how signals reach it — an unrelated behaviour change, and not
  one to smuggle into a feature PR. The split also makes the dangerous case
  unreachable by construction: a guard with no group has no pgid to `killpg`, so
  the application's own group id can never be the argument.
- **The system block outgrew its sentence.** §3.6 described one block naming the
  tools; it had already fallen a stage behind (stage 2 added the editors and
  never updated the text), and stage 3 would have made it two. Instead of a
  longer sentence that will drift again, the block now lists the tools **this
  turn actually offers**, one gloss each, built from the turn's own tool set —
  the same input `effective_tool_ids` produced. A profile with the editors off
  cannot be told it can edit, and a slot with no line contributes nothing.
- **`parse_console` needed one section, not a rewrite.** §3.3 promised "a
  console block with zero new widget code", which held — but the assembler for
  the shape lived in `python.rs` and the parser in `present.rs`. The command
  result adds a `command:` section (the line, and how it ended), so the assembler
  moved next to the parser (`present::format_console`) and `python_exec` now
  delegates to it. Truncation stayed with each caller: `python_exec` drops the
  tail, a build keeps head and tail (F12), and that is the one thing the two
  genuinely disagree about.
- **The one-at-a-time gate is process-wide, and the tests had to admit it.** Two
  `#[tokio::test]`s spawning commands in parallel refuse each other, with the
  loser decided by the scheduler. A `SERIAL` mutex makes them queue, and the
  gate's own behaviour is asserted by a test written for it rather than
  discovered as flake.

**The live run needed no fixture surgery**, unlike stages 0 and 2: the loop
`code_build` → real `E0277` → `code_read` → `code_edit` → `code_build`-green ran
at the first attempt. What the stage *did* have to argue about is the build tool
— see §4.1: `cargo build --offline` rather than a bare `rustc`, because `rustc`
has no grandchildren and the grandchild is the reason this stage exists.

### 7.8. Stage 4 — what it changed against this plan

The stage shipped §3.5 as designed — two panes, `similar` diffs, `r` behind a
confirmation, `F4` and `/changes`, no `Back`-stack extension. Three things
differ, and one defect was found the only way it could have been.

- **The diff is built in `features`, not in the screen.** §3.5 wrote "computed
  lazily for the selected file with `similar`". The codebase's own rule is
  stronger and points the other way: a screen here is a *pure projection* of a
  snapshot the orchestrator built off the runtime — the message-search screen
  says so in its own module doc. Following it is also cheaper than the plan's
  version: a rendered diff is a fraction of the two files it came from, so the
  event is small, and it is computed **once** rather than on every `↑`. The
  screen ended up with no file-system access at all, which is what FSD wanted
  from it anyway.
- **"Oversized files render as too large to display" grew into five states.**
  The plan named one; the journal can hand the screen a file that is *gone*
  (the user deleted it, or a revert already happened), *binary*, *too large*,
  *created* (nothing to compare against), or *touched and put back by hand*.
  Each has a different next move, so each says which it is — an empty diff pane
  for all five is indistinguishable from a defect (docs/lessons.md §4).
- **Revert and forget are one operation.** §3.5 listed them as two steps ("restore
  baseline …; the journal row is dropped"). Written apart they are two failure
  modes: a restored file still listed offers a second revert that does nothing,
  and a dropped row whose file was not restored loses the pre-image for good —
  and those bytes exist nowhere else. `workspace_diff::revert` writes first and
  forgets second, and `Journal::forget` takes the stored pre-image with the row.
- **The layout defect was found by looking, not by asserting.** Every screen test
  passed while the two panes ran together: a file's counts and the first diff
  line sat shoulder to shoulder — `+12 −4@@ -940,7 +940,9 @@` — and each row
  sized its counts column to its own text, so the column was ragged. Rendering
  the screen once and reading it is what caught both; both now have tests that
  pin the **symptom** rather than the layout maths.

**No live run**, per AGENTS.md §3: this stage adds no tool, touches no engine or
memory path, and changes nothing a model sees. The change set is exercised
end-to-end through the orchestrator instead
(`orchestrator::tests::project`), which is where the journal, the diff and the
revert actually meet.

### 7.9. Stage 5 — the go/no-go, and why it is a no

**Verdict: NO-GO** (user's decision, 2026-08-21). The semantic index does not
ship. The probe was never merged: its branch `spike/code-search-probe` was
deleted on 2026-08-29 and the code is frozen under the annotated tag
`probe/code-search-stage5` (`git show probe/code-search-stage5` for the
verdict, `git log probe/code-search-stage5` for the seven commits).

**What was measured.** A throwaway `code_search` over an in-memory index of this
repository (~21 000 windows, `.gitignore`-walked like `code_list`), against a
fixed set of eight questions asked in the *user's* vocabulary rather than the
code's — "why does the app sometimes shorten the conversation by itself", where
the code says `compaction`. Two arms: the shipped tools alone, and the shipped
tools plus `code_search`. gemma-4-31b on the user's stack, three runs of the set
per instrument version.

**The numbers**, over the two runs of the final instrument (48 turns per arm):

| | grep | grep + search |
|---|---|---|
| Correct, of turns that looked at the project | **16/22 (73%)** | **19/29 (66%)** |
| Correct, of all turns | 16/48 (33%) | 19/48 (40%) |
| Turns that produced no text at all | 11 | 11 |
| Turns that answered without looking | 15 | 8 |
| Tool calls | 111 | 127 |

**Read those two top rows together, because they point in opposite directions.**
By one denominator the index is ahead, by the other it is behind, and the choice
of denominator is a judgement call about what an unusable turn means. An effect
that changes sign with a definition is smaller than the instrument measuring it.
The criterion was "ship only if it measurably improves answers or reduces
rounds" (F4); it does neither, and rounds are slightly worse.

**The instrument was harder than the thing it measured, and that is the finding
worth keeping.** Six defects, each found by reading the raw turns rather than
the summary table, and every one of them moved the headline number — twice
reversing its direction:

1. The first table was 5/5 vs 5/5 with `code_search` called **zero times**. The
   tool was registered and offered — its schema reached the model — but the
   workspace block *tells the model in words* which tools it has, and it is
   built from `code::ALL`, where the probe's tool is not. The model believed the
   block. An arm that cannot use the thing under test measures the control
   twice.
2. Correctness was a **rate, not an outcome**: the control arm scored 5/5 and
   then 3/5 on the same questions. A verdict off one pass is a verdict about one
   afternoon (docs/lessons.md §9).
3. Two questions were **padding** — answerable by reasoning about the model's own
   tooling, with zero tool calls in both arms.
4. "Wrong" was **three different things**. A turn that spends itself on calls and
   thinking and emits no text is the empty-turn failure mode, not a retrieval
   failure; counting it as "this route could not find it" is a claim about
   retrieval made from evidence about token budgets.
5. The markers produced a false **positive** (a general-knowledge answer matching
   `mutex` on a turn with no tool calls) and, once tightened to source-only
   literals, false **negatives** (`machine-bound encryption` and `ADR 0008`
   graded wrong for missing `dpapi`). **A keyword grader cannot resolve an
   open-ended prose answer**; every marker set trades one error class for the
   other.
6. The harness was **starving both arms**: open-ended questions at the default
   `max_tokens` of 2048 with thinking on, when this project had already measured
   that ceiling (1024: 0/3, 2048: 2/3, 4096: 3/4 — docs/journal/ci.md, the
   second-chat-model track). Raising it to 4096 changed the per-arm numbers and
   did **not** reduce the empty turns, which is its own small finding.

**What this does and does not settle.**

- It does **not** say a semantic code index is a bad idea. It says one could not
  be shown to help *here*: on a corpus commented in unusually discursive English
  prose — which is exactly what makes `code_grep` strong — read by a 31B local
  model whose turns are unusable about half the time.
- The one effect that survived every instrument version is **not** the one F4
  asked about: with a search tool available the model answers without looking at
  the project far less often (8 turns against 15). Shipping on that would be
  shipping on an unmeasured basis, which is what the fork exists to prevent.
- **What would change the answer**: a judge-graded measurement (a model grading
  the answers, not a keyword list) at n≈80 per arm; or a corpus with sparse
  comments, where grep has much less to match on. Either is a new probe, not a
  resumption of this one.

**What the no-go buys.** Not shipping this keeps out a `cache.db` schema and its
migration, a background indexing task with progress reporting, incremental
reindex on every edit, a settings toggle, `/project reindex`, and a hard
dependency on the embedding server being configured for a feature that otherwise
does not need it. That is a permanent cost, and it was to be paid for an effect
this measurement could not detect.


### 7.10. After the track — where the shipped code differs from this plan

Written after the fact (2026-08-21), from an audit of the finished feature
rather than from a stage. §7.5–§7.8 record each stage's own deltas; these are
the ones that were never written down, found by checking every `pub` item and
every command against what this document promises. **In all of them
[spec.md](../../spec.md) §9.12 already matches the code**, so the plan is the
stale document and the spec is the one to trust.

- **`code_list` takes no `glob`.** §3.2's table says `{ path?, depth?, glob? }`;
  the schema offers `path` and `depth`. `code_grep` kept its `glob` — which is
  where a glob earns its place, since a search is the thing you narrow.
- **`code_grep` takes no `max_results`.** §3.2's table says
  `{ pattern, path?, glob?, max_results? }`; the cap is the constant
  `MAX_GREP_HITS` (100) and the answer names it when it truncates. A model
  choosing its own cap can only choose it wrong in one direction.
- **The group is `ToolGroup::Files`, not `ToolGroup::Workspace`.** §3.2 names a
  variant that was never added; the `code_*` tools sit in the existing Files
  group, which is what the settings screen shows.
- **`/project status` lists the root and the three slots — no index state, no
  changed-file count.** §3.1 promised both. The index state is not drift but
  §7.9: there is no index. The changed-file count simply never shipped, and
  `F4` is where that number lives.
- **`/project detach` clears the journal** rather than keeping it "on disk
  until the next attach" (§3.1). The stronger rule is the right one — a journal
  outliving the project it describes is the hazard F13 is about — and it is
  what the code has done since stage 2.

**F13 itself was the one delta that was a defect rather than a divergence.**
"Re-attach to a different root resets the change journal" (§3.1, and the F13
row) was implemented only inside `Journal::record`, so the reset fired on the
assistant's next write instead of on the attach, and neither reader checked the
manifest's recorded root. Fixed on `fix/workspace-journal-reattach`
(docs/journal/tools.md, "the change journal followed the chat, not the
project"), which is what prompted this audit. Everything else above is a plan
that outran its implementation, harmlessly, and now says so.

## 8. Documentation impact (AGENTS.md §4)

- spec.md: new §9.12 (workspace tools) + §11.7 command/key tables + a changes
  screen subsection in §11; architecture.md: §3 module map, §8 tool table, §10
  screens; journal: `tools.md` (stages 0–3, 5), `ui-screens.md` (stage 4);
  README + help overlay (`HELP_KEYS`/`HELP_COMMANDS`); CHANGELOG per stage;
  roadmap: the deferred git-diff mode and any F4 rejection.
- New crates (`similar`, `ignore`, `grep-regex`, `grep-searcher`) each get the
  justifying `Cargo.toml` comment the file's style demands. No ADR unless a
  fork above graduates into an architectural decision during implementation
  (the `cache.db`-for-vectors choice, F11, is the likeliest candidate).
