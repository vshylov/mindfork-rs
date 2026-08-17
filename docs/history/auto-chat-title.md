# Automatic chat titling

Status: design accepted and shipped 2026-08-17 (user's decisions recorded
below); single stage, one PR.

The model-generated chat title exists today as a **user-triggered** chat-list
operation (`Ctrl+R` in the list, journal
[ui-screens.md](../journal/ui-screens.md)): the model reads the conversation and
proposes a short title, applied via `AppEvent::ChatRenamed`. This track makes it
fire **by itself** for a new conversation, so a chat stops sitting in the list
as "New chat" until the user thinks of visiting the list. The user's
observation driving the default: cloud chat UIs title on the user's first
message, and the names are visibly worse than what the same model produces
*after* the first reply — the reply is what disambiguates a terse opening
("take a look at this" names nothing; the assistant's answer names the
subject).

## 1. What exists (survey)

- **The background task is already right.** `AppCommand::AutoRenameChat(id)` →
  `Orchestrator::handle_auto_rename` ([title.rs](../../src/app/orchestrator/title.rs)):
  digest via `features::rename_chat::build_conversation_digest` (roles tagged in
  the profile's axis-A locale, middle truncated to 4 000 chars), one
  single-turn request with no history/tools, thinking muted through three
  layers with a salvage fallback, 60 s timeout, result through `title_tx` →
  `handle_title_result` → `clean_generated_title` → `chat.title` +
  `ChatRenamed`. Errors go to `AppEvent::ChatListError` — correct for an
  operation the user just requested while looking at the list.
- **Manual renames** are two routes into one handler: the list's `F2` editor
  and typed `/rename <title>` → `AppCommand::RenameChat` →
  `handle_rename`. (`/rename` bare hands the current title back for editing —
  still the manual route.)
- **A new chat's title** is the localized `defaults.chat_title`; a profile
  greeting may already sit in `messages` as a leading assistant message.
- **The end-of-turn hook point exists.** `handle_done` (generation.rs) already
  runs four `maybe_auto_*` follow-ups (reflection, two consolidations,
  compaction) after applying a turn's messages — the "after the first reply"
  trigger is a fifth sibling. The "after the user message" variant hooks
  `handle_send`, which is where the first user message is appended.
- **Failure-visibility precedent** (spec §6.8): "Background turns (auto-title,
  reflection) log the reason instead" — the automatic trigger inherits this
  rule; nothing new to invent.

## 2. Decisions fixed by the user (2026-08-17)

1. Titling runs **fully automatically**; the manual chat-list action stays.
2. **Switchable in settings, on by default.**
3. A **when-to-run choice**: after the user's first message / after the
   assistant's first reply. Default — **after the assistant's reply**; the
   "after the user's message" option is listed **first**.
4. Chats **renamed manually are never auto-retitled** — tracked by a flag on
   the chat entity.

## 3. Design

### 3.1 The flag: `Chat.renamed_manually`

`renamed_manually: bool` on `Chat`,
`#[serde(default, skip_serializing_if = "std::ops::Not::not")]` — additive
(ADR 0006 F12: old chat files read without migration, an untouched chat writes
no new key; the `Message.new_bubble` serde shape).

- **Set** in `handle_rename` — both manual routes funnel there.
- **Never set** by model-generated titles — neither the user-requested
  chat-list action nor the automatic trigger (sub-decision D1): a model title
  is not a user's choice, and "locking" on it would make the two model routes
  behave differently for no observable reason.
- **Checked twice**: at trigger time, and again at **apply time** in
  `handle_title_result` for automatic results — the title task takes seconds,
  and a user who renames while it runs must win the race. A requested
  (chat-list) result keeps today's last-write-wins: the user explicitly asked
  for that title moments ago.

### 3.2 The setting: one tri-state field (fork F1)

`interface.auto_title: AutoTitleMode`, shown as a single Choice row in
Interface → Behavior (next to `confirm_destructive_keys`):

```rust
#[serde(rename_all = "snake_case")]
pub enum AutoTitleMode {
    AfterUserMessage,          // listed first (user's decision #3)
    #[default]
    AfterAssistantReply,       // the default
    Off,
}
```

- **F1 options considered**: (a) one tri-state enum row; (b) a Toggle
  (`enabled`, default on) plus a separate two-value Choice (trigger). The
  user's phrasing literally describes (b); (a) is the `clipboard_osc52`
  precedent — a behavior whose "when" question folds Off into one enum, one
  row, no dead state (off + a trigger picked), one label/hint to localize and
  fit. **Recommendation: (a)**; the option order
  [after user message, after assistant reply, off] keeps "after the user's
  message" first and Off last, exactly like `OSC52_MODES`.
  **User's decision: (a) tri-state**, 2026-08-17.
- Home: `InterfaceSettings` — the Behavior group already holds conduct-of-the-app
  toggles; no new config section for one field. New `FieldId::IAutoTitle`,
  `cycle`/label helpers mirroring `cycle_osc52`/`osc52_label`, i18n keys
  (`ui.settings.field.auto_title`, `.desc.`, three `.choice.` labels) in `en` +
  `ru`; the settings-hint fit gate covers the new hint automatically.
- Nothing restarts on change: the trigger reads live config; `UpdateConfig`
  needs no new handling.

