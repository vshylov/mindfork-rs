# Help hotkeys by context — track design

**Status:** done. Forks decided by the user 2026-08-24 (all as recommended).
Stage 1 — branch `feat/help-keys-sections` (PR #382, merged 2026-08-25);
stage 2 — branch `feat/help-f1-everywhere`. The journal entries live in
[docs/journal/ui-screens.md](journal/ui-screens.md).

The help dialog's "Shortcuts" tab (`F1`/`?`, spec §11.7) describes keys whose
meaning depends on the screen, and the flat list has grown three different
ways of saying so at once. This track restructures the tab into per-screen
sections and, in a second stage, makes `F1` work on every screen and open the
tab at the section for the screen it was pressed on.

## 1. Problem

The tab is one flat list (`HELP_KEYS`, `src/screens/chat/popups.rs`), and
context is expressed inconsistently:

1. **Context crammed into the description.** `Ctrl+F` reads "in a chat: find
   in this conversation; in the chat list: titles ↔ message content"
   (`ui.help.search_content`); `Esc` compresses three meanings into one row.
   Every such clause is written per locale, twice the words in each.
2. **The same key listed twice, unexplained.** `Ctrl+O` appears in two
   display groups (the chat-list meaning and the chat meaning), `Ctrl+G`
   likewise. The reader sees a duplicate, not a system.
3. **Missing meanings.** `Ctrl+R`'s chat-list meaning (ask the model to title
   the chat) is absent; the settings screen's keys (`Ctrl+N`/`Ctrl+D` on the
   profile lists, navigation, undo), the self-model screen's (`Ctrl+K` twice),
   the changes screen's (`r` — revert) and the search screen's keys are not in
   the help at all — only their own footers teach them.
4. **Help opens only from the chat screen.** `F1`/`?` is handled in
   `screens/chat/input.rs` alone; from the settings screen — the one with the
   most keys of its own — the dialog cannot be opened.

## 2. Decision

The classic TUI answer (htop, less, mc): **organize the reference by context,
open it context-sensitively.**

1. **The "Shortcuts" tab becomes a list of per-screen sections** with
   localized headers: Everywhere · Chat · Chat list · Settings · Self-model ·
   Changes · Search results. A key is listed once *per context* where it does
   something; the homonym conflict dissolves by construction, descriptions
   lose their "in the chat list: …" clauses, and the screens missing from the
   help get a home. Related-key micro-groups inside a large section keep the
   existing blank-line mechanism (`KEY_GROUP_OPENERS` pattern).
2. **A "you are here" marker** on the header of the section for the screen
   the dialog was opened from — accent style plus a localized suffix
   (`ui.help.here`).
3. **Stage 2: `F1` works on every screen** and opens the tab scrolled to that
   screen's section. The full list stays scrollable — the overview (browsing
   what other screens can do) is preserved.

No new top-level tabs: the `ru` tab strip already measures exactly the
dialog's minimum width (spec §11.7), so a seventh tab has no budget. Sections
live inside the existing tab.

### Section layout (sketch)

```
 Everywhere ───────────────────────────────────
 Ctrl+Q / F10    quit
 F1 / ?          this help

 Chat · you are here ──────────────────────────
 Enter           send message
 …               (composing · selection/clipboard · conversation ·
                  editing · panels/toggles — the existing micro-groups)

 Chat list (Esc) ──────────────────────────────
 Ctrl+O          fold/unfold sub-agent transcripts
 Ctrl+R          ask the model to title the chat
 …

 Settings (Ctrl+P) ────────────────────────────
 …
```

Headers carry the route to the screen in parentheses (`Settings (Ctrl+P)`,
`Self-model (F3)`, `Changes (F4)`), doubling as "how do I get there". The
exact rows of each section are derived from the screen's actual key handler
during implementation — completeness against the `match` arms is the point.

## 3. Forks (decided 2026-08-24)

- **F1 — the "you are here" marker**: style only, or style + localized
  suffix. **Decision: style + suffix** (`ui.help.here`) — visible and
  testable, one locale key.
- **F2 — behavior when opened from the chat screen**: non-chat screens always
  open "Shortcuts" anchored to their section; the chat keeps the current
  last-tab behavior (About/License are browsed from there). **Decision: as
  stated.** The marker shows regardless.
