# Journal — UI: screens

The screens above the feed: settings, the chat list, the self-model viewer, search, help/About, and the popups and confirmations that belong to them.

**Reference documents for this area:** architecture.md §10, spec.md §11.6–§11.8

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (32)

- Post-M9: full-screen chat list window + auto-title (done)
- Post-M9: edit/regenerate the last reply (done)
- Post-M9: copy chat conversation to clipboard (done)
- Post-M9: intuitive `Esc`/`Ctrl+C` for the chat list and quitting (done)
- Post-M9: single-line settings fields + system-message popup (done)
- Post-M9: chat list moved to a separate screen (done)
- Post-M9: configurable `Ctrl+E`/`Ctrl+R` confirmation (done)
- Post-M9: settings-screen redesign — stage 1 (IA + field groups) (done)
- Post-M9: settings-screen redesign — stage 2 (subsection tabs + contextual footer) (done)
- Post-M9: settings-screen redesign — stage 3 (profiles: grouped tools + gates) (done)
- Post-M9: settings-screen redesign — stage 4 (field search `/`) (done)
- Post-M9: settings-screen redesign — stage 5 (Choice popup, validation, `Del` reset, `•` marker) (done)
- Post-M9: settings-screen redesign — stage 6 (server-status chips on-screen) (done)
- Settings-screen redesign (stages 1–6) — summary
- Post-M9: settings — a shared value column per section (done)
- Post-M9: settings-section field counter tied to the selected mode (done)
- Post-M9: API keys in settings — stage 2 (UI: input field, masking, status) (done)
- Post-M9: help/"About" dialog with tabs (F1) (done)
- Post-M9: custom user/assistant names in profile settings (done)
- Post-M9: impersonation profiles + a newly created profile is selectable (done)
- Post-M9: full-text search over chat content — stage 1 (done)
- Post-M9: chat content search — stage 2, jump and the results screen (done)
- Post-M9: settings-screen focus model (done)
- Post-M9: undoing an edit on the settings screen (done)
- Post-M9: a settings hint always fits its panel (done)
- Post-M9: a disclaimer for what the models say and do (done)
- Post-M9: the help tabs stopped clipping their descriptions (done)
- Post-M9: the help dialog sizes itself, and its tables align (done)
- Post-M9: `/export` — a conversation to a file (done)
- Post-M9: the "Components" leaders got a step quieter (done)
- Post-M9: automatic chat titling on the first exchange (done)
- Post-M9: the settings hint panel — fixed height, scroll, its own focus stop (done)

### Post-M9: full-screen chat list window + auto-title (done)
- **The chat list window (`Ctrl+L`) is now full-screen** (`widgets/chat_list.rs`):
  `render` draws a block spanning the whole `area` (previously a centered 60×70%
  popup); `centered_rect` removed.
- **Auto-title for a chat (`Ctrl+R` in the chat-list window)**: the model reads the
  conversation (or its start+end, if long) and comes up with a short title. Pure logic —
  `features/rename_chat.rs`: `build_conversation_digest` (tags roles, skips
  system/tool/empty ones, truncates the middle to a budget
  `TITLE_CONTEXT_BUDGET`), `TITLE_SYSTEM_MESSAGE`, `clean_generated_title` (strips
  quotes + `sanitize_title`). The request runs as a **background task** (`spawn_title`):
  an independent single-turn request with no history/tools, a 30s timeout; the result
  goes into an internal `title_tx` channel → `Orchestrator::handle_title_result`. Gated by
  server readiness (`ready_backend`); an empty chat → a clear error. Contract:
  `ChatListAction::AutoRename` → `ChatIntent::AutoRenameChat` →
  `AppCommand::AutoRenameChat`.
