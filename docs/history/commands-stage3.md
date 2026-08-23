# Commands — stage 3: auto-title, personas, profile texts, and the command residue

Status: **accepted 2026-08-23** — every fork decided (F1, F3, F4 as
recommended; F2 against the recommendation — the user chose the settings
subsection's own word over the shorter noun). Continues
[docs/history/command-only-control.md](command-only-control.md) (stages
1–2, accepted 2026-08-14): the principle *"every action gets a typed route;
chords stay as accelerators"* is settled there and is not re-argued here.
Date: 2026-08-23.

## 1. What and why

A live pass in a JupyterLab browser terminal (2026-08-23) found three gaps:

1. **No typed route to auto-title.** The model-generated title exists only
   behind the chat list's `Ctrl+R` (spec §11.2) — and in the user's browser
   `Ctrl+R` reloads the tab. The 2026-08-14 host matrix scored that cell
   "passes (xterm consumes)"; the report shows the matrix drifting exactly as
   §2 of the stage-1 design predicted. Close the class: a command.
2. **No typed route to impersonation-profile CRUD, or to the profile texts.**
   Stage 2 gave assistant profiles `/profile list|new|delete`, but the
   personas (spec §11.8, `AppConfig.impersonation_profiles`) are still
   editable only behind the settings screen's `Ctrl+N`/`Ctrl+D` — the same
   browser-taken chords that motivated stage 2 — and neither list's *text
   fields* (the assistant's system message and greeting, the persona's system
   message) has any route but the settings editors. A persona is also only
   *selectable* (`Profile.impersonation_profile_id`) in settings.
3. **A command leaves residue in the input box.** After `/takeback` the box
   read `Please tell me about OpenAI./takeback` — the restored message with
   the spent command glued on. The audit (§2) shows this is one defect class
   covering `/takeback`, `/regen`, `/clone` and `/new`.

## 2. The residue defect: the draft flush lags the intent

**Mechanism** (all deterministic, default settings). Every keystroke marks the
draft dirty; the loop flushes it as `AppCommand::SetDraft` **at the top of the
next iteration** (`run_loop`), while a key's intent is dispatched **at the
bottom of this one** (`handle_input_tick`). So when `Enter` runs a command the
order on the orchestrator's channel is always: `SetDraft("/takeback")` (typed),
then the command's intent, then `SetDraft("")` (the clear) — one iteration too
late. Every handler that reads `chat.draft` between the last two sees the spent
command:

- `handle_delete_last` (`/takeback`): records the command as the recovery
  draft, then `activate()` re-loads `"/takeback"` into the box from
  `ChatActivated.draft`, and `RestoreInput` prepends the deleted message —
  producing the screenshot's concatenation. (With
  `confirm_destructive_keys = on` the popup's round trip gives the flush time
  to land, which is why the bug hides there; the setting is off by default.)
- `handle_regenerate` (`/regen`·`/retry`): `activate()` re-loads the spent
  command into the box.
- `handle_clone` (`/clone`): the clone copies `draft`, so the new chat opens
  with `/clone` in the box and saves it as its draft.
- `handle_new_chat` (`/new`): the **old** chat keeps `/new …` as its saved
  draft forever — the late `SetDraft("")` lands on the newly activated chat —
  so reopening it greets the user with the command.

**Fix — flush before dispatch** (one seam, closes the class): in the runtime's
key path (`input.rs::handle_key_event`) and the mouse path, send the pending
dirty draft **before** dispatching the intent the screen returned. The
orchestrator then applies `SetDraft("")` first, and every reader above sees the
truth. This also repairs the chord-path edge (keystrokes and `Ctrl+E` arriving
in one batch used to record a stale recovery draft), and the `/new`/`/clone`
cases where the draft otherwise lands on the wrong chat.

Two adjacent nits, same change:

- **`RestoreInput` glues without a separator** (`{text}{existing}`): after the
  fix the both-non-empty case is the chord path (typed text + `Ctrl+E`), and
  the restored message still fuses with the typed text mid-word. Insert one
  space when neither boundary has whitespace.
- The recovery draft recorded by `record_deleted` becomes the *user's* text
  again rather than the spent command — no code change, but the tests pin it.

