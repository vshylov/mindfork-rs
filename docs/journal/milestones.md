# Journal — Milestones M3–M9

The original step-by-step plan's milestones, kept verbatim for provenance. The current state of everything described here is in architecture.md and spec.md — read those first; this file answers "how did it get this way".

**Reference documents for this area:** docs/history/plan.md

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (9)

- M3 (chat UI) — done so far
- M4 (profiles + isolation) — done so far
- M5 (tools + agentic loop) — done so far
- M6 (`call_subagent`) — done so far
- M7 (web + Python) — done so far
- M8 (settings screen) — done so far
- M9 (polish) — done so far
- M9 — Gemma check (done)
- Deferred beyond M3

### M3 (chat UI) — done so far
- **`[R]` UI crates closed** → [ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md):
  `tui-textarea`/`ratatui-markdown` lock in ratatui 0.29 → we take `tui-markdown` +
  `tui-scrollview`, and input is a **custom widget** (this also resolves the spellcheck
  rendering `[R]` — we draw the underlines ourselves).
- **`Storage` wired to the orchestrator**: multi-chat, bootstrap (default profile+chat),
  load/save with debounce (800ms), new `AppCommand`/`AppEvent`
  (`NewChat/SwitchChat/RenameChat/CloneChat/DeleteChat`, `ChatList/ChatActivated`).
  `ChatSummary` lives in `entities/chat.rs` (needed by both `app` and `widgets`).
- **`shared/markdown.rs`**: rendering via `tui-markdown` + a unicode approximation of LaTeX.
  **Replaced post-M9** with an own `pulldown-cmark`-based renderer (tables +
  delimiter-scoped LaTeX + theme) → [ADR 0003](docs/decisions/0003-own-markdown-renderer.md).
- **`features/chat_search_sort.rs`, `features/rename_chat.rs`**: pure logic.
- **`widgets/chat_list.rs`**: the `Ctrl+L` overlay (search, 2 sort orders via `Tab`,
  `F2` rename, `Ctrl+N` new, `Ctrl+D` clone, `Del` delete). Integrated into `runtime.rs`.
- **`widgets/input_box.rs`**: an own multiline input widget (`Vec<Vec<char>>`, cursor by
  character, scrolling; `Shift+Enter` for a line break, `Enter` to send). Replaced the
  placeholder single-line field.
- **`widgets/message_feed.rs`**: the feed with markdown rendering, a collapsible
  "thoughts" block (`Ctrl+T`), scrolling via `PageUp/PageDown` **and the mouse wheel**
  (`scroll_up`/`scroll_down`, the `Ctrl+W` capture toggle — see post-M9 below) with
  "tail-following".
- **Word wrap (`shared/wrap.rs`)**: lines are word-wrapped **before** `Paragraph`
  (a long word breaks by character; width in columns via `unicode-width` — Cyrillic
  = 1, emoji/CJK = 2). In `message_feed` this preserves row-based scrolling/"tail"; in
  `input_box` it gives an exact cursor mapping `(line, column)→(visual row, column)` and
  field-height growth under wrapping (`visual_line_count`). `Paragraph::wrap` is
  deliberately not used (it would break the scroll math and cursor position).
- **`widgets/status_bar.rs`**: a pure status widget (`ServerStatus` moved into `shared`).
- **`screens/chat.rs`**: `ChatScreen` — all UI state, mutators projecting events,
  `handle_key → ChatIntent`, `render`. `runtime.rs` is now a thin loop: it applies
  `AppEvent` via mutators, translates `ChatIntent → AppCommand`. **FSD is respected**:
  `screens`/`widgets` don't import `app` (the screen emits `ChatIntent`, not `AppCommand`).

