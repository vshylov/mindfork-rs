# Axis B multilingualism — interface language (UI chrome)

> **Status: FULLY IMPLEMENTED** (branch `feat/i18n-ui`, 2026-07-13). All UI chrome is
> localized (`ui.*` keys in `locales/{ru,en}.json`), the setting
> `config.interface.language` + an "Interface language" field in settings, wiring a
> `&'static Locale` mirrors `Palette`. **The groundwork item is closed** (§7): tool
> labels (`ui.tool.label.*`/`ui.tool.group.*`, resolved in the settings layer via
> `Locale::get`) and displayed supervisor reasons (`ui.err.server.*`, threading `loc`
> into the `ServerSupervisor` trait) are localized. Only technical engine probe errors
> remain (`shared/api/managed.rs` — a provider layer, not UI). Outcome and decisions —
> in the CLAUDE.md journal ("axis B" + "closing the groundwork item").
>
> Below — the original design plan (decision points confirmed 2026-07-13).
> Continues [docs/history/i18n.md](i18n.md): axis A (**agent** language — prompts and
> tools) is complete (Tiers 1–2, all 35 tools). Axis B — **interface** language
> (text the *human* reads): the status bar, settings screen, help, chat list, feed
> role headers, the "self-model" screen, banners, UI errors, conversation export
> (`F5`). Mechanism — the same `shared/i18n` (`ui.*` keys), but a **new global
> setting** `config.interface.language`, **independent** of the agent language
> (docs/history/i18n.md §3.2: a Russian UI + English-speaking agents is a legitimate
> combination).

## 1. Motivation and scope

Axis A localized text for the model. But **all UI chrome is still Russian**: settings
field labels, the status bar, `F1` help, feed role headers ("✦ ASSISTANT"/"❯ YOU" in
their previous Russian form), RAG banners, orchestrator error text. A user with an
English-language install needs an English interface — while the agent language stays
**independent** of the UI language (docs/history/i18n.md §3.2, decision 2026-07-12):
one global interface language, no inheritance between axes.

**The axes intersect in one place** (docs/history/i18n.md §1): tool results and the
"self-model" render are seen by the human too (tool cards in the feed, the `F3`
screen). The principle already in force: this text follows the **agent** language
(axis A, done). Axis B only touches the **chrome around it** — role headers, the
"thoughts" pill, `F3` section labels, the status bar — not the model's own data.

## 2. Decision points (confirmed by the user 2026-07-13)

1. **Feed role headers** ("ASSISTANT"/"YOU", hardcoded in `message_feed.rs`, the feed
   doesn't use the profile's `CharacterNames`). Options: (a) `ui.*` keys (UI
   language); (b) from the profile's `CharacterNames`. **Decision: (a)** — headers are
   UI chrome, they follow the interface language. Consistent with the rest of the UI;
   doesn't mix axes (`CharacterNames` — data in the agent's language).

2. **English plurals** ("1 line" vs "2 lines"). Options: (a) number-neutral wording +
   plain `tf()`; (b) a mini-pluralization (2 forms). **Decision: (a)** — a format like
   "tokens: N", "found: N chats", "N str." without grammatical plural. Consistent with
   the current Russian wording ("tokens: N"), needs no mechanism extension. Honest
   pluralization — future work (revisit `fluent` if needed, docs/history/i18n.md §7).

3. **Default UI language** (first launch / an old `settings.json` without the field).
   Options: (a) from `defaults.json` (`default_language`); (b) always `Ru`. **Decision:
   (a)** — the installer's `default_language` (the same one used for the first profile,
   docs/history/i18n.md §3.2) also sets the UI language on a fresh install. The
   installer sets both at once; afterward the UI language is editable and independent
   of the profile language. An old `settings.json` (field absent) → `Ru` via the serde
   default (correct — existing users already saw a Russian UI).

4. **Track scope.** Options: (a) tiered by area; (b) one PR. **Decision: (b)** — all of
   axis B in one PR. Pure UI (tests on `TestBackend`, no live model needed), a large
   but self-contained diff. Internally — ordered steps (§5), but one branch/one PR.

## 3. Architecture

### 3.1 Mechanism — reuse `shared/i18n`

`ui.*` keys in the same `locales/{ru,en}.json` bundles. The `Locale`/`t`/`tf` type,
`Lang`, the gate tests (key/placeholder parity, `en_bundle_has_no_cyrillic`) — no
changes. No pluralization introduced (decision point 2 → number-neutral wording).

### 3.2 New setting `config.interface.language: Lang`

- Field `InterfaceSettings.language: Lang` (`#[serde(default)]` → an old
  `settings.json` = `Ru`). **A concrete value**, not `Option` — consumers
  (screens/widgets) read it directly, like `theme`.
- **Reuse `i18n::Lang`, no new `UiLanguage`**: the UI language is literally "which
  bundle to read `ui.*` keys from", and the bundle is the same
  `locales/{ru,en}.json` file. A separate enum would need a `UiLanguage → Lang`
  mapping with no payoff. Axis confusion is resolved by **naming the field**
  (`interface.language` — UI language; `Profile.language` — agent language), not by
  splitting the type. Both axes resolve through one `locale(lang)`.