- **"Thoughts" for the title are suppressed through three layers + a guaranteed fallback.**
  For models with thinking "baked into" the GGUF (Gemma `peg-gemma4`), the server
  ignores the `thinking`/`reasoning_effort` fields, and the model spends **all**
  of `max_tokens` on reasoning → the resulting `content` is empty ("The model didn't
  return a title"; in the llama-server logs, `thinking = 1`, cut off by the token
  budget on reasoning). Fix: (1) a new `SamplingConfig.reasoning_budget=0` field (for
  llama.cpp built-in formats); (2) `wire.rs`, when `reasoning_budget=0`, also signals
  through `chat_template_kwargs.enable_thinking=false` (models' Jinja templates); (3) a
  generous `TITLE_MAX_TOKENS=2048`/60s timeout — if turning it off didn't work, the
  model still has time to **finish** "thinking" and produce a title in `content`; (4)
  `salvage_title_source` — if `content` still ends up empty, the title is taken from the
  last substantive line of `reasoning_content` (`Thoughts`), so the user no longer sees
  an error. For regular generation `reasoning_budget=None` — the fields aren't sent,
  behavior doesn't change.
- **A dedicated error area in the chat-list window** (`AppEvent::ChatListError`):
  list-operation errors (auto-title/delete/clone) go not into the chat feed (where they
  used to sit behind the full-screen overlay until a restart), but into a dedicated
  overlay status line (`ChatListState.error`, colored `palette.error`), which **fades on
  the first keypress**. `ChatScreen::set_overlay_error` routes into the overlay if it's
  open, otherwise (a late auto-title reply after the overlay closed) — as a note in the
  feed. Previously these errors went through the generic `AppEvent::Error`.
- **`AppEvent::ChatRenamed { id, title }`**: updates the active chat's feed header
  without a rebuild (previously manual renaming also didn't update the title until
  reactivation). Emitted from `handle_rename` and `handle_title_result`;
  `ChatScreen::rename_chat` applies it. The list/overlay update as before via
  `ChatList`. The shortcut is layout-independent (`shared/keys`, physical R = `Ctrl+к`).

### Post-M9: edit/regenerate the last reply (done)
- **Regenerate (`Ctrl+R`)**: truncates the chat history up to and including the last
  user message (the previous assistant reply + tool messages are dropped)
  and reruns generation with the same request. The shared send/regenerate path is
  factored into `Orchestrator::start_generation`; the feed is rebuilt via a re-emit of
  `ChatActivated`. Command `AppCommand::RegenerateLast`, intent
  `ChatIntent::RegenerateLast`.
- **Delete the last exchange (`Ctrl+E`)**: removes the last assistant reply
  together with the user message that triggered it; the user's text is restored into
  the input box (`AppEvent::RestoreInput` → `ChatScreen::restore_input`). If the
  input box isn't empty, the text is prepended to the existing content (nothing is lost).
  Command `AppCommand::DeleteLastExchange`, intent `ChatIntent::DeleteLastExchange`.
- Both operations are **gated by `Idle` state** (ignored during generation — both at the
  screen level and in the orchestrator) and are a no-op if the chat has no user
  message (e.g. a chat that only has a greeting). Shortcuts are layout-independent
  (`shared/keys`), added to the help overlay (`F1`/`?`).
- **Server-readiness gate**: the orchestrator holds `server_status` (updated from an
  immediate `ChatSetup.status` and a background probe). Send/regenerate only start when
  `Ready` — otherwise the request would go to a still-loading managed server and return
  `503` ("engine returned an error status"). The readiness check in regenerate runs
  **before** truncating the history (otherwise the old reply would be wiped out and the
  new one wouldn't arrive); if the send is rejected, the text is restored to the input
  box (`RestoreInput`), not lost. Not-ready is shown with a clear message ("Server is
  still connecting…"). `MockSupervisor` returns `Ready` synchronously — tests don't
  depend on the status race.
- **Readiness probe accounts for model loading** (`OpenAiClient::probe`): a managed
  `llama-server` binds the HTTP port right away, but for ~seconds (a large GGUF) responds
  `503 Loading model` to inference. The old probe hit `/usage` and treated any response
  as "ready" — `Ready` status was set before loading finished, and the very first request
  (send/regenerate) failed with `503` through the readiness gate. Not reproducible on
  external (the server is already loaded). Fix: the probe hits `/health` (at root, outside
  `/v1`) and treats `503` as "still loading" (not ready), `200` as ready, `404`
  (server without `/health`) as "alive, not loading" (ready). Now `server_status`
  becomes `Ready` only after the model is actually loaded, and the readiness gate
  correctly holds back send/regenerate with a clear message.
- The client no longer **swallows the error body**: status + JSON reason
  (`{"error":{"message":…}}`) are logged to file and included in the error text (truncated
  to 500 chars) — instead of the useless "engine returned an error status". It was exactly
  this body (`503 Loading model`) that pointed to the root cause above.

### Post-M9: copy chat conversation to clipboard (done)
- **`F5` copies the whole chat conversation to the system clipboard** — both in the
  chat-list window (`Ctrl+L`, copies the selected chat), and in the main chat window
  (copies the active chat). `F5` (not `Ctrl+C`) — to avoid confusion with "quit" via
  `Ctrl+C` at the screen level; a function key doesn't depend on the layout (F1 — help, F2 —
  rename are already taken). In the main window `F5 → ChatIntent::CopyChat(active)`
  (the overlay intercepts `F5` earlier, so there's no conflict); a confirmation/error
  when the overlay is closed goes as a note into the feed (`set_overlay_notice/_error` → `push_*`).
- **Where the text is built**: the overlay only sees `ChatSummary` (title+count,
  no text), so the orchestrator — sole owner of `Chat` — assembles the conversation.
  A pure formatter `features/chat_export.rs::format_conversation(title, &messages)`:
  labels roles (`User`/`Assistant`), preserves multiline text,
  skips system/tool/empty messages, uses the chat's name as the title;
  `None` (no substantive messages) → an error "Nothing to copy".
- **Writing to the clipboard is a UI-layer side effect** (like the mouse toggle): the
  orchestrator emits `AppEvent::CopyToClipboard(text)`, `app/runtime.rs` writes via `arboard` (the
  `arboard` crate, `default-features = false` — text only, no image data). The client
  is created lazily and reused; on headless Linux without X11/Wayland the constructor
  may fail — then we show an error instead of panicking. Contract:
  `ChatListAction::Copy → ChatIntent::CopyChat → AppCommand::CopyChat`.
- **Confirmation/error** go into the same overlay status area as auto-title
  errors (`ChatListState.notice` — green `✓` for success, red `⚠` for
  error, mutually exclusive, fading on the first keypress). The overlay is **not**
  closed on copy. `ChatScreen::set_overlay_notice` routes into the overlay if it's open,
  otherwise — a note in the feed. Added to the help overlay (`F1`/`?`).

### Post-M9: intuitive `Esc`/`Ctrl+C` for the chat list and quitting (done)
- **`Esc` toggles "chat list ↔ current chat"**: in the main view (when generation isn't
  running) `Esc` opens the full-screen chat-list overlay; `Esc` inside the overlay
  closes it again. During generation, `Esc` first **cancels it** (as before).
  Implementation: the `Esc` branch in `screens/chat.rs::handle_key`, instead of `Quit`, now
  sets `self.overlay = Some(ChatListState::new(...))`; closing goes through the regular
  `ChatListAction::Close`.
- **`Ctrl+C` — quits the app both from the chat and from the list overlay**: the chat
  already had `ChatIntent::Quit`; the overlay got `ChatListAction::Quit` (a `'c'`
  branch in `chat_list.rs::on_key_search`, layout-independent via `keys::physical_char`),
  `chat.rs::handle_overlay_key` maps it into `ChatIntent::Quit`.
- **`Ctrl+L` removed** (its role — opening the list — was taken over by `Esc`). Updated
  the help overlay (`HELP_KEYS`), the status-bar hint, the overlay's help line, and the
  docs (README/spec/install). Tests were rewritten for `Esc`-opens-overlay.

### Post-M9: single-line settings fields + system-message popup (done)
- **Symptom**: editable fields on the settings screen looked single-line, but
  underneath them lived a multiline `InputBox` with word wrapping: a long value
  (a GGUF path, a URL) would wrap onto an **invisible** row (a height-3 popup → only 1
  line visible), and `↑/↓` moved by visual row and `Home/End` — by visual row, not
  across the whole value (see the post-M9 visual-row navigation section above — that's
  exactly what "broke" single-line behavior here).
- **`InputBox` single-line mode** (`widgets/input_box.rs`): a `single_line` flag +
  `set_single_line()` (off by default — chat input stays as before, multiline). In
  this mode the value **doesn't wrap**, it scrolls **horizontally** (`hscroll`,
  a separate `render_single_line` render path + a `col_at_width` helper): the cursor is
  always visible, a long value "slides" left. `↑/↓` — a no-op; `Home/End` — to the
  start/end **of the whole value**; a line break (`insert_newline`) is forbidden; line
  breaks in `set_text`/`insert_str` (a clipboard paste) collapse into a space (the
  "one logical line" invariant). `clear`/`set_text` reset `hscroll`.
- **The settings screen** (`screens/settings.rs`): the field editor opens in
  single-line mode for all text fields; **the exception is the profile's system
  message** (`FieldId::PSystem`) — multiline by nature. For it the editor is
  multiline (`set_single_line(false)`), inside a **large popup** (~80% width, ~60%
  height — `multiline_popup_height`) with long-line wrapping; `Shift+Enter`
  inserts a line break, `Enter` commits, `Esc` cancels (as in chat input).
  `Editor.multiline` drives the key/render branching. See spec §11.6.

### Post-M9: chat list moved to a separate screen (done)
- **The chat list was an overlay inside `ChatScreen`** (`overlay: Option<ChatListState>`),
  now — a **standalone screen** `screens/chat_list.rs` (`ChatListScreen`),
  offloading the chat screen. There was no hard coupling: the widget `widgets/chat_list.rs`
  (`ChatListState`) already held its own snapshot, rendered fullscreen, and answered
  `ChatListAction`. The screen is a **thin wrapper** over the widget (FSD: `screens →
  widgets`): it holds render context (active chat for the `●` marker, palette) and
  translates `ChatListAction` → a new `ChatListIntent` (parallel to `ChatIntent`/
  `SettingsIntent`). The widget and its tests are untouched.
- **Three screens via an enum** (by choice): `app/runtime.rs` holds a base `ChatScreen`
  + `enum ActiveScreen { Chat | ChatList(Box<…>) | Settings(Box<…>) }` (variants
  boxed — screens are large, `clippy::large_enum_variant`). Replaced the former pair
  «`screen` + `Option<SettingsScreen>`». An open list/settings screen gets input and
  renders **instead of** the chat (as settings did before); `Esc`/`Close` → `ActiveScreen::Chat`.
- **Contract**: on `Esc` (not while generating) the chat now returns `ChatIntent::OpenChatList`
  (instead of locally opening the overlay); `runtime::dispatch` builds a `ChatListScreen`
  from chat snapshots (`chat_summaries`/`active_chat`/`palette` — new getters).
  `dispatch_chat_list` translates `ChatListIntent` into `AppCommand`/screen management:
  `Switch`/`Clone`/`NewChat` close the list, `Copy`/`Delete`/`Rename`/
  `AutoRename` leave it open (as the overlay used to). `NewChat` closes the list and
  calls `ChatScreen::request_new_chat()` (profile selection lives on the chat screen — the overlay
  no longer duplicates when there's >1 profile).
- **Removed from `ChatIntent`**: now-dead `SwitchChat/CloneChat/DeleteChat/
  RenameChat/AutoRenameChat` (only the removed `handle_overlay_key` built them; now
  their role belongs to `ChatListIntent`). What remains is `NewChat` (`Ctrl+N`) and `CopyChat` (`F5` in chat).
- **Event routing** (`app/runtime.rs::apply_event` now takes `&mut
  ActiveScreen`): `ChatList` is applied to the chat **always** (an up-to-date snapshot for
  the next open/`Ctrl+N`) and additionally to the open list (live update);
  `ChatListError`/the `CopyToClipboard` result go into the list's status area if
  open, otherwise as a note in the feed (`ChatScreen::push_note`/`push_error` are now pub);
  `ChatActivated` updates the active-chat marker in the open list (`set_active`);
  `Settings`, while the list is open, updates its palette (`set_palette`). Loop gates
  (spellcheck recheck, RAG/impersonation spinners, mouse wheel) are switched from
  `settings.is_none()` to `active.is_chat()` — while the list/settings are open, these
  chat-related things are paused (as was already the case for settings; background tasks
  for RAG/generation continue, their animation/banner just aren't drawn on top).
- `set_overlay_error`/`set_overlay_notice` removed from `ChatScreen` (routing moved into
  runtime). 421 tests green, clippy clean.

### Post-M9: configurable `Ctrl+E`/`Ctrl+R` confirmation (done)
- **The UI-irreversible `Ctrl+R` (regenerate) and `Ctrl+E` (delete an exchange) can
  be protected with confirmation** — a new setting `interface.confirm_destructive_keys`
  (`#[serde(default)]` → old `settings.json` without migration; off
  **by default**, i.e. the previous instant behavior). A "Confirm Ctrl+R /
  Ctrl+E" toggle in the "Interface" section of settings (`FieldId::IConfirmKeys`, with a tooltip).
- **UX — a single modal popup** (by agreement: one shared toggle for both
  operations, a Yes/No popup): when the setting is on, the key press opens a
  "Confirmation" popup (`Enter` — yes, `Esc` — no; other keys are ignored, the popup
  stays open) instead of acting immediately. State — `ConfirmAction`
  (`Regenerate`/`DeleteExchange`) in the `ChatScreen.confirm` field; the intent
  (`RegenerateLast`/`DeleteLastExchange`) is only emitted on `Enter` and only if
  generation isn't in progress. Helpers `trigger_destructive` (open the popup or hand off the
  intent right away) and `handle_confirm_key`; rendering — `render_confirm` (a centered
  popup + `dim_background`, like help/spellcheck). The flag is picked up from the settings
  snapshot in `set_settings` (like the theme palette). See spec §11.7.
- **Tests** (`screens/chat.rs`): the popup opens and `Enter` confirms
  (`RegenerateLast`); `Esc` cancels; other keys neither close the popup nor
  get typed into the input box; with the setting off, the intent is emitted immediately.
  **614 tests green**, clippy/fmt clean.

### Post-M9: settings-screen redesign — stage 1 (IA + field groups) (done)
- **The settings screen's information architecture fell behind the growth in
  field count** (~120): flat sections with no groups, "Tools" turned into a
  dumping ground (gates + the embedding server + RAG chunking + 6 self-model
  fields + notes), "Inference" was a single field, and labels used
  prefix-pseudo-groups ("Embeddings:", "RAG:", "Self-model:"). Stage 1 of the
  agreed plan: `screens/settings.rs` only (the `SettingsIntent` contract and
  `AppConfig` untouched, no migrations). Branch `feat/settings-redesign`.
- **Section regrouping**: `Inference` (1 field) was removed —
  `max_tool_rounds` moved into `Tools` (an "Agentic loop" group with the
  subagent limits); a new **`Memory`** section was added — RAG chunking (a
  "Knowledge base" group), notes (auto-consolidation, @self recall), and the
  self-model (6 fields) moved there from "Tools". Result: 6 sections — Model
  · Sampling · Tools · Memory · Profiles · Interface. The embedding **server**
  stayed in "Tools" (an "Embeddings (server)" group) — moving it to a "Model"
  subsection was deferred to stage 2 (needs a tab strip with a 3rd
  subsection).
- **Group headers** — a new `FieldRow.group: &'static str` field (the
  `grouped("Group", vec![...])` helper stamps a batch). Headers **are not
  part of `fields()`** (field-to-field navigation, no "dead" steps on a
  header) — they're injected at a group boundary **at render time**
  (`render_fields`): the selected field's position among the rendered
  elements is computed (`select` = field_idx + the number of headers before
  it), the scrollbar counts against the full element count. A header is
  `header_line` (name muted+bold, a continuation line `─` in the border
  color; `─` needs no compat replacement in WGL4). The managed `llama-server`
  fields were laid out into Server/Model/Performance/Speculative decoding
  groups; sampling — `SamplingParam::group()` + a reordered
  `SAMPLING_PARAMS` (Basics/Dynamic temp/Diversity/Penalties/DRY/
  Mirostat/Order/Reasoning).
- **Value alignment — per group, not per section** (a `HashMap<group,
  max_label_w>` in the renderer, floor `MIN_LABEL_COL=20`): one long name
  (e.g. "Self observations in note_recall") no longer pushes away the value
  column of every group in the section. **Superseded** — a single per-section
  column with a cap, see "Post-M9: settings — a shared value column per
  section" below.
- **Shorter labels**: the prefix moves into the group header ("Embeddings:
  binary" → "llama-server binary" under "Embeddings (server)"; "RAG: chunk
  size" → "Chunk size (chars)" under "Knowledge base"; "Self-model: …" → "…"
  under "Self-model"; "tool: X" → "X" under "Tools").
- **A fixed bottom panel** (height 4, always reserved when there are fields —
  the list doesn't "jump"): the full value of a selected **long** text field
  (paths/URLs/system message; a >32-column threshold so short numbers/hosts
  aren't duplicated) + a description hint. Values in the list are
  **truncated with "…"** (`truncate_to_width`, a local analogue of
  chat_list) to the group column's width.
- **A section field counter** in the left menu (on the right, muted) — a
  quick sense of scope. Descriptions were added for fields that became
  noticeable (the agentic loop, the web/python master gates).
- **Tests**: navigation in tests moved from a fragile `Tab`/`Down` counter to
  stable helpers `goto_section(Section)`/`goto_field(FieldId)` (they survive
  section/group reordering); new tests — the composition of "Memory"
  (RAG/notes/self-model moved, `max_tool_rounds` in "Tools"), group markup,
  long-value truncation with "…". **773 unit tests green** (+3), clippy/fmt
  clean. No live run needed (a pure UI refactor, no engine).
- **Next (agreed plan)**: stage 2 — subsection tabs (a tab strip, embedding →
  "Model"), menu, a contextual footer; stage 3 — profiles: grouped tools with
  descriptions and honest gates (`⊘` for globally-disabled ones); stage 4 —
  `/` search; stage 5 — a Choice popup, validation without closing the
  editor, `Del`-reset, "modified vs. default" `•` markers; stage 6 — server
  statuses on-screen + an optional restart debounce.

### Post-M9: settings-screen redesign — stage 2 (subsection tabs + contextual footer) (done)
- Continuation of stage 1 (`feat/settings-redesign`, `screens/settings.rs`
  only; the `SettingsIntent`/`AppConfig` contract untouched, no migrations).
- **A subsection tab strip** replaces the pseudo-field "Subsection ‹Assistant›":
  the subsection selector (`ModelSub`/`SamplingSub`/`ProfileSub`) **remains
  field 0 in `fields()`** (navigation/the `←→` cycle unchanged — minimal
  risk), but **is no longer drawn as a list row**: `render_fields` injects it
  as a pinned line under the title (`tab_strip_line`: `Assistant │
  Impersonation │ …`, the active tab highlighted — a `keycap_bg` fill when
  the strip is focused, an accent color otherwise; `←→` shown on the right
  when focused). The section title was pulled out of the `List` block into a
  separate header (`head_area` = title + an optional tab strip); the
  scrollbar now spans the full height of `list_area`. Detecting the
  subsection field — `is_subsection`; it's excluded both from group alignment
  and from the list rows.
- **A third "Embeddings" tab under "Model"** — the embedding **server** moved
  out of "Tools" (mirroring the status bar's three chips: chat/imp/embed). A
  new type `ModelTab { Assistant, Impersonation, Embeddings }` (on
  `model_sub`; `cycle(dir)` — a 3-position cycle that respects direction),
  `Subsection` (2 variants) stayed for sampling/profiles. Tab labels — const
  `MODEL_TABS`/`SUB_TABS` (index = discriminant, `as usize`). The `EMode`/
  `EBinary`/… fields and their handlers weren't touched — only their UI
  location changed. "Tools" now holds only gates/parameters (Agentic
  loop/Web/Python/Files).
- **Profiles reordered**: `ProfileSub` (the tab strip) — field 0, then the
  section-level `PSelect` (profile picker) and `PName`, then the
  subsection-level Persona/Tools.
- **Contextual footer** (`render`): base hotkeys + section-specific ones — in
  "Profiles" `Ctrl+N new`/`Ctrl+D delete` are added.
- **Tests**: updated for `ModelTab` and the profile reordering (navigation via
  the stable `goto_section`/`goto_field`); new ones — the third "Embeddings"
  tab (fields in "Model", absent from "Tools"), tab-strip rendering (tabs
  visible, "Subsection" isn't a list row). **775 unit tests green** (+2),
  clippy/fmt clean. Pure UI refactor.

### Post-M9: settings-screen redesign — stage 3 (profiles: grouped tools + gates) (done)
- Continuation (`feat/settings-redesign`). The wall of ~32 flat "tool: X"
  toggles in a profile was replaced with a **grouped list with descriptions
  and honest gates**.
- **Catalog metadata moved into `features/tools/meta.rs`** (FSD: `screens`
  pulls it from there instead of hardcoding it): `tool_group(id)` (8 groups,
  ordered by `TOOL_GROUPS`), `tool_description(id)` (a short 2–4-word
  description), `tool_gate(id) -> Option<ToolGate>` (`Web`/`Python`/`Fs` —
  mirroring `effective_tool_ids`). Covered by a test: "every tool in
  `all_tool_ids` has a group and a description".
- **Profile toggles by group**: `profile_fields` lays out tools by
  `tool_group` (a stable sort by `TOOL_GROUPS` **without changing indices** —
  `PTool(idx)` remains the position in `tool_catalog()`, the source of truth
  for `toggle_profile_tool`; only display order is grouped). Group headers —
  the prior stage-1 mechanism.
- **Inline descriptions** (`FieldRow.hint: Option<&'static str>`): a short
  description to the right of `[x]` (`get_sampling  [x]  show sampling`).
  `render_field_line` draws the hint into the remaining width (truncated with
  "…"). The field is generic — other sections don't set it.
- **An honest gate** (`FieldRow.warn`): a tool enabled in the profile but
  disabled by the **global** switch (`web/python/fs_enabled`) is drawn in a
  **warning color** with a "disabled globally: <switch>" hint, and the bottom
  panel spells it out — "enable it in the 'Tools' section". This closes the
  "[x], but actually unavailable" trap
  (`SettingsScreen::gate_disabled` + `gate_hint`).
- **An "on/total" counter in the group header** (generic): groups with ≥2
  toggles carry `N/M` (`header_line` gained a `count` parameter) — used both
  in "Interface" (e.g. "Copy conversation (F5) 2/3") and in a profile
  ("Memory and knowledge 11/11").
- **Tests**: meta (group/description/gate for every tool); profile (toggles
  grouped + contiguous + described; a globally-disabled one is flagged with
  the gate, an enabled one isn't); `header_line` with and without a count.
  **780 unit tests green** (+5), clippy/fmt clean. Pure UI refactor (no
  engine); an "opt."-marker for optional tool groups (control/self_model) —
  left as groundwork.

### Post-M9: settings-screen redesign — stage 4 (field search `/`) (done)
- Continuation (`feat/settings-redesign`). With ~140 fields across 6 sections
  and 3 subsections, added a **global search `/`** — jump to a field without
  paging through sections.
- **Search index** (`build_search_index`): enumerates fields of **all**
  sections and **all** subsections (not just the active one). Field builders
  were parameterized by subsection — `model_fields_for(ModelTab)` /
  `sampling_fields_for(Subsection)` / `profile_fields_for(Subsection)` (the
  public `*_fields()` call them for the active one); `ModelTab::ALL`/
  `Subsection::ALL`+`from_index` for enumeration. Each hit carries jump
  coordinates (`section_idx`, the subsection discriminant, `field_idx`), a
  breadcrumb "Section · Subsection › Group › Field", and a lowercase haystack
  (label+group+description+hint+subsection). Mode-dependent fields
  (managed/cloud) are indexed for the current mode. `collect_hits` skips the
  subsection selector.
- **Overlay** (`SearchState { input, all, results, selected }`, a `search:
  Option<...>` field): `/` opens it (inside an editor `/` is a plain
  character), typing filters via an **AND over word-substrings**
  (`search_filter`), `↑/↓` select, `Enter` — `jump_to_selected` (sets the
  section/subsection/`field_idx`/field focus, closes the overlay), `Esc` —
  cancel, `Ctrl+K` — clear the query. Rendering (`render_search`): a dimmed
  background, the popup = the query line (`InputBox`) + a list of
  breadcrumbs with a value (muted) + a scrollbar; the title carries a
  "found/total" count. Clipboard paste is routed to the search line. `/
  search` was added to the contextual footer.
- **Tests**: `/` opens and filters; `Enter` jumps to a field (**including a
  subsection switch** — searching for a field on the "Embeddings" tab
  switches `model_sub`); `Esc` doesn't move navigation; the index covers
  fields of inactive subsections (>100 fields). **784 unit tests green**
  (+4), clippy/fmt clean. Pure UI refactor.

### Post-M9: settings-screen redesign — stage 5 (Choice popup, validation, `Del` reset, `•` marker) (done)
- Continuation (`feat/settings-redesign`). Four field-editing improvements.
- **A Choice-field picker popup** (`ChoiceState`, a `choice` field): `Enter`
  on a Choice field opens a list of all options with the current one marked
  (`↑↓`/`Enter`/`Esc`; `←/→` still does a quick cycle). Critical for
  `--spec-type` (9 options), the modes (5–6), themes, and **profile
  selection** (`PSelect`). `choice_menu(id)` returns (options, index) —
  reusing `FlashAttn::ALL`/`SpecType::ALL` (made `pub`), `SERVER_MODES`/
  `IMP_MODES`/`THEMES` + `sampling_choice_menu` (thinking/reasoning);
  `apply_choice` applies the pick via the **existing `cycle_field`** (steps
  from the current value to the target — no new setters).
- **Validation without closing** (`Editor.error`): an invalid numeric field
  committed with `Enter` no longer closes the editor — the title turns red
  with a `⚠`/`!` glyph + a message, an edit clears the error, `Esc` cancels.
  Classification via `field_num_kind`/`SamplingParam::num_kind` (Int/Float) +
  `field_validation_error` (a soft i64/f64 check; empty is valid; the exact
  type/range check is left to `apply_text`).
- **`Del` — reset a field to its default** (`reset_field`): compares the
  value with the field from a **default config** (`default_fields` — a
  temporary `SettingsScreen` over `AppConfig::default()` with the same
  navigation); if different — applies the default via
  `toggle_field`/`apply_choice`/`apply_text` (already default → a no-op,
  no redundant save). Profile fields (`is_profile_field`) are untouched
  (they have no config default).
- **A `•` marker** (accent-colored, 2 columns to the left, fields visually
  indented under the group header): `render_field_line` gained a `modified`
  parameter — `render_fields` compares the value against `default_fields`
  (built once per render). Profile fields aren't marked (the default config
  carries the same profiles → they're equal). `•`, `‹›`, `⚠`/`!` come from
  WGL4/GlyphSet.
- Footer: `Del reset` (when a field is focused). `FlashAttn`/`SpecType`
  gained `pub const ALL` (enumerating variants).
- **Tests**: the popup opens/applies a choice/`Esc` cancels; an invalid number
  keeps the editor + the error, an edit commits; `field_validation_error`
  classifies int/float/text; `Del` resets a modified field and is a no-op on
  a default one; the `•` marker appears on a modified field and is absent on
  the default (a render check). **791 unit tests green** (+7), clippy/fmt
  clean. Pure UI refactor.

### Post-M9: settings-screen redesign — stage 6 (server-status chips on-screen) (done)
- Completion of the track (`feat/settings-redesign`). The only stage with
  wiring outside `screens` (delivering the status snapshot, like for
  `Settings`/the palette — FSD intact).
- **A server-status chip in the "Model/server" section header**, on the
  right, contextual to the active subsection: Assistant → `chat`,
  Impersonation → `impersonation`, Embeddings → `embed` (`● ready` /
  `◐ connecting…` / `✕ not configured` / no connection with a reason;
  glyphs/colors mirror `widgets::status_bar`, the glyph is 1 column wide —
  compat-safe). Edit the engine → see `connecting… → ready` right there
  without leaving to the chat (a restart is the `Connecting` status coming
  from the orchestrator).
- **Wiring**: `SettingsScreen.statuses: ServerStatuses` + `set_server_statuses`;
  a new `ChatScreen::server_statuses()` getter. Runtime: on `OpenSettings` it
  sets the initial snapshot from the chat; `apply_event(ServerStatus)`, with
  the settings screen open, mirrors the status into it (live updates). A
  `render_field_line`-independent chip is right-aligned on the title line
  (`server_status_chip`/`span_width`).
- **Tests**: the chip shows the active subsection's server (chat→embeddings
  on tab switch), and doesn't render in other sections (a render check).
  **792 unit tests green** (+1), clippy/fmt clean.
- **Engine-restart debounce — deliberately deferred** (marked "optional" in
  the plan): the only genuinely risky change (in the orchestrator — the most
  concurrency-critical component, with the "sole owner of `Chat`" invariant;
  it changes the long-standing "applies on commit" semantics). The chips
  already give the restart observability the stage was aimed at; churn only
  happens when quickly editing several engine fields in a row, and it's now
  visible/tolerable. To be done as a separate, focused PR.
  **Closed** — see "Post-M9: engine restart debounce" below.

### Settings-screen redesign (stages 1–6) — summary
The `feat/settings-redesign` track is complete (6 commits). The settings
screen went from a flat list of ~120 fields to: **field groups** with
headers and per-section alignment + a fixed value/description panel
(stage 1); a **subsection tab strip** and moving the embedding server into
"Model" + a contextual footer (stage 2); **grouped tool toggles** with
descriptions and honest `⊘` gates (stage 3); `/` **search** across all
sections with a jump to a field (stage 4); a **Choice picker popup**,
validation without closing the editor, `Del` reset, and a "modified" `•`
marker (stage 5); **server-status chips** on-screen (stage 6). The
`SettingsIntent`/`AppConfig` contract was unchanged, no migrations; +a new
`features/tools/meta.rs`. Result: **792 unit tests**, clippy/fmt clean.
Groundwork: an "opt."-marker for optional tool groups (the engine-restart
debounce was done as a separate PR, see below).

### Post-M9: settings — a shared value column per section (done)
- **Symptom**: per-group value alignment (stage 1 of the settings redesign)
  produced a "sawtooth" — each group had its own value-column stop (in
  "Memory" three groups meant three different columns), and one long label
  would push the values of its group far from short neighbors ("Theme
  ‹auto›" was pushed past "Old-terminal compatibility").
- **Fix** (`screens/settings.rs`): the value column is now computed **across
  the whole section** — `section_label_col` (the max label width among
  visible fields; floor `MIN_LABEL_COL=20`, **cap `LABEL_CAP=28`**; the
  subsection selector is excluded — it's a tab strip). Values and inline
  hints for every group now line up on one vertical. A label longer than the
  cap does **not** push the column — its value sits locally right after the
  label (a safety net; there are no such labels in the current set),
  `value_w` is computed from the real end of the label in that case
  (so the "…" truncation doesn't lie). `FieldRow.group` is now used only for
  the group header and the toggle counter.
- **Outlier labels shortened** by moving context into the group header (the
  stage-1 technique): "Copy with X" → "With X" (the verb is already in the
  header "Copy conversation (F5)"), "Old-terminal compatibility" → "Old-terminal
  mode", "Self observations in note_recall" → ""About self" in note_recall".
  Words needed for `/` search were preserved in the field descriptions
  (`field_description` — the search haystack includes the description).
- **Tests**: `section_label_col` (floor/cap/selector exclusion);
  `all_labels_fit_alignment_cap` — a **future-proofing gate**: labels of every
  section and subsection (including the speculative-decoding draft fields,
  visible only when `spec_type=draft-*`) must fit within `LABEL_CAP`,
  otherwise the test requires shortening the label or moving the meaning into
  the group header; `value_column_is_shared_across_groups` (a render check:
  the values of "Tools"'s three groups line up in one column). **797 tests
  green** (+3), clippy/fmt clean. Docs: spec §11.6.

### Post-M9: settings-section field counter tied to the selected mode (done)
- **Reversal of a previous decision** (PR #121, "stable counter = union across modes/
  subsections"): at the user's request, the field counter for a settings section in
  the left menu is now **tied to the currently selected mode** of each server's
  engine/provider (managed/external/openai/gemini/claude), rather than a fixed union
  across all modes. Motive — **consistency with search**: the sum of section counters
  should match the number of fields in the search overlay (`/`).
- **Implementation — single source of truth**: `section_counts`/`section_field_count`
  (`screens/settings/render.rs`) are derived from the same index as search
  (`build_search_index`) — it enumerates fields of all subsections (tab strips) for
  their **current** modes, skipping subsection selectors (`collect_hits`). `section_counts`
  groups hits by `section_idx` in a single pass → the sum is identically equal to
  `build_search_index().len()`. Both functions became `&self` (no longer need to swap
  the config to iterate over modes); `section_field_count` is now `#[cfg(test)]` (the
  renderer only calls `section_counts`). The counter updates live on mode change (read
  from `self.config` on every render).
- **Tests**: `section_counts_sum_matches_search_index` (the sum of section counters ==
  the number of search fields across different modes); `section_count_tracks_selected_mode`
  (cloud shows fewer fields in the "Model" section than managed — the counter reflects
  this; matches the section's field count in the search index). The previous
  `section_field_count_is_tab_and_mode_independent` was replaced (it asserted the
  opposite). **808 tests green**, clippy/fmt clean.

### Post-M9: API keys in settings — stage 2 (UI: input field, masking, status) (done)
- Completion of the track (branch `feat/api-key-ui`, stacked on
  `feat/api-key-store`). Cloud keys are now entered **in the settings
  window** — env variables are no longer needed. The track's conclusion is
  captured in **ADR 0008** (refining ADR 0004: "no secrets on disk" → "no
  secrets on disk **in plaintext**").
- **A masked `InputBox` mode** (`set_mask`, modeled on `single_line`):
  characters render as `•`, `selected_text()` returns `None` (a secret
  can't leak via copying — deleting a selection still works, going
  through `remove_selection`); enabling it switches the field to
  single-line mode (otherwise the multiline render path would reveal the
  content). **The mask substitutes characters before width calculations**:
  `•` has width 1, so a wide glyph inside a secret (an emoji from the
  clipboard) doesn't offset the cursor relative to the visible text.
- **The "API key" field** in the cloud subsections
  Assistant/Impersonation/Embeddings (`FieldId::XApiKey`/`IxApiKey`/
  `EApiKey`, row `api_key_row`): the value is a **status** ("configured
  (this computer)" / "not set" / "unavailable on this system", when
  there's no machine-id), not a secret. `Enter` opens an **empty** masked
  editor (a stored key can't be shown — even the screen doesn't have it),
  `Del` deletes it. Committing doesn't go into the config's working copy,
  but becomes the intent `SettingsIntent::SetApiKey` →
  `AppCommand::SetApiKey` (a branch in `apply_text` ahead of the
  `field_spec` table; `reset_field` has its own branch, since comparing
  against the default doesn't apply — the value shown is a status). The
  field's provider is derived from **its own** engine's mode
  (`api_key_field_provider`) — the key is shared across all three slots.
- **Secrets never leave the backend for the UI at all** (a hardening
  beyond the initial design): `emit_settings` **strips** `config.api_keys`
  in the snapshot and adds flags `api_keys_present: Vec<CloudProvider>`;
  the screen keeps only these (`set_api_keys_present`). So the UI doesn't
  even carry ciphertexts around, and the stage-1 round-trip protection
  (`handle_update_config` restoring keys from the previous config)
  becomes not "insurance" but a required link.
- **i18n**: field/statuses/descriptions (`ui.settings.field.api_key`,
  `ui.settings.value.key_set|key_unset|key_unsupported`,
  `ui.settings.desc.api_key|api_key_unsupported`) in ru+en; the
  parity/no-dead gates covered it automatically.
- **Tests**: `input_box` (2 — masking: only `•` shows on screen, the
  secret isn't in the buffer, `selected_text` is empty, deletion works;
  width/cursor with a wide glyph); `settings` (4 — the status field
  appears only in cloud modes and reacts to the presence flag; the editor
  opens **empty and masked**, committing yields `SetApiKey`, and the
  secret **never lands in the screen's config**; `Del` deletes only when
  a key is present; each slot's field addresses its own engine's
  provider); orchestrator (1 — the snapshot carries flags, but
  `config.api_keys` is empty). Stage-1 tests were switched from
  `config.api_keys` to `api_keys_present`. **1199 unit tests green** (+7),
  53 `#[ignore]`, clippy `-D warnings`/fmt clean.
- **No live engine run needed** (pure UI + the key path: client protocols
  unchanged). The DPAPI cycle was already verified for real in stage 1;
  the Linux scheme is checked by CI. A full manual scenario (entering a
  key → a cloud chat → moving the config) is still left to the user — it
  needs a real terminal and a live key.
- **Future work** (ADR 0008): an OS keychain as an additional `scheme`;
  keys for the external proxy and MCP-server env maps via the same
  mechanism; UI management of entries from other machines ("forget this
  computer").

### Post-M9: help/"About" dialog with tabs (F1) (done)
- **The help overlay (`F1`/`?`) reworked from one long scrollable list into a modal
  dialog KDE/Qt-style** (branch `feat/help-about-tabs`): a logo lockup in the header (the
  same `widgets/logo::lockup_lines` as before), a **tab strip** of four tabs, and
  scrollable content for the active tab with a scrollbar. Tabs (in shown order):
  **"About"** (name+description, author `Vladimir Shylov`, version, links — website
  `mindfork.io`, repository, crate `crates.io/crates/mindfork`), **"Hotkeys"**
  (the former `HELP_KEYS` list), **"License"** (MIT text), **"Components"** (third-party
  dependencies + their licenses). Opens on "Hotkeys" (`F1`/`?` — the familiar
  help key; prior behavior kept, "About" is the neighboring tab).
- **New module `shared/credits.rs`** (FSD `shared`, language-neutral data): constants
  `AUTHOR`/`SITE_URL`/`REPO_URL`/`CRATE_URL`, `LICENSE_TEXT` (`include_str!("../../LICENSE")`),
  `COMPONENTS: &[(&str,&str)]` (44 direct runtime dependencies + windows-sys, licenses
  cross-checked against `cargo metadata`; dev/build dependencies `tempfile`/`winresource` excluded).
  **Gate test** `components_cover_direct_dependencies` cross-checks `COMPONENTS` against
  `[dependencies]`+`[target.'cfg(windows)'.dependencies]` in `Cargo.toml` (a line-based parser,
  like the SVG parser in `widgets/logo.rs`), plus tests "sorted/licensed" and
  "LICENSE embedded". License/component data **does not go through locales** — it is read by
  `screens/chat/popups.rs` directly (the legal text/SPDX/URL are language-neutral).
- **State** (`screens/chat/mod.rs`): `show_help: bool` + `help_scroll: usize` replaced
  with `help: Option<HelpState>` + types `HelpTab {About,Hotkeys,License,Components}` (order
  = `ALL`) and `HelpState {tab, scroll}` (`next_tab`/`prev_tab` cycling with scroll
  reset). Navigation (`screens/chat/input.rs`): `Tab`/`←→` — tabs, `↑↓`/`PgUp`/`PgDn`/
  `Home` — scroll, `Esc`/repeat `F1`/`?` — close, `Ctrl+Q`/`F10` — quit (punch through the
  dialog, as in the confirmation popup). Other keys are **ignored** (previously "any
  key closes" — with tabs that would have interfered with navigation).
- **Render** (`screens/chat/popups.rs`, `render_help(frame, &mut HelpState, palette, loc)`):
  fixed dialog size (76×34 + border, clamped to the screen — the window doesn't "jump" between
  tabs), header = (optional lockup + spacer) + tab strip + separator line `─`, below it —
  scrollable content (`Paragraph.scroll`, `total` = number of logical lines) +
  a scrollbar on the right border **along the content area** (not the full height — the
  thumb doesn't intrude on the tabs). Lockup shown **only with room to spare**
  (`inner.height >= LOCKUP_ROWS+6` and width; hard degradation, docs/branding.md §5). **The
  license is word-wrapped**: paragraphs (separated by a blank line) are reassembled and
  wrapped via `wrap::wrap_line` to the dialog width (otherwise long MIT lines would be
  clipped on the right; assembly by `lines()` — CRLF-safe). A local tab strip (does not
  reuse `settings::helpers::tab_strip_line` — that one is `pub(super)` to the settings
  module).
- **i18n**: new keys `ui.help.tab.{about,hotkeys,license,components}`, `ui.about.{desc,
  author,version,site,repo,crate}`, `ui.components.intro`, `ui.help.footer.tabs`
  (ru+en); `ui.help.title` → "About"; removed the orphaned
  `ui.help.footer.{scroll,any}`. i18n gates (parity/no-dead/Cyrillic) covered this automatically.
- **Tests**: `credits` (gate cross-check against Cargo.toml + sorted/licensed + embedded
  LICENSE); chat screen (`f1_opens_help_and_esc_closes` — other keys don't close;
  `help_navigation_scrolls_and_switches_tabs` — scroll + Tab/← with reset; clamp+
  scrollbar on a short terminal; `help_tabs_render_distinct_content` — all 4 labels in
  the tab strip + distinctive content per tab; logo shown/hidden by height).
  **1262 unit tests green** (+4), clippy `-D warnings`/fmt/i18n clean. No live run
  needed (pure UI, `TestBackend`; composition verified by dumping the buffer of all tabs).
- **Polish per feedback** (same branch): (1) **separate "Commands" tab** — input-box
  commands (`/rag …`/`/tts …`) split out of `HELP_KEYS` into a new `HELP_COMMANDS`
  (shared `key_lines` render), tab `HelpTab::Commands`, key `ui.help.tab.commands`;
  tab order is now `About/Hotkeys/Commands/License/Components` (5). (2) **Component
  versions** — `COMPONENTS: &[(&str,&str,&str)]` `(name, version, license)`; new
  gate `component_versions_match_cargo_lock` (parses `Cargo.lock`, the version must be
  among the locked ones — robust to `thiserror` 1/2, `windows-sys` duplicates). (3) **Spacer
  above the logo** (an extra blank line in the header). (4) **Remembering the last tab** —
  new field `ChatScreen.help_last_tab` (like `emoji_last`): closing saves `help.tab`,
  `HelpState::open(tab)` restores it on the next open (`new()` removed;
  `DEFAULT_HELP_TAB = Hotkeys`). (5) **Spacing between "About" items**
  (a blank line before each). (6) **Brand name `mindfork`** in the popup title
  (`credits::APP_NAME` instead of `CARGO_PKG_NAME=mindfork-rs`) — and in the "About"
  tab body. (7) **Version in the terminal window title** (`app/runtime`: `SetTitle`
  is now `mindfork v<version>`, matching the popup; Windows). Tests: `help_remembers_
  last_tab`, the "Commands" tab in `help_tabs_render_distinct_content` (+ checking that
  there are NO commands on the hotkeys tab), component version in the dump. **1264 unit tests green**
  (+2), clippy `-D warnings`/fmt/i18n clean.
- **Groundwork**: clickable links (OSC 8 hyperlinks — terminal-dependent), generating the
  component list (versions/licenses) from `cargo metadata` in build.rs (currently a
  curated list + gates against Cargo.toml/Cargo.lock).

### Post-M9: custom user/assistant names in profile settings (done)
- **The role names shown to the user became editable** (branch `feat/custom-role-names`,
  user's request): two fields in the settings "Profiles" section (the "Persona"
  group) — "User name" / "Assistant name". A set name replaces the feed's role
  header (**uppercased** to match the header style: `GAIA` instead of `YOU`) and the
  label in the `F5` conversation export (`Gaia:` instead of `User:`). Both are
  **empty by default**: empty = "not set", and each surface falls back to the
  interface language's own label, so the chat follows axis B until the user
  overrides it. Whitespace-only counts as unset (the field isn't a way to blank the
  label out).
- **The type already existed and was dead** — `CharacterNames {user, assistant,
  system}` has been on `Profile` (and copied onto `Chat`) since M2, but nothing ever
  displayed it. The work was wiring it to the two surfaces, not adding a field.
- **Key decision — the profile is the source of truth, resolved at render time**
  (a deliberate exception to spec §10 "editing a profile doesn't affect existing
  chats"): the copy semantics exists so the assistant can change a chat's
  `system_message` per chat, but a *display* name the user just typed in settings has
  to show up in the chat they're looking at. So the feed and the export read
  `Profile.character_names`; `Chat.character_names` stays (import format + a possible
  future per-chat override) but is documented as not used for display.
- **Plumbing**: a new `AppEvent::CharacterNames` — the orchestrator (which owns both
  chats and profiles) resolves the active chat's profile and pushes the names; emitted
  from `activate()` (switch/create/bootstrap) and from `handle_update_profile` (so a
  rename lands on the open chat immediately). `ChatSummary` carries no `profile_id`,
  so the screen can't resolve this itself — hence a push, not a pull. The names ride
  the feed's **render-cache key** (`CacheKey`), otherwise a rename wouldn't repaint
  the already-cached header lines.
- **A one-time cleanup of legacy seeds** (`features/profiles::clear_seed_character_names`,
  called from the same `bootstrap` loop as `reconcile_tools`): `CharacterNames::default()`
  used to seed a Russian placeholder triple, migrated to `You`/`Assistant`/`System` by
  the english-source migration (`b83ae37`). Those were never displayed, so with the
  fields now live a Russian-interface user would suddenly get Latin `YOU`/`ASSISTANT`.
  Fields still holding a seed value are cleared (per-field, both sets); a name that
  came from an import/hand edit survives. Idempotent, additive — **no schema bump**
  (ADR 0006 F12).
- **Not included** (deliberate, out of the requested scope): `character_names.system`
  gets no field (system messages appear in neither surface); TTS role prefixes
  (`speak.role.*`, axis A — the *model's* language) keep their localized text.
- **Tests**: entity (empty default, trimming, blank = unset); export (custom labels,
  one-sided naming, blank fallback); feed (headers replaced and uppercased, cache
  invalidated on rename); settings (the fields commit into `character_names`, an
  empty value is a valid edit, grouped with Persona + described); orchestrator (names
  follow the profile and are re-sent after an edit; `F5` labels; bootstrap clears
  seeds but keeps a chosen name); runtime (the event reaches the feed even with
  another screen on top). **1294 unit tests green** (+13), 58 `#[ignore]`, clippy
  `-D warnings`/fmt/i18n gates/`cyrillic_scan` clean.
- **A live run isn't required** — no engine/memory/tool path is touched: this is UI
  rendering, a pure export formatter, and a profile-field edit, all covered by
  `TestBackend`/unit tests.

### Post-M9: impersonation profiles + a newly created profile is selectable (done)
- Two defects in the settings "Profiles" section, reported by the user; branch
  `feat/impersonation-profiles`. Forks confirmed by the user 2026-07-25 (per the
  recommendations): the impersonation persona is bound **through the assistant
  profile**, and the legacy field is **migrated automatically**.
- **(1) `Ctrl+N` created a profile you couldn't then select.**
  `handle_create_profile` emitted only `AppEvent::ProfileList` — which the **chat**
  screen consumes; the settings screen keeps its own copy of the list from the
  `Settings` snapshot, so a new profile stayed invisible there (not selectable, let
  alone editable) until a restart. `handle_create_profile`/`handle_delete_profile` now
  also `emit_settings()` — `SettingsScreen::refresh`'s doc comment ("after create/delete
  of a profile") had described this contract all along; only the emit was missing.
  Auto-selection: the orchestrator owns the list, so the screen can't know the new id —
  `Ctrl+N` raises a one-shot `pending_profile_select`, and `refresh` selects whichever
  profile isn't in the previous snapshot (comparison by id, not "the last one" — robust
  to ordering). One-shot by construction: the flag is cleared whatever the snapshot
  brings, so a later unrelated re-emit can't hijack the selection.
- **(2) The "Impersonation" subsection edited the assistant's profile.** It showed the
  assistant profile selector and name (both section-level, shared with the "Assistant"
  subsection) and, under them, a single system message — an asymmetry inherited from
  storing the impersonation prompt as `Profile.impersonation_system_message`. Now the
  two subsections edit **different lists**: "Assistant" — the AI-interlocutor profiles,
  "Impersonation" — the user personas, each with its own name and system message
  (`IpSelect`/`IpName`/`IpSystem`); `Ctrl+N`/`Ctrl+D` act on whichever list the active
  subsection shows. The assistant profile ties them together with a new
  "Impersonation profile" field (`PImpProfile` → `Profile.impersonation_profile_id`),
  so "who the assistant is" and "who I am in this conversation" travel together and
  several assistant profiles can share one persona.
- **Storage — `AppConfig.impersonation_profiles`, not `profiles.json`** (a deliberate
  departure from "profiles live in profiles.json"): impersonation is already configured
  globally in `settings.json` (`impersonation_engine`, `impersonation_sampling`), and
  `profiles.json` is a bare array with no room for a second list. The payoff is large —
  the settings screen edits the list through the ordinary config-save path, so
  **no new `AppCommand`/`AppEvent`/`SettingsIntent`, no storage artifact, no schema
  registration**, and creating a persona needs no orchestrator round-trip (hence no
  auto-select problem for that list). The cost is a cross-file reference: a dangling id
  is legal and reads as "not set" → the shared default text (so deleting a persona
  doesn't have to rewrite every referencing profile). Both fields are additive
  (`#[serde(default)]` + skip-if-empty) → **no schema bump** (ADR 0006 F12).
- **Migration** (`Orchestrator::migrate_impersonation_profiles`, called from
  `bootstrap`): every profile still carrying a non-empty legacy message and no
  reference gets a persona "«name» (impersonation)" created and linked. Idempotent (a
  profile with a reference is skipped) → safe across upgrade/downgrade cycles; the
  config is written **before** the profile links (a link persisted without its target
  would dangle), and on failure the next launch simply retries. The legacy field is
  **kept** on disk — nothing reads it for prompt building any more.
- **Resolution** was pulled out of `handle_impersonate` into a pure
  `Orchestrator::impersonation_system(profile, loc)` — reference → persona → non-empty
  message, with every miss (no reference / dangling id / blank message) falling back to
  `prompt.impersonation.default`. That made it unit-testable without a streaming
  backend.
- **Ripple**: `ProfileEdit.impersonation_system_message` → `impersonation_profile_id`
  (`Some(None)` unlinks); `FieldId::PImpSystem` removed, `PImpProfile`/`IpSelect`/
  `IpName`/`IpSystem` added (registered in `is_profile_field` — user data has no
  "config default", so no `•` marker and no `Del`-reset); `choice_menu` gained the
  persona lists (`PImpProfile`'s option 0 is "not set"); the import format is
  unaffected (v1 never carried impersonation). Along the way a latent bug was fixed:
  the "no profiles" branch of `profile_fields_for` used to return a lone row **without**
  the subsection tab strip, stranding the user in the section.
- **Tests**: settings (the impersonation subsection edits its own list and has no
  assistant selector/name/tools; create → edit name+message → delete; the assistant's
  reference cycles "not set" ↔ persona; a new profile is selected on the re-emit and the
  flag is one-shot); orchestrator (create/delete re-emit `Settings`; the legacy
  migration links and is idempotent across two launches; resolution covers reference/
  dangling/blank); config (additive default + round-trip); render (user data never gets
  the `•` "modified" marker — a false positive found by dumping the rendered section:
  a chosen persona was flagged only because the *default* config has no personas at
  all; `is_profile_field` now gates the marker as its doc comment always claimed).
  **1281 unit tests green** (+8), 58 `#[ignore]`, clippy `-D warnings`/fmt/i18n gates/`cyrillic_scan` clean.
  **A live run isn't required** — no engine/memory/tool path is touched: the
  impersonation *request* is unchanged, only where its system message is read from
  (covered by unit tests); the rest is settings UI.

### Post-M9: full-text search over chat content — stage 1 (done)
- **The roadmap item "Search within chat content ... via SQLite FTS"**, stage 1
  of 2. Research
  [docs/research/chat-content-search.md](../../docs/research/chat-content-search.md);
  **forks F1–F6 decided by the user 2026-07-29** (all as recommended). The user's
  framing set the shape: chats stay in JSON, the index goes in a **separate
  database that can be deleted with no risk**, synced in the background, and able
  to hold other cheap-to-recompute data later. Branches
  `docs/chat-search-research` → `feat/chat-content-search`.
- **Everything load-bearing was measured on the real 171-chat dev corpus, not
  estimated** — and measuring is what decided the design twice and caught two
  defects (below). FTS5 turns out to be **already compiled into the bundled
  SQLite 3.53.2** (`ENABLE_FTS5`), so the whole feature needed **no new
  dependency**. Cyrillic folds correctly under `unicode61` *and* under `trigram`
  — the latter verified rather than assumed, since trigram's case folding was
  historically ASCII-only.
- **F1, the one fork the user really had to settle: `trigram`.** The existing chat
  filter is substring (`title.contains`), so users are trained on `естов` finding <!-- cyrillic-ok -->
  `тестовое` — and **FTS5 ships no Russian stemmer** (`porter` is English-only), <!-- cyrillic-ok -->
  so `unicode61` would not find `памяти` from `память`, a daily miss rather than <!-- cyrillic-ok -->
  an occasional one. Auto-appending `*` mitigates but does not close it
  (`память*` still misses `памяти` — they diverge at the last letter). Cost of <!-- cyrillic-ok -->
  trigram: 2× index, a 3-character floor, weak `bm25`. Hence **stage 1 filters
  rather than ranks** — content mode narrows the chat list and the user's existing
  Created/Modified sort still orders it, sidestepping weak ranking entirely.
- **`cache.db`, and why a second file removes machinery rather than adding it.**
  `data.db` holds irreplaceable content, hence ADR 0006 (steps in transactions,
  downgrade guards, pre-migration backups). An index needs **none of it**: a
  version mismatch, a corrupt file or an unreadable schema all have the same right
  answer — delete and rebuild. So `CacheDb::open` **self-heals instead of
  bailing**: a disposable index must never be able to block startup. Two things
  fell out for free: `features/backup.rs` uses an **allowlist**, so the new file
  is excluded from archives with **no code change** (and a restore correctly lands
  without an index and rebuilds), and "delete it" becomes a supported repair
  instead of data loss.
- **Schema — external-content FTS5, decided by probe.** A standalone FTS5 table
  can only carry `chat_id` as `UNINDEXED`, so re-indexing one chat means a full
  scan; external content (`content='messages'`) keeps metadata in a real table
  with a real index, and `snippet()` still works (verified — it reads through to
  the content table). Triggers make a full rebuild ~4× slower, which is the right
  trade: rebuilds are rare and background, deletes happen on every incremental
  write.
- **Message-level diff — the finding that mattered most.** A chat is saved every
  ~800 ms while a reply streams, and re-indexing a large chat wholesale costs
  ~385 ms *per save*. Diffing on `(message_id, text_hash)` reduces a streaming
  save to **one row**. The hash is FNV-1a inline — stable across Rust versions,
  unlike `DefaultHasher`, which would silently re-index everything on a toolchain
  bump.
- **Sync rests on an invariant the project already keeps.** The orchestrator is
  the sole writer of chats, so in-app changes have an exact hook (`flush_saves`)
  and need no polling; only *external* changes — import, restore, a hand edit, a
  deleted cache — need the startup pass, and that is a **stat-only walk**
  (~0.3 ms for the whole directory), so it runs unconditionally on every launch in
  `spawn_blocking`.
- **Two defects that only the real corpus exposed** (both invisible to the
  synthetic tests, which is the lesson):
  - **A lost update.** The startup pass reads a chat, the app saves and indexes
    that same chat, and the pass's older snapshot lands last — the chat silently
    absent from search until the next launch. That is "launch the app and start
    typing", the common case, and on a 3 s pass the window is wide. Fixed with a
    **compare-and-set on the bookkeeping row inside the transaction**
    (`index_chat_if_unchanged`; `index_chat` stays the unconditional primitive the
    live hook uses), plus reading the bookkeeping **before** the directory — the
    other order forgets a chat created between the two reads.
  - **Hidden chats re-parsed on every startup, forever.** They were *forgotten*,
    which drops the bookkeeping too, so the next pass found no record, parsed the
    file again and dropped it again. Soft delete is the only delete here (spec
    §12.3), so that set only grows: **43 of 171 chats** on the real corpus.
    Recording them with an **empty message set** (which both clears their rows and
    records the file state) took the warm pass from 27 ms with 43 re-parses to
    **0 ms with nothing re-parsed**. Pinned by a regression test that asserts on
    the *second* pass — a single-pass test cannot see it — and mutation-tested.
- **Two checks I specified turned out vacuous**, caught by an agent probing
  instead of assuming: scanning an external-content FTS table reads values back
  *through* the content table, so a `LEFT JOIN` can **never** see a stale index
  row (it returned 0 while `MATCH` still returned a deleted message — blind to
  exactly the user-visible bug); and the bare `integrity-check` only verifies the
  index against itself unless passed an explicit `1`. Both now pinned by a
  mutation test against a deliberately-broken trigger, and the deviation is
  recorded in executable form rather than prose.
- **Query escaping is the pitfall most likely to bite.** Raw input cannot reach
  `MATCH`: measured, `C++`, `cost-benefit`, `50%`, `AND` and `(` are all SQL
  errors on ordinary text — and `cost-benefit`/`a:b` are read as **column
  filters**, so the error names a column the user never typed. Every token is
  quoted with inner quotes doubled; tokens under trigram's 3-character floor are
  dropped rather than allowed to zero out the whole query, counted in
  **characters** (a 3-character Cyrillic token is 6 bytes — a byte floor would
  keep 2-character ones; mutation-tested). The rule lives in **one** place,
  `features/chat_search.rs`, called by the orchestrator: `shared/storage` may not
  depend on `features` (FSD), so `CacheDb::search_chats` takes an already-escaped
  query.
- **Contract**: `AppCommand::SearchChats(String)` →
  `AppEvent::ChatSearchResults { query, chat_ids: Option<Vec<Uuid>> }`, where
  `None` means "not a searchable query — do not filter" (deliberately an `Option`
  rather than "all ids", so the event never claims every chat matched);
  `ChatListAction/Intent::SearchContent`. The widget stays dumb — it sends the raw
  query and filters by whatever comes back, so the floor rule isn't duplicated.
  Stale results are still applied (keeping the last set avoids flashing the full
  list between keystrokes).
- **Measured after the fixes** (171 chats, 13.5 MB of JSON): cold pass 3.1 s
  indexing 1213 messages into 12.7 MB; warm pass **0 ms**; `естов` → 14 chats by <!-- cyrillic-ok -->
  infix, `C++` → 26 (an FTS5 syntax error unescaped), `rust память` → 23 (implicit <!-- cyrillic-ok -->
  AND across scripts), a nonsense query → 0, `ми` → below the floor. <!-- cyrillic-ok -->
  **1531 unit tests green** (+45), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run required** (AGENTS.md §3): no
  engine, memory or provider protocol is touched — but the reconciliation was
  nonetheless exercised against the **real corpus**, which is what found both
  defects above.
- **Stage 2** — done, see the next entry.

### Post-M9: chat content search — stage 2, jump and the results screen (done)
- Completes the track ([stage 2 plan](../../docs/history/chat-search-stage2.md), forks
  **S1–S6 decided by the user 2026-07-29**, all as recommended). Stage 1 answered
  *"which chats mention this?"*; stage 2 answers *"where exactly, and take me
  there."* Split along the risk line: **2a** the jump infrastructure, **2b** the
  screen that rides on it.
- **An investigation of the feed came first, and it paid for itself** — it closed
  off the obvious approach before any code was written:
  - **A jump can only be applied inside `render`.** The per-block render cache
    makes the row offset nearly free (`cache[..idx].lines.len()` summed), but it
    is only valid after a `build_lines` at the current width — width and palette
    are the cache key. So "compute the row in `activate_chat` and call a setter"
    is impossible; a jump is necessarily a **deferred request** consumed by the
    next render.
  - **`FeedMessage` carries no identity, and the projection is lossy.**
    `from_messages` merges consecutive assistant messages of agentic rounds into
    one bubble and drops `Tool`/`System` entirely, so N domain messages become
    M ≤ N feed items. Unrecoverable afterwards — hence `message_ids: Vec<Uuid>`
    recorded *at the merge site*. A single `Option<Uuid>` would have lied about
    merged bubbles.
  - **Highlighting a match inside the feed by source offset is not feasible.**
    The renderer receives `pulldown-cmark` byte ranges and discards them — and
    threading them through would not help, because `normalize_delimiters`
    rewrites the string **before** parsing, so the ranges do not address
    `Message.text` at all; downstream, LaTeX→unicode, mermaid substitution, table
    re-layout, syntect→ANSI, two wrapping passes and rail-prepending each destroy
    the correspondence independently. Stage 1's research flagged this as "worth a
    look"; it is now settled, and fork **S3** took "mark the message, don't
    highlight the match".
- **Three fields, not one — decided by measurement.** The first implementation had
  a single `focus` in `CacheKey`, cleared by manual scroll. Measured on the
  largest real chat: the first scroll after a jump cost **38 ms against 17 ms**,
  because clearing it wiped the block cache and re-ran markdown+syntect over the
  whole chat, scaling linearly with chat size. Splitting it fixed that (**38 → 18
  ms**, identical to any warm frame) — and exposed something worse hiding in the
  original design: since `scroll_to_bottom` would have been what cleared the
  marker, and `push_user_message` calls it, **every send would have re-rendered
  the entire chat**. Final shape: `pending_focus` (consumed by the next render),
  `anchor` (row re-derivation on rewrap, released by manual scroll), `marker`
  (the accent rail — the only one in `CacheKey`, surviving scrolling).
- **A behaviour change that fell out, worth its own CHANGELOG line**: the seven
  `scroll_to_bottom()` sites are now split by *who asked*. User-initiated ones
  (activating a chat, sending, starting a generation) still go to the bottom
  unconditionally; content arriving on its own (tool cards, notes, follow-ups)
  respects `follow`, so a reader who scrolled away is no longer yanked back. Without
  this a jump is worthless — the first tool card would undo it.
- **2b, the screen**: `Ctrl+G` from the chat list's content mode opens results
  **grouped by chat** (fork S2 — 163 hits for a common word is not a flat list you
  scroll), each hit a Rust-built snippet with the match highlighted. `Enter` jumps
  to that exact message; `Esc` returns with the search still live. Capped at 200
  with an honest "showing N of M" — the true total comes from a separate count,
  since the rows only equal the total below the cap. Also in content mode, `Enter`
  on a chat now opens it **at its first matching message** rather than at the end.
- **Snippets are built in Rust, not by SQLite `snippet()`** (fork S4): we already
  store the text, we need byte offsets to highlight the list, and under trigram
  the budget counts 3-grams — 64 "tokens" yields ~70 characters, against a
  documented ceiling this build silently exceeds. The measurement that settled a
  worry: a trigram hit containing no literal token would fall back to the head of
  the message, but on the real corpus that is **0% across every query tried**,
  including `памяти` at 163 hits — so no clever trigram-overlap positioning was <!-- cyrillic-ok -->
  needed.
- **Adding an `ActiveScreen` variant meant auditing nine match sites that compile
  silently.** Two were real: the `SelfModelView` arm would have **replaced the
  results with a self-model snapshot**, and the `Settings` broadcast would have
  left the new screen's theme and UI language frozen. Both are now exhaustive by
  variant, so the next screen is forced to decide.
- **A plan-vs-implementation divergence, caught in review and fixed in the code,
  not the spec.** Fork S2 as agreed said "chats ordered by your existing sort",
  but the screen hardcoded `modified_at` and ignored the list's `Tab` toggle —
  invisible because the default toggle position *is* `Modified`. The sort is now
  threaded through the contract, pinned by a test that asserts the *other*
  position, and mutation-tested. Rewriting the spec to match the code would have
  been the wrong direction.
- **1604 unit tests green** (+73, including the live-run fixes below), 69
  `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run required** (AGENTS.md §3):
  storage, pure logic and TUI rendering — no engine, memory or provider protocol.
  Snippet quality and the miss rate were nonetheless measured against the **real
  corpus**, as in stage 1.
- **The live run found two things, and one of them reversed a fork.** The user ran
  it, and both reports were fair:
  - **The match was highlighted in the results list but not in the feed** you
    landed in — visibly inconsistent the moment you arrive. This was fork
    **S3(a)**, which I had recommended and which they had accepted, so the honest
    answer was to say so *and* revisit: they had now seen it. Implemented as
    **S3(b), post-render span matching**. This is **not** a reversal of §1.4 —
    what that ruled out was mapping *source* byte offsets onto rendered markdown
    (the renderer discards `pulldown-cmark` ranges, and `normalize_delimiters`
    rewrites the string before parsing, so the ranges do not address
    `Message.text`). Matching the text that was actually *rendered* needs none of
    it, which is exactly why the reversal was cheap. The archived plan records the
    revision rather than pretending (a) was never chosen.
    Two consequences the plan had not anticipated, both **over**-highlighting
    where §4a had only foreseen under-highlighting: the role header would have
    matched a plain search for "assistant" in every assistant bubble (excluded),
    and thoughts/tool cards get highlighted although fork F3 indexes
    `message.text` only — so *highlighted* does not mean *this is what matched*.
    Kept, since the word is genuinely on screen, and documented.
  - **`Esc` from a chat opened out of the results threw the results away** and
    went to the chat list. A genuine gap, not a decision — we never designed the
    way back. Now a one-deep back-stack in `runtime` holding the live
    `SearchScreen` (not the query — re-running it would lose selection and
    scroll). Cleared in the **one funnel** every chat-opening route ends in
    (`ChatActivated`) rather than by enumerating routes, and on a *different*
    chat rather than any activation, because `activate()` also rebuilds the same
    chat's feed after a regeneration or `Ctrl+E`. The chat screen learns nothing
    about search: `ChatIntent::OpenChatList` already means "go back", and *where*
    back is, is app-layer knowledge (FSD).
- **A third fix, from disagreeing with an agent's judgement call**: it had left
  the status bar saying `Esc чаты` — reasoning that a runtime→screen flag was <!-- cyrillic-ok -->
  real contract surface for one word. Sound in general, but that word sits in
  exactly the flow the user had just called confusing, and `F1` enumerating both
  meanings does not help someone reading the bottom of the screen. It is
  **derived** from the back-stack once per frame rather than mirrored into state —
  the alternative would have meant writing the rule at four set/clear sites. The
  label was **measured**, which changed the obvious choice: `результаты` costs an <!-- cyrillic-ok -->
  extra row in the hotkey grid at 120 columns, right where everything otherwise
  fits on one line; `к поиску`/`to search` costs none at any width. <!-- cyrillic-ok -->
- **Still open**: in-feed `/` search with next/prev (fork S5 kept it out — a
  different, same-chat interaction), now cheaper still since both the jump *and*
  the highlight machinery (`match_ranges` + span re-splitting) exist — what it
  needs on top is widening the matcher past the focused message and next/prev;
  `cache.db` holding chat-list summaries to remove the 94 ms startup parse.

### Post-M9: settings-screen focus model (done)

- **Reported from real use**: users step from the section list into the parameters
  with `→`, then try to come back with `←` — and on a switch that changes its value
  instead; confused, they press `Esc`, leave the screen entirely, and come back
  trying to remember what they just changed. Plan with forks R1–R6 —
  [docs/history/settings-navigation.md](../../docs/history/settings-navigation.md) (**user's decision,
  2026-07-31**, all as recommended). Branch `feat/settings-focus-model`, stacked on
  `docs/settings-navigation`.
- **The damage is larger than "a setting changed", and reading the code is what
  showed it.** In "Model/server" `→` lands on field 0, which is the **subsection tab
  strip** — so `←` there switches Assistant → Impersonation and the whole field set
  changes under the user's hands. One `↓` further is `XMode`, and `←` cycles the
  server mode, which is emitted at once as `SaveConfig` and, after the debounce,
  **restarts the server**. So the "way back" was a silent, server-restarting edit
  that the UI offers no way to undo.
- **Root cause — a key collision that cannot be removed while `→` enters.** As long
  as `→` enters the pane, users build the model "`←` leaves it"; but `←` must cycle
  a `Choice` value, and the first field of most sections *is* a `Choice` (`XMode`,
  `ITheme`). The value binding is the one that has to stay (`←/→` is the convention,
  and the tab strip uses it too), so the entry binding is the one that goes.
- **The `Esc` ladder already half existed** — the field editor, the `Choice` popup
  and the `/` overlay all close *into* the pane rather than closing the screen, and
  the footer already read "Esc back", which was strictly speaking a lie. So R3
  completes an existing rule instead of introducing one.
- **Target model, one sentence**: *the arrows change, `Enter` goes in, `Esc` goes
  out, `Tab` switches section.* Four points in `apply.rs`: `Esc` became focus-aware
  (`Fields → Menu`, `Menu → Close`); the menu's `Enter | Right` became `Enter`
  (guarded on a non-empty field set — defensive, no section produces one);
  `Left`'s "fall through to the menu" tail is gone, so the `Left`/`Right` arms are
  symmetric; `move_section` no longer resets the focus (it still resets
  `field_idx` — field sets differ per section). Untouched: the initial focus and
  the `/`-search jump (`Focus::Fields` is exactly right there — "Enter on a result
  goes to that field" — and `Esc` now steps back out of it).
- **The footer became focus-contextual** (`render.rs`) — it is the only place the
  model is stated, so leaving it flat would have made the rules undiscoverable. The
  mechanism already existed (`Del` was already focus-conditional). Three new i18n
  keys in both bundles (`hint.enter_fields`/`hint.close`/`hint.to_sections`);
  `hint.back` **had to be deleted**, not just left unused — the
  `bundle_keys_are_not_dead` gate fails on a dead key.
- **Follow-up from a live run of the branch (user):** the section menu's `▸` marker
  was *shown or hidden* by focus, which flickered and shifted the title text
  sideways on every change, while the field pane's `◆` didn't track focus at all.
  Both now stay put and only their **colour** moves — `success` for the pane that
  holds the focus, `muted` for the other — through one shared
  `helpers::focus_marker_style`, since they are a pair encoding the same fact from
  opposite sides. Green already means "you are here" on this screen (the active
  section's rail, the selected row's rail), so this reuses a meaning rather than
  adding one. Pinned by `pane_markers_stay_put_and_swap_colour_with_focus`, which
  asserts **both** halves — the colours swap *and* the glyph positions are
  unchanged; mutation-tested against restoring either old behaviour (the
  show/hide title fails it with "marker not drawn at all", the fixed-colour `◆`
  with the muted assertion).
- **The test helpers encoded the old rules, and R4 breaks them silently.**
  `goto_section` was documented as "after the call, focus is in the menu (Tab
  resets it)", and `goto_field` builds on that by pressing `Enter`; with the focus
  preserved that `Enter` would open an editor instead of entering the pane — in
  many tests at once. The helper now returns to the sections itself instead of
  relying on `Tab`'s former side effect, and the one test that used `Left` as "back
  to the menu" moved to `Esc`.
- **Mutation-tested, and it corrected one of my own comments.** Each of the five
  changed lines was reverted in turn: `right_does_not_enter_the_field_pane`,
  `esc_steps_out_of_the_field_pane_then_closes`,
  `left_on_a_non_choice_field_is_a_no_op`,
  `tab_preserves_focus_and_resets_the_field_index` and
  `footer_hints_differ_by_focus` each failed for its own revert. The `←`-on-Choice
  test, which I had commented as pinning R2, turned out **not** to distinguish the
  versions — a `Choice` field returns early in both — so its doc comment was
  corrected to say what it actually is: a regression guard that symmetrizing the
  arms didn't break value cycling.
- **Tests**: `esc_closes` split into "closes from the sections" + the ladder test;
  five new behaviour tests (one per adopted fork) + `footer_hints_differ_by_focus`
  (`TestBackend`) + `up_on_the_first_field_stays_in_the_pane` (R5 — unchanged
  behaviour, pinned against a future "helpful" change) + the marker test below.
  **1665 unit tests green** (+8), 70 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`
  clean.
- **A live run isn't required** (AGENTS.md §3): key handling and rendering on one
  screen — no engine, memory, tool or provider path is touched. The established
  precedent for settings-screen work (the whole redesign, stages 1–6).
- **Deliberately out of scope** (fork R6, a follow-up): an accidental edit is still
  saved immediately and can't be undone from the UI. This change removes the main
  *source* of accidental edits, not the consequence — and the existing `•` marker
  means "differs from the **default**", not "I just touched this", so it doesn't
  answer "what did I change?". The cheap candidate is a second marker driven by a
  config snapshot taken when the screen opens; full `Ctrl+Z` would have to interact
  with the server-restart debounce and with profile edits, which travel as a
  different intent. See docs/history/settings-navigation.md §7.

### Post-M9: undoing an edit on the settings screen (done)

- **The follow-up deferred as fork R6** of the focus-model track
  ([settings-navigation.md §7](../../docs/history/settings-navigation.md)): that change
  removed the main *source* of accidental edits, this one removes the
  *consequence*. Plan with forks U1–U4 —
  [docs/history/settings-undo.md](../../docs/history/settings-undo.md) (**user's
  decision, 2026-07-31**, all as recommended). Branch `feat/settings-undo`.
- **The finding that made it cheap, and it came from reading the contract rather
  than the screen:** `SettingsIntent::SaveConfig` already carries the **whole**
  working `AppConfig`, and `SaveProfile` a full `ProfileEdit` snapshot of the
  profile's fields. So undo is *restore an older snapshot into the working copy
  and emit the same intent again* — **no new `AppCommand`, no new `AppEvent`, no
  orchestrator change at all**. The three config fields the orchestrator owns
  (`last_active_chat`, `api_keys`, MCP TOFU pins) are already restored by
  `handle_update_config` on every update, so replacing a whole older config
  cannot clobber them — that trap was closed before this feature existed.
- **Recording goes through one funnel, not nine call sites.** Config/profile
  mutations happen at ~9 places (`toggle_field`/`cycle_field`/`apply_text`/
  `reset_field`/`apply_choice`/`toggle_profile_tool`/`apply_profile_text`/persona
  create+delete); hooking each is shotgun surgery and easy to forget when a field
  type is added later. Instead `handle_key` was split into a thin wrapper over
  `handle_key_inner`: snapshot before, dispatch, keep the snapshot **only if a
  `SaveConfig`/`SaveProfile` intent came back**. The §2.1 exclusions
  (API key / assistant-profile create+delete / MCP catalog confirmation) then
  fall out of that `match` instead of needing guards of their own — the same
  "single funnel" property the codebase already uses for `mark_feed_changed` and
  `InputBox::touch`.
- **The snapshot is cheap where it matters**: `pre_edit_snapshot` returns `None`
  unless the key *could* commit an edit (`Enter`/`Space`/`Del`/`←`/`→`/`Ctrl+N`/
  `Ctrl+D`), so typing inside a text editor clones nothing per keystroke — only
  the committing `Enter` does. The transient snapshot holds both stores (the kind
  is only known from the intent afterwards); `record_edit` keeps the relevant
  half, so a *stored* step stays small.
- **Coalescing by field (U2)**: a run of edits to the same field is one step,
  keeping the *oldest* "before" value — cycling `managed → external → openai`
  undoes to `managed` in one press, mirroring `InputBox`'s own snapshot
  coalescing and producing fewer saves (and server restarts) on the way back.
  `None` for persona `Ctrl+N`/`Ctrl+D`, which therefore never coalesce —
  otherwise creating two personas would be undone by a single press.
- **The jump (U4) reuses the search index.** `build_search_index` already
  enumerates every field of every section/subsection *with its rendered value*,
  and `jump_to_selected` already moves section+subsection+field — so undo
  compares the index before and after the restore and lands on the first field
  present in **both** whose value differs. `SearchHit` gained an `id: FieldId`
  for this: positional comparison would be wrong exactly where it matters, since
  changing an engine mode changes *which* fields are visible. Fields that only
  appear or disappear are skipped — they are the consequence of the change, not
  the change — which leaves the mode field itself as the match. `jump_to_selected`
  was split so both callers share `jump_to`.
- **Key layering**: `Ctrl+Z`/`Ctrl+Y` sit **below** the editor/search/choice
  branches in `handle_key_inner`, so while any of those is open the keys stay
  that widget's text undo. Matched by the physical Latin key (`keys::hotkey_char`)
  — layout-independent. Footer gained one entry `Ctrl+Z/Y` in both focus states:
  the settings screen isn't in the `F1` overlay, so the footer is the only place
  these are discoverable.
- **Known consequence, recorded rather than fixed** (plan §5.1): an engine edit
  and its undo each mark a restart, so the debounce coalesces them into **one**
  restart that reloads the server with the values it already had. Correct, merely
  wasteful. Avoiding it means diffing against the *last applied* config rather
  than the previous one — an orchestrator change, deliberately not bundled here.
- **Tests**: 9 behaviour tests, one per fork plus the boundaries — config undo,
  profile undo, coalescing (and that two different fields stay two steps), redo +
  its invalidation by a fresh edit, empty-stack no-op, the API-key exclusion, and
  the jump across sections. **All five changed lines were mutation-tested**;
  worth noting that reverting the key layering also broke the **pre-existing**
  `ctrl_k_clears_and_ctrl_z_restores_multiline_editor`, so the editor's own undo
  is independently guarded. **1674 unit tests green** (+9), 70 `#[ignore]`,
  clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): key handling and screen state on
  one screen — no engine, memory, tool or provider path is touched.

### Post-M9: a settings hint always fits its panel (done)

- **Reported from a screenshot**: the API-key hint ran past the bottom panel's
  last row and was cut mid-sentence — precisely on the half that says what to do
  ("on another computer the key has to be entered again"). Branch
  `fix/settings-hint-fits` (a simple task by AGENTS.md §1: one screen, no
  cross-layer contract, no new dependency — no design doc).
- **Cause**: the panel was a fixed `Length(4)` — a border plus three content
  rows — deliberately constant so the field list wouldn't jump between fields.
  The description was then handed to `Paragraph`'s `Wrap`, which wraps *after*
  layout, so nothing could know it needed a fourth row. **Measured** rather than
  eyeballed: the longest descriptions are ~310 characters (`mcp_enabled`,
  `sm_protocol`, `spec_type`, `mcp_server`, `embed_convention`, `api_key`), i.e.
  four rows at a typical pane width and more on a narrow terminal — a standing
  limit, not a corner case.
- **Sized per field set, not per field** — that is the whole design decision.
  A height following the *selected* field would fix the clipping and shift the
  list on every step down, trading one annoyance for a worse one; taking the
  **maximum hint** over the current field set keeps the panel constant exactly
  where the user is navigating (it can only change when the field set does — a
  section or subsection switch, which already replaces the list wholesale).
  Bounded on both sides: never below the three rows it has always had (short
  sections look unchanged), never above `HINT_MAX_ROWS = 12` and a third of the
  pane — an **MCP server's tool description is arbitrary server text**, so
  without a ceiling one field could push the list off the screen.
- **The full-value preview now gives way to the hint.** It used to be pushed
  first and could eat the whole panel; the height is reserved for the hint, so
  the preview takes only what the hint leaves. The right priority because the
  value is *also* in the list row above (truncated with `…`) while the hint
  exists nowhere else — and it means a long system message can't inflate the
  panel for a whole section.
- **Pre-wrapping is what makes the measurement possible**: two small helpers
  (`wrapped_rows` counts, `wrap_text` builds) over the existing `shared::wrap`,
  so the panel's content is wrapped **before** the vertical layout instead of by
  `Paragraph::wrap`, which is dropped. Both split on `\n` first — `wrap_line`
  treats a newline as an ordinary zero-width character, so a multiline system
  message would otherwise have run its lines together.
- **Tests**: the reported symptom end to end (the API-key field focused at three
  widths — the whole description present in the rendered pane, not a prefix of
  it), the preview priority (a 400-character value plus a described field: both
  the preview and the *complete* hint are on screen), and the height rule as a
  pure function (floor, growth to the longest, cap). **Mutation-tested**: pinning
  the height back to three rows fails the first, dropping the preview's
  `truncate` fails the second. A test-helper trap worth recording — the pane's
  text has to be extracted **between** the panel's borders, since a `│` at either
  end lands between the joined rows and breaks a match on wrapped text.
  **1690 unit tests green** (+3), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): layout and rendering on one
  screen — no engine, memory, tool or provider path is touched (the precedent set
  by the settings redesign and the focus-model work).

### Post-M9: a disclaimer for what the models say and do (done)
- **The question that started it** (user, 2026-08-09): the MIT text disclaims
  liability "in connection with **the Software**" — but this app ships no model.
  The user downloads any GGUF they like, including a fine-tune with its safety
  training removed, and every word on screen comes from it. Whether "the
  Software" reaches that far is exactly the kind of question one does not want
  to be arguing after the fact.
- **The MIT file is not the place to answer it.** The obvious move — append a
  couple of paragraphs to `LICENSE` — quietly breaks three things: the `MIT`
  SPDX identifier published in `Cargo.toml`, `packaging/nfpm.yaml` and the
  README badge stops being truthful; GitHub's `licensee` and distribution audits
  match the file by *similarity* against the canonical text, so extra prose
  makes them report "Other" (MIT is short, so a few lines is a large fraction);
  and downstream users lose the right to treat it as plain MIT. So: a separate
  root `DISCLAIMER.md`, and `LICENSE` stays byte-identical. A gate test
  (`credits::license_file_carries_nothing_but_the_mit_text`) holds that line —
  it asserts the file ends on `SOFTWARE.` and contains no markdown heading,
  which is precisely the shape the tempting edit would take.
- **What it covers**, seven sections: generated output carries no warranty of any
  kind; you choose the model and accept its license/AUP, and the app applies no
  filtering or moderation *by design*; not medical/legal/financial advice and not
  for safety-critical use; the autonomous side (Python sandbox, `fetch_url`, MCP
  servers, sub-agents, a self-model that rewrites its own system message and
  sampling) runs on your machine at your risk, and the confirmation prompts are
  not a security boundary against prompt injection; what leaves the machine for a
  cloud provider; the liability limit itself; and a mandatory-law carve-out.
- **It has to reach the user, not just the repository.** New `F1` tab
  "Disclaimer" next to "License", plus `DISCLAIMER.md` in the release archives,
  the `.deb`/`.rpm`/`.pkg` doc directory and the Windows installer.
- **The tab reuses our own markdown renderer** (ADR 0003) instead of the
  license tab's paragraph-reflow: the source is markdown, and headings and lists
  are what make a seven-section legal notice readable in a terminal. That gave
  `markdown::render` its first production caller, so its `#[allow(dead_code)]`
  came off. One addition on top of the feed's treatment — a wrapped list item is
  hung under its own marker (the feed's lists are short; these run four rows, and
  without the hang a continuation row reads as the next item).
- **A layout trap the tab strip was one label away from**: the dialog is a fixed
  76 columns and the strip is a single line, so a sixth tab put the `ru` strip
  six columns over the edge — and what gets truncated is the *rightmost* tab, in
  one locale only, which nobody working in the other locale would ever see. Fixed
  by shortening the `ru` hotkeys label, and pinned by
  `the_help_tab_strip_fits_the_dialog_in_every_locale`, which measures the
  rendered strip for every bundled locale rather than trusting the next label to
  be short.
- **Two fixes from the user's screenshot of the finished tab.** The `ru` label is
  the borrowed "disclaimer", not the native word first tried: the borrowing is
  established in Russian and reads unambiguously, where the native one mostly
  means a slip of the tongue. It costs two more columns, which puts the `ru`
  strip at exactly 76 of 76 — the gate test above is what makes that safe to ship
  rather than lucky.
- And the heading's underline **ran out to the left of its text**: a level-1
  markdown heading is accent+bold+`UNDERLINED`, the writer puts that on the
  `Line` rather than on its spans, and a line style covers every column of the
  row — including the two-space indent prepended here. Folded into the content
  spans instead (`line_style.patch(span.style)`, so a span's own overrides
  win), leaving the indent unstyled. Pinned by
  `the_disclaimer_indent_does_not_inherit_the_heading_style`, which reads the
  cells to the left of the `#` out of the rendered buffer;
  **mutation-checked** — restoring `out.style = line_style` fails it.
- **Tests**: **1956 unit tests green** (+4), 81 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check`/`doc_index_check` clean.
- **A live run isn't required** (AGENTS.md §3): a document, a help tab and
  packaging file lists — no engine, memory, tool or provider path is touched.

### Post-M9: the help tabs stopped clipping their descriptions (done)

- **Symptom**, spotted on a screenshot of the "Commands" tab in `ru`: `/image
  paste`'s description ended mid-word at the dialog's edge — the last word cut
  short, with no ellipsis and nothing to say it had been cut.
- **Cause.** `key_lines` builds one `Line` per entry and hands them to a plain
  `Paragraph` with no `.wrap()`, inside a dialog of `HELP_WIDTH = 76` columns.
  Anything wider is dropped by the renderer, silently. Measured across both
  bundled locales, the overflow was not one row and not one language: in
  `HELP_COMMANDS`, `/image paste` was 78 columns in `ru` (75 in `en`) and the
  `/image attach` row 76; in `HELP_KEYS`, `Ctrl+F` was **89** in `en` and
  84 in `ru`. Exactly the shape lessons §7 describes — a shared fixed-width strip
  where the label's width *also* differs per locale, so whoever writes the row
  sees it fit in the language they happen to be reading.
- **Wrapping, not shortening.** Shortening means writing the help twice — once
  short enough for `en`, once for whichever locale the label is widest in — and it
  loses the sentence's content to a layout constraint. The description now wraps
  through `shared::wrap::wrap_ranges` (already used by the License/Disclaimer
  tabs) and the continuation is **hung under the column the description starts
  in**, so the label stays a column rather than a paragraph's first word. The
  column is measured per row and per locale, in **display columns** — a label can
  carry `↔` or `│`, where `.len()` would be bytes — and both branches of the label
  span (`palette.keycap` and the command colour) pad with a space on each side, so
  one measurement covers both tabs. The scroll model is by line count, so the extra
  rows need nothing: the tabs already scrolled.
- **`wrap_ranges` spills the break's whitespace into the row it ends** (so the next
  row starts on a word, not a space). Invisible when drawn, but it makes a row
  *measure* wider than it draws — and the gate below measures. Trimmed per row.
- **The gate, mutation-checked in both directions** (lessons §2). Forward: every
  row of both tables, in every bundled locale, must fit `HELP_WIDTH`. Reverse: a
  deliberately over-long description must come back as *several* rows that fit,
  hung at the right indent, with no word lost. Disabling the wrap (`body =
  usize::MAX`) turns both red — the width gate reporting the real pre-existing
  overflow (`HELP_KEYS` at 86 columns in `ru`), which is what makes it more than a
  restatement of the wrapper. Without the second test the first would be nearly
  vacuous: after wrapping, the only way to overflow is a label wide enough to eat
  the dialog on its own.
- Found while adding `/exit` — sizing that row against the table is what turned up
  the neighbours ([ui-input.md](ui-input.md), "a typed route out").

**Tests** (+2), 2163 green. **A live run isn't required** (AGENTS.md §3): help-tab
layout only — no engine, memory or tool path.

### Post-M9: the help dialog sizes itself, and its tables align (done)

- **Asked from a screenshot of the dialog** (branch `feat/help-adaptive-layout`):
  the window was a fixed 76×34 whatever the terminal, and the "Hotkeys"/"Commands"
  tabs read as a ragged list — each description started right after its own label,
  so the left edge of the text wandered per row, and a wrapped continuation hung
  at a different column on every row that wrapped.
- **The size now follows the terminal between bounds** (`help_size`):
  content 76–96 columns × 34–44 rows, keeping `HELP_AIR = 6` screen cells free
  while growing. The floor is the old size — deliberately: 76 is the `ru` tab
  strip's exact budget, and the strip gate now measures against
  `HELP_MIN_WIDTH` by name. The ceiling caps line length for readability (a
  90-char license line is where reading starts to hurt), and it is a cap, not a
  target — `centered_rect` still clamps below the floor on a cramped screen, the
  same hard degradation as before. One size for every tab, so the window still
  doesn't "jump" on switching; the constants stay in `popups.rs` next to their
  consumer.
- **One description column per tab** (`key_lines`): labels are resolved and
  measured **up front** — the column is one past the tab's widest localized
  label, so it is per tab *and* per locale (the previous per-row measurement kept
  each row inside the dialog but aligned nothing). The gap to the column is a
  plain unstyled span; wrapped continuations hang under the same column, which
  makes them read as the description's second line rather than as a new entry.
- **Groups, not headers.** Related entries (composing / selection / navigation /
  conversation / editing / toggles / exit; files / images / RAG / housekeeping /
  TTS / exit) are separated by one blank line at render time. Headers were
  considered and rejected: they would add ~13 locale keys for labels the
  grouping already implies. Getting the breaks past the duplication gate took
  **three shapes**. The first cut nested the tables into `&[&[(&str, &str)]]` —
  SonarCloud failed the PR at **16.1% new-code duplication** (bar ≤ 3%):
  regrouping rewrote every row, and 50 same-shape tuple rows in *changed* lines
  are exactly the sliding self-duplicate lessons §2 describes (third
  recurrence). The second kept the rows byte-identical to `main` and inserted a
  `GROUP_BREAK` sentinel *row* between them, betting that an identifier token
  would sever the run the detector slides over — measured: **3.3%**, still red.
  The residual was the sentinels themselves: whatever the tool's normalization,
  the tables' line ranges stay flagged, and every one of the 12 inserted lines
  was a *new* line inside a flagged range. The conclusion is stronger than the
  first lesson: not "don't rewrite the rows" but **"no new line may sit among
  them at all"** — so the shipped form encodes the breaks *outside* the table:
  `KEY_GROUP_OPENERS`/`COMMAND_GROUP_OPENERS` name the row that opens each
  group, and `key_lines` draws the blank line before it. The label-keyed
  indirection can silently orphan a break when a row is renamed, so a gate
  (`group_openers_open_real_rows`) pins every opener to exactly one row, never
  the first, and — by position — a blank line immediately before each opener's
  rendered row.
- **The "Components" tab became two leader tables** (`leader_table`) — the
  user's follow-up screenshot: at 94 columns the two narrow tables hugged the
  left edge and the right 40 columns sat empty. The first fix centered the tab
  as one block; **rejected by the user** (decision 2026-08-13): nothing else in
  the app or on the site centers text, and the floating block read as
  accidental. The shipped layout is the one the user specified — the name on
  the left margin, the version and license columns aligned under each other
  against the right margin (which mirrors `HELP_PAD`), and the run between
  bridged by a dotted leader `.....` in `border_style(false)`, the dimmest
  color the palette has. The leader is what makes a right-anchored column
  readable at all: it carries the eye across the gap, which is why the earlier
  column-spreading and bare flush-right variants had been rejected. Both
  tables (crates; grammars, whose long repository pins set a different middle
  column) share one helper and one geometry.
- **Sonar's second finding, cognitive complexity 17 > 15 on `key_lines`**
  (rust:S3776), resolved by the same reshape: the per-entry wrap-and-push moved
  to `push_key_entry`, leaving `key_lines` as resolve → measure → dispatch.
- **Tests** (+4, one replaced): the width gate now runs at **both bounds** of the
  range; `descriptions_share_one_column_per_tab` pins the alignment itself
  (mutation-checked — freezing the gap to one space turns it red);
  `the_dialog_follows_the_terminal_between_its_bounds` pins floor/middle/ceiling
  of `help_size`; `group_openers_open_real_rows` pins the opener contract;
  `components_columns_anchor_right_with_leaders` pins the leader tables — every
  row fits, names on the margin, one shared column per anchored pair, the
  widest license touching the mirrored right margin, dots in the border color —
  at both width bounds. The commands-tab dump tests
  moved to a 90×50 backend: with the group separators the tab is 23 rows, and
  the last row (`/exit · /quit`) fell below the fold of the old 90×40 — the
  assertion caught it, which is the "assert the symptom" family working as
  intended. The short-terminal scroll-clamp bound grew by the opener count for
  the same reason.
- **A restore trap worth naming**: after un-mutating the file via
  `os.replace(backup, file)`, `cargo test` **reused the mutated binary** — the
  backup's mtime predates the build, so cargo saw nothing to rebuild and the
  gate stayed red on correct code. `touch` after a byte-level restore. (Related:
  a Python `io.open(..., 'w')` writes CRLF on Windows — normalize before git
  sees it.)

**Tests**: 2167 green (+4), 96 `#[ignore]`, clippy `-D warnings`/fmt clean.
**A live run isn't required** (AGENTS.md §3): help-dialog layout only — no
engine, memory or tool path.

### Post-M9: `/export` — a conversation to a file (done)

- **Why now.** The roadmap has wanted this since the chat list was written, and
  the OSC 52 track promoted it: JupyterLab drops the escape and its pty is
  server-side, so a conversation there has **no** route to the user's machine —
  a file on the server does, since the notebook interface can open it. Design
  doc: [docs/history/chat-export-file.md](../history/chat-export-file.md), all
  seven forks decided by the user on 2026-08-15.
- **Two of the user's decisions went against the recommendation, and both were
  right.** F6: no Markdown decoration — the file gets the clipboard's text
  verbatim, because the *content* is already Markdown (that is how models write)
  and the labels are just labels. That removed the `Style` parameter the design
  had planned to thread through the formatter, and with it any chance of the two
  routes drifting; the test asserts the file equals what `F5` copies. F3: the
  current working directory rather than an `exports/` folder beside the data.
  Checked before implementing that the app never calls `set_current_dir`, so
  "current" means where the user launched it — which in the JupyterLab case is
  the folder their file browser already shows.
- **JSON is the format we already read.** `mindfork-import` v1 with explicit
  `id`s, so `mindfork-rs import` puts an export back onto the *same* chat rather
  than making a copy. The cost is stated where it cannot be missed: the format's
  messages are `{role, text, thoughts?, timestamp?}`, so **tool calls are
  dropped**, and every JSON export's note names `md` as the format that keeps
  them. The round trip is a test, not a claim — the export is parsed by our own
  `parse_import`.
- **The orchestrator finishes the job.** Unlike `CopyChat`, which hands text
  back for the UI to put on the clipboard, an export ends in a **path** — and
  the orchestrator already owns both the conversation and the disk. It answers
  with the absolute path (canonicalized, with Windows' `\\?\` prefix stripped:
  the user is going to paste it somewhere).
- **A test found a real off-by-one in the slug.** The length check sat *after*
  the dash and the character were pushed, so a 60-character budget could produce
  61 — and `to_lowercase` can yield more than one character, which would have
  made it worse. The budget is now checked before anything is written. The
  boundary case was in the test plan (§6: "a title longer than the filesystem
  tolerates"), which is why it was caught at all.
- **A surviving mutation, and the wrong first attempt at covering it.** Removing
  the "refuse an existing file" check passed every test. The fix I reached for
  first — an async orchestrator test that sends a message and waits for the
  export event — **hung**: with no backend the loop sits in connect timeouts, and
  the shared `wait_for` is unbounded (lessons §2: a test that waits on an event
  must bound the wait). The sibling test for the sibling feature was already
  the right shape and three lines away: `copy_chat_emits_clipboard_text_or_error`
  drives a `bare_orch_rx` synchronously, no loop and no backend, because copying
  neither starts a turn nor needs one — and neither does exporting. Rewritten
  that way the four tests run in 30 ms and the mutation dies.
- **One line is deliberately untested and says so in the code**: resolving a
  generated name against the current directory. A test for it would have to
  `chdir` the process, which is global state shared with every test running in
  parallel; what the line does with the name (`export_filename`, `slugify`) is
  unit-tested, and what is left is `PathBuf::from`.
- **The i18n key scanner read `"notes.json"` as a bundle key** in a parser test's
  fixture — the `notes.` prefix belongs to the notes tools (lessons §7). There is
  already a whitelist entry for `notes.md` from an older test, but the lesson
  says to rename the fixture rather than grow the exception list, so the test
  files became `transcript.*` and the gate keeps its teeth.

**Tests** (+13, 2269 → 2282 green). The command grammar as one table: bare, a
format word, a path, a format inferred from the extension, the word winning over
the extension, paths with spaces and quotes, absolute paths untouched, quotes
around nothing reported, near-words falling through, the per-locale gate. The
formatter: the date-led filename; the slug over punctuation, separators,
Cyrillic, empty and all-punctuation titles; the character-boundary trim; the
JSON round trip through `parse_import`; the documented tool-call loss. The
orchestrator: the file written and its note, the refusal to overwrite with the
first file left intact, the Markdown file equalling the clipboard byte-for-byte,
the JSON note naming `md`, and an empty chat refused with nothing written.

**A live model run is not required** (AGENTS.md §3) — no engine, memory or tool
path. One machine is enough for acceptance: `/export`, then open the file.

### Post-M9: the "Components" leaders got a step quieter (done)

- **Asked from a screenshot** of the help dialog's "Components" tab: the dotted
  leaders read as content rather than as alignment. When the tables were built
  the leader took `border_style(false)` on the reasoning that the border is "the
  dimmest color the palette has" — which was true of the *named* roles, but not
  of the palette as a whole. `keycap_bg` is a step quieter in every theme
  (dark `#363a42` → `#21242a`, auto `DarkGray` → `#24272d`, light `#bec1c6` →
  `#dee0e4`, i.e. quieter in the light theme's direction too), and it is the
  backdrop the **active tab** already sits on two rows above, so the tab strip's
  highlight and the leaders below it are now literally the same color — which is
  what the user asked for by name.
- **The dots are structure, not text**, which is why borrowing a background role
  as a foreground is right here rather than a shortcut: the leader carries the
  eye across the gap and should not compete with the crate name at either end.
  Nothing else changed — the geometry, the column math and the degradation on a
  clamped dialog are untouched.
- **Tests**: no new test; `components_columns_anchor_right_with_leaders` already
  pinned the leader's color, so the one-line change had to be made in the gate
  too — the assertion now names `keycap_bg`. That is the gate doing its job: the
  color is pinned, so it cannot drift silently, and changing it on purpose costs
  one line.

**Tests**: 2282 green (unchanged), 97 `#[ignore]`, clippy `-D warnings`/fmt clean.
**A live run isn't required** (AGENTS.md §3): a color in the help dialog — no
engine, memory or tool path.

### Post-M9: automatic chat titling on the first exchange (done)

- **The model-written title stopped waiting to be asked** (design plan
  [docs/history/auto-chat-title.md](../history/auto-chat-title.md), spec §11.2):
  a new conversation names itself once, on its first exchange, per
  `interface.auto_title` — a tri-state (after the user's message / after the
  assistant's reply / off, default **after the reply**). The user's observation
  set the default: cloud UIs title on the user's first message, and the names
  are visibly worse than what the same model writes *after* the reply, because
  the reply is what disambiguates a terse opening. The task itself is the one
  `Ctrl+R` in the chat list has always run (`orchestrator/title.rs`) — the
  track added a **trigger and an origin**, not a second mechanism.
- **Reading first shrank it to plumbing** (lessons §3): `handle_done` already
  runs four `maybe_auto_*` follow-ups after applying a turn, so "after the
  reply" is a fifth sibling; "after the message" is a line in `handle_send`,
  deliberately fired **after** `start_generation` so on a single-slot
  `llama-server` the title request never queues ahead of the answer — the
  mechanical reason the cloud-UI timing is not the default here.
- **"First reply" means first substantive reply**, judged by a pure predicate
  (`rename_chat::has_assistant_reply`, greeting-blind) read *before* the turn
  is applied: a cancelled or failed first turn defers the title to whichever
  turn actually answers; an existing conversation can never match, so nothing
  mass-retitles on upgrade; and regenerating the first reply re-titles on
  purpose (D2) — the truncation removed the only reply, so the next one is
  again the first.
- **A person's choice outranks a model's, at both ends** (D1):
  `handle_rename` — both manual routes, the list's `F2` and `/rename <title>` —
  sets the additive `Chat.renamed_manually`, checked when the trigger fires
  *and again* when a result lands, so a rename made during the task's seconds
  of flight wins. Model-written titles never set the flag: a requested
  `Ctrl+R` after an automatic title still works, and re-asking is always legal.
- **Origins differ in visibility, not mechanism** (D4): `TitleResult` carries
  `Requested`/`Auto`; automatic failures go to the log — the spec §6.8 rule
  for background turns — while the chat-list action keeps reporting into the
  overlay the user is looking at. No new `AppCommand`/`AppEvent`: the trigger
  is orchestrator-internal and the result rides `title_tx` → `ChatRenamed`.
- **Settings**: one Choice row in Interface → Behavior (`FieldId::IAutoTitle`),
  the `clipboard_osc52` shape — fork F1, user's decision 2026-08-17: a
  tri-state over a toggle + trigger pair, because it leaves no dead
  "off but a trigger picked" state and costs one row; "after the user's
  message" is listed first (user's decision), Off last like every tri-state
  here. The settings field counter moved by one, which is exactly the
  one-character drift the demo-dump gate caught (`"2"` → `"3"` in four
  settings dumps) — dumps and the four settings PNG/SVG pairs regenerated;
  fonts converted from the site's woff2 per lessons §1, fidelity confirmed by
  the untouched frames coming back byte-identical.
- **Test-fixture interference was measured, not guessed** (lessons §3): with
  the trigger on by default, 8 orchestrator tests broke — engines that are
  finite scripts had an entry consumed out of turn, and `CapturingBackend`
  tests had `last` overwritten by the title request. Fixed at the call sites
  with a shared `no_auto_cfg()` and a one-line comment each, keeping
  `spawn_orch` on the true production default so the trigger's own e2e tests
  (`tests/title.rs`) prove **on-by-default** against it. A ninth failure was
  the demo-dump drift above.
- **Tests**: +9 unit (flag serde additive; predicate table; on-by-default e2e
  with the second-exchange absence anchored to the first fire; AfterUser e2e;
  regenerate-retitles; manual-outranks with the requested arm as positive
  control; quiet-vs-loud failures both arms; settings row + cycle order;
  config default) and +1 live smoke.

**Tests**: 2299 green, 99 `#[ignore]`, clippy `-D warnings`/fmt clean.
**Live run** (AGENTS.md §3, the trigger sits on the send/done path of every
turn): against gemma-4-31B (`llama-server`, 192.168.1.20) —
`auto_title_first_reply_e2e_live` replaced the default title with one naming
the topic — "Why the sky is blue", in the conversation's own Russian — with no
command sent (GO), `i18n_en_profile_title_e2e_live` still green
on the requested path, and the full `orchestrator::tests::live` set run as the
turn-path regression scope — **35 passed / 0 failed** in one sweep (755 s),
every first-exchange smoke now firing a real title request on the way.

### Post-M9: the settings hint panel — fixed height, scroll, its own focus stop (done)

- **Reported from use** (user, 2026-08-18, with screenshots): switching sections
  resized the bottom hint panel abruptly — the per-field-set height from *"a
  settings hint always fits its panel"* kept the panel still while stepping
  **within** a section, but each section got its own height, so `Tab` jumped the
  border by up to nine rows. Asked for: one height for every section, a
  scrollbar when the text doesn't fit, and the panel focusable from the fields —
  `↓` past the last field selects it (green `▌` rail like a field's), further
  `↓`/`↑` scroll, `↑` at the top steps back out. Branch
  `feat/settings-hint-scroll` (one screen, no cross-layer contract — no design
  doc per AGENTS §1).
- **The fix inverts the previous track's mechanism but keeps its guarantee.**
  That track sized the panel to the longest hint *because* clipping was
  permanent; once the panel scrolls, clipping is recoverable, so the height can
  be a constant (`HINT_PANEL_ROWS = 5` content rows + border, a third of the
  pane on small terminals — deliberately independent of the per-section header
  height, or the "constant" would still wobble by one row around tab strips).
  The guarantee "no hint is ever unreadable" survives via a different route —
  which is why **PgUp/PgDn scroll the panel from the fields focus too**, beyond
  what was asked: the panel's own focus stop sits past the *last* field, so from
  a mid-list field (where the long hints actually live — the API-key rows) it is
  unreachable going down without changing what the panel shows. Without that
  addition the old fix's own test case (the external key hint at 70 columns)
  would have regressed to clipped-and-unreachable.
- **The panel shows the field the cursor left.** In `Focus::Hint` the hint is
  `fields[field_idx]`'s and `field_idx` cannot move — so what you scroll is what
  you were reading, and `↑`/`Esc` return to the same field. The list keeps its
  selection (unhighlighted — the single green rail moves into the panel) so a
  long section doesn't jump to its top the moment focus crosses the border.
- **Order flip inside the panel: description first, value preview after.** The
  old panel drew the preview first and truncated it to what the hint left; with
  a fixed viewport that priority had to become an *order*, and the description
  wins the first rows for the old reason — the value is also in the list row,
  the description exists only here. The preview keeps its 400-char cap (the
  panel is a peek; the full value is one `Enter` away) but now marks the cut
  with `…`, so scrolling to the end doesn't read as the value's end.
- **Scroll state is one offset plus three render caches** on the screen struct:
  the key handler cannot re-wrap text without the frame width, so the ceiling
  (`hint_scroll_max`), the page size (`hint_view_rows`) and the content owner
  (`hint_for`) are written by `render_desc_panel`, and the offset resets when
  the owner changes (fields *and* sections alike — every section's first field
  id differs) and clamps when content shrinks under it. The gutter prepend
  preserves each wrapped line's own style (`wrap_text` styles the `Line`, and a
  `Line`'s style covers the whole row — lessons §5), with the rail span's green
  fg winning over it.
- **Mutation-tested, nine mutations, one survivor worth recording**: dropping
  the offset reset survived its first fixture because the *next* section's hint
  was short and the render clamp zeroed the offset by itself — the reset is only
  observable on a long→long switch, so the Tab test narrowed to 46 columns where
  the subsection selector's hint overflows too. The other eight (height
  constant, enter/exit transitions, rail, scrollbar, order, kept selection,
  write-back clamp) died on first run. Footer note: the fields footer gained
  `PgUp/Dn`, which re-wrapped it at 92 columns and cut the `•`-marker test's row
  off screen — the test's subject is the marker, so it got a taller window, and
  the two panel-position tests pin their width wide enough that footer wrap
  (whose height is the footer's behaviour, not the panel's) stays out of frame.
- **Demo screenshots regenerated** (the drift gate went red as designed): the
  two settings frames in both themes; every other frame came back byte-identical,
  which is the pipeline's own faithfulness check (lessons §1).
- **The PR analysis was read before calling it done** (lessons §10), and it had
  one finding the green gate didn't block on: `render_desc_panel` at cognitive
  complexity 20/15 (S3776) — the content build moved out into
  `hint_panel_lines`/`value_preview_lines` beside `wrap_text`, leaving the
  render method the scroll state and the viewport.
- **Tests**: 2311 green (+7: one height-rule test replaced, seven added —
  constancy as a pure rule and as a rendered row, the enter/scroll/exit flow,
  mid-list reachability by PgUp/PgDn, panel-scoped scrollbar, the Esc ladder,
  Tab keeping focus while restarting the scroll, the kept list selection), 99
  `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): layout, key handling and
  rendering on one screen — no engine, memory, tool or provider path is touched
  (the precedent of the settings redesign and focus-model tracks).
