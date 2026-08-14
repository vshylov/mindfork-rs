# Command-only control — hosted terminals (JupyterLab, VS Code)

Status: **accepted 2026-08-14** — every fork decided (the recommendations,
confirmed by the user). **Stage 1 shipped** in `feat/command-only-control`
(journal ui-input.md); stage 2 (§4.5) is open. Two things shipped differently
from the design below, both marked *"shipped instead"* where they are described:
the argument-taking commands joined the registry rather than getting parser
modules, and bare `/rename` prefills the input box rather than opening a popup.
Date: 2026-08-14.

## 1. What and why

Make the application fully operable by **typed commands alone, without hotkey
chords** — for terminals embedded in a host that claims keys for itself: the
JupyterLab terminal in a browser tab, VS Code's integrated terminal. Today a
user in those hosts loses whole features (§2): the host consumes the chord
before crossterm ever sees it, and nothing the application does — raw mode,
kitty protocol, layout resolution — can win a key the host never forwards.

The principle is already established in this project, twice, in the same
words: *the command is the route that works everywhere; the key is the
convenience.* `/image paste` exists next to `Ctrl+V` because Windows Terminal
keeps that key (spec §9.10); `/exit` and `/quit` exist because VS Code claims
**both** advertised quit keys, `Ctrl+Q` and `F10` (spec §11.7, journal
ui-input.md "a typed route out"). This track generalizes the principle from
two actions to all of them: **every action gets a typed route; chords stay as
accelerators.**

One precision that shapes the whole design: "no hotkeys" cannot mean "no
keys". Typing itself is keys, and a safe subset survives every host, because
the host's own UI depends on it: **printable characters (including `/` and
`?`), `Enter`, `Esc`, `Backspace`, `Delete`, `Tab`, the arrows,
`Home`/`End`, `PageUp`/`PageDown`, `Shift`+arrows**. What dies in hosts is
the chord class — `Ctrl`+letter, `F`-keys, `Alt` combinations. So the real
target is:

1. every **chat-screen action** has a slash command (§4.1), and
2. every **modal screen** is operable with the safe subset alone (§4.4 —
   this is already nearly true by construction).

The two halves meet in one invariant the code already has: **`Esc` always
means "one level up", and every `Esc` chain terminates at the chat screen** —
settings → sections → chat; search results → chat list → chat; self-model →
chat; every popup → its screen. The chat screen is where the input box lives,
and the input box is where commands are typed. So "safe keys navigate,
commands act" covers the whole application with no third mechanism.

## 2. What the hosts actually claim

Two independent layers take keys before the application. **The browser**
reserves tab/window management outright — a web page (so, xterm.js, so
JupyterLab) cannot intercept these in Chromium-family browsers: `Ctrl+T`,
`Ctrl+N`, `Ctrl+W`, their `Ctrl+Shift` variants, `Ctrl+Tab`. `Ctrl+W` is the
worst of them: it does not no-op, it **closes the tab with the running
session in it**. Firefox forwards some of these but claims `F10` (menu) and
is its own matrix. **VS Code** dispatches keys to the terminal except those
bound to commands in `terminal.integrated.commandsToSkipShell`; the default
list was read from the VS Code source (`DEFAULT_COMMANDS_TO_SKIP_SHELL`,
`src/vs/workbench/contrib/terminal/common/terminal.ts`, `main` as of
2026-08-14), plus per-contrib additions (the terminal find feature) and the
`allowChords` rule (a chord prefix like `Ctrl+K` skips the shell by
default).

The application's chat-screen chords against that, worst case per host:

| Key | Action (spec §11.7) | JupyterLab (browser) | VS Code terminal (defaults) |
|---|---|---|---|
| `Ctrl+Q` | quit | passes | **taken** — `quickOpenView` |
| `F10` | quit | risky (Firefox menu) | **taken** — `debug.stepOver` |
| `F1` | help | passes (xterm consumes) | **taken** — `showCommands` |
| `Ctrl+P` | settings | passes | **taken** — `quickOpen` |
| `Ctrl+E` | delete last exchange | passes | **taken** — `quickOpen` (alt binding) |
| `Ctrl+F` | in-feed search | passes | **taken** — terminal find |
| `F3` | self-model screen | passes | **taken** — terminal find-next |
| `F5` | copy conversation | passes (xterm consumes) | **taken** — `debug.start`/`continue` |
| `Ctrl+K` | clear input | passes | **taken** — chord prefix (`allowChords`) |
| `Ctrl+N` | new chat | **reserved** (new window) | passes |
| `Ctrl+T` | fold thoughts | **reserved** (new tab) | passes |
| `Ctrl+W` | mouse capture | **reserved — closes the tab** | passes (panel terminal) |
| `Ctrl+L` | `chat://` picker | risky (omnibox focus) | passes |
| `Ctrl+R` | regenerate | passes (xterm consumes) | risky (`runRecentCommand` with shell integration) |
| `Ctrl+B` | emoji picker | risky (JupyterLab sidebar) | passes |
| `Ctrl+U`, `Ctrl+O`, `Ctrl+G`, `Ctrl+D`, `F2` | impersonate, fold tools, search screen / spell, clone, rename | pass | pass |
| `Esc`, `Enter`, arrows, `PgUp/PgDn`, `Home/End`, `Del`, `Tab`, `Shift`+arrows, typing | navigation, editing | pass | pass (scroll keybindings are gated on *not* alt-buffer) |
| `Shift+Enter` | newline | **= `Enter`** (no kitty protocol in xterm.js; `Alt+Enter` is the existing fallback) | ditto |

Summed up per host, **today**:

- **VS Code**: no settings screen, no delete-exchange, no in-feed search, no
  copy-conversation, no `F1` help (`?` still opens it, but only on an empty
  box), no self-model screen, and the message-level search is unreachable
  because its entry sits behind the chat list's `Ctrl+F` content mode. Quit
  survives only because `/exit` was added.
- **JupyterLab**: no new chat, no thoughts fold, no mouse-capture toggle —
  and a `Ctrl+W` habit **destroys the session**.

The cells will drift — a host update or a user keybinding edits the matrix
silently (VS Code users *can* free keys with
`"-workbench.action.quickOpen"`, but the application cannot ask that of every
user). The design must close the **class**, not chase the cells. That is
what a typed route does.

A related boundary, recorded here because this track's pitch is "works in
remote hosts": `/copy` (`F5`) writes the clipboard via `arboard` — the
clipboard of the machine the *process* runs on. Under JupyterLab the pty is
server-side, so the copy lands where the browser user cannot paste from. The
fix is OSC 52 (the terminal escape that sets the *client's* clipboard;
xterm.js and VS Code both support it) — a separate roadmap item, not this
track (§7).

## 3. What already exists (inventory)

The whole mechanism exists; this track adds table rows to it, not
architecture.

- **The command chain.** `handle_enter` (`src/screens/chat/input.rs:321`)
  tries `/rag`, `/reindex`, `/compact`, `/file`, `/image`, `/tts`, `/exit`
  in order, each a pure parser in `src/features/<name>_command.rs`; anything
  unrecognized goes out as a message. The chain runs **before** the
  `generating` gate, so commands work mid-generation (that is what lets
  `/exit` quit during an answer — and would let `/stop` cancel one).
