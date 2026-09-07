# Journal — UI: screens

The screens above the feed: settings, the chat list, the self-model viewer, search, help/About, and the popups and confirmations that belong to them.

**Reference documents for this area:** architecture.md §10, spec.md §11.6–§11.8

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (63)

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
- Post-M9: the settings hint panel — one height for every section (done)
- Post-M9: the "About" tab became a leader table (done)
- Post-M9: the code workspace — stage 4, the changes screen (done)
- Post-M9: the licence and the disclaimer in Russian (done)
- Post-M9: sub-agent chats, PR 4 — the transcript in the list, opened read-only (done)
- Post-M9: sub-agent chats, PR 5 — transcripts in search and the cross-chat tools (done)
- Post-M9: sub-agent chats, PR 6 — auto-title at landing, the run chip, the demo transcript (done)
- Post-M9: sub-agent chats, PR 7 — the transcript while it runs (done)
- Post-M9: sub-agent chats — the sub-agent's text streams into its transcript (done)
- Post-M9: sub-agent chats — the parent's round in progress is mirrored too (done)
- Post-M9: command-only control — stage 3 (`/autotitle`, the profile texts, `/impersonation`) (done)
- Post-M9: the chat list scrolls symmetrically (done)
- Post-M9: the same scrolling rule for every other list (done)
- Post-M9: sub-agent chats — the child view arrives before the feed is built (done)
- Post-M9: the chat list folds a chat's sub-agent transcripts (done)
- Post-M9: help hotkeys by context — stage 1, per-screen sections (done)
- Post-M9: help hotkeys by context — stage 2, `F1` everywhere as a runtime overlay (done)
- Post-M9: the "Globally" section, and `?` dropped as a help key (done)
- Post-M9: the value column holds against an MCP tool's name (done)
- Post-M9: the confirmation toggle is named by what it does (done)
- Post-M9: the build date on the "About" tab (done)
- Post-M9: the chat list counts messages as the conversation reads (done)
- Post-M9: the self-model screen reads as two named halves (done)
- Post-M9: the self-model screen lists its observations newest first (done)
- Post-M9: one hint grid, and footers that name only the keys that work (done)
- Post-M9: the privacy policy joins the disclaimer on one "Legal" tab (done)
- Post-M9: the background run on the list, in the feed and on the bar (done)
- Post-M9: the stop key on a background transcript, and the unread chat (done)
- Post-M9: the tasks screen — every background run on one surface (done)
- Post-M9: a title is cut where it is drawn, not where it is stored (done)
- Post-M9: the "Sessions" hint names the knob that widens the group (done)
- Post-M9: stopping a silent task from the tasks screen (done)

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
- Post-M9: one hint grid, and footers that name only the keys that work (done)
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

### Post-M9: the settings hint panel — one height for every section (done)

- **Reported from screenshots** (three at once, all the same panel): switching
  sections resized the bottom hint panel by several rows — 3 in Sampling, ~5 in
  Profiles, more in Tools — jerking the field list with it; a long system
  message's preview stopped mid-word (the cap fell inside the last word on
  screen) with no sign that text follows; and the cloud "Model" field showed the *show-model-name
  toggle's* description instead of its own. Branch `fix/settings-hint-panel`
  (a simple task by AGENTS.md §1 — one screen, no new contract, no design doc).