- **F3 — staging**: one track, two PRs. **Decision: as stated.** Stage 1 is
  self-contained value even without stage 2.

## 4. Rejected alternatives

- **Key-first matrix** (one row per key, meanings per context) — formalizes
  today's failure: optimizes "what does `Ctrl+O` do" while killing "what can
  I do here", and brings the multi-clause descriptions back.
- **A "current screen only" filter** — loses the overview and adds hidden
  state; anchor + scroll gives the same without the loss.
- **A full keymap registry driving both dispatch and help** — the
  single-source ideal, but it would rework the idiomatic `match` handlers of
  six screens. The command registry earned its keep by *parsing*
  (`command_rows` derives the "Commands" tab from it); a key registry would
  exist only for the help. Per-screen tables next to the handlers (stage 2)
  keep that door open.
- **A which-key-style interactive prompt** — a different problem
  (in-the-moment hints); the per-screen footers already do the minimal
  version, and the `F1` tab stays a reference.

## 5. Stage 1 — sections (`feat/help-keys-sections`)

Content and presentation; the dialog stays chat-owned.

- `HelpContext` enum (Chat · ChatList · Settings · SelfModel · Changes ·
  Search) and `HelpSection { title, context: Option<HelpContext>, rows,
  openers }` in `screens/chat/popups.rs`; `HELP_KEYS` +
  `KEY_GROUP_OPENERS` become per-section consts composed into
  `HELP_SECTIONS`. Sections for all seven contexts, rows completed from each
  screen's handler (the missing meanings of §1.3 included).
- `key_lines` generalized to render sections: a header line (title, optional
  route annotation, a `─` rule to the dialog's width) before each section's
  rows; one shared description column across the whole tab; the marker suffix
  on the section matching `HelpState.context`. The "Commands" tab renders as
  one untitled section — its rows and grouping do not change.
- `HelpState` gains `context: HelpContext` (always `Chat` in stage 1 — the
  only opener). No anchor scrolling yet (from the chat, "Everywhere" plus the
  chat section are at the top anyway — fork F2).
- i18n: `ui.help.sec.*` (7 keys), `ui.help.here`, split/new description keys;
  the crammed clauses removed; `ru` + `en` at parity. Dead keys deleted from
  both bundles (the i18n gates hold this).
- Tests: the existing gates extend naturally
  (`help_rows_fit_the_dialog_in_every_locale` at both width bounds in every
  locale now covers header rows; `group_openers_open_real_rows` — per
  section). New: every section is non-empty, headers render, the marker
  appears exactly once and on the chat section.
- Docs: spec §11.7 (the tab's structure), README's key table, CHANGELOG
  (Changed), journal `ui-screens.md` entry, CLAUDE.md status line.

## 6. Stage 2 — the overlay and `F1` everywhere

Ownership and routing; behavior of the sections from stage 1 is unchanged.

- The dialog's renderer and the `HelpSection`/`HelpContext` types move to
  `widgets/help_dialog.rs`; `HelpState` moves to `runtime.rs` beside
  `active`, and the dialog is drawn **over whatever screen is active**. The
  chat screen loses its `help` field; its `F1`/`?` (and `/help`) route
  through a new `ChatIntent::OpenHelp`.
- Section tables distribute to their owners — the chat's stay in
  `screens/chat`, the list's next to its `handle_key` in
  `widgets/chat_list.rs`, the settings' in `screens/settings`, and so on —
  and the **app layer composes them** (it is the one place that already knows
  every screen exists). All imports stay downward; proximity of a table to
  the `match` it documents is the anti-drift force.
- `F1` is routed at the runtime level for every screen, opening the dialog
  with `context` = the active screen; per fork F2 a non-chat opener forces
  the "Shortcuts" tab anchored to its section (initial scroll = the section's
  first row, computed at render where wrapping is known). `Esc` closes back
  to the covered screen; `Ctrl+Q`/`F10` punch through as today. `?` stays
  chat-only (typing owns it elsewhere).
- On hosts that claim `F1` (VS Code) the overlay screens still have no help
  route — same as today, and `Esc` → `/help` remains; recorded, not solved,
  here.

## 7. What this does not need

A live run: a help tab and its documentation — the precedent recorded for the
`F1` tabs track (journal `ui-screens.md`) applies; unit tests and the locale
gates cover the change. Engine, storage, tools are untouched.
