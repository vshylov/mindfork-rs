# Undoing an edit on the settings screen

A design plan for `Ctrl+Z`/`Ctrl+Y` on the settings screen (spec
[§11.6](../../spec.md#116-the-settings-screen)). Scope — one screen
(`src/screens/settings/`), one PR.

This is the follow-up deferred as **fork R6** of the focus-model track
([settings-navigation.md §7](settings-navigation.md)): that change removed the
main *source* of accidental edits, this one removes the *consequence*. Forks are
recorded in §3 (**user's decision, 2026-07-31**).

The plan is filed straight into `docs/history/` rather than `docs/` first: the
track completes within this single PR, so this is already its end state
(AGENTS.md §4 archives a finished plan here anyway).

---

## 1. The problem

The screen applies every field commit **immediately** — each one emits
`SaveConfig`/`SaveProfile`, the orchestrator writes `settings.json`, and an
engine field additionally restarts the server after the debounce. There is no way
back from the UI.

The existing **`•` marker doesn't answer the question**: it means "differs from
the **default**", not "I just touched this". A field the user deliberately set
months ago is marked exactly like the one they nudged by accident a second ago.

---

## 2. What makes this cheap: the contract already carries whole snapshots

- **`SettingsIntent::SaveConfig(Box<AppConfig>)` sends the entire working config**,
  and `handle_update_config` replaces `self.config` wholesale.
- **`SettingsIntent::SaveProfile { id, edit }`** carries `ProfileEdit` — a full
  snapshot of the profile's editable fields (name, system message, greeting,
  character names, sampling, tools, language, impersonation reference).

So "undo" is expressible as *restore the previous snapshot into the working copy
and emit the same intent again*. **No new `AppCommand`, no new `AppEvent`, no
orchestrator change.**

Three config fields are owned by the orchestrator and are **restored by it** on
every `UpdateConfig` — `last_active_chat`, `api_keys`, and the MCP TOFU pins
(inherited by server id). So restoring a whole older `AppConfig` cannot clobber
them; the trap is already closed in `handle_update_config`.

### 2.1. What cannot be undone, and why

| Action | Why it's excluded |
|---|---|
| `SetApiKey` | The screen never holds the key — not even while it is being typed (the editor opens empty and masked). There is nothing to restore. |
| `CreateProfile` / `DeleteProfile` | The assistant-profile list is owned by the orchestrator, and deletion is a **soft-delete cascade over the profile's chats**. Reversing it needs an unhide command, not a re-emit. |
| `ConfirmMcpCatalog` | A trust decision (TOFU), not a value. Silently reversing it would be a security regression. |

All three are deliberate actions behind their own keys, not the stray-arrow class
this feature exists for.

**A consequence worth naming:** impersonation personas live *inside*
`AppConfig.impersonation_profiles`, so their `Ctrl+N`/`Ctrl+D` **is** a config
edit and therefore *is* undoable — unlike assistant profiles. The asymmetry
follows from where each list is stored, not from a decision; the rule stays
simple ("anything that is a config edit is undoable").

---

## 3. Forks (user's decision, 2026-07-31)

- **U1. Scope.** Undo only — **no** "changed in this visit" marker. *Adopted, as
  recommended.* Undo answers the user's actual question directly ("take it
  back"), and a third marker meaning would crowd the row, which already carries
  `•` (differs from default) and the gate warning.
- **U2. Granularity.** **Coalesce consecutive edits to the same field** into one
  step. *Adopted, as recommended.* Cycling `managed → external → openai` and
  pressing `Ctrl+Z` returns to `managed` in one press; it mirrors `InputBox`'s
  own snapshot coalescing and produces fewer saves (and server restarts) when
  undoing.
- **U3. Redo.** **Yes, `Ctrl+Y`.** *Adopted, as recommended.* The field editor on
  this very screen already binds `Ctrl+Z`/`Ctrl+Y` (`InputBox`), so undo without
  redo would make the same two keys behave differently depending on whether an
  editor happens to be open.
- **U4. After an undo.** **Jump to the affected field** (switching section and
  subsection if needed). *Adopted, as recommended.* Otherwise undoing a change
  made in another section is invisible — and "show me what I changed" is the
  request behind the whole task.
- **U5. Lifetime.** The stack lives on `SettingsScreen`, so it covers **one
  visit**: the screen is constructed fresh on every `Ctrl+P`
  (`runtime/dispatch.rs`). *Decided in prose, not offered as a fork* — persisting
  it would mean moving the stack into the orchestrator and inventing commands for
  it, and once the user has left the screen the accidental-edit moment has passed.

---

## 4. Implementation

All changes are in `src/screens/settings/` (plus one i18n key pair).

### 4.1. The step

```rust
/// The value as it was *before* one edit — the mirror of the intent that edit produced.
enum EditValue { Config(Box<AppConfig>), Profile(Box<Profile>) }

struct EditStep {
    before: EditValue,
    /// The field the edit acted on — the coalescing key (U2). `None` for edits with
    /// no focused field (persona `Ctrl+N`/`Ctrl+D`), which therefore never coalesce.
    field: Option<FieldId>,
}
```

Each edit touches exactly one of the two stores (`save_config()` **or**
`save_profile()`, never both), so a step restores exactly what that edit changed.

### 4.2. Recording: one funnel, not nine call sites

Config and profile mutations happen at ~9 places (`toggle_field`, `cycle_field`,
`apply_text`, `reset_field`, `apply_choice`, `toggle_profile_tool`,
`apply_profile_text`, persona create/delete). Sprinkling `push_undo()` across all
of them is shotgun surgery and easy to forget when a field type is added later.

Instead, wrap the single entry point:

```rust
pub fn handle_key(&mut self, key) -> Option<SettingsIntent> {
    let before = self.pre_edit_snapshot(&key);   // None for keys that cannot edit
    let intent = self.handle_key_inner(key);
    self.record_edit(before, &intent);           // keeps it only if an edit happened
    intent
}
```

- `pre_edit_snapshot` returns `None` unless the key **could** commit an edit
  (`Enter`, `Space`, `Del`, `←`, `→`, `Ctrl+N`, `Ctrl+D`) — so typing inside a
  text editor doesn't clone anything per keystroke.
- The transient snapshot holds both stores; `record_edit` keeps only the half the
  returned intent names (`SaveConfig` → the config; `SaveProfile{id}` → that one
  profile) and drops the rest, so a **stored** step stays small.
- Any other intent (`SetApiKey`, `CreateProfile`, `DeleteProfile`,
  `ConfirmMcpCatalog`, `Close`, `Quit`) or `None` → nothing is recorded. §2.1
  falls out of this by construction rather than needing its own guard.

The coalescing key is the field the key would act on: `editor.field` when an
editor is open, otherwise the focused row's id.

### 4.3. Undo / redo

`Ctrl+Z`/`Ctrl+Y` are matched by the **physical** key (`keys::hotkey_char`, so
they work under any layout) and are placed **after** the editor/search/choice
branches in `handle_key` — an open editor keeps them for its own text undo, which
is the existing behaviour and must not change.

Undo pops a step, pushes the mirror of the current state onto the redo stack,
restores the working copy and returns the corresponding intent (`save_config()`
or a by-id `save_profile_for(idx)`). A profile that no longer exists → the step
is dropped. Any new edit clears the redo stack. Cap: `UNDO_CAP = 50`.

### 4.4. Jumping to the affected field (U4)

The field-search index already enumerates **every** field of every section and
subsection together with its rendered value (`build_search_index`), and
`jump_to_selected` already knows how to move section + subsection + field index.
So: snapshot the index before the restore, rebuild it after, and jump to the
first field that **exists in both and whose value differs**.

`SearchHit` gains an `id: FieldId` for this — positional comparison would be
wrong precisely in the interesting case: changing an engine mode changes *which*
fields are visible, so the two indexes have different lengths and different
contents. Fields that only appear or disappear are ignored (they're a consequence
of the change, not the change), which leaves the mode field itself as the match.
Nothing found → stay put.

`jump_to_selected`'s body is extracted into a reusable
`jump_to(section_idx, subsection, field_idx)` used by both callers.

---

## 5. Ripple

- **Footer**: one entry `Ctrl+Z/Y` → "undo/redo" in both focus states (a new
  i18n key `ui.settings.hint.undo` in both bundles). The settings screen isn't
  listed in the `F1` help overlay, so the footer is the only place these keys can
  be discovered.
- **spec §11.6** — a sentence in the "Navigation"/"Field editing" area.
- **CHANGELOG** (`Added`) + a CLAUDE.md journal entry.
- **Live run not required** (AGENTS.md §3): key handling and screen state — no
  engine, memory, tool or provider path is touched. The established precedent for
  settings-screen work.

### 5.1. Known consequence: one wasted restart

`handle_update_config` marks a server restart by diffing the incoming config
against the previous one. An engine edit followed by its undo therefore marks the
restart twice; the debounce coalesces them into **one** restart that reloads the
server with the values it already had. Correct, merely wasteful, and only when
the undone field was an engine field. Avoiding it would mean diffing against the
*last applied* config rather than the previous one — a separate change in the
orchestrator, deliberately not bundled here.

---

## 6. Tests

Behaviour (`handle_key`-level), each mutation-tested:

- undo restores a config value and emits `SaveConfig`; undo restores a profile
  field and emits `SaveProfile`;
- three consecutive cycles of one field undo in **one** press (U2), while edits to
  two different fields stay two steps;
- redo re-applies, and a fresh edit clears the redo stack (U3);
- undo on an empty stack is a no-op returning `None`;
- `Ctrl+Z` **inside the field editor** goes to `InputBox` (text undo) and leaves
  the settings stack alone — the layering that must not regress;
- an API-key commit records **no** step (§2.1);
- undoing a change made in another section switches section and field (U4);
- the cap evicts the oldest step.
