# Settings-screen navigation: focus model

A design plan for reworking how focus moves between the section menu and the
field pane on the settings screen (`Ctrl+P`, spec
[§11.6](../../spec.md#116-the-settings-screen)). Scope — one screen
(`src/screens/settings/`), one PR. No cross-layer contract change: the
`SettingsIntent` surface, `AppConfig` and storage are untouched.

Forks are recorded in §3 (**user's decision, 2026-07-31**).

---

## 1. The defect

Reported from real use: *"users step from the sections into the parameters with
`→`, then try to come back with `←`. When a toggle is selected they change a
setting instead — the server mode, for example. Then, confused, they press `Esc`,
leave the settings screen entirely, come back and try to remember what they just
changed."*

The current key table (`screens/settings/apply.rs`):

| Key | Focus: Menu | Focus: Fields |
|---|---|---|
| `→` | **enters the field pane** (`field_idx = 0`) | `Choice`: cycle forward; otherwise nothing |
| `←` | nothing | `Choice`: **cycle backward**; otherwise leave to the menu |
| `↑`/`↓` | move between sections | move between fields |
| `Tab`/`BackTab` | switch section | switch section + **focus is reset to the menu** |
| `Enter` | enters the field pane | edit / toggle / open the Choice popup |
| `Esc` | close the screen | **close the screen** |

### 1.1. The exact failure chain

In the "Model/server" section, `→` from the menu lands on field 0, which is the
**subsection tab strip** — so `←` there switches Assistant → Impersonation and
the entire field set changes under the user's hands. One `↓` further is `XMode`,
and `←` there cycles the server mode (managed → claude), which is emitted
immediately as `SaveConfig` and, after the debounce, **restarts the server**
(`RestartQueue`, spec §11.6). So the accidental edit is not cosmetic, and the UI
offers no way to undo it.

`Esc` then closes the whole screen rather than stepping back one level, which is
what turns a small misstep into "I've lost my place and I don't know what I
changed".

### 1.2. Root cause

**While `→` enters the field pane, the user inevitably builds the model "`←`
leaves it" — and `←` must cycle the value of a `Choice` field.** The two meanings
collide on the same key, and the collision cannot be removed while `→` enters:
`←`/`→` are the natural pair for cycling a value (the tab strip uses them too),
so the value binding is the one that has to stay.

Note that the `Esc` ladder **already half exists**: the field editor, the Choice
popup and the search overlay all close *into the field pane* rather than closing
the screen. And the footer already reads "Esc — back", which is, strictly
speaking, a lie today.

---

## 2. The target model

Focus is a two-level stack, and every transition is explicit:

```
        Enter ──►
[ Sections ]        [ Parameters ]        [ editor / Choice popup / search ]
        ◄── Esc                    ◄── Esc (already the case today)
   Esc → close
```

- **`Enter`** is the only way into the field pane.
- **`Esc`** is the only way out of it, and it means the same thing everywhere on
  the screen: "one level up".
- **`←`/`→` inside the pane mean exactly one thing: change the value** (or switch
  the subsection tab). They never move focus.
- **`Tab`/`BackTab` mean exactly one thing: switch section.** They no longer have
  the hidden side effect of moving focus.

The whole model is one sentence: *arrows change, `Enter` goes in, `Esc` goes out,
`Tab` switches section.*

---

## 3. Forks (user's decision, 2026-07-31)

- **R1. Entering the field pane.** `→` no longer enters; **`Enter` only**.
  *Adopted* (the user's proposal — it removes the root cause of §1.2).
- **R2. `←` on a non-`Choice` field (text/number/toggle).** **Remove the "leave to
  the menu" path entirely** — `←`/`→` in the pane mean only "change the value".
  *Adopted, as recommended.* The alternative (keep the exit only where the field
  isn't a `Choice`) preserves an existing habit, but leaves `←` context-dependent
  — precisely the ambiguity this change exists to remove; it would just fire less
  often.
- **R3. `Esc`.** Focus in the pane → return to the sections; focus on the
  sections → close the screen. *Adopted* (the user's proposal).
- **R4. `Tab`/`BackTab`.** Switch the section and **preserve the current focus**
  (on the sections it stays there; in the pane it stays there). `field_idx` is
  still reset to `0` — field sets differ per section. *Adopted* (the user's
  proposal).
- **R5. `↑` on the first field of the pane.** **Stays put** (as today,
  `saturating_sub`). *Adopted, as recommended.* Returning focus to the menu would
  reintroduce an implicit arrow-driven focus move — the same category as the `→`
  entry being removed.
- **R6. Undoing an accidental edit.** **Out of scope, a separate task.** *Adopted,
  as recommended.* See §7.

---

## 4. Implementation

All changes are in `src/screens/settings/`.

**`apply.rs::handle_key`** — `Esc` becomes focus-aware. It currently sits in the
`match (key.code, key.modifiers)` block *ahead of* the focus dispatch and always
returns `Close`; it stays in that block (so it keeps working from either focus)
but branches:

```rust
(KeyCode::Esc, _) => match self.focus {
    Focus::Fields => { self.focus = Focus::Menu; None }
    Focus::Menu => Some(SettingsIntent::Close),
},
```

**`apply.rs::handle_menu_key`** — `KeyCode::Enter | KeyCode::Right` becomes
`KeyCode::Enter`. `→` falls through to the existing `_ => {}` (a no-op).

**`apply.rs::handle_fields_key`** — the `KeyCode::Left` arm loses its
`self.focus = Focus::Menu` tail: for a `Choice` field it cycles as before,
otherwise it returns `None`. This makes the `Left` and `Right` arms symmetric,
which is the point.

**`apply.rs::move_section`** — drops `self.focus = Focus::Menu;`, keeps
`self.field_idx = 0;`.

Not touched: the initial focus (`catalog.rs`, `Focus::Menu`) and the search jump
(`search.rs::jump_to_selected`, which sets `Focus::Fields`) — "Enter on a result
means go to that field" is exactly right under the new model, and `Esc` then
steps back to the sections.

**Defensive note:** with the pane empty, `Enter` would focus nothing. No current
section produces an empty field set (the "no profiles" branch still returns the
tab strip row), and `Esc` gets the user out regardless, so this is not a trap —
but gating `Enter` on `!self.fields().is_empty()` costs one line and removes the
dead state by construction.

---

## 5. Ripple

### 5.1. The footer must become contextual by focus — this is what teaches the model

Today the hint line is the same in both focus states. Under the new model it has
to differ, otherwise the rules are undiscoverable. The mechanism already exists
(`render.rs` adds `Del` only when the pane is focused).

- **Sections focused:** `Tab`/`↑↓` section · **`Enter` parameters** · `/` search ·
  \[`Ctrl+N`/`Ctrl+D` in "Profiles"\] · **`Esc` close** · `Ctrl+Q` quit
- **Parameters focused:** `↑↓` fields · `←→` choose · `Enter` edit · `Space`
  toggle · `Del` reset · `Tab` section · \[`Ctrl+N`/`Ctrl+D`\] · **`Esc` to
  sections** · `Ctrl+Q` quit

### 5.1a. Where the focus itself is shown

Both panes carry a marker in their title — `▸` on the sections, `◆` on the
parameters — and its **colour** follows the focus: `success` (green) for the pane
that holds it, `muted` otherwise. They are a pair and share one helper
(`helpers::focus_marker_style`), because they encode the same fact from opposite
sides and drifting apart would make the screen lie about where the focus is.

The menu's marker used to be *shown or hidden* instead, which both flickered and
shifted the title text sideways on every focus change; the `◆` didn't track focus
at all. Green is already "you are here" on this screen (the active section's rail
and the selected row's rail), so this reuses an established meaning rather than
adding one.

### 5.2. i18n

New keys in **both** bundles (`locales/ru.json`, `locales/en.json`) — the ru
column is bundle data, not prose (cyrillic-ok:start):

| Key | ru | en |
|---|---|---|
| `ui.settings.hint.enter_fields` | параметры | parameters |
| `ui.settings.hint.close` | закрыть | close |
| `ui.settings.hint.to_sections` | к секциям | to sections |

`ui.settings.hint.back` ("назад"/"back") becomes unreferenced and **must be
deleted** (cyrillic-ok:end) — the `bundle_keys_are_not_dead` gate fails on a dead key. The
`link_check`/`cyrillic_scan`/i18n parity gates cover the rest automatically.

### 5.3. Tests (`tests.rs`)

The test helpers encode the old rules and break under R4:

- **`goto_section` is documented as "after the call, focus is in the menu (Tab
  resets it)"**, and `goto_field` builds on that by pressing `Enter`. With focus
  preserved, that `Enter` would open an editor instead of entering the pane. The
  helper must force menu focus itself (send `Esc` when `focus == Fields`) rather
  than relying on `Tab`'s side effect.
- **`tests.rs:554`** uses `Left` with the comment "back to the menu
  (`goto_field` starts from there)" — becomes `Esc`.
- **`esc_closes`** splits: `Esc` on the sections closes; `Esc` in the pane
  returns to the sections and a second `Esc` closes.

New tests, each pinning one adopted fork (all mutation-testable — reverting the
corresponding line must fail exactly one):

- `right_does_not_enter_the_field_pane` (R1);
- `left_on_a_choice_field_cycles_and_keeps_focus` (R2) — asserts *both* that the
  value still changes and that `focus` stays `Fields`; without the second half a
  revert would pass;
- `left_on_a_text_field_is_a_no_op` (R2);
- `esc_steps_out_of_the_field_pane_then_closes` (R3);
- `tab_preserves_field_focus_and_resets_the_field_index` (R4);
- `footer_hints_differ_by_focus` (§5.1, via `TestBackend`).

### 5.4. Documentation

- **spec §11.6** — a "Navigation" paragraph stating the focus model of §2.
- **CHANGELOG** (`Changed`) — `Esc` no longer closes the screen in one press, and
  `→` no longer enters the field pane. Both are visible behaviour changes to
  muscle memory and belong in the changelog per AGENTS.md §4.
- **CLAUDE.md** — a journal entry.

### 5.5. Live run

**Not required** (AGENTS.md §3): pure UI key handling and rendering — no engine,
memory, tool or provider path is touched. Covered by `handle_key`/`TestBackend`
tests, which is the established precedent for settings-screen work (the whole
settings redesign, stages 1–6).

---

## 6. Why not the alternatives

- **Keep `→` to enter and make `←` always leave.** Impossible: `←` has to cycle a
  `Choice` value backwards, and there are `Choice` fields at the very top of most
  sections (`XMode` in "Model", `ITheme` in "Interface").
- **Move value cycling off the arrows** (to `+`/`-`, or `Space` only). `←`/`→`
  for a `Choice` is a strong convention and the subsection tab strip already uses
  them; changing that would cost far more habit than it buys.
- **Make `Esc` in the pane close the screen but warn first.** Trades a navigation
  problem for a modal one, and doesn't stop the accidental edit that precedes it.

---

## 7. Out of scope (follow-up)

**An accidental edit is saved immediately and cannot be undone from the UI.**
This change removes the main *source* of accidental edits, not the consequence.
The screen applies every field commit at once (`SaveConfig`/`SaveProfile`), and
the existing `•` marker means "differs from the **default**", not "I just touched
this" — so it does not answer "what did I change?".

Two candidate directions for the separate task, in increasing cost:

1. A distinct marker for fields changed **since the screen was opened** (needs a
   snapshot of the config taken on open — cheap, and directly answers the
   question the user asks after the mistake).
2. Undo (`Ctrl+Z`) on the settings screen — needs an edit stack plus interaction
   with the server-restart debounce and with profile edits, which travel as a
   different intent (`SaveProfile`).