## 3. The new commands

### 3.1 `/autotitle` — ask the model to title the open chat

One registry row (`Arity::None`), the same `AppCommand::AutoRenameChat` the
chat list's `Ctrl+R` sends (`TitleOrigin::Requested`), for the **open** chat —
the same "on the open chat" re-framing stage 1 applied to `/rename`/`/clone`.
Preconditions that answer (lessons.md §4): no chat open → the `no_chat` note; a
sub-agent transcript is fine (the list's action already titles those). Failures
already reach the feed: `ChatListError` routes to the open list or falls back
to a feed note (`report_chat_list_error`). Success is the visible rename plus
the `ChatRenamed` event — same as `/rename`, no extra note. A short "asking the
model…" note *is* pushed on dispatch: the task takes seconds on a local model,
and a typed command that sits silent reads as refused.

### 3.2 `/profile system [text]` · `/profile greeting [text]` — the assistant's texts

Two subcommands on the existing `features/profile_command.rs` (it is already
the module for "subcommand + free text"). Target: the **active chat's**
profile — the command answers `no_chat` without one. Route:
`AppCommand::UpdateProfile { id, edit }` with the one field set — the same
command the settings editors commit through, so validation, persistence and
the re-emitted snapshots are shared.

- **With text** — set it. The note states the scope honestly: the system
  message and greeting are copied into a chat at creation
  (`Chat::from_profile`, `new_chat_value`), so both edits apply to **new
  conversations** with this profile; the open chat keeps its own.
- **Bare** — hand the current text back as an editable command line
  (`/profile system <current…>`), the `/rename` pattern; multi-line values are
  fine, the box is multi-line. With nothing set — a note teaching the usage
  and, for `greeting`, saying none is set.
- **`clear`** — the reserved word removes the value (`greeting → None`,
  `system → ""`); a literal greeting of "clear" stays reachable via settings.
  (Fork F3.)
- `/profile new`'s success note (`ui.profile.created`) gains the typed route:
  `/new <name>` then `/profile system <text>` — today it points only at
  settings.

### 3.3 `/impersonation …` — the impersonation profiles (fork F2 for the word)

A new module `features/impersonation_command.rs` (subcommands + free text — the
`profile_command` shape), acting on `AppConfig.impersonation_profiles` through
the chat screen's settings snapshot and one new intent,
`ChatIntent::UpdateConfig` → the existing `AppCommand::UpdateConfig` — the
very path the settings screen's persona editors use, so the orchestrator's
config-save semantics (engine-diff restarts, key/pin preservation) are
inherited, not reimplemented. The screen refuses all of them with a note while
the settings snapshot has not arrived (`/settings`'s rule).

| Command | Does | Answers |
|---|---|---|
| `/impersonation list` | the personas, the active profile's marked | none yet → a note naming `/impersonation new` |
| `/impersonation new [name]` | create (default name — the settings screen's) | note: created; `/impersonation use <name>` links it, `/impersonation system <text>` writes it |
| `/impersonation delete <name>` | delete, **always confirmed** (the `/profile delete` rule: a typed prefix can resolve to the wrong target; its system message is unrecoverable) | dangling references read as "not set" — the popup says referencing profiles fall back to the default |
| `/impersonation use <name>` | link to the active chat's profile (`UpdateProfile.impersonation_profile_id`) | `use default` unlinks — the shared default text (the settings choice's option 0) |
| `/impersonation system [text]` | the **linked** persona's system message; bare — the `/rename`-style prefill; `clear` empties it (falls back to the default text) | no persona linked → a note naming `/impersonation use`/`/impersonation new` |

Name resolution: the shared rule (case-insensitive exact, then unambiguous
prefix; a miss/ambiguity names the candidates and `/impersonation list`) — the
resolver generalizes `resolve_profile` rather than copying it (the duplication
seam, lessons.md §2). The exact-word match keeps `/impersonation` and
`/impersonate` apart; the near-miss risk F2 names is accepted with the choice.

Unlike the assistant's texts, a persona edit applies to the **next `Ctrl+U`
everywhere** — `impersonation_system` resolves live — and the notes say so.

## 4. Forks

- **F1. The auto-title command's word.**
  (a) **`/autotitle`** — the feature's name in spec §11.2, the setting
  (`interface.auto_title`, "Auto-title new chats") and the track doc.
  *(recommended)*
  (b) `/autoname` — matches the chat list's hotkey hint label
  (`ui.chatlist.hk.autoname`, "auto-name").
  (c) A subcommand on `/rename` (`/rename auto`) — rejected: `/rename`'s
  argument is a free-text title, and reserving a word inside it makes the
  literal title "auto" unreachable while teaching a second syntax for the
  same command.
  User's decision: **(a) `/autotitle`** (2026-08-23).
- **F2. The persona command's word.**
  (a) `/persona` — the noun the spec and settings descriptions already use
  ("the user persona"), and visibly distinct from the action `/impersonate`.
  *(recommended)*
  (b) `/impersonation` — matches the settings subsection's title, but is one
  letter group away from `/impersonate`: a typo runs the other command.
  User's decision: **(b) `/impersonation`** — the subsection's own word wins
  over brevity; the near-miss risk is accepted (2026-08-23).
- **F3. Clearing a text by command.**
  (a) **The reserved word `clear`** on the three text subcommands
  (`/profile system|greeting`, `/impersonation system`) — explicit,
  symmetrical with `/self clear`; the literal text "clear" stays settable in
  settings. *(recommended)*
  (b) No clearing by command (settings only) — leaves a bare-prefill loop
  with no way to express "empty" from the box.
  User's decision: **(a) `clear`** (2026-08-23).
- **F4. Does `/impersonation new` link the new persona to the active profile?**
  (a) **No — create only**, the settings screen's `Ctrl+N` parity; the note
  names `/impersonation use <name>` as the next step. *(recommended: creating
  and selecting are different decisions, several assistant profiles can share
  one persona, and the settings flow keeps them separate too)*
  (b) Yes — one step shorter for the common case, but a surprise when
  creating a second persona for a *different* profile.
  User's decision: **(a) create only** (2026-08-23).

## 5. Test plan

The stage-1/2 patterns, driven off the registries so nothing ships
half-covered:

- **Residue fix**: a runtime-loop test per command class — after `/takeback`
  the box holds exactly the restored text (no concatenation) and the saved
  draft matches; after `/regen` the box is empty; a `/clone`'d chat opens with
  an empty box and draft; after `/new` the *old* chat's draft is empty. The
  separator: restore over typed text yields one space, the existing
  trailing-space fixture stays byte-identical. Mutation check: re-order the
  flush after dispatch and the takeback test must go red.
- **Parsers**: every new word/subcommand bare/padded/cased; stray and missing
  arguments answer with the usage line; near-words fall through as prose;
  per-locale error gates (no `{…}`, `en` free of Cyrillic, every note names a
  route).
- **Screen**: table-driven intent equality against the chords' intents where
  one exists (`/autotitle` ↔ the list's `Ctrl+R`); the preconditions with
  controls (no chat, no snapshot, no persona linked, last profile); the
  prefill round trips (`/profile system` bare → edit → set); `/impersonation
  delete` always confirms; `input_is_command` covers every new spelling.
- **Orchestrator**: none needed beyond what exists — every route reuses
  `AutoRenameChat`/`UpdateProfile`/`UpdateConfig`, already covered.
- **Live**: not required — pure UI routing over existing orchestrator surface
  (AGENTS.md §3); the motivating host behaviour is re-checked manually in
  JupyterLab as in stage 1.
- **Duplication**: the impersonation module reuses the profile module's resolver and
  test harness shapes; Sonar-check the PR before calling it done (lessons §2).

## 6. Scope and documentation

One PR (the fix and the commands land together; the commands are the reason
the fix is visible). Updates on completion (AGENTS.md §4): spec §11.7 (the
command tables and the stage-3 paragraph), §11.2 (`/autotitle`), §11.8 (the
persona commands), §5.1 if the greeting note needs it; README command table;
the help overlay rows (registry-derived plus the module rows); CHANGELOG
(Added: the commands; Fixed: the residue); journal **ui-input.md** (the
residue fix is input/draft mechanics) and **ui-screens.md** (the commands, as
stage 2's entry lives there); this plan → `docs/history/`.