- **The height rule moved one level up, and that is the whole design.** The
  panel was sized to the longest hint of the *current field set* ("a settings
  hint always fits its panel", above) — correct against per-field jitter, but
  every `Tab` replaced the field set and re-derived the height. The panel is
  now the longest hint of **every** field set — all sections, all subsections —
  so one terminal size and locale give one height, and switching sections
  moves nothing. The measure reuses the same per-set rule (`hint_panel_rows`,
  floor/cap intact) over the concatenated catalog; at the gallery width the
  driver turns out to be the backup-password warning (569 chars ≈ 7 rows),
  which is a real hint a user can land on, not an artifact. Two mechanical
  consequences: the cap now subtracts a **constant** header allowance
  (`HEAD_MAX_ROWS`), because deriving it from the current section's real header
  would give tabbed and untabbed sections different caps in a small terminal —
  the very jump being removed; and the enumeration behind the search index was
  extracted into `visit_field_sets` and shared (search, the height, and the
  tests' `field_desc` all walk one list — a sibling copy of the walk would have
  been the duplication-gate shape lessons §2 warns about).
- **Truncation now says it truncated.** The value preview used to be capped at
  a flat 400 characters — on a wide panel that is *less* than the visible rows
  hold, so the text stopped mid-word with empty rows below it, reading as the
  message's actual end. The cap is now derived from the rows the hint leaves
  (`value_preview`: rows × width plus one row's slack — never the whole value,
  a system message can be huge), and anything cut — by rows, by characters, or
  a hint clipped by the cap in a tiny window — gets a visible `…` on its last
  line (`ellipsize_last`, riding `truncate_to_width`'s own marker so it
  survives both the fits and doesn't-fit paths).
- **The wrong hint was a duplicated bundle key, not a wrong lookup.** The
  show-model-name track reused `ui.settings.desc.model_name` for its toggle —
  the key the cloud model-name field had owned since the API-key track — by
  *adding a second entry* to both bundles. JSON map parsing keeps the later
  duplicate with no error anywhere, and no key gate could object: the key
  exists and is used. The toggle's text now lives under
  `ui.settings.desc.show_model_name`, the original text is back on the model
  field, and a new i18n gate (`builtin_bundles_have_no_duplicate_keys`, with a
  planted-duplicate self-check per lessons §2) bans the class; the scan found
  exactly this one duplicate across both bundles. Recorded in lessons §7.
- **Tests**: +6 — panel height equal across all eight sections (measured from
  the screen's bottom border, not absolute y: the contextual footer
  legitimately grows a row in Profiles/Plugins); preview ellipsis with a
  fits-whole control arm; a cap-clipped hint ends with `…` (with an
  it-really-doesn't-fit guard); the model field describes a provider model id
  and differs from the toggle's text; `value_preview` marks only what it cuts;
  the duplicate-key gate. Two existing tests raised their windows (the taller
  panel left a 24-row frame short of the `-ngl` row, a 50-row one short of the
  Sampling list). Extraction traps hit and fixed in-test: the screen's own
  bottom border row matches an all-`─` scan (filter on the corner glyph), and
  "the last two rows" is not a bottom anchor when the footer wraps.
- **Demo dumps**: the four settings captures changed (the panel grew to the
  catalog height — border one row lower, both sections now byte-identical in
  panel height; every showcase needle stayed in frame) — dumps and the four
  settings PNG/SVG pairs regenerated; fonts recovered from the site's woff2 per
  lessons §1, fidelity confirmed by a pre-change render coming back
  byte-identical across all ten images.

**Tests**: 2310 green, 99 `#[ignore]`, clippy `-D warnings`/fmt clean, all four
repo gates clean. **Live run not required** (AGENTS.md §3): pure settings-UI
rendering and locale data — no engine, memory or tool path touched.

### Post-M9: the "About" tab became a leader table (done)

- **Asked from a screenshot** of the help dialog's "About" tab: on a wide
  terminal the right side is empty — "maybe the same trick as the 'Components'
  tab". It was the same complaint, and it had the same cause: the labels sat in
  a column sized to the widest of them (the `ru` label for the repository, 12
  columns) and every value
  started one step past it, so at 84 columns of content the rows ended around
  column 50 and the right ~30 sat blank, while the dialog's width is earned by
  the "Hotkeys" tab and cannot shrink for this one.
- **The shipped fix is the geometry the "Components" tab already had**: labels
  on the left margin, the values in **one** column anchored so the widest of
  them (`REPO_URL`, 38 columns) touches the mirrored right margin, and the run
  between bridged by a dotted leader in `keycap_bg`. The three links keep a
  shared left edge, which is why the values form one column rather than each
  row being flushed right on its own — the per-row variant fills the width
  completely and was drawn for the user, who chose the shared column (decision
  2026-08-19). The shared column is also what "Components" needs (two columns
  must line up under each other), so the two tabs share one helper instead of
  owning two geometries.
- **`leader_row` is that helper**, extracted from `leader_table` rather than
  written beside it: it takes the left span, the tail spans and the column the
  tail starts in, and owns the dot run, its color and the space on each side.
  `leader_table` now computes its two columns and calls it; `about_lines`
  computes one column and calls it. The dot color and the below-minimum-width
  degradation (the dots run out, the row clips) are therefore defined once.
  The per-item blank line stays — it was asked for when the tab was built, and
  the leader is what makes an anchored column readable across that spacing.
- **Two rows were added while the table was open** (the user's choice from the
  same question): **license** — `credits::LICENSE_ID`, read from `Cargo.toml`'s
  `license` field via `env!("CARGO_PKG_LICENSE")` so the row and the manifest
  cannot drift, the full text staying its own tab — and **build target**,
  `credits::platform()` over `std::env::consts::OS`/`ARCH`, which is what the
  binary was *built* for and so answers "it does X on my machine" with the
  actual build rather than with what the user believes they downloaded. A third
  candidate, the portable **data directory**, was offered and **not taken**: it
  needs `Paths::resolve()` cached in `HelpState` (calling it in `render_help`
  would be file I/O every frame), and it would put a real user path on a tab
  that may yet be screenshotted for the site. With the two rows the tab fills
  26 of its ~33 content rows, so the vertical emptiness went with the
  horizontal one.
- **Tests** (+1): `about_rows_anchor_right_with_leaders` mirrors
  `components_columns_anchor_right_with_leaders` — every row fits, labels on the
  margin, one shared value column, the widest value touching the mirrored right
  margin, leaders that are dots in `keycap_bg` — at **both** width bounds and in
  **both** locales (`ru` is the one with the long labels; a squeezed-out value
  column would never show in `en`), plus the facts themselves, `LICENSE_ID` and
  `platform()` included. Mutation-checked: freezing `value_col` to a constant
  turns it red. The restore trap from the "Components" entry applies and was
  paid again — `touch` after putting the backup back, or cargo reuses the
  mutated binary.

**Tests**: 2311 green (+1), 99 `#[ignore]`, clippy `-D warnings`/fmt clean.
**A live run isn't required** (AGENTS.md §3): layout in the help dialog — no
engine, memory or tool path is touched.

### Post-M9: the code workspace — stage 4, the changes screen (done)

`F4` / `/changes`: what the assistant changed in the attached project, as a
unified diff, with per-file revert. Stage 4 of
[docs/history/code-workspace.md](../../docs/history/code-workspace.md); behaviour — spec §9.12.
Branch `feat/code-workspace-changes`.

- **This screen is what makes the editing tools safe.** Stage 2 chose to apply a
  change without asking (design fork F1: no per-edit popup), and the argument for
  that was always "the user sees it afterwards and can put it back". Until this
  stage, only the first half of that existed — the journal has been storing
  pre-images since stage 2 with `entries`/`baseline_of` marked
  `#[allow(dead_code)]`, waiting for their consumer.
- **The diff is built in `features`, not in the screen, and that reverses the
  plan.** §3.5 sketched a screen diffing the selected file lazily. The
  codebase's own rule points the other way and is written down in the
  message-search screen's module doc: a screen is a **pure projection** of a
  snapshot the orchestrator built off the runtime. Following it is also the
  cheaper design — a rendered diff is a fraction of the two files it came from,
  so the event stays small, and it is computed once instead of on every `↑`. The
  screen ended up with no file-system access at all, which is what FSD wanted
  from it anyway.
- **"Too large to display" turned out to be five states.** The plan named one.
  The journal can hand the screen a file that is *gone* (deleted by the user, or
  by a revert that already ran), *binary*, *too large*, *created* (there is
  nothing to compare it against), or *touched and put back by hand*. Each has a
  different next move, so each says which it is; an empty diff pane for all five
  is indistinguishable from a defect (docs/lessons.md §4).
- **Revert and forget are one operation.** The plan listed them as two steps.
  Written apart they are two failure modes, and they are not symmetric: a
  restored file still listed offers a second revert that does nothing, while a
  dropped row whose file was not restored loses the pre-image **for good** —
  those bytes exist nowhere else. So the write happens first and the row is
  dropped second, and `Journal::forget` takes the stored pre-image with the row
  rather than leaving a directory of orphaned copies of the user's source.
- **The selection follows the path, not the index.** Reverting removes a row, so
  a snapshot refresh with an index-based selection silently moves the cursor to
  the next file down — and the next `r` would revert something the user never
  looked at. Keeping the path and falling back to a clamp is two lines and closes
  a class of "it deleted the wrong thing" that no amount of confirmation would.
- **The layout defect was found by looking, not by asserting.** All fifteen
  screen tests passed while the two panes ran together — a file's counts and the
  first diff line shoulder to shoulder, `+12 −4@@ -940,7 +940,9 @@` — and each
  row sized its counts column to its own text, leaving the column ragged. A
  `contains()` assertion cannot see either. Rendering the screen once and reading
  it is what caught them, and both now have tests that pin the **symptom**.
- **The duplication gate found the screen-chrome opening, and it was right.**
  1.4%%, under the 3%% bar, so nothing was blocked — but what it matched was the
  six lines every full-screen screen here repeats verbatim: build the hotkey
  grid, size the status row from it, split vertically, build the titled panel,
  take its `inner`, render it. The changes screen was the fourth copy. So the
  new code got the seam (`shared::ui::screen_chrome`) instead of a fifth, and
  the three older screens keep their copies for now — a mechanical refactor does
  not share a PR with a feature, and they are the seam's obvious next callers.
  Same disposition as stage 1's `/project` parser (docs/lessons.md §2).
- **One hoist, and it removes a copy rather than adding one.** The confirmation
  popup has existed since the dangerous-tool track, `pub(super)` inside
  `screens::chat`; `screens::changes` is a sibling and could not reach it. It
  moved to `shared/ui.rs`, with the *keys* left at each call site — what confirms
  differs (`Enter` here, `Enter`/`A`/`Esc` for a tool call) and only the drawing
  is common.
- **No live run**, per AGENTS.md §3: pure UI, no tool, no engine or memory path,
  and nothing a model sees changes. What would otherwise go untested is the seam
  where the journal, the diff and the revert meet — so that is covered end to end
  through the orchestrator (`orchestrator::tests::project`), with the journal
  written the way the editing tools write it rather than by fabricating a
  manifest.
- **Tests**: 2416 unit (+28). New dependency `similar` (Apache-2.0) — Myers with
  the usual heuristics and a hunk-grouping writer; the only crate in the graph
  that needed it.

### Post-M9: the licence and the disclaimer in Russian (done)
- **The task** (user, 2026-08-22; branch `feat/legal-ru-translations`): Russian
  versions of the disclaimer and the licence, shown both on the `F1` tabs and in
  the Windows installer. This entry is the app half; the wizard and the packaging
  are in [release.md](release.md) ("the installer's legal pages speak Russian").
  Plan: [docs/history/legal-ru-translations.md](../history/legal-ru-translations.md).
- **It reverses a decision that was written down three times** — `credits.rs`,
  `mindfork.iss` and `wizard_rtf.py` each said the legal texts stay English and
  only the chrome around them is localized. The chrome is exactly the problem:
  the `ru` tab strip has carried the Russian label since the notice was written,
  and behind it sat 4 500 words of legal English.
- **A translated licence is not the licence.** The grant is the English text; a
  translation that disagrees with it somewhere would be a second, unintended
  contract. Both files therefore open with one paragraph saying they are
  unofficial, that the English original governs, and that it wins on any
  discrepancy — the practice CC, the FSF and every distribution that ships
  translated licences follow (user's decision, 2026-08-22). A gate test asserts
  that paragraph is still there, because it is the one part of this that cannot
  be walked back after a release.
- **Where they live, and why not next to their originals.** `docs/legal/`, not
  the repository root: the root's `LICENSE*` namespace belongs to the scanners.
  A root `LICENSE.ru.md` is matched by `licensee` and friends by glob, and a
  second licence-shaped file there is how the `MIT` identifier we publish stops
  being believed — the same failure the disclaimer was split out to avoid.
- **The licence translation is `.txt` and the disclaimer's is `.md`**, each
  keeping its original's shape, because each is read by the renderer written for
  that shape. The "License" tab reflows paragraphs and renders no markdown: a `#`
  or a `[link](target)` in that file would reach the screen verbatim — and the
  wizard's RTF converter would carry the same text into the installer. The
  extension says so, and `the_russian_license_is_paragraphs_only` holds it.
- **The tab follows the interface language** (user's decision), which is also the
  only option the geometry left: a separate pair of tabs was out because the `ru`
  strip already measures exactly 76 of the dialog's 76 minimum columns — the gate
  test `the_help_tab_strip_fits_the_dialog_in_every_locale` exists because the
  sixth tab overflowed it once. `Lang::Ru` gets the translation, everything else
  — an external `data/locales/<code>.json` bundle included — the English
  original, since a user-supplied bundle bringing legal text would mean shipping
  someone else's text as ours.
- **The mapping is `credits`', not the call site's.** `license_text(lang)` /
  `disclaimer_text(lang)` next to the `include_str!`s they choose between, so the
  help dialog and any later reader cannot disagree about which text is "the" one
  for a language — the reasoning that put `secret_key()` on the settings struct
  in the external-API-key track. The tabs just pass the `Locale` they already had.
- **The structural gate is the interesting one.** A section quietly missing from
  one language is invisible to anyone reading the other, so
  `the_russian_disclaimer_mirrors_the_originals_structure` compares the *shape* of
  the two files — the sequence of heading levels, the number of list items, the
  rules — rather than trusting a translator (human or model) to have kept all
  seven sections.
- **One wording fix came from looking at it.** Our markdown renderer prints a
  link's target after its text (a terminal cannot click), so
  `[LICENSE.ru.txt](LICENSE.ru.txt)` drew "LICENSE.ru.txt (LICENSE.ru.txt)". The
  preamble's link texts were reworded to describe rather than repeat the target —
  worth knowing before writing markdown that this renderer will show.
- **Tests**: **2443 unit tests green** (+5), 106 `#[ignore]`; `fmt`/`clippy -D
  warnings`/`cyrillic_scan`/`link_check`/`doc_index_check`/`wizard_rtf --check`
  clean. The two translations join `cyrillic_scan.py`'s allowlist for the reason
  `locales/ru.json` is on it: Russian *is* their content.
- **A live run isn't required** (AGENTS.md §3) — two documents, an accessor pair
  and a tab's argument list; no engine, memory, tool or provider path is touched.
  The tabs were still rendered in both languages through the whole screen (the
  new test does it, and the layout was eyeballed from a 100×44 dump).

### Post-M9: sub-agent chats, PR 4 — the transcript in the list, opened read-only (done)

- **What**: PR 4 of the sub-agent track
  ([docs/research/subagent-chats.md](../research/subagent-chats.md) §3.7–§3.8).
  A sub-agent transcript (the run on the call's record, PR 2) is now a row of
  the chat list nested under its parent, and opens as a read-only chat with the
  persona on top. Spec §11.2/§11.3; architecture §10.
- **The list** — `ChatSummary.children` (cards built by `Chat::summary()` from
  the records, in call order); the widget's `visible()` now returns `Row`s
  (chat or transcript: `parent`, `outcome`, `dimmed`) and is where the tree
  rule lives: *a row is shown iff it matches; a chat is also shown, dimmed,
  when a transcript of it matches*. Chats keep the sort mode, transcripts
  follow their parent in call order — "newest first" reads the steps of a
  delegation backwards. The row is `  └ title`, with a muted outcome word
  beside the count when the run did not complete. `Del`/`Ctrl+D` on a
  transcript are not advertised in the hotkey grid; the widget still sends the
  action and **the orchestrator refuses** (`refuse_on_child` →
  `ChatListError`) — one authority, whatever route sent the command.
- **Opening** — the ordinary `SwitchChat` with a transcript's id: the
  orchestrator's single resolver `view(id) → Top | Child{parent, run}` finds
  it; `activate_focused` emits `ChatActivated { child: Some(ChildView) }` with
  the transcript's messages and the **parent's** `feed_view`. **`active_id`
  holds the transcript's id** — the read-only mechanism by construction: every
  handler that looks the active id up in `self.chats` fails closed, and the
  few that must work on a transcript opt in through `view()`/`with_child_mut`:
  rename (manual, sets `renamed_manually`), the title task (`Ctrl+R` — digest
  from the run's messages, result onto the run, the same "manual wins" rule),
  copy, export (a transient `Chat` from the run — an export is a copy),
  speech, `SetFeedView` (routed to the parent), `SetDraft` (ignored), the
  startup restore of `last_active_chat`. `handle_send` additionally answers a
  send into a transcript with an error **and the text returned** — the
  route-independent belt under the screen's braces. Names come from `names_of`
  at activation (the parent persona as `user`, the run's `name` as
  `assistant`) so a profile rename never goes stale.
- **The screen** — `child: Option<ChildView>` set by `app` right before
  `activate_chat` (the one production caller) so the 28 test call sites kept
  their signature; a new `FeedRole::System` bubble (muted rail, `§` icon,
  headed by `CharacterNames.system` — its first use) prepended to the feed;
  `refuse_read_only(what)` — one note naming the parent and the way a
  transcript goes away (lessons §4) — from the three chords, from
  `try_read_only_refusal` ahead of every parser for the nine blocked commands
  (the line is cleared), and at the send (the line is **kept**: the box is
  still there for commands). The input title says *read-only · commands
  only*, the status bar a quiet `≡ transcript` chip. The `chat://` address
  book and the link picker resolve transcripts through `summary_card` —
  a sub-agent's result address is a link from this PR on.
- **Clone** re-ids the copied transcripts (`Chat::reid_children`, the archive's
  too) — two chats answering to one `chat://` prefix would refuse the link.
- **Decided on the way**: the widget does not refuse by itself — it has no
  locale at key time, and the orchestrator already answers through
  `ChatListError`; content mode cannot name transcripts until PR 5's `sub_id`
  reaches the index, so a transcript shows there only while no result is
  pending (the rule's membership test is already in place).
- **Tests**: 2488 green (+20): the widget's tree (order, both filter cases,
  content ids, keys on a transcript, the hotkey grid, the rendered row);
  the screen's read-only mode (the system bubble, the refused send with the
  text kept, every blocked chord and command with its note, the commands that
  still work, the status model, a transcript's address in the book); the
  orchestrator (the nested list snapshot, activation with `ChildView` and
  names and the refused send, the startup restore of a remembered transcript,
  rename, delete/clone refusals and a clone's re-id, copy, a requested title).
  No live run — UI and orchestration over a scripted engine; the engine path
  is PR 2's.

### Post-M9: sub-agent chats, PR 5 — transcripts in search and the cross-chat tools (done)

- **What**: PR 5 of the sub-agent track
  ([docs/research/subagent-chats.md](../research/subagent-chats.md) §3.9).
  A sub-agent transcript is now a conversation for every search surface:
  the chat list's content filter (`Ctrl+F`), the message-level results
  (`Ctrl+G`), `Enter` on a row in content mode, and the model's
  `chat_search`/`chat_read` pair (fork F4, taken). Spec §9.11, §11.2,
  §11.2.1; architecture §7, §8, §10.
- **The index** — `messages.sub_id` (nullable; `CACHE_SCHEMA` 1→2, a free
  bump: the file is wiped and rebuilt by the startup pass, ~350 ms on the
  real corpus). `indexed_messages(chat)` walks the file's records and emits
  each run's messages under the **parent's** `chat_id` with `sub_id =
  run.id`, so the per-file bookkeeping, the guarded re-index and
  `forget_chat` are untouched — the diff key `(message_id, text_hash)` holds
  because message ids are uuids on both levels. One spelling of the
  conversation id for every scoped query, `COALESCE(m.sub_id, m.chat_id)`
  (`SCOPE_ID`): `search_chats` returns the distinct set of it, the tools'
  `search_messages_in`/`count_matching_messages_in` filter on it, and
  `MessageHit::scope_id()` is the grouping key on the consumers.
  `matching_messages_in_chat` takes an `IndexScope` — `Chat(id)` is `chat_id
  = ? AND sub_id IS NULL`, `Transcript(id)` is `sub_id = ?` — so a parent's
  first match is among its *own* messages.
- **The list rule cost nothing**: `visible()` already did one membership test
  per row, and a transcript's id now stands for itself in the id set. A
  parent whose only matches are inside a transcript is *not* in the set, so
  it shows dimmed under F16 exactly as in title mode.
- **`Ctrl+G`** — `SearchGroup.parent`; `group_hits` buckets by `(chat_id,
  sub_id)`, orders chats by the list's sort, and emits a chat's group before
  its matched transcripts' in call order — the chat's group **even with no
  hits**, so the screen heads the transcripts with "0 matches" rather than
  orphaning them; navigation is over hits by construction, so the extra header
  is free. Child headers are `  └ title` with no spacer; `OpenHit` carries
  the transcript's id and `switch_to` opens it read-only on the message.
- **The tools** — `ChatRef.parent: Option<ParentRef {id, title}>`;
  `snapshot_other_chats` adds the transcripts of the profile's chats *and of
  the current chat* (they are not in the model's context — the reason the
  current chat itself is excluded does not apply). Results label a transcript
  as "«title» — a sub-agent transcript from the conversation „parent“"
  (`tool.chat_search.child`), and one guarded `load_transcript` serves both
  tools: the profile/hidden/current checks, then the chat's own messages or
  the run found in the parent's file — a run the file no longer holds (its
  exchange taken back) reads as unavailable.
- **Tests**: 2495 green (+7): the two-level contract in the cache (id set,
  scoped search and count, both `IndexScope`s; transcript rows diffed and
  dropped with the parent), the orchestrator's grouping and the id set
  (parent-then-child, the "0 matches" parent, the transcript standing for
  itself), `first_match_in_chat` per level, the screen's child header and
  `Enter`, the snapshot's transcripts (the current chat's in, a hidden
  parent's out) and the pair reaching a transcript through its parent.
  **Smoke — GO**: an index and UI change needs no live run of its own;
  `cross_chat_search_answers_from_another_chat_live` was re-run against
  Gemma 4 31B (`llama-server`, 192.168.1.20) — the pair still answers from
  another chat with the new scope (a transcript only adds rows to it).
- **Decided on the way**: the label wording avoids nesting the header's
  quotes («…«…»…»); the search screen uses the literal `└` the list uses
  rather than a new glyph (box-drawing is allowed in both glyph sets). Next:
  PR 6 — auto-title at landing and the status-bar chip.

### Post-M9: sub-agent chats, PR 6 — auto-title at landing, the run chip, the demo transcript (done)

- **What**: PR 6 of the sub-agent track
  ([docs/research/subagent-chats.md](../research/subagent-chats.md) §3.5,
  §3.10, §7 stage 6). Three small things the earlier stages left: a landed
  transcript names itself, the user sees the run while it is in flight, and
  the demo world has a transcript to show. Spec §9.3.2, §11.7; architecture
  §5, §6.
- **Auto-title at landing** — `handle_done` collects the runs that landed
  with the turn (any record's `subagent` with a substantive `final_reply`)
  and, after the chat borrow, calls `maybe_auto_title_run` for each: both
  `interface.auto_title` points fire there, since a transcript's question and
  reply arrive together (fork F8, resolved by construction); only `Off` is
  quiet, and `renamed_manually` (a migrated or hand-named run) is left alone.
  The title task itself is PR 4's `view()`-based one (`Ctrl+R` on a
  transcript), so there is one digest, one sampling recipe and one "manual
  wins" apply rule for chats and transcripts alike. Pinned by a test that
  runs a second delegation in an existing chat under each mode and checks
  the one title request on the engine is the transcript's digest and the
  parent keeps its title.
- **The run chip** — `AppEvent::SubagentProgress { generation_id, progress:
  Option<SubagentProgress { name, round, tool }> }`. The child loop carries
  a `persona` (`name`, else the initial title) and `report_progress` sends
  the event **around** its muted `RoundSink` at each round's start and each
  tool's entry — the one child event meant for the parent's screen; the
  parent sends the clearing `None` after the run, while its own turn goes
  on. `tool_round` counts the round before executing the calls, so a tool
  is reported in the round already counted and a stream opens the next —
  the test pins `1 · 1+tool · 2 · None` for the standard delegation. The
  screen words it (`ui.chat.bg.subagent[_tool]`, "sub-agent «name» · round
  N · tool") into `background_hint` between the background tasks and the
  retry chip, guarded by the generation id and cleared by `Finished`.
- **The demo transcript** — `demo::reviewer_run()`: a completed "Reviewer"
  run (fixed ids and times, titled as the automatic titling would) on the
  refactoring chat's answer, through `reviewer_record()`; `chat_summaries()`
  nests its `ChildSummary` under that row and `filler_chats()` seeds the
  record, with a test that the capture and the seeded world agree. The list
  frame fills `PANEL_H` exactly, so one filler row went ("Wool or
  synthetic") rather than pushing the last row off — 21 dialogs and a
  transcript. Dumps and the four chat-list images regenerated; JetBrains
  Mono came from the site's woff2 via `fontTools` (the JBR bundle on this
  machine lacks Regular/Bold), and the old PNGs were re-rendered
  byte-identical first, as lessons §1 asks. `tools/screenshots.py`'s path
  guard refuses a dump directory outside the repo — the check went through
  `target/`.
- **Tests**: 2498 green (+3): the chip sequence in the standard delegation
  test, the landing title under both modes, the screen's chip (worded,
  composed, cleared by the run's end and the turn's end, stale-dropped), the
  demo agreement test; the drift gate re-pinned on the new dumps.
  **Smoke — GO**: `subagent_with_tools_e2e_live` against Gemma 4 31B
  (`llama-server`, 192.168.1.20) — the nested run with `fs_read` completed
  (4 messages, 121 tokens); the chip and the title task are exercised by the
  scripted engine, which is where their sequence can be asserted.
- **Decided on the way**: `/copy`/`/export` of a parent do not carry its
  transcripts — stated in spec §11.7 rather than solved (research §3.6: the
  v1 import document has no place for tool calls). Next: PR 7
  `feat/subagent-live` — the in-flight side table and the child's stream in
  its own feed, once a live run argues for it.

### Post-M9: sub-agent chats, PR 7 — the transcript while it runs (done)

- **What**: stage 2 of the sub-agent track, by the plan
  [docs/history/subagent-live.md](../history/subagent-live.md) (forks F1–F8 by
  recommendation, user's go 2026-08-23). A running sub-agent is a row of the
  list, opens read-only and grows by rounds, and the parent ↔ transcript
  switch does not cancel the turn. Spec §9.3.2, §11.2, §11.3; architecture
  §5, §10.
- **The channel** — `done_tx` carries `GenMessage::{Progress, Done}`; the
  progress is `TurnProgress::{RoundFiled, ChildStarted, ChildRoundFiled,
  ChildEnded}`, sent from `file_round` (by `depth`) and around the child in
  `run_subagent`, whose run id is now minted **before** the run so the list
  and the landed record agree. One channel was the whole point (F1): every
  progress message precedes the result, so the mirror can be dropped at
  landing with no race and no sequence numbers.
- **The mirror** — `InflightTurn { generation, chat, rounds, child,
  parent_needs_refresh }` on the orchestrator (F2). `view()`/`with_child_mut`
  gained a third arm over it, which is what made the rest small: opening,
  naming, copying, exporting and renaming a running transcript are the PR 4
  paths unchanged; `first_match_in_chat` answers `None` (nothing indexed).
  `emit_chat_list` appends the card with `ChildSummary.running` (additive);
  the widget draws *running* where an outcome goes.
- **Growth by rounds (F3)** — `TranscriptGrew { id, messages }` for the open
  transcript only; the screen keeps a `transcript: Vec<Message>` copy while a
  transcript is open and rebuilds the feed from all of it, so rounds stitch
  into one bubble exactly as a landed transcript renders, and the scroll
  follows the tail only if it already did.
- **The switch (F4, F6)** — `switch_within_turn`: the parent and its
  in-flight child, either direction, skip `request_cancel`; everything else
  cancels as before (`Ctrl+N` never did — it activates without switching,
  which the test had to respect). `ChatActivated.live_turn` carries the
  running generation: on the parent the screen resumes `generating`/
  `current_gen` and its feed is built from `chat.messages + inflight.rounds`;
  on the transcript only the chip's guard is set — `current_gen` stays
  `None`, because the parent's final-round chunks would otherwise land in the
  transcript's feed (found while designing the screen side; the plan's
  §3.4 already had the rule, the field split is the implementation of it).
  A return to the parent marks `parent_needs_refresh`, and `handle_done`
  re-activates it whole — the text of the round in progress is the one thing
  the mirror cannot hold.
- **Rename while running (F5)** — edits the mirror; `handle_done` copies
  `title`/`renamed_manually` onto the landed run before the titling step.
- **Tests**: 2504 green (+6): the running row with its count, opening with
  the rounds so far and `live_turn`, the parent ↔ child switch leaving the
  engine waiting, `Esc` landing the cancelled run under the same id with the
  list un-marked and the parent refreshed; a switch to a third chat
  cancelling; the rename landing; the bare `handle_progress` (growth only
  for the open transcript, a foreign generation dropped); the screen growing
  by rounds with the chip kept and the parent's chunk refused, and resuming
  the generation on the chat. **Smoke — GO**: `subagent_with_tools_e2e_live`
  against Gemma 4 31B (`llama-server`, 192.168.1.20) — the progress path is on
  every delegation now; the nested `fs_read` run completed (4 messages).
- **Traps met**: `gen` is a reserved identifier in edition 2024
  (`generation`); a `wait_for` on an event the orchestrator emits *before*
  the one already consumed hangs the test forever (lessons §1, again) — the
  landed `ChatList` precedes the parent's refresh, so the test reads them in
  that order. Next: the running card (`ToolCallStarted`, plan §3.7) as its
  own PR, then the plan moves to `docs/history/`.
### Post-M9: sub-agent chats — the sub-agent's text streams into its transcript (done)

- **What**: the first of the track's open items
  ([docs/history/subagent-live.md](../history/subagent-live.md) §8, forks by
  recommendation, user's go 2026-08-23). A running transcript no longer
  grows only by rounds: the sub-agent's text, thoughts and tool cards stream
  into it, and a transcript opened mid-round starts with what has already
  streamed. Spec §9.3.2, §11.3; architecture §5, §10.
- **Why it became cheap**: stage 2's fork F3 chose rounds because token
  streaming seemed to need a second stream on the screen and lost the partial
  text on every switch. Treating the child's stream as **progress** — the
  child's `RoundSink` routes `Chunk`/`Thoughts`/`ToolCallStarted`/`ToolCall`/
  `AssistantContinue`/`AssistantRewrite`/`TokenUsage` to `done_tx` as
  `TurnProgress::Child*` instead of muting — answers both: one channel with
  `ChildRoundFiled` means a round's chunks always precede its filing (the
  feed rebuild cannot erase the next round's bubble), and the orchestrator
  holding `child_partial` means a late opening is seeded whole.
- **The stream id**: `InflightTurn.child_stream`, minted at `ChildStarted`.
  The screen's guard is the existing one — events under the run's stream
  land only where `current_gen` is that stream, i.e. the transcript view; the
  parent's screen drops them with no new rule. `ChatActivated.live_turn`
  became `LiveTurn { turn, stream, partial }`: the chip stays keyed on
  `turn`, the feed accepts `stream`. On a transcript `set_live_turn` now
  does `begin_generation(stream)` + the partial as chunks; the `current_gen =
  None` rule of PR 7 is gone with its reason.
- **The transcript view is generating**: its own token counter
  (`ChildTokens` = the re-based counter minus the run's `token_base`), the
  running card of #363 for the run's own calls, `Finished` from `ChildEnded`
  (outcome → reason: completed/round-limit → `Stop`, cancelled/timed-out →
  `Cancelled`, failed → `Error`) closing the bubble and clearing the chip.
- **Tests**: 2506 green (+1, two reworked): the bare `handle_progress`
  (a stream id of its own; chunks kept and not forwarded while the parent is
  open; the activation seeding the partial; forwarding under the stream id
  once open; the reset on a filed round; `Finished` on the run's end), the
  loop test now asserting the hanging round's `thinking…` in the opened
  transcript's `live_turn.partial`, the screen test reworked for a seeded,
  generating transcript that accepts its own chunks and refuses the
  parent's. **Smoke — GO**: `subagent_with_tools_e2e_live` on Qwen 3.6 27B
  (`llama-server`, 192.168.1.20), 279 tokens, 28 s.
- **Next**: the parent's own in-progress text the same way (`RoundText`),
  retiring `parent_needs_refresh` (plan §8, last bullet).
### Post-M9: sub-agent chats — the parent's round in progress is mirrored too (done)

- **What**: the last bullet of
  [docs/history/subagent-live.md](../history/subagent-live.md) §8. Coming
  back to the running turn's chat now shows the round in progress — its
  text, thoughts and tool cards, running or answered — and the stream resumes
  into that bubble; the one whole re-activation at landing that stage 2
  used to close this gap (§3.5, fork F4) is gone with `parent_needs_refresh`.
  Spec §11.2; architecture §5, §10.
- **How**: the child's `Child*` progress variants became one `StreamStep`
  shape used by both loops — `TurnProgress::OwnStep` from the live loop
  (whose sink sends the screen its events exactly as before and the
  orchestrator a mirror), `ChildStep` from the sub-agent's. One `apply_step`
  feeds `InflightTurn.partial` or `child_partial`; `LivePartial` gained
  `tools: Vec<LiveTool { call_id, name, arguments, result: Option<(String,
  usize)> }>` — a `ToolCall` completes the matching started entry — and a
  filed round or a rewrite resets it. `set_live_turn` seeds the bubble in
  both cases (thoughts, text, then the cards via `push_tool_call_started`
  / `push_tool_call`), so the return and the transcript opening are one
  path. `ChatActivated.live_turn` is boxed: the variant had pushed the enum
  past clippy's size bar.
- **Tests**: 2507 green (+1): the bare mirror of the parent's round (text,
  thoughts, an answered and a running call; nothing forwarded from here —
  the live loop talks to the screen itself; the seed on a return; the reset
  on a filed round and on a rewrite); the switch test now asserts the
  return carries the running `call_subagent` card and that **no**
  re-activation follows the landing; the screen test seeds a chat with a
  partial holding two cards and checks the running one completes in place.
  **Smoke — GO**: `subagent_with_tools_e2e_live` on Qwen 3.6 27B, 29 s.
- **Closed with it**: the last structural item of the sub-agent track. What
  remains open is product scope — the two-agent dialogue (research §3.14)
  and a parent's JSON export without its transcripts.


### Post-M9: command-only control — stage 3 (`/autotitle`, the profile texts, `/impersonation`) (done)

- **What and why**: a live pass in a JupyterLab terminal (2026-08-23) found the
  next layer of chord-only actions after stages 1–2: the model-written title
  lived only behind the list's `Ctrl+R` — which the user's browser spends on
  reloading the tab (the 2026-08-14 host matrix had scored that cell "passes";
  the matrix drifts, the class-closing answer stands) — the impersonation
  profiles were editable only behind the settings screen's `Ctrl+N`/`Ctrl+D`
  exactly as the assistant profiles had been before stage 2, and the three
  free-text fields (profile system message, greeting, persona text) had no
  route but the settings editors. Design and forks:
  [docs/history/commands-stage3.md](../history/commands-stage3.md); the same
  track fixed the command-residue defect (journal ui-input.md).
- **Key decisions** (all confirmed 2026-08-23): `/autotitle` over `/autoname`
  (the feature's name in spec §11.2 and the setting) and over a `/rename`
  subcommand (a reserved word inside a free-text title makes the literal
  title "auto" unreachable); **`/impersonation` over `/persona`** — the
  user's call, against the recommendation: the settings subsection's own word
  wins over brevity, the one-letter-group distance to `/impersonate` accepted
  (exact-word matching keeps them apart); the reserved word **`clear`** on
  the three text subcommands, symmetric with `/self clear`; `new` creates
  **unlinked** (settings `Ctrl+N` parity — several assistant profiles can
  share one persona), the note naming `/impersonation use <name>` as the next
  step.
- **The shape**: `/autotitle` is a registry row reaching the list's own
  `AppCommand::AutoRenameChat` (its `ChatListError` failures already fell
  back to a feed note when the list is closed — checked, not built);
  `/profile` grew `system`/`greeting` subcommands and a shared `TextEdit`
  (`Show`/`Clear`/`Set`) whose raw-remainder parsing keeps internal newlines
  (a system message is a document; names still collapse whitespace and shed
  quotes); `features/impersonation_command.rs` is the `/profile` module's
  shape over the personas, committing through the settings screen's own
  paths — `UpdateProfile` for the link and the profile texts, `UpdateConfig`
  over a working copy of the settings snapshot for the persona list. The
  name resolver generalized to `resolve_named` over `(id, name)` pairs
  rather than being copied (the budget-for-the-seam rule). Sub-decision D1,
  recorded before implementing: the outcome notes are the command's own,
  worded as fact — the settings paths have no success event, the settings
  screen deliberately stays silent (the field itself is its answer), and a
  failed save still reports through the orchestrator's error path.
- **Doors closed**: bare text subcommands prefill the current value
  (the `/rename` pattern) but an *empty* value answers with the syntax that
  sets one — an empty prefill teaches nothing; `/impersonation system` with
  no persona linked names both routes that give the profile one; the notes
  state each edit's scope (profile texts — new conversations, the chat's
  snapshot being creation-time; persona text — the next `Ctrl+U`, resolved
  live); `/profile new`'s success note now names the typed route to a
  persona, not only the settings screen.

**Tests** (suite 2526 → 2543): parser tables for both modules
(case/padding, newline-preserving text, `clear` only as the whole argument,
near-words as prose, per-locale error gates), the stage-3 screen module
(`staffed` fixture: intent equality, the prefill round trip, the
always-confirm delete through `Enter`/`Esc`, the linked-persona marker in
`list`, every precondition with its note), plus the registry-driven suites
absorbing `/autotitle` for free. Sonar's offline duplication heuristic run
before the PR: clean.

**A live model run is not required** (AGENTS.md §3) — UI routing over
existing orchestrator surface (`AutoRenameChat`, `UpdateProfile`,
`UpdateConfig`, all already covered). The motivating host behaviour is the
JupyterLab report this stage answers.
### Post-M9: the chat list scrolls symmetrically (done)

**The report**: in the chat list `↓` behaved as expected — the selection walks
down the visible rows and the list follows only at the bottom edge — while `↑`
scrolled the list from the very first press, the selected chat glued to the
bottom row instead of climbing to the top one.

**The cause** was one line in `ChatListState::render`: the `ListState` handed to
`render_stateful_widget` was built fresh every frame (`ListState::default()`).
ratatui's list moves the offset only as far as it must to bring the selection
into view — from an offset of `0` that means "scroll down until the selection is
the *last* visible row". Rebuilt per frame, that rule ran from scratch on every
draw, so the offset was always derived from the selection rather than remembered:
downward it coincided with what a scrolling list should do, upward it produced a
scroll per press. The asymmetry was in the state's lifetime, not in the key
handling — `KeyCode::Up` was always a plain `selected -= 1`.

**The fix**: the offset is a field of the widget (`ChatListState::offset`),
seeded into the per-frame `ListState` (`with_offset`) and read back after the
draw — the value the scrollbar was already reading. Clamped to
`visible.len() - list_area.height` before the draw: a filter change or a deletion
can shorten the list under an offset that has scrolled past its new tail, and the
clamp belongs where the height is known. Everything that resets the selection
(a new query, `Home`, a restored content query) needs no offset bookkeeping —
ratatui pulls the window back up when the selection is above it.

**Doors closed**: the same per-frame-`ListState` shape lives in the settings
screen, the profile popup, the link picker and the message-search screen; those
lists are short enough that no one has hit it, and mixing four unreported
behaviour changes into a reported one-line fix is how a fix stops being
reviewable. Left as a note here.

**Tests** (suite 2543 → 2544): a render-level regression in
`widgets::chat_list` — 30 chats with explicitly descending timestamps (so the
sort order is not at the mercy of the clock's resolution), `End`, then one `↑`
that must leave the rows where they are, then enough presses to reach the top
and scroll. It fails on the old `ListState::default()` line, which is the point.

**A live model run is not required** (AGENTS.md §3) — pure UI, no engine,
storage or tool surface is touched.
### Post-M9: the same scrolling rule for every other list (done)

The chat list's fix noted that the same per-frame `ListState` shape lived in
five other places and left them alone, on the grounds that folding unreported
changes into a reported one-line fix stops it being reviewable. The user asked
for the sweep next, so this is that PR.

**The inventory first**, because "every list" is a claim that has to be checked
rather than assumed. Eight sites built a `ListState` inside `render`: the chat
list (fixed in the previous PR), the settings screen's section menu, field pane,
field search overlay (`/`) and Choice popup, the profile picker, the reference
picker (`Ctrl+L`) and the spellcheck suggestions. Three more screens scroll lists and
were already **correct** — the message search, the `F3` self-model viewer and
the changes screen each keep their own `scroll` and adjust it minimally
(`adjust_scroll`/`keep_visible`); they are the precedent the rest should have
followed. The emoji picker draws a fixed grid with no scroll model at all.

**One implementation instead of eight.** The rule moved into
`shared::ui::ListScroll` — a stored offset, seeded with `with_offset`, read back
after the draw, and clamped to `len - view_h` before it (the caller passes the
rows the list actually draws into, since a bordered block eats two of them).
It is now the **only** place in the codebase that builds a `ListState`, which is
the point: eight copies of four subtle lines is how one defect became eight. The
chat list was moved onto it in the same PR — keeping its inline version would
have meant two implementations of the rule on the day the helper was written.

**What the sweep turned up.** The settings field pane inserts **group headers**
as rows the selection skips over. Keeping the offset alone left the pane one row
short of its top: with the selection on the first field (row 1) ratatui has no
reason to show row 0, so the group's header stayed scrolled out and the
scrollbar read "not at the top" while the user was. `scroll_padding(1)` on that
list is the fix — one row of context, which at the top edge means the header,
and mid-list means you can see what group you are entering.

**Doors closed**: the popups whose height is their item count (Choice,
suggestions, both pickers) are capped at the screen's height, so they *do*
scroll on a short terminal or a long list — they got the same treatment rather
than an argument about whether anyone would notice.

**Tests** (suite 2544 → 2548): the rule itself is tested where it now lives —
`ListScroll` walking a selection through a window in both directions, and a list
shrinking under a stored offset (the clamp). On top of that, one behavioural
test per shape that a caller can get wrong: the settings field pane (through
`handle_key` + `render`, so the group headers and the padding are in it) and the
reference picker (a popup capped at the screen height).

**A live model run is not required** (AGENTS.md §3) — pure UI, no engine,
storage or tool surface is touched.

### Post-M9: sub-agent chats — the child view arrives before the feed is built (done)

- **What**: a transcript opened without its persona bubble, and the chat opened
  right after one inherited it. The `AppEvent::ChatActivated` dispatch arm
  applied the event as `activate_chat` → `set_child_view`, while
  `activate_chat` is what **reads** the child view — to head the feed with the
  persona (spec §11.3) and to keep the message copy a running transcript's
  `TranscriptGrew` extends. So every feed was built with the *previous* chat's
  view: the persona missing where it belonged, present where it was not (the
  parent opened right after its transcript — the "unstable, every other time"
  symptom: it depended on what was open before), and a running transcript's
  first filed round rebuilding the feed from the new rounds alone, the
  pre-open messages gone. One reorder in the arm fixes all three.
- **Why 2554 tests were green**: the ordering contract was written twice — on
  `set_child_view` ("applied **before** `activate_chat` builds the feed") and
  on the `child` field ("set by `app` right before `activate_chat`") — and the
  screen tests obeyed it by hand, calling the pair in the documented order.
  The one real call site had it backwards from the day it was written (PR 4,
  #359), and no test drove that seam. The regression test goes through
  `apply_event` with the real event and asserts the **rendered frame** (the
  feed is private to the screen): the persona present on the transcript,
  absent on the chat opened next. Locale-free on purpose — it checks the
  persona text, not the role header, which is the screen tests' business.
- **No changelog entry**: the transcript feature is still in `[Unreleased]`,
  whose Added text already promises the correct behaviour; released paths
  never carry a child view, so nothing a 0.9.7 user sees changed.
- **Tests**: 2555 green (+1), 107 `#[ignore]`. A live model run is not
  required (AGENTS.md §3) — pure UI wiring; no engine, storage or tool
  surface is touched. Lesson recorded (lessons §2).

### Post-M9: the chat list folds a chat's sub-agent transcripts (done)

- **What**: the transcript rows under a chat (spec §11.2) sit behind a
  per-chat fold now, **collapsed by default** — a long list of delegating
  chats was mostly transcripts. `Ctrl+O` in the list folds/unfolds the
  selected chat's (on a transcript row — its parent's, parking the selection
  on the parent first, so it never rests on a row the fold is hiding);
  `/subagents` typed in the chat is the same flip for the open conversation —
  bare a toggle like the key, `expand`/`collapse` setting the state outright —
  and it answers *which way and where* — the rows it moves live on another
  screen, so a silent toggle would read as a refusal (lessons §4). A folded
  chat shows a muted `▸ n` beside its count (without it, a chat with
  transcripts is indistinguishable from one without — the collapsed-pill
  lesson), the hint is advertised only on chats that have any and carries the
  direction it would take, and opening the list while a fold-hidden
  transcript is the active conversation selects its parent.
- **Key decisions**. *Storage*: `Chat.children_expanded`, additive with the
  `renamed_manually` serde shape (no key until someone expands; ADR 0006 F12,
  `CHAT_SCHEMA` stays 2), mirrored onto `ChatSummary` so the widget renders
  and toggles from the snapshot it already holds. *The route* is the rename's,
  not `SetFeedView`'s: addressed by id (`AppCommand::SetChildrenExpanded`),
  since the list toggles any row's chat — but with `SetFeedView`'s write rules
  (debounced save, `modified_at` untouched) and its parent resolution for a
  transcript's id; the handler re-emits the list so an open overlay redraws.
  The senders compute the **absolute** value from their snapshot, so the two
  routes cannot double-flip. *Search outranks the fold*: a query match (title
  or content) surfaces a transcript under a collapsed parent — the fold hides
  rows only under the blanket "everything matches" of an empty query or a
  content search with no answer yet; hiding a hit would make the search lie.
  *Import* stays view-state-free (an imported chat opens collapsed, the
  `feed_view` rule). *The demo fixture* pins `children_expanded: true` on the
  one gallery chat with a transcript, keeping the committed dumps
  byte-identical — the fold itself is pinned by widget tests instead.
- **Tests**: 2571 green (+15), 107 `#[ignore]`: the serde round trip on chat
  and summary; the widget's fold/marker/toggle/selection-fallback/hint and
  both search-override scopes; the orchestrator handler (persists by id
  without bumping `modified_at`, re-emits the list, resolves a transcript to
  its parent, no-op chatter-free); `/subagents` toggling with the note, the
  explicit words, from an open transcript (read-only leaves it be — folding
  changes no conversation), and answering when there is nothing to fold.
  Test-fixture debt paid down on the way: the four copies of the
  `ChatSummary` test literal (and two more same-shape in-test ones) collapsed
  into one `ChatSummary::fixture` — the offline duplication probe read 2.8%
  of changed lines inside duplicated windows before, 1.9% after, against
  Sonar's 3% new-code bar (lessons §2, eighth entry). A live model run is not
  required (AGENTS.md §3) — UI plus one additive stored field; no engine or
  tool surface is touched.

### Post-M9: help hotkeys by context — stage 1, per-screen sections (done)
- **The `F1` "Shortcuts" tab reorganized from one flat list into per-screen
  sections** (branch `feat/help-keys-sections`; design
  [docs/help-hotkeys-context.md](../history/help-hotkeys-context.md), forks decided by
  the user 2026-08-24, all as recommended). The flat `HELP_KEYS` had grown
  three ways of expressing context at once: clauses inside descriptions
  ("in a chat: …; in the chat list: …" — `Ctrl+F`), the same chord listed
  twice in different groups unexplained (`Ctrl+O`, `Ctrl+G`), and meanings
  missing outright (`Ctrl+R`'s list meaning; every key of the settings,
  self-model, changes and search screens). Now: `HelpSection { title, context,
  rows, openers }` and `HELP_SECTIONS` (`screens/chat/popups.rs`) — Everywhere
  · Chat · Chat list · Settings · Self-model · Changes · Found messages — each
  header a localized `ui.help.sec.*` title that also names the route
  ("Settings (Ctrl+P)"), rendered as the title plus a `─` rule with one
  description column shared across the whole tab; the intra-section
  blank-line groups keep the opener mechanism, now per section.
- **Section rows were written against the actual key handlers**, inventoried
  match-arm by match-arm (an Explore pass over `widgets/chat_list.rs`,
  `screens/settings/apply.rs`, `self_model.rs`, `search.rs`, `changes.rs`) —
  which surfaced keys no help listed: the list's `Tab` (sort toggle),
  `Ctrl+D` (clone), `Del` (delete); the settings' `/` search, `Ctrl+Z/Y`
  settings-undo, `Del` field reset; the changes screen's `r`. The chat
  section also gained the missing `F2` row.
- **"You are here"** (fork F1): `HelpState.context` (`HelpContext`, stage 1
  always `Chat` — the only opener) marks the matching section's header with
  an accent `· you are here` suffix (`ui.help.here`). Stage 2 threads the
  real invoking screen, routes `F1` on every screen (today only the chat
  handles it) and anchors the tab's scroll to the opener's section (fork F2:
  the chat keeps its last-tab behavior).
- **Deliberate reversal, recorded**: the "groups carry no headers, so they
  need no locale keys" rule stays true for the *micro*-groups, but the seven
  section headers are new locale keys on purpose — cheaper than the context
  clause every dual-meaning description was paying per locale.
- **The `/` key exposed a latent assumption**: `key_lines` styled any label
  starting with `/` as a command. The settings' search key *is* `/`, so the
  command styling became a per-table flag (`table_lines(_, command_labels)`)
  — the hotkeys tab is keycaps throughout, the "Commands" tab unchanged.
- **Gates extended, not weakened**: width-budget and one-column tests run
  over `hotkeys_lines` (headers included; header rows are excluded from the
  column check by their `─` rule); `group_openers_open_real_rows` pins
  openers per section and looks each opener up inside its own section's
  slice, delimited by the (unique) headers — key labels repeat across
  sections now ("Esc", "Enter"), which is exactly what made the old
  whole-tab lookup wrong. New test: the marker appears exactly once and
  follows the context (`the_invoking_screens_section_is_marked`, pinned for
  two contexts).
- i18n: ~50 new keys (`ui.help.sec.*`, `ui.help.here`, the section rows),
  two crammed keys deleted (`ui.help.search_content`,
  `ui.help.search_messages`), `esc`/`subagents_fold` reworded shorter;
  `ru`+`en` at parity, the i18n gates green. Docs: spec §11.7, README,
  AGENTS.md §3 (`HELP_SECTIONS`), CHANGELOG. **2572 unit tests green.** A
  live run is not required (AGENTS.md §3) — a help tab; no engine, storage
  or tool surface is touched.

### Post-M9: help hotkeys by context — stage 2, `F1` everywhere as a runtime overlay (done)
- **The dialog left the chat screen** (branch `feat/help-f1-everywhere`,
  stage 2 of [docs/help-hotkeys-context.md](../history/help-hotkeys-context.md);
  three commits — a verbatim move, the lift, the table distribution — so the
  mechanical and the behavioral steps review separately). The renderer, tabs,
  sizing and `HelpState` (now with `handle_key` → `HelpKeyOutcome` and
  `open_at`) live in `widgets/help_dialog.rs`; the runtime owns a
  `HelpOverlay { open, last_tab }` beside `active` and draws the dialog over
  whatever screen is active in the same `terminal.draw` closure, theme and
  locale taken from the chat screen — the base that always exists and
  receives every settings event.
- **`F1` is routed in `handle_key_event`, above every screen** — no screen
  keeps an `F1` arm (the inventory had shown none handled it at all; there
  is no global key handler, each screen even quits by itself). It therefore
  works inside sub-modes too (a rename field, an armed confirmation) —
  deliberate: the dialog is read-only and `Esc` returns the sub-mode exactly
  as it was. While the dialog is open, keys go to `HelpState::handle_key`,
  and pastes/mouse events are swallowed (the rule the chat used to apply).
  The chat's `?` and `/help` became `ChatIntent::OpenHelp`, intercepted at
  the same level; `dispatch` keeps a documented no-op arm so the exhaustive
  match still asks about new intents.
- **Fork F2 landed as designed**: a non-chat opener forces the "Shortcuts"
  tab with `pending_anchor`; the anchor — the marked section header's row —
  is computed by `hotkeys_tab` at render time (wrapping above the section
  depends on width) and consumed once, so the user's scrolling sticks. The
  chat restores the remembered tab at the top. The `F1 / ?` row moved to the
  "Everywhere" section.
- **The section tables moved next to the handlers they document** (design
  §6): `HELP_SECTION` statics in `screens/chat/mod.rs`,
  `widgets/chat_list.rs`, `screens/settings/mod.rs`, `self_model.rs`,
  `changes.rs`, `search.rs`; the widget keeps the type, the "Everywhere"
  rows and the renderer (now taking the composed list as a parameter); the
  app composes `HELP_SECTIONS` — the one layer that knows every screen.
  `help_context` is an exhaustive match over `ActiveScreen` and the new
  `help_sections_cover_every_context` gate pins one section per context, so
  a new screen cannot ship without deciding — and filling — its section.
- **Repaint discipline**: the overlay's open/close toggle counts as a screen
  switch for `prime_full_redraw` — the dialog's keycaps and arrows are the
  wide-glyph risk group that leaves artifacts under a cell diff.
- **Tests follow their subjects** (net +7, **2579 green**): dialog
  navigation, clamp/scrollbar, logo, tab strip, legal tabs, disclaimer
  indent, distinct content and the commands-tab gates live in
  `widgets::help_dialog::tests` (rendered directly — no `ChatScreen`);
  overlay routing (`F1` per screen, tab memory, quit punch-through, inert
  paste, `?`→overlay) plus the whole-tab gates over the composed sections
  (width per locale, one column, opener structure per section, the marker,
  the anchored open) live in `app::runtime::tests`, joined by the coverage
  gate; the chat keeps the `?`-intent test and swapped its modal-routing
  test's subject to the in-feed search (the emoji picker closes on foreign
  keys — found by the first swap attempt going red). A live run is not
  required (AGENTS.md §3) — pure UI; no engine, storage or tool surface is
  touched.

### Post-M9: the "Globally" section, and `?` dropped as a help key (done)

- **Why**: two reports from the same look at the shortcuts tab. The first
  section's header read "Everywhere", and the word for keys that work anywhere
  in a program is *globally* — renamed to "Globally" (and its `ru` pair), key
  `ui.help.sec.everywhere` → `ui.help.sec.global` and the static
  `help_dialog::EVERYWHERE` → `GLOBAL` with it (an internal identifier that
  disagrees with the string it holds is exactly the drift the per-screen tables
  were placed next to their handlers to avoid).
- **The second report was the real one**: the section advertised `F1 / ?`, and
  `?` did nothing on the chat list, in settings, or in any input box with text
  in it — it typed a question mark. **The inventory**: `?` was registered in
  exactly one place, `screens/chat/input.rs`'s `handle_plain_key`, guarded by
  `self.input.is_empty()`, plus a close arm in `HelpState::handle_key`. So it
  worked on one screen out of six, and there only on an empty box — which is
  why the user's own attempts (a chat with a draft, the list's filter, the
  settings' search) all produced a character. That is a stricter condition than
  a hotkey table can express, and it predates the dialog being a runtime
  overlay: `?` was the chat's alias for `F1` back when the chat *was* the only
  screen that could open the help (stage 2 above lifted `F1` out; `?` stayed
  behind by inertia).
- **Decision — remove it, don't extend it.** Making `?` global would mean
  claiming a printable character on every screen with a text field (the list's
  filter, the settings' search, the rename box, the chat itself), each with its
  own "only when empty" caveat — a key that means "help" or "?" depending on
  state the user cannot see. `F1` already opens the dialog from anywhere and
  `/help` is the typed route (command-only control), so nothing is lost. Gone
  from the input handler, from the dialog's close arm (a stray `?` under the
  open dialog is now inert, like any other character) and from the row, which
  reads `F1`.
- **Tests** (2579 green, count unchanged — every touched test was replaced in
  place): the chat's `question_mark_reports_open_help_only_when_input_empty`
  became `question_mark_is_typed_never_a_help_key` (empty box included); the
  typed/chord equivalence table lost its `/help` ↔ `?` pair and now checks
  `/help` against the intent directly, since that route has no chord left to
  compare with; `app::runtime`'s `the_chats_question_mark_opens_the_overlay`
  became `the_chats_typed_help_opens_the_overlay` (`?` opens nothing, `/help`
  does, with the chat context); the dialog's navigation test pins `?` among the
  keys that are consumed and change nothing. A live run is not required
  (AGENTS.md §3) — pure UI.
### Post-M9: the value column holds against an MCP tool's name (done)
- **Symptom** (user report, with a screenshot of the "Profiles" section): in the
  "Plugins (MCP)" group the `[x]` of `mcp__fs__list_directory_with_sizes` and
  `mcp__fs__list_allowed_directories` stood a step to the right of every other
  toggle's — the group's checkboxes were ragged where the whole section is
  otherwise one vertical.
- **Cause** — the safety net from "settings — a shared value column per section"
  meeting data it was not written for. `section_label_col` took the longest
  label **among those ≤ `LABEL_CAP` (28)** and a longer one kept its full width,
  its value sitting locally right after it. That was sound while every label was
  a translated string the app itself names, and the gate
  `all_labels_fit_alignment_cap` keeps it so. An MCP tool's label is neither: it
  is `mcp__<server>__<tool>`, composed from a server id and a remote tool name,
  and `mcp_tool_id` allows it up to 64 characters — six of the fs server's
  fourteen tools cross the cap. The "no such labels exist" premise held only
  until a server with long tool names was attached.
- **Fix** (`screens/settings/{helpers,render}.rs`): the cap changed meaning from
  "a label this long is excluded from the vote" to "the column stops here" —
  `section_label_col` is now the longest label **clamped** to
  \[`MIN_LABEL_COL`, `LABEL_CAP`\], and `render_field_line` clips the label to
  that column with `…` (`truncate_to_width`, the same helper values use). The
  column therefore holds unconditionally, whatever a server names its tools.
  `build_field_items` no longer computes `value_w` off "the real end of the
  label" — there is no local overflow left for it to correct.
- **Why not the alternatives.** Raising the cap does not close it (the id may be
  64 wide, and the column would eat the pane); dropping the `mcp__<server>__`
  prefix from the label would make two servers' identically named tools
  indistinguishable in the list. Clipping loses the least: the row already
  carries the bare tool name as its inline hint (`read_multiple_files`), the
  server's own description is in the panel below, and the id is what the model
  sees, not what the user types.
- **Blast radius** — only sections that actually contain an over-cap label move:
  "Profiles" with an MCP server attached (column 27 → 28), and an MCP server's
  env-var rows, whose labels are user data as well. Everywhere else the longest
  label is already ≤ cap, so the clamp returns exactly what the old max did.
- **Tests** (2616 green, +2): `overlong_label_keeps_the_value_column` — four fs
  tool ids through `render_field_line`, the toggles at one cell, the clipped
  ones ending in `…`; `mcp_tool_toggles_share_the_column` — the same at the
  screen level (an `McpSnapshot` of those tools, the selection walked down into
  the group, every `[x]`/`[ ]` in the rendered buffer at one **cell** — a byte
  offset is meaningless with the Cyrillic section menu to the left). Both fail
  on the pre-fix code (columns 31 vs 37). `section_label_col_has_floor_cap_and_
  skips_subsection` now expects the outlier to raise the column to the cap.
  A live run is not required (AGENTS.md §3) — pure UI. Docs: spec §11.6.

### Post-M9: the confirmation toggle is named by what it does (done)

- **The setting was named after its keys.** "Confirm Ctrl+R / Ctrl+E" told the
  user which chords it intercepts, not which operations it guards, and the
  description repeated both chords in parentheses. A label built out of key
  names goes stale the moment a key is rebound or renamed — the help overlay
  (`F1`) and README already own the key tables, and a settings row does not need
  to be a third copy of them.
- **Now** (`locales/{en,ru}.json`, `ui.settings.field.confirm_keys` +
  `ui.settings.desc.confirm_keys`): "Confirm regenerate / delete" (27 columns)
  and the ru "confirm regeneration/deletion" (35). The description names the
  operations in full — regenerating the last reply, deleting the last exchange —
  says both discard what is already written, and drops the chords entirely
  rather than keeping them as a hint that would rot the same way. No code
  changed: the row is the same `FieldId::IConfirmKeys` toggle over
  `interface.confirm_destructive_keys` (spec §11.6, §11.3).
- **`LABEL_CAP` 28 → 35** (`screens/settings/helpers.rs`). The ru label is the
  widest static label in the app and did not fit; clipped, it lost exactly the
  half the rename adds, and the gate `all_labels_fit_alignment_cap` failed. The
  cap is the ceiling of the value column, so this is a layout change, not a text
  one: it was set by the widest label of the day, and the widest label grew.
  **User's decision (2026-08-27)**, taken over shortening the label and against
  a stated cost — the "Interface" section's values now start 7 columns further
  right.
- **This does not reverse "the value column holds against an MCP tool's name".**
  That entry's clamp is untouched: `section_label_col` still clamps to
  \[`MIN_LABEL_COL`, `LABEL_CAP`\] and `render_field_line` still clips the label
  to the column, so an `mcp__<server>__<tool>` id (up to 64) cannot bend the
  vertical — it now stops at 35 instead of 28. What that entry rejected was
  raising the cap *as the fix for a 64-wide dynamic id*; raising it to fit one
  static label the app itself names is the case the constant is for.
- **Measured** — the "Interface" section rendered at 80×30: the full label fits,
  the section's toggles and choices stay on one vertical, and the values narrow
  from 18 columns to 11, where a long Choice truncates in the row with `…` and
  stays whole in the hint panel below.
- **Tests** (2616 green, ±0): `all_labels_fit_alignment_cap` and
  `section_label_col_has_floor_cap_and_skips_subsection` cover the new cap as
  they did the old one. `value_column_is_shared_across_groups` needed its
  fixture widened — its four `mcp__fs__*` ids top out at 34 columns, under the
  new cap, so the test would have asserted the clamp while never reaching it;
  the server id is now `mcp__filesystem__` (up to 42). A live run is not
  required (AGENTS.md §3) — pure UI.

### Post-M9: the build date on the "About" tab (done)
- **What.** `F1` → "About" gained a **"Build date"** row directly under
  "Version" (`ui.about.build`, en/ru; `widgets/help_dialog.rs::about_lines`):
  the day the running binary was built, `YYYY-MM-DD` in UTC. Requested by the
  user. The version number cannot separate two builds of `0.9.7`, and a bug
  report needs to name the copy that actually ran — the same reason the
  neighbouring "Platform" row exists.
- **Where the value comes from.** `build.rs::embed_build_stamp` prints
  `cargo:rustc-env=MINDFORK_BUILD_EPOCH=<unix seconds>`;
  `shared/credits.rs::build_date` parses it and formats it with `chrono`.
  Seconds in, formatting at the point of display: `chrono` is already a runtime
  dependency, so this costs **no build dependency** and no second date
  implementation. `SOURCE_DATE_EPOCH` overrides the clock when it is set — the
  cross-distribution convention for reproducible builds (Debian, Nix,
  openSUSE), where a wall clock baked into the binary is precisely what breaks
  a byte-identical rebuild — with `rerun-if-env-changed` so cargo notices the
  variable appearing.
- **The row is release-only, and that is the design decision, not a shortcut.**
  A build script re-runs only when one of its declared `rerun-if-changed` paths
  moves; ours are `dictionaries/`, `assets/` and `syntaxes/`. Editing `src/`
  rebuilds the binary and does **not** re-run the script, so a development
  binary would carry the date of whenever one of those directories last changed
  and keep showing it for weeks. `build_date()` therefore returns `Option` and
  is `None` under `debug_assertions`; in a debug build the tab simply has no
  such row. A missing row says nothing, a wrong date says something false —
  the same standard the `Theme::Auto` fix was held to. The user had offered
  "release builds only" as the fallback in the request; it turned out to be the
  honest answer rather than the cheap one.
- **Rejected: forcing the script to re-run on every build.** It would rebuild
  the syntect dump (~130 ms, plus the write) on every `cargo check`, `test` and
  `clippy` — paid on every cycle to keep fresh a row nobody reads in a debug
  build.
- **Rejected: adding `rerun-if-changed=src`.** Accurate after a source edit and
  silently stale after a `Cargo.toml`/dependency change that relinks the binary
  without touching `src/`. *Partial* accuracy is the worse failure here: the row
  looks equally confident in both cases, so the reader has no way to tell which
  one they are looking at.
- **Rejected: the executable's mtime** (`current_exe()` → metadata), which would
  be accurate in both profiles and needs no build script at all. It is a
  filesystem fact rather than a build fact: a copy, a backup restore, an
  unpacker that does not preserve timestamps or any tool that rewrites the file
  silently turns "built on" into "touched on". Same confident-looking row, now
  reporting something else entirely.
- **Tests** (2617 green, +1): `credits::the_build_stamp_is_a_valid_iso_date`
  goes through the raw `MINDFORK_BUILD_EPOCH` rather than `build_date()`,
  because that function is deliberately `None` in the only profile the suite
  runs in — testing it alone would have left the parse/format path unexercised
  until a **release** build failed far from here — and it pins the profile rule
  itself (`build_date().is_some() == !cfg!(debug_assertions)`).
  `about_rows_anchor_right_with_leaders` now counts fact rows as
  `7 + usize::from(build_date().is_some())` instead of the literal `7`, so the
  leader-table geometry is asserted in both profiles and the release run checks
  the extra row instead of skipping it; the tab's content test asserts the date
  is on screen whenever there is one.
- **Measured** — `cargo test --release --bin mindfork`: 2617 green, which is
  where the release-only assertions actually fire — the eighth fact row is
  present, anchored in the same value column as the other seven at both bounds
  of the dialog's width range and in `ru`, and the tab's text carries the
  stamp's date. A live run is not required (AGENTS.md §3) — pure UI.

### Post-M9: the chat list counts messages as the conversation reads (done)
- **The row's `N msg` now counts the feed's bubbles, not storage rows**
  (spec §11.2). `ChatSummary`/`ChildSummary` used `messages.len()`, and an
  agentic loop stores every round as its own assistant message plus a `Tool`
  message per result — so a chat holding one question and one tool-assisted
  answer said "34 msg" while opening it showed two messages. The counter now
  reports what the user perceives: user messages and assistant replies, with
  tool/system rows and a loop's extra rounds folded into the reply they
  belong to.
- **One rule, one place**: `entities::chat::visible_message_count(&[Message])`
  — tool/system messages draw no bubble of their own; a run of consecutive
  assistant messages is one bubble unless a round opts out via
  `Message::new_bubble` (`send_followup_message`, spec §9.3). Both cards go
  through it — `Chat::summary()` and `ChildSummary::of()` — so the chat rows,
  the nested transcript rows and the running transcript's live mirror
  (`emit_chat_list`) all agree with no widget change.
- **The rule mirrors `FeedMessage::from_messages`, which FSD keeps out of
  reach** (`entities` cannot import `widgets`), and two statements of one rule
  is the drift lessons.md keeps recording — so
  `bubble_count_agrees_with_the_list_counter` (message_feed.rs) asserts
  `from_messages(prefix).len() == visible_message_count(prefix)` at **every
  prefix** of a history exercising every branch: a leading assistant greeting,
  a pure tool-call round, a tool row between rounds, a `new_bubble` followup,
  a system row, the next question. A drift in either implementation breaks
  some prefix.
- **The live-mirror expectation moved with the semantics**: the PR-7 fixture
  `running_delegation` waited for the running transcript at
  `message_count == 3` (instruction + round's reply + its tool result); the
  same filed round now reads `2`, and the predicate matches the same emission
  as before — the mirror appends a round as one batch, so no earlier list
  event can show `2` first. Spec §11.2's "a count that grows as its rounds
  file" became "updated as its rounds file": later rounds merge into the
  reply, so the number no longer climbs per round.
- **Tests** (2653 green, +3): the fold/`new_bubble`/system branches and both
  cards in `entities/chat.rs`, the every-prefix equality lock in
  `message_feed.rs`. A live run is not required (AGENTS.md §3) — a pure UI
  projection. The demo dumps are untouched: the gallery counts are fixture
  literals, and the demo transcript is user+assistant, which counts `2`
  either way.

### Post-M9: the self-model screen reads as two named halves (done)
- **The `F3` screen now says whose side each half is** (spec §17.7). It listed
  the self-description, the goals, then a bold "Interlocutor" header over
  traits/interests/relationship — so the top half was headed by nothing at all,
  and the one header that existed named a role rather than a person. The screen
  now opens with an **"Assistant"** header over the self-description and goals,
  and the second half is headed **"User"**; where the profile has set a
  `character_names` field (spec §5.1), the name replaces the label — the same
  rule the feed's role headers follow, so the two surfaces call the same two
  parties the same thing. The `ui.self_model.interlocutor` key retired in favour
  of `ui.self_model.assistant`/`ui.self_model.user`.
- **Blank rows between every section, and between the observations.** The values
  here are free prose the model writes: the demo self-description alone wraps
  over four rows, and with the fields stacked flush the description ran into the
  goals, `Traits:`/`Interests:`/`Relationship:` read as one paragraph, and the
  observations were an undifferentiated wall. A spacer row now separates the
  summary from the goals, each of the user half's three fields from the next, the
  header from what it heads, and each observation from the one below it.
  Decorations were already a row kind the cursor skips (`RowAction::Decoration`),
  so navigation, editing and the scroll rule needed no change — but row 0 is now
  a header, so `SelfModelScreen::new` moves the selection off it, the way
  `set_model` already did for a re-emitted snapshot.
- **The names come from the profile, not from the active chat.** The obvious
  source was `ChatScreen`'s `CharacterNames`, which is wrong for the one case
  where the two diverge: `Orchestrator::names_of` re-labels a subagent
  transcript's sides for the *run* (its `User` is the parent persona, its
  `Assistant` the subagent's name), while the self-model belongs to the
  **profile**. So the names ride the snapshot instead — `AppEvent::SelfModelView`
  became a struct variant `{ model, names }`, and the three emit sites collapsed
  into one `emit_self_model_view(pid)` that reads both from the same `pid`; a
  path that forgets the names is now unrepresentable.
- **The demo gallery lost one observation and kept its meaning.** The separators
  cost the 30-row `F3` capture a row each, so the third observation ("the 12 GB
  VRAM budget") scrolled out of the frame; the showcase needle `"VRAM"` was
  dropped and `"Assistant"`/`"User"` added in its place — every section of the
  screen is still pinned as visible, now including the two new headers. Dumps and
  images regenerated (`dump_demo_frames`, `tools/screenshots.py`).
- **Tests** (2679 green, +3): the section/blank-row geometry over a two-
  observation model, the profile names replacing both labels, and the selection
  landing below the first header on open and on a fresh snapshot. A live run is
  not required (AGENTS.md §3) — pure UI; verified against the real render through
  the screenshot pipeline, which draws the actual screen headlessly.

### Post-M9: the self-model screen lists its observations newest first (done)

**What.** `F3`'s observation list is now ordered by the observation's **own
date**, newest first, and the direction is a setting —
`interface.self_model_note_order` ("Interface" section, Appearance group:
*newest first* / *oldest first*). Default: newest first (spec §17.7, §11.6).

**Why it was a defect and not only a missing setting.** The screen rendered
`m.narrative.iter().rev()` under a comment reading "newest on top" — and it was
neither. The `F3` snapshot's narrative is built by
`Orchestrator::self_model_view_snapshot` from `self_notes_recent` →
`Db::note_list`, whose SQL is `ORDER BY n.updated_at DESC`: the list already
arrives **newest first**, so reversing it put the **oldest** observation under
the header. A person opening the screen to see what the assistant noticed last
had to scroll past the entire narrative to find it.

**Why nothing caught it.** The demo fixture `features::demo::self_model` lists
its six observations **oldest first** (a readable table in the source), so
`.rev()` produced a correct-looking screenshot, and the `F3` capture is the one
render of this screen the gates actually look at. The screen's own tests checked
the geometry (headers, blank rows, `Del`) and never the order. Lesson-shaped:
**a fixture ordered the opposite way to production hides an ordering bug behind
a green screenshot** — the new tests build the snapshot the way the orchestrator
actually does, newest first, and assert what comes out.

**Key decisions.**
- **Sort by `created_at`, not by the arrival order.** `created_at` is the date
  each row *shows*; the arrival order is `updated_at`-descending, which is what
  caps the list by recency (`self_model.max_narrative`) — keeping it as the
  display order would let a revised observation jump to the top under a date
  saying it belongs further down. One rule, `entities::self_model::order_narrative`
  (a stable sort, so same-instant observations keep their incoming order).
- **The truncation stays recency-based.** The cap is applied in the orchestrator
  before the screen sees the list; sorting afterwards means "oldest first" shows
  the *newest* `max_narrative` observations read forward, not the oldest ones.
- **Sorted in the screen, not in the orchestrator.** The order is a display rule
  and the screen is where it belongs — and, incidentally, this is what kept the
  `F3` screenshot byte-identical (the demo fixture's ascending order re-sorts to
  exactly what `.rev()` used to produce). Only the settings captures drifted, by
  the Interface section's field count, 14 → 15.
- **The setting is read when the screen opens** (`ChatScreen::self_model_note_order`
  → `SelfModelScreen::new`, the `clipboard_osc52` shape) rather than broadcast on
  every `Settings` event: `ActiveScreen` holds one overlay, so settings cannot be
  edited while `F3` stands, and every `F3` builds the screen anew.
- **Home in `InterfaceSettings`, row in the "Interface" section.** It changes
  nothing the model is told and nothing the narrative cap keeps, so it does not
  belong in `SelfModelSettings` (narrative sizes and injection volume) next to
  the `Sm*` rows in "Memory" — and every other `interface.*` field is edited in
  the section of the same name. The `/` field search finds it from either word.

**Tests** (2717 green, +6): `order_narrative` both ways over an input ordered
like neither, and its stability on equal timestamps; the screen's list newest
first by default and flipped by the setting, dates included; `Del` deleting the
row the cursor stands on in either direction (the ids ride the rows, so the
reversed list is not a mirror); the settings row present, described, and cycling
back to where it started; the default in `AppConfig`, and an `interface` section
written before the key existed keeping it. A live run is not required
(AGENTS.md §3) — pure UI; the real render is exercised through the screenshot
pipeline.

### Post-M9: one hint grid, and footers that name only the keys that work (done)

**What.** Every screen's hotkey footer is now drawn by one renderer,
`shared::ui::render_hint_grid` — **right-aligned**, columns lined up vertically,
an incomplete bottom row landing under the columns above — and every footer is
built from the state its own key handler reads, so a key that would do nothing
on the current row is not advertised. Plan and forks:
[docs/history/status-hints-unified.md](../history/status-hints-unified.md).

**Why.** Two defects with one cause: the screens were never part of the chat
bar's two recent reworks (the corner block, PR #423; the live `Esc` label,
PR #424), so they had both a different grid and a static hint list.

- **Two grids.** `status_bar::hotkey_lines` (right-aligned, no `danger` flag)
  drew the settings footer; `Palette::hotkey_grid` (**left**-aligned, `danger`
  flag) drew the chat list's, search's, the self-model screen's and the changes
  screen's. Both computed the same cell widths and both picked "the most columns
  that fit"; they differed only in where the block landed. Side by side in the
  committed demo dumps, the chat bar and settings ended flush right while the
  chat list and `F3` began flush left and frayed at the right edge.
- **Hints that named keys which do nothing.** The rule was already in the code
  and in spec §11.2 — *an advertised key that is a no-op is worse than a missing
  hint* — and exactly one surface followed it (the chat list drops `Del` and
  `Ctrl+D` on a transcript, `Ctrl+G` outside content mode, `Ctrl+O` on a chat
  with no transcripts). Reading each key handler against its footer turned up
  eight more places. The reported one: standing on an observation on the `F3`
  screen, the footer read "Enter edit" while `begin_edit` has returned `None`
  there since the screen was written — *insights aren't edited, only deleted*.

**What each footer learned.**

| Screen | Was | Is |
|---|---|---|
| self-model | `Enter edit · Space goal status · Del delete` on every row | `Enter` on the six editable rows (worded *add a goal* on the add row), `Space` on a goal, `Del` on a goal or an observation |
| changes | `↑↓ file` in both panes, `R` on every file | `↑↓ file` in the file pane, `↑↓ PgUp/Dn scroll the diff` in the diff pane; `R` only where `is_revertable()` holds |
| settings | `←→ choose · Space toggle · Del reset` on every field | `←→` on a `Choice`, `Space` on a `Toggle`, `Del` where `reset_field` would do something |
| search | `Enter open` with no results | `Enter` only while there is a hit |
| chat list | (already contextual) | `Enter` also drops on an empty list |
| all five | — | `F1 help`, plus `Ctrl+Q quit` on the two screens that handled it silently and `↑↓ select` on `F3` |

**Key decisions.**

- **The one renderer lives in `shared/ui.rs`, not in `theme.rs`.** `shared::ui`
  already owns `screen_chrome`, the helper that renders these rows, and it may
  depend on `shared::theme` while the reverse cannot. `Palette` keeps only what
  is genuinely about color — `hint`, `hint_highlight_value`, and the new
  `hint_marked`, the one place the danger keycap is chosen. `grid_layout` and
  `right_grid` moved out of `status_bar` into the same module, so the chat bar's
  corner block and the screens' footers now share their geometry; what stays
  chat-specific is only `corner_cols`/`trim_to_fit`/`keep_order` — the capped,
  shedding column choice.
- **The screens wrap; they do not shed** (user's decision, 2026-08-31). The
  chat bar's `HINT_ROWS_MAX = 2` and its shed order exist because the hints
  share a row with a pill that swells at every turn boundary. A full-screen
  panel's footer has the whole width and no competitor, so it grows a row
  instead — which is what the settings footer already did. The awkward part of
  the corner block, a priority order over the keys, stays confined to the one
  surface that needs it.
- **Hide, don't dim.** Dimming would keep the block's geometry stable as the
  selection moves, at the cost of a third keycap style beside normal and danger
  — and it would contradict a rule the code and the spec already state. The
  accepted cost is a footer that reflows: on the self-model screen at 116
  columns a goal's eight hints wrap to two rows where a summary's six fit one,
  so the panel's bottom border moves by a row as the cursor enters and leaves
  the goal block. Reserving the tallest set's height instead would put a
  permanent blank row under every other row, which is worse.
- **The `F1` dialog keeps listing everything.** That is what makes hiding safe,
  and it is why the chat bar sheds `F1` last. A test walks every selectable row
  of `F3` and asserts each key its footer names is in the screen's `F1` section
  (matching on normalized spelling — the footer packs a pair as `↑↓`, the dialog
  spaces it as `↑/↓`).
- **`default_fields()` is built once per frame.** The settings footer's `Del`
  gate is the read-only twin of `reset_field` and needs the default field set —
  which the `•` "modified" marker already built. Rather than take one each, the
  two now share one build threaded from `render`; `reset_changes_something`
  follows `reset_field`'s three branches in its order (a secret row resets while
  a secret is stored, user data never resets, a config row resets while it
  differs from the default).
- **The clear confirmation on `F3` keeps its one-row strip** by passing
  `screen_chrome` no hints at all: the question never trails a blank row, and
  the panel keeps the space the hidden hints would have cost.
- **Three more screens moved onto `screen_chrome`** (search, self-model,
  settings) — the callers its own doc comment named as obvious. `ScreenChrome`
  gained `panel`, since a screen drawing a scrollbar *on* the right border
  measures it from the panel rather than from `inner`.

**Tests** (2735 green, +11): the shared grid — right-aligned and padded to the
width, wrapping without shedding, a wrapped cell's column position equal to the
column above it, every cell placed exactly once at every column count, a danger
keycap red and its neighbour not; per screen — the `F3` walk over every
selectable row against `selected_action`, its help cross-check, the changes
footer by pane and by revertability (with `r` asserted to arm nothing on a
`Gone` file), search's `Enter` gone with no hits, the chat list's `Enter` gone
on an empty list and its block flush right on every row, and the settings
footer walked across a section's field kinds plus a `Del` hint that appears when
a toggle is flipped and goes away with the cursor. A live run is not required
(AGENTS.md §3) — pure UI; the real render is exercised headlessly through the
screenshot pipeline, whose dumps and images are regenerated here.

### Post-M9: the privacy policy joins the disclaimer on one "Legal" tab (done)
<!-- cyrillic-ok:start: the ru tab labels are the measurement -->
- **The ask, and why it could not be a tab.** With the policy shipping in the
  installer and the archive (stage 4 of the code-signing track), the app itself
  was the one place it could not be read. A seventh `F1` tab is not available:
  `HELP_MIN_WIDTH` is 76, the **`ru` tab strip already measures exactly 76**
  (`О программе│Клавиши│Команды│Лицензия│Дисклеймер│Компоненты`) and the English
  one 72, and `the_help_tab_strip_fits_the_dialog_in_every_locale` holds that
  line — the rightmost tab is the one that would be silently truncated, for one
  language only. spec §11.7 had already recorded the constraint when the
  translations landed; this is the first change to run into it.
- **So: one tab, renamed.** `HelpTab::Disclaimer` → `HelpTab::Legal`,
  `ui.help.tab.disclaimer` → `ui.help.tab.legal`, "Legal" / «Правовое».
  Renaming rather than quietly appending, because the point of putting the
  policy in `F1` is that it can be *found*: a tab called "Disclaimer" holding a
  privacy policy is a tab nobody opens looking for one. The same rule the
  installer's wizard page was named by — `mindfork.iss` renamed Inno's stock
  "Information" page to "Disclaimer" for exactly this reason — applied a second
  time, now that the tab holds two documents instead of one. **User's decision
  (2026-09-02)** between "Legal"/«Правовое», "Documents"/«Документы» and keeping
  the old name; «Правовое» (8 chars) also *frees* two columns on the ru strip.
<!-- cyrillic-ok:end -->
- **Two documents, one scroll.** `legal_lines` renders
  `disclaimer_text(lang)` + `---` + `privacy_text(lang)` through the markdown
  renderer in a single pass, so the boundary is a rule between two `#` headings
  rather than a mode to be in or a second scroll position to remember.
  `credits::PRIVACY_TEXT`/`PRIVACY_TEXT_RU`/`privacy_text` mirror the disclaimer
  trio exactly, so the language rule ("`ru` gets the translation, everyone else
  the authoritative English") needed no new semantics.
- **A test that would have passed while asserting nothing.** The obvious check —
  render the tab and look for the policy's heading — is a *false* green: the
  drawn frame is a 60-row viewport and the second document starts ~130 rendered
  rows down, so the marker is simply not on screen. Caught by writing it that way
  first and watching it fail. The real test reads `legal_lines` directly and
  asserts both documents in both languages, and the viewport-based neighbour got
  a comment saying why it only checks the first.
- Docs that named the tab followed: spec §11.7 (the tab list, the renderer note,
  the "not a tail on License" paragraph), README's key table and legal section,
  `docs/install.md`, and the `mindfork.iss` comment that had justified the ru
  wizard caption by pointing at the F1 tab's word — now the same rule stated
  twice rather than one borrowed from the other. 2745 unit tests green,
  129 `#[ignore]`.

### Post-M9: the background run on the list, in the feed and on the bar (done)
- **What**: the surfaces of a sub-agent run out in the background (spec
  §9.3.2, [docs/research/background-subagents.md](../research/background-subagents.md)
  §4.9). The chat list: the run's row comes from its placeholder record the
  moment the parent lands, and the orchestrator's `background_runs` seat
  marks it *running* and keeps its count growing (`emit_chat_list` updates
  the card in place rather than pushing a second one); a stored run with
  `background` and no outcome reads **unfinished** (`ChildSummary::background`,
  `ui.chatlist.run.unfinished`) — the app was closed while it was out — where
  a turn's would read *interrupted*. The transcript opens and streams through
  the same `view()`/`forward_child`/`activate_focused` paths as a turn
  child's (`LiveTurn` keyed by the run's own generation id), and moving to
  it from the parent while a turn runs there cancels nothing
  (`switch_within_turn` covers the parent's background runs). The feed: the
  task notification is a `System` row, so it gets the note look for free
  (`FeedRole::Note`) — no new bubble kind. The status bar: a quiet
  *"in background: n"* indicator (`AppEvent::BackgroundRuns { out }`,
  `ChatScreen::set_background_runs`) beside the silent tasks', cleared at
  zero; the sub-agent chip stays the turn's. `/subagents` gained `stop [n]`
  (`ChatIntent::StopSubagentRun`, resolved on the screen from the list's
  cards: the open transcript's run, or the n-th one out under the chat, or
  the only one), with notes for "which one" and "none out". Settings: three
  rows in the agentic-loop group, the labels kept under `LABEL_CAP`; the
  gallery panel height rose to 33 so the Tools capture still fills its area.
- **Not yet**: a key on the open transcript that stops the run (the
  footer rule of spec §11.2 — advertise only what works — is why the
  command came first).

### Post-M9: the stop key on a background transcript, and the unread chat (done)
- **What**: the two surfaces of the background track's stage 2 (spec §9.3.2,
  §11.2). **The chat screen**: on the open transcript of a run that is still
  out, `F6` stops it and `Esc` goes back to the list — until now `Esc` there
  dispatched `ChatIntent::Cancel` against a turn that did not exist, an
  advertised key that was a no-op, which is precisely why stage 1 shipped
  `/subagents stop` and no key at all. The screen learns which kind of feed
  it is showing from `LiveTurn::background` rather than guessing from
  `generating`; the status bar's `Esc` label follows (`esc_hint_key` reads
  both halves off the one `StatusModel`), and the corner block gains its
  first **conditional** hint — `F6`, right behind `F1` in the shedding order,
  present only while the run streams. **The chat list**: a chat whose
  background result landed while it was not the open one is marked *unread*
  beside its count with its dot in the accent colour, so the one result that
  starts no turn is still announced; the mark is `ChatSummary::unread` from
  `Chat::unread`, cleared by opening the chat. A transcript row never carries
  it — its own row already says how its run ended.
- **Live**: not required for the keys and the row (pure UI), and covered
  anyway by the track's live run — see
  [tools.md](tools.md), the same stage.

### Post-M9: the tasks screen — every background run on one surface (done)
- **What**: `F7`/`/tasks` (spec §11.10; the accepted design
  [docs/research/tasks-screen.md](../research/tasks-screen.md), every fork at
  its recommendation, the user's decisions of 2026-09-06): a full-screen
  projection in two sections — every sub-agent and dialogue run of every
  chat, running first with its position (`round 3 · fs_read`, `line 5`,
  `director`) and elapsed time, then landed with its outcome and finish
  clock (the most recent 50, the rest counted), then the four silent tasks
  as running/idle. `Enter` opens the transcript, `P` the parent chat, `F6`
  stops a running background run through `AppCommand::StopSubagentRun` —
  the footer offers each only where it works (spec §11.2); `Esc` from a
  chat opened here returns to the list (`Back::Tasks`, the third way down;
  `EscTarget::Tasks` on the bar).
- **How**: `AppEvent::TaskList(Box<TaskList>)` + `AppCommand::RequestTasks`;
  the snapshot built in `orchestrator/tasks.rs` off the seats, the turn's
  children and `Chat::children()`; the position via a new
  `TurnProgress::ChildProgress` step that `report_progress`/`dialogue_chip`
  send beside the status-bar chip, stored on `InflightChild.position`;
  `BackgroundRun::is_out` shared by the bar's count and the running rows;
  the emit rides `emit_chat_list` (same sources) plus the position step and
  the silent tasks' begin/done. `keep_visible` moved to `shared::ui` and the
  run-state words to `chat_list::run_state_key` — the seams the second
  caller asks for (docs/lessons.md §2). `HELP_SECTIONS` is 8, the gate
  lists `HelpContext::Tasks`.
- **Two calls the design left open** (recorded in the research doc's status
  too): (1) the turn's own children are listed as running rows — hiding a run
  the chat list shows as running would contradict the screen's premise —
  without `F6` (no seat to cancel; the turn's `Esc` ends them), and the
  bar-count test is pinned on the seats; (2) every emit is a full snapshot
  rather than the live/landed split §5 sketched: the landed walk is the same
  `children()` walk `emit_chat_list` already makes on the same events, and
  tracking the landed half's invalidations (deletion, takeback, a profile
  switch) is exactly the bug farm the split would open. A third shape
  decision: the screen opens **synchronously**, waiting, and asks — because
  the snapshot is also sent unasked, and an event that could open a screen
  would steal the one being read (the stage-2 search trap, verified by the
  steal-gate test). A run's tokens while it runs come from
  `ChildTokens` onto the mirror and ride the next coarse emit — no snapshot
  per usage report.
- **Rejected**: a runtime-side map fed by `SubagentProgress` (fork F3(b) —
  dies with the screen); reusing `ChatList` + `BackgroundRuns` and assembling
  in the screen (F4(b)); a `requested` flag on the event to let a reply open
  the screen (a synchronous open needs none and can never steal).
- **Not stored, not timed**: closing the app forgets the ordering, not the
  runs; the once-a-second repaint runs only while a run is out.
- **Tests**: 26 new — 17 on the screen (the footer against the row, the
  columns lined up, the waiting/empty/capped states, selection by identity,
  the tick), 5 on the snapshot (a run out → landed by id with position and
  outcome, the archive skipped, the cap newest-first, the bar count against
  the running rows through the shared predicate, the request and the silent
  tasks), 4 on the runtime (`F7` and `/tasks` one route, the unasked
  snapshot never opens, the steal gate + the settings broadcast, `Esc` back
  to the list re-asked and `P`) — 2857 unit tests green (2830 before; 138 `#[ignore]`).
- **Live**: not required (pure UI over existing routes); the two background
  e2e smokes re-run once because the orchestrator gained a progress step —
  **GO**, 2/2 in 322 s against the LAN `llama-server` (Qwen3.6-27B-Q4_K_M,
  4 slots, 16k ctx): the scene landed `RoundLimit` at 7 lines / 4907
  tokens with its notification, the sub-agent `Completed` and the wake turn
  named the planted codename — the position step changed nothing on the wire.

### Post-M9: a title is cut where it is drawn, not where it is stored (done)

**Symptom** (the user, on a maximized window, the day the tasks screen
shipped): the first column — the run's title — ends mid-word with no marker,
while the parent-chat column two spaces to its right elides properly, marker
and all. And there were **95 free columns** to the right of the cut, so the
row was not short of room.

**Two cuts, only one of them marked.** The screen's own cut is by width and
carries the marker every column cut in this app carries
(`wrap::truncate_to_width`, `screens/tasks.rs`); it was not the one firing.
The other lives in storage: `shared::title::sanitize_title` capped every title
at `MAX_TITLE_LEN = 100` characters and returned the head, so a run named
after the first line of its instruction (`CallSubagentArgs::initial_title`)
arrived at the screen **already cut and looking whole**. Measured on the
screenshot: the titles were exactly 100 characters. A cut made in storage is
the one cut no screen can mark for itself — by the time a row draws the value,
the fact that something was lost is gone with it.

**The user's decision (2026-09-06): remove the cap.** Not "mark it at 100"
(the option offered) but *remove the limit entirely; if it does not fit, it is
cut with an ellipsis*. So `sanitize_title` is now normalization only (whitespace collapsed, trimmed,
empty rejected), and a title is bounded by the surface that draws it, in
columns, with the marker. The `/rename` route's own duplicate ceiling
(`.take(MAX_TITLE_LEN)` in `screens/chat/commands.rs`, added so the two routes
to a title would "agree on its bounds") went with it — they agree by having
none.

**The consequence is an obligation, so the surfaces were audited.** Every place
that draws a title in a **fixed row** must now cut it itself:

- the chat list (`widgets/chat_list.rs`) and the tasks screen — already did,
  by width, with the marker;
- the search screen — **wraps** every line it builds, so a long group header
  costs a second row and loses nothing; left alone;
- the feed's panel border (`widgets/message_feed.rs`) — did **not**: ratatui
  clips a `Block` title at the corner silently, and a long title also ran into
  the right-aligned model/ctx caption. The title now takes what the ◆ marker,
  that caption and the two corners leave, cut with "…";
- the chat-reference picker (`widgets/chat_link_picker.rs`) — did **not**: a
  long title pushed the date (the thing that tells two same-named
  conversations apart) off the row. Same treatment.

<!-- cyrillic-ok:start (the ru locale values this paragraph is about) -->

**And the Russian section header.** `ui.tasks.sec.runs` was "ПРОГОНЫ" — a
literal rendering of "RUNS" that reads as machine-shop jargon in Russian and
says nothing about what is in the section. It is now **"СУБАГЕНТЫ И ДИАЛОГИ"**
— exactly what the section lists, the wording its own empty state already uses,
and a natural pair for "РАБОТА ПРИЛОЖЕНИЯ" below it (the user's choice from
three; the runners-up were "ЗАПУСКИ" and "ПОРУЧЕНИЯ"). The word "прогон" stays
in the body text, where the app has used it since the sub-agent track. One
stray spelling fixed on the way: "саб-агентов" → "субагентов", the only
hyphenated one in `ru.json`.

<!-- cyrillic-ok:end -->

**Tests**: 3 new — the tasks screen (a title whole at 160 columns, cut with the
marker at 90, the columns after it still lined up), the feed's border (cut and
marked at 60, whole at 100, the caption keeping its corner in both), the picker
(cut and marked, the date surviving) — plus `sanitize_title`'s ceiling tests
replaced by one asserting a 400-character title comes back whole. 2860 unit
tests green (2857 before; 138 `#[ignore]`).

**Live**: not required — a pure string function and three render paths.

**Stage 2 — the data already on disk.** The user rebuilt, opened `F7`, and the
column was cut exactly as before. Of course it was: the cap ran **at write
time**, so every existing title was already 100 characters in
`chats/*.json` — verified against their own data, four run titles at exactly
100. Removing a cap fixes what is written from now on and can do nothing for
what it has already thrown away.

Except here it had not thrown it away: a sub-agent run stores its instruction
as its own first `User` message, and the title was the first line of it. So
`CHAT_SCHEMA` 3→4 (`chat_to_v4`) rewrites a cut title with what
`initial_title` would have produced had the cap never existed — the same seed
rule the language-model history used, *what the recorder would have written had
it existed then*. Deliberately narrow, three guards: the title is **exactly**
100 characters, it is not `renamed_manually`, and the re-derivation **starts
with it** — so the step can only give a title its tail back, never replace one,
and a hand-edited file or a rename the flag missed is safe. Chat titles are
left alone (a model-written or typed one has no source), and so are dialogue
runs (`A ↔ B` over labels that may be a localized default `shared` cannot
reproduce). Idempotent for free: a restored title is no longer 100 characters,
so the first guard stops the second pass.

Probed against the user's real data before the fixture was written: 4/4
restored, to 802, 165, 162 and 159 characters. The 802 is the honest answer
rather than an argument for a cap — that run's instruction is one paragraph
with no line break, the title *is* its first line, and the row now shows as
much of it as the column has and ends in "…".

- **Tests**: 6 more — a golden `chat_v3_cut_run_title.json` (the shape the cap
  stored: a cut run, the same title marked as the user's own, a persona-named
  run, a fourth cut run in the `deleted` archive) with the fixture's own
  parse check, the restore + `v = 4` stamp, everything-else-untouched, the
  not-a-prefix guard, the dialogue guard, and idempotency. 2866 green.
- **Not a shape change**, but a version bump all the same: the step rewrites
  stored values, so it goes through the framework's backup-then-write path and
  runs exactly once (ADR 0006).

**Not done, deliberately**: the model-facing surfaces (`chats` listings,
`chat_search` labels) print a title verbatim and now have no bound at all. The
text is the model's own instruction line, one row per chat, and inventing a
budget for a surface with no columns would be a second invisible cut of exactly
the kind this entry removes.

### Post-M9: the "Sessions" hint names the knob that widens the group (done)

**Symptom** (the user, 2026-09-06, on gpt-5.6 with "Sessions (parallel
streams)" set to 4): the model delegated four `call_subagent` calls in one
reply and the tasks screen showed them landing one at a time. The chat's
records make it exact — each child's `created_at` equals the previous
child's `finished_at` to the microsecond (the four runs at 18:03:37,
18:04:34, 18:05:32 and 18:06:35, each starting the instant its predecessor
ended): the width-1 group of docs/research/parallel-subagents.md §4.2, not a
scheduler fault. Nor was it the background track the user had in mind —
`tools.subagent_background` was off, so `start_subagent` was never offered
and the four ran inside the turn.

**Cause: two knobs, and the hint of the first read as if it were the only
one.** `sessions` is a budget — the semaphore on the streams a turn may keep
open, a ceiling. The width of a round's sub-agent group is
`tools.subagent_parallel` (`run_group`'s `buffer_unordered`,
`orchestrator/generation.rs`), a separate field in *Tools*, default 1 — and
still 1 in the user's settings. The "Sessions" hint said *"1 (default): they
take turns, as before"* — true, and the natural reading is that 4 makes them
stop taking turns. Nothing on the Model tab pointed at the field that does.
The `sub_parallel` hint already pointed the other way (it names the *Parallel
sessions* budget it shares), so the reference was one-directional.

**Fix: the hint names the second knob.** In both locales the sentence after
the default now says the value is a ceiling, not a switch, names "Subagent:
parallel runs" in Tools with its default, and says to raise both; the
"Parallel sessions" paragraph of `docs/install.md` got the same sentence.

**The first draft cost every tab a row, and the drift gate said so.** The
settings screen sizes its hint panel by the **tallest hint of the whole
catalog** (`render.rs::desc_panel_height` — one constant per terminal size
and locale, so that `Tab` never jerks the layout), and the added sentence
made `sessions` that hint: 801 characters in `en` against `sub_background`'s
694, 885 in `ru` against 758. At the demo's width the panel grew by one row,
the field list lost one, and `committed_dumps_match_the_code` went red on
**all four** settings dumps — the Tools tab's included, which no edit had
touched. Rather than regenerate the screenshots for a row the user loses on
every tab, the hint was tightened elsewhere ("at no extra memory" for "so
the memory cost is unchanged", "the open streams" for "the running
conversations", the padding words) until it sits below the tallest hint in
both locales — 659 and 711 characters, measured with the row counts at 60,
88 and 110 columns — and the regenerated dumps came back byte-identical to
the committed ones. A lesson in [lessons.md](../lessons.md) §5 now names the
trap. Not done, recorded so it is not re-derived: folding the two knobs into one by
deriving the group's width from `sessions`. The research keeps them apart on
purpose — a wide group on one session interleaves the children's *rounds*,
measured free on llama.cpp (§3.2), which a single knob could not express —
and a wording fix is not the place to reopen a design decision.

**Tests**: none new — a locale string and two documents; the bundle tests
in `shared/i18n.rs` (every key of `en` present in `ru`, arrays joined with a
space) cover the edit, and the screenshot drift gate covers its height.
2866 unit tests green (138 `#[ignore]`). No live run required: text only.

### Post-M9: stopping a silent task from the tasks screen (done)

**What.** The item the tasks-screen design left out (§8) and the
silent-preemption track pointed back at: `F6` on a task row of the tasks
screen that reads *running* or *waiting* stops that task. The design
[docs/research/stop-silent-task.md](../research/stop-silent-task.md), every
fork at its recommendation (the user's decision, 2026-09-07); no stage-0
probe — nothing about a model's behaviour was in question.

**What was actually missing.** Not the mechanism — every slot already held a
`CancellationToken` (`Quit`'s `cancel_all_bg`), every stream ends
`Finished(Cancelled)` on it, and the holders tell their own token from a
displacement since the preemption track — but the *reading*: the loops
landed a cancelled task as **`Ok`** (`run_rounds` broke out of the round and
reported success — the streak reset, `SelfModelChanged` announced for an
unfinished window), and the roll reported the **timeout's** wording. Only
`Quit` could reach those paths, and nobody reads an outcome during a quit;
a user's stop is read at once.

**How.** The screen's `selected_stoppable()` answers `Stoppable::{Run(id),
Task(kind)}` — a running background run, or a task whose row is running
(streaming or waiting) — and `F6` maps them to `TasksIntent::{StopRun,
StopTask}`; the footer offers `F6` for either, worded *stop*, and an idle
task row keeps it silent. `dispatch_tasks` sends
`AppCommand::StopBackgroundTask { kind }` (a command that changes nothing in
the conversation, on `works_on_the_open_chat`'s false side);
`handle_stop_background_task` cancels the slot's token if the slot is
active and ignores a kind with nothing running. The outcome channel carries
`BgOutcome::{Done, Cancelled, Failed(reason)}` instead of
`Result<(), String>`: `handle_bg_done` on `Cancelled` clears the slot and
the indicator, re-sends the task list, sends `SelfModelChanged` for the
two self-model kinds (a partial run may have written) and touches the streak
not at all. The holders: `run_rounds` returns `RoundsEnd::Cancelled` when
its wait was cancelled or a round's stream ended `Cancelled` without a
displacement; `spawn_compact`'s result is `Result<String, CompactEnd>` with
`CompactEnd::{Failed, Cancelled}`, and `handle_compact_result` answers a
manual roll's `Cancelled` with one notice (`ui.compact.cancelled`) and an
automatic one's with silence — the next landing plans the roll again if the
conversation is still over the threshold. The spawn-time bookkeeping (the
watermark, the counters) stays: a stop skips the window, as a failure does.

**Tests**: +9 — `screens/tasks.rs` (`F6` on a running task row is
`StopTask`, on a waiting one too, on an idle one nothing, the footer
following), `runtime` (the intent becomes the command, the screen stays
open), `tests/silent.rs` (a loop stopped mid-stream, while waiting for room,
and during a displacement's retry lands `Cancelled` with no retry; a manual
`/compact` stopped answers with the notice and the next one runs; an
automatic roll stopped is quiet and planned again at the next landing),
`tests/reflection.rs` (`Cancelled` leaves the streak at two, clears the
slot, emits the indicator, the list and `SelfModelChanged`, no error;
stopping an idle kind does nothing) — **2903 unit tests green, 144
`#[ignore]`**.

**Live** (the paths are engine paths): `stop_silent_task_e2e_live` on the
LAN stack — `/compact`, the stop 1.0 s later, the notice 0.00 s after the
stop (the roll had not opened its stream yet: a cancelled wait returns at
once), nothing folded, the next `/compact` in 5.8 s; with
`silent_roll_e2e_live` and `background_subagent_e2e_live` 3/3 in 57 s.

**Rejected**: a command (`/tasks stop <kind>` — the screen is the surface);
a notice per stop (a stop is not a failure and the feed is not a log);
refunding the window on a stop (the task would come back sooner, the
opposite of what a stop asks); suppressing the automatic roll after a stop
(the protection stays; a stop is per attempt).