### 3.3 Trigger mechanics

Two thin call sites, one pure predicate module-mate in
`features/rename_chat.rs`, one shared task starter.

- **`AfterAssistantReply`** — in `handle_done`, after the turn's messages are
  applied (a fifth `maybe_auto_*`). Fires when **this turn delivered the
  chat's first substantive reply**: before the apply, no assistant message
  with non-empty text followed a user message
  (`has_assistant_reply(&chat.messages)` — a greeting doesn't count, nothing
  precedes a user message); the applied turn contains one; the flag is unset.
  Notably this is *not* "user messages == 1": a first turn that was cancelled
  before any text, or errored, leaves the chat unreplied — the title then
  fires on whichever later turn actually produces the first reply, and
  **existing pre-feature chats can never match** (they already have replies),
  so nothing mass-retitles on upgrade.
  - Corollary (D2): **regenerating the first reply re-fires** — the regenerate
    truncated the only reply, so the next one is again "the first", and the
    title follows what the conversation actually says. Guarded by the flag as
    everywhere.
- **`AfterUserMessage`** — in `handle_send`, when the message just appended is
  the chat's **first user message** (`is_first_user_message`). Fired **after**
  `start_generation` (D3): both requests then race to the same server, and on
  a managed/local `llama-server` with one slot the title request would
  otherwise queue *ahead of the reply* — firing second makes the reply
  request win the race in practice. This contention (and a long first reply
  starving the title task's 60 s timeout on a single-slot server) is the
  mechanical reason the cloud-UI ordering is not our default; the wording of
  the setting stays neutral, the spec records the caveat.
  - A regenerate does not re-fire here (no new user message) — asymmetric
    with D2 and correct: this mode's title never depended on the reply.
- **Failure policy (D4)**: automatic runs are **quiet** — reasons go to the
  log, per the spec §6.8 rule for background turns; no `ChatListError`, no
  feed note. The requested chat-list path keeps its loud reporting untouched.
  Mechanically: `TitleResult` gains an origin (`Requested`/`Auto`);
  `handle_auto_rename` and the trigger share one `start_title_task(id, origin)`.
- No new `AppCommand`/`AppEvent`: the trigger is orchestrator-internal, the
  result rides the existing `title_tx` → `ChatRenamed` path.

### 3.4 Edge cases

| Case | Behavior |
|---|---|
| Profile greeting present | Greeting is an assistant message *before* any user message — `has_assistant_reply` ignores it; the digest still includes it (context helps the title). |
| First send is image-only (empty text) | AfterUser: digest may be empty (greetingless chat) → `build_conversation_digest` returns `None` → quiet skip. AfterAssistant unaffected. |
| First turn cancelled with partial text | Partial text is a substantive reply → fires; the digest carries user message + fragment, which names the topic. |
| Manual rename while the title task runs | Apply-time flag check drops the automatic result (D1); a requested result still applies. |
| Chat imported/cloned with history | `has_assistant_reply` is already true → never fires; titles from elsewhere survive. |
| Demo mode (`mindfork demo`) | A fresh chat's first exchange consumes one scripted reply as the title; the demo script is explicitly self-contained against out-of-turn consumption (demo.rs), and seeded chats have history → no fire. |
| Server not ready at fire time | AfterUser: impossible in practice (`handle_send` already gated on `ready_backend`). AfterAssistant: the engine just streamed; a mid-turn settings swap loses the race → quiet log. |
| Both triggers racing twice (regenerate quickly after first title) | Two model titles, last write wins — harmless, no dedup machinery. |

### 3.5 Tests

- `entities::chat` — flag is additive: old JSON reads `false`, `false` writes
  no key, `true` round-trips.
- `features::rename_chat` — `has_assistant_reply` / `is_first_user_message`
  tables: greeting-only, tool/system messages, empty-text replies.
- Orchestrator (`tests/title.rs`): manual rename sets the flag, a generated
  title does not; AfterAssistant fires on the first reply and **not** on the
  second exchange (the first fire is the positive control for the absence
  assertion, lessons §2); Off and flagged chats never fire; AfterUser fires on
  send; regenerated first reply re-fires; an automatic result after a manual
  rename is dropped; automatic errors emit no `ChatListError` while requested
  ones still do (same test, both arms).
- Settings screen: row + hint present, cycle walks
  after_user → after_assistant → off → back (the `osc52` test shape); config
  default is `AfterAssistantReply`; an old `settings.json` without the key
  reads as the default.
- **Live smoke** (`#[ignore]`): fresh chat against the real stack, one short
  send, assert `ChatRenamed` arrives with a non-default title — the trigger
  end-to-end, not just the task the manual path already covers.

### 3.6 Documentation

spec §11.2 (the auto-title paragraph + the automatic trigger and its setting)
and §11.6 (Interface field list); README (features/settings); CHANGELOG
(Added); journal `ui-screens.md` entry + CLAUDE.md status header;
architecture.md's `title.rs` map line gains "+ automatic trigger". Roadmap: no
open item corresponds — nothing to close.

## 4. Out of scope

- Retitling an ongoing conversation later (periodically, or on a topic
  shift) — the manual chat-list action covers it.
- A per-chat opt-out UI beyond the implicit "rename it manually".
- Title style/length knobs — `prompt.title.system` stays as is.