- **`features/spellcheck/`**: `spellbook` (Hunspell) + an own word segmenter +
  a personal dictionary. `SpellChecker`: `check`/`misspellings`/`suggest`/
  `add_to_personal`. A word is correct if accepted by at least one dictionary.
  Dictionaries load **in the background** from `dictionaries/` (missing directory → the
  check is off, no crash). UI: underlining errors (`UNDERLINED`/red) with a 300ms
  debounce; a suggestions popup on `Ctrl+G` (word replacement / "add to dictionary" →
  `personal_dictionary.txt`).

### M4 (profiles + isolation) — done so far
- **`features/profiles.rs`**: pure profile operations (create/edit/
  name validation `sanitize_name`, `ProfileEdit`/`apply_edit`). UI section — at M8.
- **`Chat::from_profile` copies** `system_message`/`character_names` (the chat has its
  own copy — profile edits don't change them), but **does NOT copy** `default_sampling`.
- **Three-level sampling resolution** ([`sampling::resolve`], spec §8.3):
  `Chat.sampling_override → Profile.default_sampling → global` resolved **at request
  time** (not snapshotted at creation); a snapshot of what was applied goes into
  `Message.metadata`. `sampling_override` stays `None` until an explicit `set_sampling`
  (M5).
- **Contract**: `AppCommand::NewChat{profile_id}`, `CreateProfile`/`DeleteProfile`;
  `AppEvent::ProfileList`. `ProfileSummary` — in `entities/profile.rs`.
- **Orchestrator**: owns profiles, emits `ProfileList`, creates a chat from the selected
  profile (+ a greeting), `DeleteProfile → chats` cascade (`hide_profile_cascade`),
  a "can't delete the last profile" guard.
- **`widgets/profile_list.rs`**: the profile-picker overlay. `Ctrl+N` opens it when
  there's >1 profile, otherwise creates one right away. Wired in `screens/chat.rs` +
  `runtime.rs`.
- **Isolation** of notes/RAG by `profile_id` is confirmed by tests (db + `Storage`
  facade). The real `ToolContext` is an M5 deliverable (here `profile_id` is already
  available from the chat).

### M5 (tools + agentic loop) — done so far
- **`[R]` Embeddings closed** → [ADR 0002](docs/decisions/0002-embeddings-dedicated-server.md):
  a **dedicated** embedding server (the `Embedder` trait is split off from `EngineBackend`);
  `XinferClient: Embedder`; env `MINDFORK_EMBED_URL`/`_BIN`/`_MODEL`/`_PORT`;
  when not configured — `UnavailableEmbedder` (RAG returns an error, doesn't crash).
- **Engine: tool calls** — `ChatChunk::ToolCall(ToolCallDelta)` + `ToolCallAccumulator`,
  `ChatRequest.tools`/`tool_choice=auto`, serializing `assistant.tool_calls` into
  history and parsing `delta.tool_calls` (wire/client).
- **`features/tools/`**: the `Tool` trait, `ToolContext` (a snapshot), `ToolOutcome`+`ChatEffect`,
  `ToolRegistry` (`schemas_for` = profile ∩ registry, `invoke`). `standard_registry`/
  `default_tool_ids`. Tools: introspection (`get/set_sampling` with merge,
  `get/set_system_message`, `get_last_user_message_time`), notes (`note_save`/
  `note_recall`), RAG (`rag_add`/`rag_search`: chunking+embedding+kNN). All with
  `profile_id` isolation.
- **Orchestrator: client-side agentic loop** (spec §6.3): stream → `ToolCalls` →
  execution → a new request, up to `max_tool_rounds`; effects are applied by the
  orchestrator (owner of `Chat`) starting the next turn (§6.6). `Storage` → `Arc<Storage>`.
- **UI**: `AppEvent::ToolCall` → tool blocks (🔧 name/arguments/result) in the feed;
  tool messages aren't duplicated (shown as blocks inside the assistant's reply).
- Still pending: manual `#[ignore]` verification of the full agentic cycle against a
  real model (needs xinfer with tool calling), and the quality of the dedicated
  embedding model.

### M6 (`call_subagent`) — done so far
- **`features/tools/subagent.rs`**: `call_subagent` (spec §9.3.2) — an independent
  single-turn request via `ctx.engine`: `system` = as given, a single `user`
  = `message`, **no history, no tools** (`tools: []` → bans nesting/
  recursion). Token limit (≤1024) and a timeout (60s, cancellable via `CancellationToken`).
- Registered in `standard_registry`/`default_tool_ids`; in the UI — a regular
  tool block (name/arguments/result). Subject to `max_tool_rounds` (one round).

### M7 (web + Python) — done so far
- **`[R]` DDG endpoint closed**: `lite.duckduckgo.com/lite` (POST `q=`) — plain
  stable HTML. `reqwest` now with `rustls`+`form`; `scraper` added.
- **`features/tools/web.rs`**: `web_search` — **multi-provider with fallback**
  (`PROVIDERS`), each with its own infrastructure and markup: DDG lite
  (`a.result-link`/`td.result-snippet`) → DDG html (`a.result__a`/`a.result__snippet`)
  → **Mojeek** (`GET ?q=`, `a.title`/`p.s`) → **Ecosia** (`GET ?q=`, stable
  `data-test-id`; title and link are DIFFERENT tags, the parser aligns href/title/
  snippet by index). The first one with a non-empty result set wins; the real
  URL is decoded from `uddg=`. **Anti-bot throttling is detected** (`is_throttled`: DDG `HTTP 202`/
  the `anomaly` marker, Mojeek/others `403`/`429`) — previously a 202 passed as "success"
  (`error_for_status` lets 2xx through) and parsed to empty → the model saw "no
  results" for a valid query, and a 403 surfaced as a fatal "provider
  unavailable" error. Now on throttling the next provider is tried right away (throttling
  is sticky per-IP, retries only make it worse; providers throttle independently → almost
  always someone answers); if **all** are unavailable — an explicit error (not "no
  results"), so the model retries later. Request timeout 15s.
- **Content extraction + reranking (post-M9, done)**: after the results page
  arrives, results are fetched in parallel (`enrich_with_content` →
  `futures::join_all`, browser-like `Accept`/`Accept-Language` headers —
  otherwise some sites serve a block page); readable text is extracted from the HTML
  (`extract_readable` via `scraper`: paragraphs/lists `<p>`/`<li>` from `<article>`/`<main>`,
  otherwise from the whole document; **everything inside `nav`/`header`/`footer`/`aside` is
  dropped** — `in_boilerplate` walks ancestors, otherwise mega-menus leaked into
  content on sites without semantic markup; fragments < 40 chars are also filtered out;
  script/style outside `<p>` → dropped themselves; truncation to `MAX_CONTENT_CHARS=1500`).
  Fetch failures/non-2xx/empty extraction are logged to `logs/` (`tracing::debug`, for
  diagnostics). Results are then **reordered via embeddings**
  (`rerank_by_embeddings` through `ctx.embedder`, ADR 0002): the query + each result's
  `title`+content (≤`RERANK_EMBED_CHARS=800`) are embedded in one request,
  sorted by decreasing cosine similarity
  (`rerank_order`, stable — ties keep the provider order). All of this is
  **"best effort"**: one page's fetch failure just leaves its `content`
  empty (bot-hostile sites like Bloomberg return 403 — that's expected); if the embedder
  isn't configured/unavailable (RAG is off) or returned a mismatched vector count →
  reranking is skipped, provider order remains (graceful degradation, like RAG's).
  Output includes a "Content:" block with the extracted text.
- **`fetch_content` gate**: the `fetch_content` call argument (`false` → the fast path
  "titles/snippets only", no fetching/reranking) overrides the default from
  `config.tools.web_fetch_content` (default `true`). The default is threaded through
  `ToolConfig.web_fetch_content` → `WebSearch::new(bool)`; a "Web: fetch pages" toggle
  is in the "Tools" section of the settings screen (with a hint).
- **`features/tools/python.rs`**: `python_exec` — a separate process (`python -c`),
  10s timeout (kill_on_drop), output truncation. Interpreter path from config.
- **Global switches** (`config.tools`: `web_enabled=true`, `python_enabled=
  false`, `python_path`). The effective set = profile ∩ switches
  (`effective_tool_ids`); the agentic loop **also gates the call itself** (a disabled tool
  is rejected, not executed). Python is off by default on Windows.
- Real network/Python runs are `#[ignore]` (need network/interpreter).

### M8 (settings screen) — done so far
- **`AppConfig` extended** (`shared/config.rs`): `EmbedSettings` (the dedicated
  embedding server, ADR 0002), `ToolSettings.subagent_max_tokens/_timeout_secs`,
  `InterfaceSettings` (`Theme` auto/dark/light, `spellcheck_enabled`,
  `selected_dictionaries`). All through `#[serde(default)]` — old `settings.json`
  files load without migration.
- **`ToolConfig`** (`features/tools`): the registry is built from config
  (`standard_registry(&ToolConfig)`); `CallSubagent::new(max_tokens, timeout)` —
  limits aren't hardcoded; rebuilt on `config.tools` edits.
- **The orchestrator owns the whole `AppConfig`** and the servers via
  **`ServerSupervisor`** (`app/supervisor.rs`, real `XinferSupervisor` + mock):
  `apply_chat`/`apply_embed` bring up servers from config; `ServerHandle` lives in
  the orchestrator (`kill_on_drop`). `main.rs` no longer resolves the backend — it just
  loads the config and seeds it via env vars (`apply_env_overrides`, dev workflow).
- **Settings commands/event**: `AppEvent::Settings { config, profiles }` (a full
  snapshot); `AppCommand::UpdateConfig`/`UpdateProfile` — edits are persisted
  (`save_config`/`upsert_profile`), **switching `xinfer` restarts the server**, switching
  `tools` rebuilds the registry, switching `embed` re-raises the embedding server.
  An internal `ServerStatus` channel (background supervisor probe → `AppEvent`).
- **`screens/settings.rs`** (`SettingsScreen` + `SettingsIntent`, entry `Ctrl+P`):
  a left section menu (Model/Inference/Sampling/Profiles/Tools/Interface) +
  a right field list. `Tab` for section, `↑↓` for fields, `Space` toggle, `←→` enum,
  `Enter` a text/number editor. Edits apply **immediately on commit**
  (`SettingsIntent → AppCommand`). Profiles: select via `←→`, edit fields, tool
  toggles, `Ctrl+N`/`Ctrl+D` to create/delete. **FSD is strict**: `screens` doesn't
  import `app`.
- **`runtime`**: the settings screen sits over the chat; events keep applying to the chat
  (generation isn't interrupted); a `Settings` re-emit refreshes the working copy (create/
  delete of profiles is visible right away).
- Theme/keyboard layout in "Interface" are, for now, just **fields** (values are saved);
  actually applying the theme (`shared/theme.rs`) and custom layouts — planned for **M9**.
  Spellcheck on/off and dictionary selection are also editable, but actually
  applying them to dictionary loading — M9.

### M9 (polish) — done so far
- **LameLLaMA (.NET) importer** (`features/migration.rs`, spec §12.2): CLI
  `mindfork import-lamellama <dir>` (originally the `--import-lamellama` flag; later
  moved to a clap subcommand, see post-M9 backup/restore). `Settings.json` (`Configurations` → profiles
  with a deterministic UUIDv5 → idempotent) + `Conversations/*.json` → chats
  (keeps the original Id; `*.deleted` are skipped). Sampling with unsupported fields
  dropped, spellcheck/theme → `config.interface`. A UTF-8 BOM is stripped. **Verified on
  real data: 6 profiles, 226 chats.**
- **Crate-wide `#![allow(dead_code)]` removed**: dead code was deleted (fields
  `State`/`GenResult`, `shared/error.rs`, unused `is_empty`) or marked
  with a targeted `#[allow(dead_code)]`/`#[cfg(test)]` and a comment.
- **Chat file backup** (atomic write + `.bak`) finalized with a test (§12.3).
- **UX**: a keyboard-shortcut help overlay (`F1`/`?`), a status-bar hint.
- **Release**: `[profile.release]` (LTO/strip, `panic=unwind`); `docs/install.md`
  (Win/Linux build, portable data, xinfer managed/external + env, dictionaries,
  import, launch). `cargo build --release` builds.
- **Themes** (`shared/theme.rs`, branch `m9-themes`): a semantic `Palette`
  (roles user/assistant/tool/success/warning/error/accent) driven by
  `config.interface.theme` (auto = named ANSI; dark = bright; light = RGB-
  darkened). Threaded into widgets with real colors (`message_feed`,
  `status_bar`, `input_box`, `chat_list`); modifiers (dim/bold/reversed) are
  theme-independent. `ChatScreen` refreshes the palette from the `Settings` event.
- **Spellcheck obeys settings**: `dict::load(enabled, selected)` loads
  only the selected dictionaries (or nothing when disabled). `app/runtime.rs` owns the
  loading — `SpellLoader` reloads dictionaries in the background on changes to
  `interface.spellcheck_enabled`/`selected_dictionaries` (generation-guarded), so the
  toggle and dictionary selection apply live.
- **Layout-independent Ctrl shortcuts** (`shared/keys.rs`): a character is normalized
  to its "physical" Latin key (a Russian JCUKEN lookup table), so `Ctrl+L/N/G/T/C/P`
  work even on a Cyrillic layout (where crossterm reports `Ctrl+д` etc.).
  Applied in `screens/chat.rs`, `screens/settings.rs`, `widgets/chat_list.rs`.
  **The settings screen moved from `Ctrl+,` to `Ctrl+P`** (`Ctrl+,` on Windows
  is intercepted by Windows Terminal).

### M9 — Gemma check (done)
- **Gemma 4 E4B-it verified** against a live `llama-server` (llama.cpp). Added/
  extended `#[ignore]` smokes in [client.rs](src/shared/api/openai/client.rs):
  anti-self-cutoff for both families (`<|im_end|>` + `<end_of_turn>`), tool calling
  (`finish_reason=tool_calls` + parsing `delta.tool_calls`), "thoughts"
  (`reasoning_content` → `Thoughts`). All green. Nuance: Gemma reasoning "thinks"
  before calling a tool — smokes use a generous `max_tokens` (512).
- **`xinfer` doesn't work for Gemma 4** (too raw); the working path is external `llama-server`.

### Deferred beyond M3
- **Per-message collapse/selection** in the feed — "thoughts" (`Ctrl+T`) and tool
  calls (`Ctrl+O`) collapse **for the whole feed at once**, with the state stored
  per chat; folding one *particular* block still needs a way to select it.
  **Account for the mouse toggle** (`Ctrl+W`, see post-M9): per-message selection/copy
  in the feed must coexist with wheel capture — either as in-app selection
  (mouse clicks go to the app when capture is on), or relying on native
  terminal selection when it's off (then copying is a plain drag-select).
- **Spellcheck dictionaries** (Hunspell pairs `*.aff`+`*.dic`: `en_US`/`en_GB`/`ru_RU`)
  live in the project root's `dictionaries/` **and are committed to the repository**. `build.rs`
  copies them to `target/<profile>/data/dictionaries/` at build time (out-of-the-box
  dev spellcheck); the release workflow packs them into the archive as
  `data/dictionaries/` (out-of-the-box spellcheck in the release too). Open `[R]`: ru_RU
  quality (in practice — it works).
- ~~Remove `#![allow(dead_code)]` from `main.rs`~~ — **done at M9**: dead code
  removed or marked pointwise (`#[allow(dead_code)]`/`#[cfg(test)]`).