- **The house pattern per command** (journal ui-input.md, "`/exit` and
  `/quit` — a typed route out"): an `ALIASES` const the parser, the help
  label and the tests all iterate; case/padding-insensitive match on the
  exact word; a stray argument answers with a localized note naming the
  typed spelling and a working route (never silence — lessons.md §4);
  the parser is reused by `input_is_command` (input.rs:790) so highlighting
  and dispatch cannot disagree; the command words themselves are **not**
  localized (protocol, the same decision as CLI flags — docs/roadmap.md).
- **Help surface.** `HELP_COMMANDS` + group openers
  (`src/screens/chat/popups.rs`), rows width-budgeted per locale by gate
  tests; README tables; spec §11.7.
- **Modal screens are safe-key-operable already**, with named exceptions
  (§4.4): settings (arrows/`Enter`/`Esc`/`Tab`, `/` field search, `Del`
  reset, `Enter`-committed editors), chat list (arrows/`Enter`, typing
  filters, `Tab` sorts, `Del` deletes), the search screen, the self-model
  screen (arrows/`Enter`/`Space`/`Del`), the help dialog, and every popup —
  profile picker, confirmations, spellcheck, emoji, dangerous-tool
  confirmation — all `Enter`/`Esc`/arrows.
- **Intents for every action already exist** — each proposed command maps
  1:1 onto the intent its key produces today (`ChatIntent`/`AppCommand`).
  The orchestrator gains no new surface; the track is a dispatch table, its
  help rows, locale strings for errors, and documentation.
- **Adjacent, not competing**: the "custom keyboard layout" roadmap item
  (a config field exists, application was never built). Rebinding helps a
  power user free a specific key; it cannot be the answer here, because the
  hint labels teach the defaults, the defaults are what a first-run user
  has, and per-host retuning is exactly the burden the command route
  removes. It stays its own roadmap item (fork F1).

## 4. Design

### 4.1 The command set (stage 1)

Tier 1 closes a hole that exists **today** in a mainstream host; tier 2 is
symmetry and discoverability at near-zero marginal cost (same table, same
gates). Recommendation: ship both in one stage — the registry (§4.2) makes a
tier-2 row two lines plus two locale strings.

| Command | Action | Today's key | Tier |
|---|---|---|---|
| `/settings` | open the settings screen | `Ctrl+P` | 1 (VS Code) |
| `/find [text]` | in-feed search; with text — prefilled, jumped to the first match | `Ctrl+F` | 1 (VS Code) |
| `/search <text>` | the message-level results screen, directly | list `Ctrl+F`→`Ctrl+G` | 1 (VS Code; also collapses a two-step journey into one) |
| `/takeback` | delete the last exchange (the confirmation popup, if enabled, applies) | `Ctrl+E` | 1 (VS Code) |
| `/copy` | copy the conversation to the clipboard | `F5` | 1 (VS Code; see the OSC 52 note, §2) |
| `/self` | the self-model screen | `F3` | 1 (VS Code) |
| `/new [profile]` | new chat; bare — the profile picker popup; with a name — that profile | `Ctrl+N` | 1 (browsers) |
| `/thoughts` | fold/unfold "thoughts" in the feed | `Ctrl+T` | 1 (browsers) |
| `/mouse` | toggle mouse capture | `Ctrl+W` | 1 (browsers — the key closes the tab) |
| `/regen` · `/retry` | regenerate the last response (confirmation applies) | `Ctrl+R` | 1 (VS Code, conditional) |
| `/links` | the `chat://` picker | `Ctrl+L` | 1 (browsers, risky) |
| `/help` | the help dialog | `F1` / `?` | 2 (`?` needs an empty box) |
| `/chats` | open the chat list | `Esc` | 2 (`Esc` is safe but overloaded: during generation it cancels) |
| `/stop` | cancel the current generation | `Esc` | 2 (explicit; resolves the same overload from the other side) |
| `/rename <title>` | rename the open chat; bare — the rename popup, prefilled | `F2` | 2 |

> **Shipped instead** for bare `/rename`: it hands `/rename <current title>` back
> in the **input box** for editing rather than opening a popup. The box is where
> a command is typed and holds nothing else at that moment, so this needs no new
> modal, key routing or render path, and it teaches the syntax by example.
| `/clone` | clone the open chat | list `Ctrl+D` | 2 |
| `/toolcalls` | fold/unfold tool calls | `Ctrl+O` | 2 |
| `/impersonate [seed]` | write a message as the user; the rest of the line seeds it | `Ctrl+U` | 2 |
| `/emoji` | the emoji picker | `Ctrl+B` | 2 |

Deliberately **not** commands:

- **Input-box editing** (cursor movement, selection, clipboard, undo,
  word-ops, the spellcheck popup): a command is typed *in the box*, so it
  cannot operate on the box's content — typing it destroys the object it
  would act on. The safe basics (characters, `Backspace`, `Delete`, arrows,
  `Shift`+arrows, `Home`/`End`) keep editing possible everywhere; the chords
  (`Ctrl+Z`/`Y`/`K`/`C`/`X`/`V`, `Ctrl`+arrows) degrade to conveniences.
  Paste already works by terminal injection in both hosts (that is how
  paste works at all, spec §11.5), and `Ctrl+C` copy passes in both.
- **Scrolling** (`PgUp`/`PgDn` are safe; the wheel comes back via
  `/mouse`).
- **Quit** — already done (`/exit` · `/quit`).

### 4.2 One registry, not eighteen parser files

Lessons.md §2 records five duplication-gate recurrences, the worst at 19.8%,
and the rule: *when a new thing is a sibling of an existing thing, budget for
the seam at design time.* Eighteen copies of `exit_command.rs` is exactly
that failure. The seam:

- **No-argument commands** (most of the table) become **one declarative
  registry**: `&[(&["/settings"], UiCommand::OpenSettings, "ui.help.cmd_settings"), …]`
  — one parser walks it (the `ALIASES` discipline generalized: exact word,
  any case, padding tolerated, stray argument → localized note naming the
  typed spelling).
- **Argument-taking commands** (`/new`, `/find`, `/search`, `/rename`,
  `/impersonate`) get real parsers on the `file_command`/`image_command`
  precedent — they have syntax to explain, so they earn files.
  > **Shipped instead**: they joined the registry, which grew an `Arity`
  > column (`None`/`Optional`/`Required`) and returns the rest of the line as
  > one free-text argument. Checked against `file_command` while implementing,
  > the premise was wrong: that module earns its file from `attach|remove|list`
  > plus path and `#N` parsing, and none of these five has a subcommand, a flag
  > or a target format — a title, a query and a seed are all "the rest of the
  > line". Five more modules would have been the boilerplate §4.2 exists to
  > avoid.
- **The help tab derives its rows from the registry** (the
  `supported_sampling_fields` single-source pattern): a command cannot exist
  without its help row, and the existing per-locale width gates extend to
  the new rows automatically.
- `handle_enter`'s chain folds into: registry lookup → the argument parsers
  → `/exit` (kept last, as today).

### 4.3 Semantics: a command is its key, plus a voice

- **Same intent, same gates, same popups.** Each command produces exactly
  the intent its key produces today, through the same path — including the
  `confirm_destructive_keys` popup for `/regen` and `/takeback` (the popup
  is safe-key: `Enter`/`Esc`). No second semantics anywhere.
- **A gated-off command answers; a gated-off key may stay silent.** `Ctrl+R`
  during generation no-ops — a keypress is cheap. A typed `/regen` that
  vanishes silently reads as breakage (the `/exit` lesson: silence reads as
  refusal). Every command blocked by state answers with a localized note
  naming what blocks it and the route that works (`/stop` while idle, `/regen`
  while generating, `/links` with no references in the chat, `/impersonate`
  while generating).
- **Draft collision, stated honestly.** A command needs the box to itself;
  with a draft present the user deletes it first (`Backspace`/selection —
  safe keys; `Ctrl+K` is a VS Code casualty). Restoring the draft after a
  command executes (popping the command's keystrokes off the undo stack) was
  considered and **rejected for v1**: undo units coalesce by word, so the
  boundary between "the draft" and "the command" is not reliably a unit
  boundary. Commands that take text (`/impersonate seed…`, `/rename title`)
  naturally consume the box instead.
- **Highlighting**: the registry feeds `input_is_command`, so every new
  command (and its malformed variants) colors as a command while typed, by
  the same parser that will dispatch it.
- **Locale**: command words English-only (existing decision); every error
  and help string in both axes' gates as usual.
- **Quit from a modal screen in VS Code** (both quit keys taken): `Esc`
  walks to the chat screen, then `/exit`. Two steps, always available —
  documented in help/README rather than "fixed" with a third mechanism.

### 4.4 The modal screens: what safe keys do not cover

The screens are navigable, but a few **actions** inside them are chord-only
today. With the §4.1 set, every one of them has a command counterpart *for
the open chat*, which re-frames the chat list as a navigation surface
(arrows/`Enter`/typing — all safe) rather than a management surface:

| In-screen chord | Covered by |
|---|---|
| list `Ctrl+N` (new), `Ctrl+D` (clone), `F2` (rename), `F5` (copy) | `/new`, `/clone`, `/rename`, `/copy` on the open chat; for another chat — open it first (safe keys), then the command |
| list `Ctrl+F` (content mode), `Ctrl+G` (results screen) | `/search <text>` goes straight to the results screen; the list's title filter stays plain typing |
| list `Del` (delete) | already safe |
| settings `/` search, `Del` reset, editors | already safe |
| settings `Ctrl+Z`/`Ctrl+Y` (undo a commit) | **gap, accepted**: every edit is re-editable by hand through the same safe-key path; undo stays a convenience |
| settings, Profiles tab: `Ctrl+N` (new profile), `Ctrl+D` (delete profile) | **the one real leftover** — fork F5 |
| self-model `Ctrl+K` ×2 (clear the whole model) | **gap, accepted for stage 1**: rare, destructive, and reachable nowhere else; fork F5 decides its route with the profiles one |

The profiles leftover is genuine: profile CRUD exists only behind those two
chords. Physical-key alternates (`Insert`) fail on Mac client keyboards (no
such key — the *host* machine's keyboard matters in a browser terminal, and
Mac browsers reaching a Linux JupyterLab is a normal setup). Hence fork F5
leans toward commands.

### 4.5 Stage 2 (small)

- The F5 fork's outcome (decided: commands): `/profile new [name]` ·
  `/profile delete <name>` (with a confirmation popup; deletion cascades
  are already confirmed today) and `/self clear` (confirmed, as the
  `Ctrl+K` ×2 route is today).
- Optional polish (fork F7): host detection (`TERM_PROGRAM=vscode`,
  `JUPYTER_*` env) to prefer command spellings in the status-bar hints. The
  hint grid is width-budgeted, so this is not free; the help dialog and
  README may be enough.

## 5. Forks

- **F1. Scope of the mechanism.**
  (a) **Command parity on the chat screen + safe-key modals** (this design).
  Smallest new surface; generalizes two precedents the project already
  trusts; the `Esc`-to-chat invariant makes it complete. *(recommended)*
  (b) A leader-key "command mode" / palette available on **every** screen
  (vim's `:`, VS Code's palette): one more input surface to build, teach and
  localize, and redundant the moment (a) exists — reconsider only if a
  modal screen ever grows an action that safe keys cannot reach *and* that
  cannot be a chat-screen command.
  (c) Rely on the existing "custom keybindings" roadmap item instead:
  rejected as the primary answer — first-run users have the defaults, the
  help teaches the defaults, and the browser-reserved class (`Ctrl+W`)
  cannot be rebound around. Stays a complementary roadmap item.
  User's decision: **(a) — command parity + safe-key modals** (2026-08-14).
- **F2. Names.** The table in §4.1 as proposed; the contested cells:
  `/takeback` (project prose: "take back an exchange") vs `/retract` vs
  `/undo` (collides with input undo) — recommended **`/takeback`**;
  `/self` vs `/selfmodel` vs `/model` (collides with LLM-model expectations)
  — recommended **`/self`**; `/toolcalls` vs `/tools` (collides with tool
  *settings*) — recommended **`/toolcalls`**; in-feed `/find` vs cross-chat
  `/search` split as proposed. Alias policy: a second spelling only where
  both words are pre-trained elsewhere (`/exit`·`/quit` precedent) —
  recommended pairs: **`/regen`·`/retry`** only.
  User's decision: **as recommended** — `/takeback`, `/self`, `/toolcalls`,
  the `/find`/`/search` split, aliases only for `/regen`·`/retry`
  (2026-08-14).
- **F3. `/new <profile>` matching.** (a) **Case-insensitive exact, then
  unambiguous prefix; ambiguity/miss answers by naming the candidates and
  the bare-`/new` picker route** *(recommended)*; (b) exact-only.
  User's decision: **(a)** (2026-08-14).
- **F4. Bare `/search`.** (a) **A localized note teaching
  `/search <text>`** *(recommended — the results screen without a query is
  meaningless)*; (b) open the chat list in content mode instead.
  User's decision: **(a)** (2026-08-14).
- **F5. The profiles/self-model-clear leftover (stage 2).**
  (a) **Commands**: `/profile new [name]` · `/profile delete <name>`,
  `/self clear` — uniform with the track, no new in-screen surface, works on
  Mac-client browsers. *(recommended)*
  (b) In-screen safe-key alternates + contextual footer hints.
  (c) Defer entirely, documented as a known gap.
  User's decision: **(a) — commands, in stage 2** (2026-08-14).
- **F6. `/stop` and `/chats`** (both have safe-key routes via `Esc`):
  include *(recommended — they resolve `Esc`'s double meaning during
  generation from both sides, and cost two registry rows)* / drop.
  User's decision: **include both** (2026-08-14).
- **F7. Host-adaptive status-bar hints.** Defer *(recommended)* / stage 2.
  User's decision: **defer** (2026-08-14).
- **Considered and rejected here:** localizing command words (existing
  protocol decision); auto-rebinding at-risk chords per host (fights the
  host, silently wrong after its next update, and unteachable); making the
  feed or lists focusable text surfaces with their own `/` (the in-feed
  search design already rejected `/` as a trigger for the same reason —
  spec §11.3.1).

## 6. Test plan

Per the house pattern, everything driven off the registry so a command
cannot ship half-covered:

- **Parser**: every alias bare/padded/upper-cased; stray arguments answer
  and never send; near-words (`/settingsx`, `/self-model`, prose) fall
  through as messages; per-locale error gates (no `{…}`, `en` free of
  Cyrillic, every note names a working route).
- **Screen**: each command yields exactly its key's intent (table-driven
  against the registry); the generating-gate arms with controls
  (`/stop` idle vs generating, `/regen` both ways, `/exit` unchanged);
  the box is cleared and the draft handed back empty; confirmation-popup
  routing for `/regen`/`/takeback` with the setting on and off;
  highlighting for every alias and malformed variant.
- **Help**: every registry row renders in both locales inside the width
  budget (existing gates extended); the Commands tab's group openers still
  open real rows.
- **Duplication**: the new files through the Sonar snippet analyzer before
  the PR (lessons.md §10); the registry exists precisely to keep the
  sibling-boilerplate density at zero.
- **Live**: not required for the Rust gates (pure UI routing, AGENTS.md
  §3) — but the *motivating claim* is host behavior, which unit tests
  cannot reach. Acceptance: a manual pass in a real VS Code integrated
  terminal and a real JupyterLab terminal — every tier-1 command exercised,
  every modal screen operated with safe keys only — recorded in the journal
  entry with host versions.

## 7. Scope and documentation

Stage 1 = §4.1 + §4.2 + §4.3. Stage 2 = §4.5 per forks. Updates on
completion (AGENTS.md §4): spec §11.7 (the table gains the commands; a
paragraph states the parity principle and the safe-subset invariant),
§11.3.1 (`/find`), §11.2.1 (`/search` as a direct entry), §17.7 (`/self`);
README command/key tables; the help overlay (in code, via the registry);
CHANGELOG (Added); journal ui-input.md; docs/roadmap.md — new item: OSC 52
clipboard for remote hosts (§2's boundary); CLAUDE.md status line.