- **Default from `defaults.json`** (decision point 3): on a **fresh install** (no
  `settings.json`), `main.rs` sets `config.interface.language = paths.default_language()`
  before handing it to the orchestrator. Freshness is determined by
  `paths.settings_file().exists()` before `load_config`. An existing `settings.json`
  without the field → `Ru` (untouched — the user already saw a Russian UI).
  Idempotent: `default_language` from `defaults.json` is stable across launches, so
  even without an immediate persist the UI language is restored to the same value.
- **Independence from the agent language**: `interface.language` governs **only** UI
  chrome; `Profile.language` (axis A) governs the agent's scaffold. No overlap
  (docs/history/i18n.md §3.2).
- Editable in the "Interface" section of the settings screen (a Choice field
  "Interface language" from `Lang::ALL`, labels via `Lang::label()`), applies live
  (like the theme).

### 3.3 Wiring the UI locale (mirrors `Palette`)

`Palette` already reaches every screen/widget; axis B travels the **same path**. The
locale `&'static Locale` (a cheap reference) is stored/passed alongside the palette.

| Consumer | How it gets the UI locale |
|---|---|
| Screens (`ChatScreen`/`SettingsScreen`/`ChatListScreen`/`SelfModelScreen`) | a `loc: &'static Locale` field, set in `set_settings`/`refresh` from `locale(config.interface.language)` (alongside recomputing `palette`) |
| Widgets (`status_bar`/`chat_list`/`message_feed`/`emoji_picker`/`profile_list`/`impersonation_preview`) | as a render parameter (alongside `palette`), or from the owning screen |
| Broadcast to open overlay screens | `ActiveScreen::set_palette` extended to `set_ui_context(palette, loc)` (or a second method `set_locale`) — the `Settings` event already carries `config` |
| `chat_export.rs` (`F5`) | the orchestrator (the owner of `Chat`) resolves `locale(config.interface.language)` and passes it to `format_conversation` |
| Orchestrator errors/notices in the UI (`AppEvent::Error`, banners, notices) | the orchestrator builds them via `locale(config.interface.language)` |

**A nuance**: today the `set_palette` broadcast (`runtime`) recomputes the palette from
`config.interface.{theme,compat}`. The UI locale joins the same broadcast — either
`set_palette` → `set_ui_ctx(palette, loc)`, or a parallel `set_locale(loc)`. The
settings screen updates via `refresh` (broader than the palette) — the UI locale is
recomputed there from the fresh config too.

### 3.4 What is NOT localized (deliberately, axis B)

- **Model data** (notes, "self-model", tool results) — axis A, follow the agent
  language (the intersection in §1).
- **Tool ids, config keys, `/rag …` command names** — not UI text.
- **Logs** (`tracing`) and **comments/docs** — the "Russian" convention (CLAUDE.md).
- **The profile's `CharacterNames`** ("You/Assistant/System") — profile data (axis A);
  feed headers come from `ui.*` (decision point 1), not from these.

## 4. UI-string inventory (areas and estimate)

Refined by an exhaustive grep pass (2026-07-13, excluding test modules and
`tracing`/`expect`): **~365 live UI literals** (i18n.md §2.2 estimated ~600–800, but
that count included tests).

| Area | Files | ~count | Nature / pitfalls |
|---|---|---|---|
| Settings screen | `screens/settings/{mod,catalog,helpers,render}.rs` | ~200 | sections (6) + groups (~17) + field labels (~60) + long descriptions (`describe`/`DESC_*`/`SamplingParam::description`, ~50) + footer hotkeys + Choice labels (theme/language/modes) + `format!`-built chip statuses. `choice/search/apply.rs` — no UI literals (logic). **Value-column alignment by label width** — en is a different length: run the gate `all_labels_fit_alignment_cap`/`section_label_col`/`LABEL_CAP` against en |
| Status bar | `widgets/status_bar.rs` | ~14 | `HOTKEYS` (5), "chat/emb/imp" chips + `{why}`, "tokens:/(reasoning)", "{busy} generating…", "mouse: scroll/select". **The hotkey grid lays out by visible width** — check en |
| Chat list | `widgets/chat_list.rs` | ~16 | "Chats", "{n} dialog(s)", "Rename", the search placeholder, "{n} msg", "sort:", hotkeys (10) |
| Popups/help | `screens/chat/popups.rs` (`HELP_KEYS` ~30, footers/confirm), `widgets/{emoji_picker,profile_list,impersonation_preview}.rs` (2 each) | ~46 | the hotkey table — **don't break combos/commands inside descriptions** ("/rag add <path> [-r]", "undo — Ctrl+Z") |
| Feed role headers | `widgets/message_feed.rs` | ~6 | "YOU"/"ASSISTANT" (decision point 1 → `ui.*`), the "thinking"/"· N para. ·" pill, the empty state. `model_meta` ("· Nk ctx") — ASCII; tool cards — axis A (untouched) |
| "Self-model" screen | `screens/self_model.rs` | ~12 | "About me:"/"Interlocutor"/"Traits:"/… (chrome), "add goal", hotkeys, the clear confirmation. Goal statuses are glyphs (not text); **the model's data is axis A** |
| Chat notices/banners | `screens/chat/{rag,mod,render,feed}.rs` | ~20 | RAG banners (`format!` with numbers), regenerate/delete confirmation, the input placeholder, the input footer, "(generation cancelled)", `background_hint` ("reflection · notes sleep") |
| Conversation export | `features/chat_export.rs` | ~6 | "User:"/"Assistant:"/"[Thoughts]"/"[Tool:]"/"Arguments:"/"Result:" (`F5`, plain text) |
| UI errors/notices | `app/orchestrator/{rag,profiles,title,chats,impersonation,engines,background,mod}.rs`, `supervisor.rs`, `app/runtime/{input,dispatch}.rs` | ~45 | `AppEvent::Error`/`push_note`/`set_notice`/`RagProgress::Failed`/`ServerStatus::Disconnected` — visible to the human. A nested `{err}`/`{reason}` from anyhow/the system is **not translated** — only the wrapper is translated |

