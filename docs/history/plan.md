# mindfork-rs implementation plan

> 🗄 **Archived document.** This step-by-step plan has been fully executed
> (M0–M9) and is kept in `docs/history/` as a historical record of the
> development process. Current project state — in
> [CLAUDE.md](../../CLAUDE.md) and [spec.md](../../spec.md).

A step-by-step implementation plan derived from [spec.md](../../spec.md).
Organized into stages **M0–M9** (see [spec §15](../../spec.md#15-what-is-dropped-not-carried-over)).
Each stage: goal → tasks (naming the FSD-structure modules/files from
[§4.2](../../spec.md#42-layers-top-to-bottom-dependencies-point-only-downward)) →
tests → done criteria (DoD).

The plan is optimized for **iterative development by AI agents**: tasks are
narrow, with explicit boundaries and a verifiable result; dependencies run
downward through the FSD layers.

> ✅ **All stages M0–M9 are complete.** ⚠️ **The engine changed after
> implementation:** instead of `xinfer` (turned out to be too raw for
> Gemma 4), the project runs on **llama.cpp `llama-server`** (external — any
> OpenAI-compatible server). Mentions of `xinfer` below are historical — read
> as "OpenAI-compatible inference server". Current setup —
> [docs/install.md](../install.md).

---

## Conventions and workflow

### Code conventions
- Rust edition 2024. Before every commit: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`.
- Errors: `anyhow::Result` in application layers (`app`, `screens`, `widgets`, `features`), `thiserror` in internal library modules (`shared/api`, `shared/storage`, `features/spellcheck`).
- Logging — `tracing` to a file next to the binary (stdout is taken by the TUI). No `println!` at runtime.
- Every public type/function in a layer gets a unit test as it's written (not as a separate stage).

### FSD dependency discipline
`app → screens → widgets → features → entities → shared`. Sideways/upward
imports are forbidden. The backend core (`shared/api`, `shared/storage`) is
reached through traits.

### The "engine behind a trait" principle
Everything that depends on `xinfer`/HTTP sits behind `EngineBackend`
([§6.1](../../spec.md#61-the-engine-layer-sharedapi)). This gives mock/replay in
tests and room to swap the transport later. **No layer above `shared/api`
knows about HTTP/`xinfer` directly.**

### Branches and commits
- One branch per stage: `m0-skeleton`, `m1-inference`, … Merge into `main`
  once the stage's DoD is met.
- Atomic commits per task within a stage.

### Research tasks (R)
Marked `[R]` — require checking the current API of the libraries involved
(see [spec §16.2](../../spec.md#162-open-questions-to-be-settled-during-implementation)).
The outcome of an `[R]` is recorded as a short ADR comment in the code or in
`docs/decisions/`.

---

## M0. Skeleton

**Goal:** a buildable binary; an empty TUI starts and shuts down cleanly; the
FSD structure and infrastructure are laid down.

### Tasks
1. **Cargo and dependencies.** Add: `tokio` (features: `rt-multi-thread`,
   `macros`, `sync`, `process`, `time`), `ratatui`, `crossterm`, `anyhow`,
   `thiserror`, `tracing`, `tracing-subscriber`, `tracing-appender`, `serde`,
   `serde_json`, `uuid`, `chrono`. `[R]` Pin versions via `cargo add`.
2. **FSD skeleton.** Create the module tree from
   [§4.2](../../spec.md#42-layers-top-to-bottom-dependencies-point-only-downward):
   `app/`, `screens/`, `widgets/`, `features/`, `entities/`, `shared/` with
   `mod.rs` stubs. Set up visibility (`pub(crate)` by default).
3. **`shared/paths.rs`.** Data-directory resolution **next to the binary**
   (portable); paths for `settings.json`, `chats/`, `data.db`,
   `dictionaries/`, `personal_dictionary.txt`, the log file.
4. **`shared/error.rs`.** Base error types.
5. **Logging.** `tracing` → file (via `tracing-appender`), level from
   env/settings.
6. **Single instance.** `[R]` Pick a mechanism (`single-instance` crate /
   lock file / named mutex on Windows). A second launch exits with a
   message.
7. **TUI loop (`app/runtime.rs`).** Terminal init (raw mode, alternate
   screen), main loop: `event::poll(timeout)` → handle `Esc`/`Ctrl+C` →
   `draw`. **RAII terminal restoration** (panic hook + drop guard, so the
   terminal doesn't stay "broken" after a panic).
8. **Entry point (`main.rs`).** Init: single-instance → tokio runtime →
   terminal guard → `app::run()`. Thin.
9. **Event skeleton (`app/events.rs`).** Declare empty `AppCommand`,
   `AppEvent` enums (filled in from M1 onward).

### Tests
- `shared/paths.rs`: correctness of path computation (against a temp "root").
- Smoke: the app starts and exits on `Esc`/`q` without panicking (manual
  check + a `ratatui::TestBackend` test where possible).

### DoD
- `cargo build` is green; `cargo clippy -D warnings` is clean.
- An empty TUI starts, status line shows "mindfork-rs", exits on `Esc`; the
  terminal is restored (including after a panic).
- A second instance won't start.

---

## M1. Inference (minimal end-to-end chat)

**Goal:** chat with Qwen, no tools, in a minimal TUI; streaming and
cancellation work.

### Tasks
1. ✅ **OpenAI-compatible protocol locked in** — see
   [docs/install.md §3](../install.md) for the current engine and launch
   details (`llama-server` launch args, SSE format with
   `reasoning_content`/`tool_calls`, sampling fields, EOS, embeddings).
   *(Historically the contract was checked against `xinfer`'s sources; it's
   the same OpenAI protocol llama.cpp speaks too.)*
2. **HTTP dependencies.** `[R]` Choose a client: `async-openai` (with
   extended types) vs `reqwest` + our own types. Add `futures`/
   `tokio-stream`, `tokio-util` (CancellationToken).
3. **`shared/api/mod.rs` — trait `EngineBackend`**
   ([§6.1](../../spec.md#61-the-engine-layer-sharedapi)): `chat_stream`, `embed`
   (the latter a stub until M5). Types `ChatRequest`, `ChatChunk`,
   `SamplingParams`, `ToolSchema`, `FinishReason`.
4. **`shared/api/xinfer_client.rs`** — HTTP implementation of
   `EngineBackend`: request body construction (`messages`, empty `tools`,
   sampling), SSE stream → `ChatChunk::{Text,Thoughts,ToolCallDelta,Finished}`.
   No string `stop` ([§7](../../spec.md#7-eos-and-stop-token-handling)).
5. **`shared/api/server.rs` — server supervisor**
   ([§3.4](../../spec.md#34-managing-the-server-lifecycle)): managed
   launch of a child `xinfer` process, readiness polling, clean shutdown on
   exit (drop/SIGTERM/kill on Windows), parsing stdout/stderr into progress
   events. External mode (URL only).
6. **"Thoughts" parser (`shared/api/thoughts.rs`)**
   ([§6.5](../../spec.md#65-parsing-thoughts-cot)): from the reasoning field or
   from `<think>…</think>` in the main stream; correct tag stitching across a
   chunk boundary.
7. **Sampling mapping (`entities/sampling.rs` + `shared/api`)**:
   `SamplingConfig` → request fields; dropping unsupported ones
   ([§8](../../spec.md#8-sampling-parameters)).
8. **Minimal orchestrator (`app/orchestrator.rs`)**: one tokio task, an
   `AppCommand` queue (`SendMessage`, `Cancel`), calling `chat_stream`,
   emitting `AppEvent` (`GenerationStarted/Chunk/Thoughts/Finished/Cancelled`),
   `CancellationToken`. `Idle/Generating/Cancelling` state machine and
   `generation_id` ([§4.4](../../spec.md#44-execution-flows-and-ui-backend-state)).
9. **tokio↔TUI bridge (`app/runtime.rs`)**: `mpsc` for commands
   (UI→orchestrator) and events (orchestrator→UI); non-blocking event drain
   in the render loop.
10. **Minimal chat UI**: one input field (a plain string for now) + a feed
    (plain text, no markdown), server status. Stop on `Esc`.

### Tests
- `thoughts.rs`: parsing `<think>` whole and split across chunks; a stream
  with no "thoughts"; the reasoning field.
- Sampling mapping: passing through supported fields, dropping unsupported
  ones, generating a random seed.
- Orchestrator against a **mock `EngineBackend`** (a stream of fixture
  chunks): `SendMessage` → events in order; `Cancel` → `Cancelled` + partial
  text; drops chunks with a mismatched `generation_id`.
- `[ignore]` integration test against real `xinfer`: load a small Qwen,
  stream, cancel mid-generation; **anti-self-truncation** on a prompt that
  provokes printing `<|im_end|>`.

### DoD
- In the TUI you can send Qwen a message, see the streamed reply and
  "thoughts"; `Esc` interrupts generation, the partial reply stays in the
  feed.
- The managed server starts/stops; its status is visible; launch errors show
  in the UI.

---

## M2. Domain model + storage

**Goal:** domain types and persistence (JSON + SQLite); chat/profile CRUD;
soft delete.

### Tasks
1. **`entities/`** ([§5.1](../../spec.md#51-core-entities-entities)):
   `MessageRole`, `Message`, `ToolCallRecord`, `Chat`, `Profile`,
   `CharacterNames`, `Note`, `RagDocument`, `SamplingConfig`,
   `MessageMetadata`, `ToolId`. All `serde`. A `schema_version` field in the
   config.
2. **`shared/storage/json.rs`**: repositories `ChatRepository`,
   `ProfileRepository`, `ConfigRepository`. Read/write `chats/{id}.json`,
   `profiles.json`, `settings.json`. **Atomic write** (write-temp + rename) +
   a chat-file backup ([§5.2](../../spec.md#52-data-storage)).
3. **`shared/storage/db.rs`**: SQLite init (`rusqlite`), schema migrations,
   wiring up `sqlite-vec`. `notes`, `rag_documents` tables + the virtual
   vector table. `[R]` Check building/linking `sqlite-vec` on Windows and
   Linux.
4. **`NoteRepository`, `RagRepository` repositories**: CRUD with a
   **mandatory `profile_id` filter**
   ([§9.5](../../spec.md#95-per-profile-isolation)). kNN search (sqlite-vec).
   Embeddings are written, but generated later (M5) — for now they accept a
   ready-made vector.
5. **Soft delete**: `is_hidden` on chats/profiles; a "hide profile → hide its
   chats" cascade; queries with `WHERE is_hidden = 0`.
6. **`Storage` facade (`shared/storage/mod.rs`)**: `Arc<Storage>`, internal
   synchronization (an SQLite connection pool/mutex), **never calls outward
   while holding the lock** (invariant).
7. **Orchestrator integration**: load profiles/chats on startup; save the
   chat after a turn and on a debounce timer.

### Tests
- Serde round-trip for all entities; `schema_version` stability.
- JSON repositories on `tempfile`: CRUD, atomicity (simulated interruption),
  backup.
- SQLite on `:memory:`: notes/RAG CRUD; **isolation by `profile_id`**
  (negative tests: another profile's data isn't returned); soft delete and
  cascade; kNN on small vectors.

### DoD
- Profiles and chats are created, saved, read back after a "restart"
  (reopening storage).
- Soft delete works; hidden objects don't show up in queries.
- Profile isolation confirmed by tests.

---

## M3. Chat UI (full)

**Goal:** the full chat cycle in the TUI: chat list, a feed with
markdown/thoughts/tool blocks, `tui-textarea` input, spellcheck.

### Tasks
1. **`[R]` Choosing UI crates**: `tui-textarea` vs `ratatui-textarea`
   (compatibility with the `ratatui` version); `ratatui-markdown` (coverage:
   tables, highlighting, collapsing) with a fallback to `tui-markdown`. Write
   up an ADR.
2. **`widgets/message_feed.rs`**: feed rendering, scrolling,
   message/block selection. **Collapsible blocks** for "thoughts" and
   tool-calls ([§11.3](../../spec.md#113-the-message-feed)). Per-message
   Markdown mode.
3. **`shared/markdown.rs`**: render markdown to `ratatui::Text` +
   **unicode approximation of LaTeX** (a substitution table: Greek letters,
   arrows, operators, sub/superscripts)
   ([§11.4](../../spec.md#114-markdown-cot-and-tool-blocks-latex)).
4. **`widgets/input_box.rs`**: `tui-textarea` (multiline), `Shift+Enter` —
   line break, `Enter`/`Ctrl+Enter` — send (configurable).
5. **`widgets/chat_list.rs`** (overlay)
   ([§11.2](../../spec.md#112-the-chat-list-an-overlay)): opened by `Ctrl+L` and
   a button; **search filter** by title; **two sort modes** (created/
   modified) with a toggle; **rename** (`F2`); create (with profile choice —
   a profile stub until M4), clone, delete; virtualization.
6. **`widgets/status_bar.rs`**: model, tokens/context, profile, server
   state.
7. **`screens/chat.rs`**: widget layout, focus routing, hotkey handling
   ([§11.7](../../spec.md#117-keybindings-preliminary)).
8. **`features/chat_search_sort.rs`, `features/rename_chat.rs`**:
   filter/sort/rename logic (pure functions, testable without UI).
9. **`features/spellcheck/`** ([§11.5](../../spec.md#115-input-and-editing-spellcheck)):
   - `dict.rs`: load `spellbook` Hunspell dictionaries from `dictionaries/`
     (en_US, en_GB, ru_RU) in the background; multiple active dictionaries
     (correct if accepted by at least one); a personal dictionary.
   - `segment.rs`: segmentation (`unicode-segmentation`, apostrophes/hyphens).
   - `check.rs`: background checking with a ~300 ms debounce; `suggest`.
   - `[R]` **Rendering errors in `tui-textarea`**: prototype — pick a
     mechanism (styling / a custom widget / a hotkey-triggered suggestion
     popup). Write up the decision.
10. **View-model projection**: applying `AppEvent` to read-only UI state;
    ignoring a "stale" `generation_id`.

### Tests
- `chat_search_sort`, `rename_chat`: pure logic (substring filter, both sort
  orders).
- `spellcheck`: `check`/`suggest` on en_US and ru_RU; segmentation (mixed
  ru/en, apostrophes/hyphens); multiple dictionaries; a personal dictionary
  (adding + persistence). `[R]` Check ru_RU quality.
- `markdown`: LaTeX substitutions; table/code rendering (a snapshot via
  `TestBackend`).
- View-model: applying a sequence of events, including a stale
  `generation_id`.

### DoD
- Full chat cycle in the TUI: chat list (search, 2 sort modes, rename), feed
  (markdown, thoughts, tool blocks, scroll), `tui-textarea` input.
- Spelling errors are visible, suggestions work (en + ru), "Add to
  dictionary" persists.

---

## M4. Profiles + isolation

**Goal:** profiles with an id, chat binding, notes/RAG isolation, greeting.

### Tasks
1. **Profile `features/`**: create/edit/soft-delete `Profile`; CRUD logic
   (the UI section is M8 — here it's the model and the operations).
2. **Creating a chat from a profile**
   ([§10](../../spec.md#10-ai-companion-profiles)): copy
   `default_system_message → Chat.system_message`, `character_names`,
   `greeting` (as the assistant's first message), `default_sampling`.
3. **Profile choice on `Ctrl+N`** (a selection overlay).
4. **Soft-delete cascade for a profile** → chats + notes/RAG exclusion.
5. **Wiring up isolation**: `profile_id` is threaded through every
   notes/RAG call (preparing `ToolContext` for M5).
6. **Sampling priorities**
   ([§8.3](../../spec.md#83-override-levels)):
   `Chat.sampling_override → Profile.default_sampling → global`; a snapshot
   of what applied goes into `Message.metadata`.

### Tests
- Creating a chat from a profile: correct copying of fields and the
  greeting.
- Sampling priorities (every override combination).
- Isolation: chat under profile A doesn't see profile B's notes/RAG (an
  end-to-end test through the orchestrator+storage).
- Soft-delete cascade for a profile.

### DoD
- A chat is created from a profile with a greeting; editing a profile
  doesn't change existing chats.
- Data is isolated by `profile_id` (confirmed by tests).

---

## M5. Tools (base) + client-side agentic loop

**Goal:** a tool registry, a client-side agentic loop, notes + RAG
(+ on-demand embeddings) + introspection.

### Tasks
1. **Contract (`features/tools/mod.rs`)**
   ([§9.2](../../spec.md#92-the-tool-contract)): trait `Tool`,
   `ToolContext` (a snapshot), `ToolOutcome { result, effects }`,
   `ChatEffect`, `ToolRegistry`.
2. **Client-side agentic loop in the orchestrator**
   ([§6.3](../../spec.md#63-client-side-agentic-loop)): handling
   `finish_reason = ToolCalls`, accumulating `tool_calls` from the stream,
   invoking tools, appending assistant(tool_calls)+tool messages to history,
   applying `effects`, looping up to `max_tool_rounds` (8). Tool-call
   progress events in the UI.
3. **Building the `ToolContext` snapshot** at the start of a turn
   (`system_message`, `effective_sampling`, `last_user_message_at`,
   `Arc<Storage>`, `Arc<dyn EngineBackend>`).
4. **Passing tool schemas** in `ChatRequest.tools` (only ones enabled in the
   profile ∩ enabled globally).
5. **On-demand embeddings**
   ([§16.1 #4](../../spec.md#161-accepted-decisions)): `EngineBackend::embed` —
   `[R]` bring up a secondary `xinfer` embedding server on a separate port,
   use it, stop it; choose model/dimensionality. Fallback — a lightweight
   in-process embedder. Write up an ADR.
6. **Tools:**
   - `note_save`, `note_recall` (`features/tools/notes.rs`).
   - `rag_add`, `rag_search` (`features/tools/rag.rs`): chunking,
     embedding, kNN.
   - Introspection (`features/tools/introspection.rs`): `get_sampling`,
     `set_sampling` (→ `effect`), `get_system_message`, `set_system_message`
     (→ `effect`), `get_last_user_message_time`.
7. **Showing tool blocks** in the feed (name/args/result, collapsible) —
   wired to `ToolCallRecord` ([§5.1](../../spec.md#51-core-entities-entities)).

### Tests
- Each tool against a mock `Storage`/mock engine: correct result; mutating
  ones return `effects` instead of touching `Chat`; valid JSON schemas.
- Agentic loop on a replay fixture: message → tool-call → effect →
  tool-result → final answer; `max_tool_rounds` respected; `set_system_message`/
  `set_sampling` apply starting from the next turn.
- `[ignore]` with a real model: a full agentic cycle with a test tool.

### DoD
- The assistant saves/retrieves notes and knowledge, reads/changes the
  system message and sampling; tool blocks are visible in the UI; the round
  limit is respected.

---

## M6. call_subagent

**Goal:** a subagent for a second opinion, with nesting forbidden and
limits.

### Tasks
1. **`features/tools/subagent.rs`**
   ([§9.3.2](../../spec.md#932-call_subagent)): through `ctx.engine` — an
   independent single-turn request (`system = system_message` from the main
   agent, one user message = `message`), **no history, no tools, no
   nesting**.
2. **Limits**: tokens/time per call; subject to `max_tool_rounds`.
3. **Recursion protection**: the subagent isn't given any tools (including
   `call_subagent`).
4. **UI**: showing the subagent call as a tool block (system_message +
   message + reply, collapsible).

### Tests
- The subagent gets no tools and no history; limits apply.
- `[ignore]` with a real model: the call returns a string; no
  recursion/looping.

### DoD
- The model calls `call_subagent`, gets a string result back; no recursion;
  limits are respected.

---

## M7. Web + Python

**Goal:** a homegrown web search (DuckDuckGo) and Python execution, behind
switches.

### Tasks
1. **`features/tools/web.rs`**
   ([§9.3.1](../../spec.md#931-the-web-tool-our-own-implementation)):
   `web_search` — a query to **DuckDuckGo** (`reqwest`), parsing results
   (`scraper`), `[R]` pick an endpoint (HTML/lite/Instant Answer);
   readable-content extraction from pages (a readability crate/`scraper`);
   optional reranking via `engine.embed`. A global switch.
2. **`features/tools/python.rs`**
   ([§13.2](../../spec.md#132-python-execution)): a Python subprocess (the
   interpreter path from settings), stdout/stderr capture, **a timeout**, an
   output-size cap. **Off by default on Windows** (no OS sandbox) + a
   warning. On Linux — a timeout + a separate process.
3. **Global tool switches**
   ([§9.4](../../spec.md#94-enabling-tools)): the effective set =
   `enabled_tools ∩ globally enabled`.

### Tests
- `web`: parsing a fixture DDG HTML response (no network); content
  extraction from a fixture.
- `python`: running simple code (if the interpreter is available — otherwise
  `[ignore]`); a timeout interrupts; output is truncated; off by default on
  Windows.
- Global switches correctly filter tools.

### DoD
- Web search and code execution work behind switches; Python is off by
  default on Windows.

---

## M8. Settings screen ✅

**Goal:** every settings section, profile CRUD, server/model management.

**Status:** done (branch `m8-settings`). `AppConfig` extended
(embed/subagent/interface); the orchestrator owns the config and servers
through the `ServerSupervisor` trait (real + mock), changing the model
restarts the managed server; `screens/settings.rs` has every section with
live edit application (`UpdateConfig`/`UpdateProfile`). Theme and custom key
bindings — fields are saved, actual application lands in M9.

### Tasks
1. **`screens/settings.rs`** ([§11.6](../../spec.md#116-the-settings-screen)):
   navigation by section (a side menu/tabs), entered via `Ctrl+P`.
2. **Model/server section**: mode (managed/external), path to `xinfer`,
   model source (`--m/--w/--f`), quantization/format, devices (`--d`), port;
   viewing metadata and the launch log. **Changing the model = restarting
   the server.**
3. **Inference section**: `max_tool_rounds`, timeouts, applicable
   context/KV parameters.
4. **Sampling section**: temperature, top-k/p, min-p (if supported),
   penalties, max_tokens, seed.
5. **Profiles section**: CRUD, editing the system message, role names,
   greeting, tool set, sampling defaults.
6. **Tools section**: global switches (web, Python), the embeddings model,
   `call_subagent` limits, the Python path.
7. **Interface section**: theme, spellcheck (on/off, dictionary selection),
   key layout.
8. **Saving** every setting into `settings.json`/`profiles.json` (via the
   orchestrator — the sole writer).

### Tests
- Settings serialize/deserialize round-trip; sampling/tool-setting changes
  apply to the request.
- Changing the model triggers a server restart (against the mock
  supervisor).
- Settings view-model: navigation, field editing.

### DoD
- Every setting and `xinfer` launch parameter is editable on a dedicated
  screen; changing the model restarts the server; profiles are managed
  through the UI.

---

## M9. Gemma + polish ✅

**Goal:** verify Gemma 3/4, themes, migration, backups, release builds for
Windows and Linux.

**Status: done.** LameLLaMA (.NET) importer (idempotent, verified against
real data), removed the crate-wide `allow(dead_code)`, finalized the
chat-file backup, UX polish (the `F1`/`?` help overlay), a release profile +
`docs/install.md`, **themes (`shared/theme.rs`, auto/dark/light)**, **live
application of spellcheck settings**. **Gemma 4 E4B-it verified** against a
live `llama-server` (llama.cpp): added `#[ignore]` smokes — anti-
self-truncation (`<|im_end|>`+`<end_of_turn>`), tool-calling, "thoughts"
(`reasoning_content`), all green. Engine: `xinfer` turned out too raw for
Gemma 4 → the working path is external `llama-server` (OpenAI protocol,
`--jinja`).

### Tasks
1. **`[ignore]` Gemma 3/4 verification**: chat templates, **EOS** (anti-
   self-truncation on `<end_of_turn>`/`<eos>`), tool-calling, parsing
   "thoughts". Run the same scenario set used for Qwen.
2. **Themes (`shared/theme.rs`)**: light/dark/auto; applied across widgets.
3. **Migration** ([§12.2](../../spec.md#122-schema-versioning-and-migration)): an importer
   from `lamellama-rs` (if such data exists) and optionally from LameLLaMA
   (.NET): profiles (a new id), chats, sampling (dropping unsupported
   fields), the spellcheck config; impersonation/the KV cache aren't
   migrated; idempotency.
4. **Backups**: finalize atomic write + chat-file backup.
5. **UX polish**: hotkey help (`?`), status indicators, server/network
   error handling in the UI.
6. **Release build**: build profiles for Windows and Linux; install/
   `xinfer`-path documentation
   ([§16.2](../../spec.md#162-open-questions-to-be-settled-during-implementation)).
7. **Run every `[ignore]` test** against Gemma and Qwen; a prefix-caching
   smoke (a second turn is faster).

### Tests
- Importer: fixtures of old formats → a correct result; re-import
  idempotency.
- Full unit + integration run without a model; a manual `[ignore]` run
  against both model families.

### DoD
- Chat and tools work on Gemma the same as on Qwen; old data imports;
  release builds for Windows and Linux.
- The end-to-end quality bar is met
  ([§15.1](../../spec.md#151-cross-cutting-quality-criteria)).

---

## Research-task `[R]` summary (close as early as possible)

| `[R]` | Stage | Question | Impact |
|---|---|---|---|
| ~~Engine API~~ ✅ | M1 | **closed**: OpenAI-compatible protocol, see [docs/install.md §3](../install.md) | `shared/api` structure |
| HTTP client | M1 | `async-openai` vs `reqwest`+our own types | engine implementation |
| sqlite-vec | M2 | building/linking on Windows and Linux | RAG storage |
| ~~UI crates~~ ✅ | M3 | **closed** → [ADR 0001](../decisions/0001-ui-crates-ratatui-030.md): `tui-markdown`+`tui-scrollview`, input — our own widget | widgets |
| ~~Spellcheck in a textarea~~ ✅ | M3 | **closed** by the same ADR: our own widget → we draw the underlines ourselves | input UX |
| `spellbook` ru_RU quality | M3 | affix rules | spellcheck |
| `xinfer` embeddings | M5 | on-demand secondary server; model/dimensionality | RAG |
| ~~DuckDuckGo endpoint~~ ✅ | M7 | **closed**: `lite.duckduckgo.com/lite` (POST `q=`) | web tool |
| `xinfer` distribution | M8/M9 | install/path/packaging | distribution |

---

## Stage order and dependencies

```
M0 ─▶ M1 ─▶ M2 ─▶ M3 ─▶ M4 ─▶ M5 ─▶ M6 ─▶ M7 ─▶ M8 ─▶ M9
                    │            │
              (UI on top of  (tools on top of
               domain+engine) domain+profiles+engine)
```

- **M1 and M2** are independent after M0 (can run in parallel: engine vs.
  domain model), converging at M3.
- **M5/M6/M7** build on M4 (isolation/profiles) and the M5 agentic-loop
  infrastructure.
- **M8** requires M4–M7 (there has to be something to configure). **M9** —
  final verification and release.

Each stage wraps up with a merge into `main` once its DoD is met;
`[ignore]` tests against a real model are run manually at M1, M5, M6, M9.