**Border-case text (axis A, NOT axis B):** `tool_loop.rs`/`generation.rs` "Tool error
{name}", "Tool unavailable", "Round limit reached…" go out into the feed as tool/
assistant text and are **seen by the model** → already localized under axis A (`loop.*`
keys, Tier 2c). "exit code:" in the python card (`message_feed.rs:727`) — pairs with
the `python.console.exit` label (axis A, Tier 2c); the feed only displays it.

## 5. Work plan (one PR, ordered steps)

1. **Infrastructure**: `InterfaceSettings.language: Lang` + a default from
   `defaults.json` in `main.rs` + a Choice field "Interface language" in the
   "Interface" section of settings. Wiring `&'static Locale` into screens/widgets
   (a field + `set_*`, a `runtime` broadcast). `ui.*` keys in the bundles as areas are
   translated.
2. **Widgets**: the status bar, the feed (role headers + the pill + tool chrome), the
   chat list, popups/help, emoji/profile/impersonation.
3. **Screens**: settings (the largest area — labels/groups/sections/Choice/footer/
   search; check alignment in en), "self-model", chat notices/banners.
4. **Orchestrator/features**: `chat_export`, UI error/notice text.
5. **Tests** (along the way, `TestBackend`): per-locale structural (rendering under
   every `Lang::ALL` — key screens with no panic, headers from that language's
   bundle); gates key/placeholder parity + `en_bundle_has_no_cyrillic` cover the
   completeness of `ui.*`; substantive pins of key phrases (docs/history/i18n.md
   §3.5); a settings-alignment check in en. Existing ru assertions → the reference
   `ru()` locale.

## 6. Risks

1. **Widths/alignment** depend on translation length: the status bar's hotkey grid,
   the settings value column (`section_label_col`/`LABEL_CAP`), label truncation.
   Mitigation — per-locale render tests + the `all_labels_fit_alignment_cap` gate in
   en.
2. **Test churn**: hundreds of UI assertions on Russian substrings → bundle keys + a
   loop over locales (docs/history/i18n.md §3.5). Mechanical but voluminous; the ru
   bundle is extracted verbatim (ru-UI behavior doesn't change).
3. **Compat mode** (`terminal_compat`): glyphs are already separated (`GlyphSet`) from
   text — text localization and glyph substitution are orthogonal; check that en
   labels also fit in the compat set.
4. **`defaults.json` default**: freshness is determined by the presence of
   `settings.json` — the only nontrivial logic, localized in `main.rs`.

## 7. Out of scope / future work

Pluralization/grammar (decision point 2 → number-neutral), RTL, custom keyboard
layouts, external `data/locales` (axis A Tier 3), translating documentation/logs.

**The groundwork item is CLOSED** (same PR, see the CLAUDE.md journal "axis B —
closing the groundwork item"):
- **Tool labels in profile toggles** — localized by resolving in the settings layer:
  `Locale::get(&'static self, key)` (a dynamic key → `&'static str`),
  `ToolGroup::i18n_key()` (a stable ASCII key, `title()` — a Russian fallback), the
  bundle `ui.tool.label.*` (32) + `ui.tool.group.*` (8). Gate test
  `ui_label_and_group_keys_exist_in_all_bundles`.
- **`ServerStatus::Disconnected(...)` reasons** — threaded `loc` into the trait
  `ServerSupervisor::apply_chat`/`apply_impersonation` → `cloud_chat_setup`;
  `resolve_api_key` → a structured `ApiKeyError`; bundle `ui.err.server.*` (3).

**Remaining (genuinely out of scope):** technical engine probe errors
(`shared/api/managed.rs`: a corrupt GGUF, an early process exit) — a provider layer,
doesn't and shouldn't have `config.interface.language`; more like log-style technical
messages. Axis A Tier 3 (external `data/locales`).

## 8. Live run

**Not needed** (AGENTS.md §3): pure UI, tests on `TestBackend`. A manual check (a
look at the en interface on a real terminal) — a final, optional step.
