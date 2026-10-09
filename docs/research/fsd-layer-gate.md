# The FSD layer rule — what the layers really import, and a gate that keeps it

**Status:** decided 2026-10-09 (the owner's decision rule, §3); implemented in
`refactor/fsd-layer-gate`.

[docs/architecture.md](../architecture.md) §2 states the dependency rule —
`app → screens → widgets → features → entities → shared`, *strictly downward* —
and calls one consequence **the key FSD invariant in the code**: `screens` /
`widgets` **never import `app`**. [spec.md](../../spec.md) §4 and
[AGENTS.md](../../AGENTS.md) §3 say the same. This document measures the claim,
spells out the forks, and records the decision and its design.

## 1. The claim, measured

Measured on `main` at `f5599770` (2026-10-09) with a search for
`crate::<layer>::` in `src/`, by the layer of the file naming it, with mentions
inside `//` comments set aside (a rustdoc link is prose, not a dependency).

| Direction | Production code | Test code only | Verdict |
|---|---|---|---|
| `screens → app` | **5 sites**: `screens/tasks.rs:27`; `screens/chat/mod.rs:23`, `:1065`; `screens/chat/feed.rs:494`, `:504` | `screens/tasks.rs:723`; `screens/chat/tests.rs`, 20 lines | **violation** |
| `widgets → app`, `widgets → screens` | none (two rustdoc links) | none | clean |
| `features → app/screens/widgets` | none (rustdoc links) | none | clean |
| `shared → features` | none | 6 sites, all inside `#[cfg(test)]` modules of `shared/api/openai/{client,wire}.rs` | clean in production |
| `shared → entities` | **storage** (`json`, `db/*`, `schema`, `chat_steps` store `Chat`, `Profile`, `Note`, `RagDocument`, `SelfModel`, `AttachmentChunk`, `LlmChange`); **api** (`SamplingConfig`/`ReasoningEffort` in `contract.rs` and every provider's `wire.rs`); `config.rs` (`SamplingConfig`) — 45 non-comment sites in 31 files | — | **contradicts the chain as written** |
| `entities → shared` | 18 sites: `shared::config::{ServerMode, CloudProvider, AttachmentSettings, SelfModelSettings, NoteOrder}`, `shared::i18n::{Lang, Locale}`, `shared::tokens`, `shared::storage::schema::CHAT_SCHEMA` | — | as documented |
| `main.rs → app` | the crate root | — | above the layers |

Two findings.

**`screens → app` is real, and only event payload types leak** — never
`AppCommand`, `AppEvent` or the orchestrator. The set, closed under what the
leaked types reference: `AppTask`, `BackgroundKind`, `RunProgressKind`,
`TaskList`, `TaskRun`, `TASK_LANDED_CAP`, `SubagentProgress`, `ChildView`, and —
missed by the opening survey because they are named by path rather than
imported — `LiveTurn`, `LivePartial`, `LiveTool`. All eleven are plain data:
their only dependencies are `uuid`, `chrono`, `entities::message::MessageRole`
and `entities::subagent::{RunKind, RunOutcome}`; the one method,
`BackgroundKind::label_key`, returns a bundle key; `TaskRun::fixture` is
`#[cfg(test)]`. The first of them landed on 2026-08-23 (`80797130`, the
sub-agent transcript in the chat list) and the rest followed it by copying the
neighbour, as the eight scroll states once did
([docs/lessons.md](../lessons.md) §5). By the tasks-screen track the leak was
being cited as a given — *"The chat screen already imports `app::events`"*
([tasks-stop-command.md](tasks-stop-command.md) §3.2) — which is what a convention
with no gate looks like after seven weeks. The correct shape was already in the
tree the whole time: `RagProgress` (`features/rag_ingest.rs`), `ToolDecision`
(`features/tools/confirm.rs` — "an FSD correction caught mid-implementation",
[journal/tools.md](../journal/tools.md)), `FeedFocus`, `FileProgress`,
`ImageProgress` and `ServerStatus` are each defined *below* both layers and
re-exported by `app/events.rs` for the orchestrator's convenience.

**`shared ⇄ entities` is a pair, not an arrow.** `shared/storage` reads and
writes the domain types and `shared/api` serializes the sampling config, while
the domain types take their settings and locale from `shared/config` and
`shared/i18n`. This is spec §4's own note — the engine and storage are backend
cores living in `shared`, a *deliberate adaptation* of FSD — and it has been the
shape since M2 (2026-06-14, `75eeffcb`). Nothing is wrong with the code here; the sentence "strictly
downward" is wrong about the bottom of the chain.

**Nothing enforces any of it**: no test, no `tools/*.py` gate, no CI step
(`rg -i 'fsd|layering' tools .github` finds nothing).

## 2. Forks

**F1 — the `screens → app` leak.**
- **(a) Move the eleven types down**, re-export them from `app/events.rs` as
  the six precedents are, and point the screens at the lower path. A verbatim
  move: no behaviour, no new or removed test, the orchestrator's imports
  untouched. *Recommended.*
- **(b) Amend the rule** to "screens may import `app::events` DTOs, never
  commands or the orchestrator". Legalizes the direction the documents call the
  key invariant, turns `app/events.rs` into a de-facto `shared` module, and
  leaves the six precedents in one shape and the eleven in another.

**F2 — where the types go** (under (a)).
- **(i) By subject, into `entities`**: a new `entities/task.rs` (the app's
  silent tasks and the tasks screen's snapshot: `BackgroundKind`, `AppTask`,
  `TaskRun`, `TaskList`, `TASK_LANDED_CAP`); `entities/subagent.rs` gains a
  nested run's position and a transcript's view (`RunProgressKind`,
  `SubagentProgress`, `ChildView`) beside `RunKind`/`RunOutcome`; a new
  `entities/live_turn.rs` (the generation in flight on an activated
  conversation: `LiveTurn`, `LivePartial`, `LiveTool`). *Recommended*:
  `entities` already holds the orchestrator's projections — `ChatSummary`,
  `ProfileSummary`, `AttachmentInfo`, `ImageInfo`, `FeedView` — and these are
  data with no logic.
- (ii) One `entities/view.rs` for all eleven: a module named after its
  consumer rather than its subject.
- (iii) `features/`: the home of the precedents that carry *logic*
  (`RagProgress` is the ingest's own progress type); these carry none.

**F3 — `shared ⇄ entities`.**
- (a′) Make the arrow true: move storage and the engine out of `shared`, or the
  domain types' settings out of `entities` — 31 files, a workspace-crate
  question spec §4 explicitly defers, and a track rather than a refactor.
- **(b′) Document the pair** as mutually visible and have the gate encode it.
  *Recommended.*

**F4 — the gate's shape.**
- **A Python script in CI's `lint` job**, `tools/layer_check.py`: needs no
  toolchain, runs before the Rust setup, the precedent
  `tools/list_scroll_check.py`. Its planted-violation runs are a `--self-test`
  arm that CI runs too (the precedent `release_guard.py --self-test`), so the
  mutation test of the gate is permanent rather than a journal paragraph.
  *Recommended.*
- A Rust unit test walking `src/`: needs a build, runs under `cargo test`, and
  every clean run costs a compile.

**F5 — test code.** A test may stand outside the layer it tests: the client's
tests assert its typed errors against `features::compaction::is_context_overflow`
— the cross-layer contract itself — and the live tests drive the tools. The gate
therefore exempts test code, recognized **structurally** (a `#[cfg(test)]` item:
a `mod … { }` block, a `mod …;` declaration and the file it names, a `fn`, a
`use`; and anything under a `tests/` directory), never by a file-name pattern or
an allowlist — "a gate that gets edited to shut it up" (list-scroll gate entry).
The screens' tests are rewritten regardless: the lower path is the canonical
one.

**F6 — rustdoc links upward.** Ignored, with every other `//` comment: a link is
prose, resolved by rustdoc and by nothing else, and the tree links upward
already (`shared/storage/mod.rs` → `crate::app::orchestrator`).

**F7 — one pull request or two.** One: the gate is what proves the move
complete, a gate without the move is red, and neither is application behaviour
— the separation AGENTS.md §2 asks for is between a mechanical move and a
behaviour change, and there is no behaviour change.

## 3. Decision

The owner's rule, given with the task: **(a)+(c) if the move is a pure
mechanical refactor** (a `refactor/` branch, the test count unchanged),
otherwise (b)+(c). §1 shows it is: eleven data types with no `app` dependency,
moved verbatim behind the re-export convention that `app/events.rs` already
follows for six others. So: **F1 (a), F2 (i), F3 (b′), F4 the Python gate with
a `--self-test`, F5 test code exempt structurally, F6 comments ignored, F7 one
pull request** — `refactor/fsd-layer-gate`.

## 4. Design

### 4.1 The move

| Type | From | To |
|---|---|---|
| `BackgroundKind`, `AppTask`, `TaskRun`, `TaskList`, `TASK_LANDED_CAP` | `app/events.rs` | `entities/task.rs` (new) |
| `RunProgressKind`, `SubagentProgress`, `ChildView` | `app/events.rs` | `entities/subagent.rs` |
| `LiveTurn`, `LivePartial`, `LiveTool` | `app/events.rs` | `entities/live_turn.rs` (new) |

`app/events.rs` re-exports all eleven (`pub use`), so every `crate::app::events::X`
in `app/` keeps compiling unchanged; the five production sites and the test
lines in `screens/` are rewritten to the `entities` paths. Doc comments travel
verbatim; an intra-doc link to `AppEvent` from its new home is written with its
path (`[…](crate::app::events::AppEvent)`), which is how `entities` links upward
already. The move also mends `AppEvent`'s doc comment, which the inserted structs
had split in two ("…its read-only" / "projection.").

### 4.2 The gate — `tools/layer_check.py`

The rule, as the code has it:

```
main.rs → app → screens → widgets → features → { entities ⇄ shared }
```

- A production line in layer *L* may name `crate::M::` only when *M* is *L* or
  below it; `entities` and `shared` see each other; `main.rs` sees everything.
  Any path counts, not only `use` lines (`screens/chat/feed.rs:494` named
  `crate::app::events::SubagentProgress` in a signature).
- `//` comments are skipped. Test code is skipped as F5 says.
- **The gate fails rather than passes on a missing subject** (the
  `doc_index_check.py` rule): no Rust files under `src/`, or a layer directory
  the table names that is not there.
- `--list` prints the files scanned per layer and the test files set aside;
  `--self-test` builds fixture trees in a temporary directory and checks every
  arm **in both directions** (docs/lessons.md §2): a planted `screens → app`
  `use` is red, the same line in a comment is green, the same inside
  `#[cfg(test)] mod tests { }` is green, in a `#[cfg(test)] mod tests;` file is
  green, after the test module's closing brace is red again, with a `}` inside
  a string literal before it still red (the brace counter reads strings), on a
  `#[cfg(test)] use` line is green; `widgets → screens` by bare path is red,
  `features → widgets` is red, `shared → features` is red, `shared → entities`
  and `entities → shared` are green, `main.rs → app` is green, same-layer paths
  are green, a tree with no `src/` is red, a tree missing a layer is red.

Deliberately not checked: `super::` chains (they cannot leave a layer), the
order of modules *inside* a layer, and what `app/events.rs` chooses to
re-export (its business). Not a replacement for the one invariant that is
about values rather than shapes — that the orchestrator is the sole owner of
`Chat` — which stays with the tests that exercise it.

### 4.3 Documents

- `docs/architecture.md` §2: the chain rewritten with the bottom pair and the
  gate named; the diagram gains the `shared → entities` edge; §3's module map
  gains the two new files and the moved types.
- `spec.md` §4: the rule's sentence, likewise.
- `AGENTS.md` §3, `CLAUDE.md` (the structure header, the commands list, a
  Status line): the same rule in one line each.
- `docs/journal/quality.md`: the entry; `docs/lessons.md` §3: the lesson — a
  documented *never* with no gate had drifted by the time anyone read it.

## 5. Outcome (2026-10-09)

- **The move is pure**: `cargo test` reports the same 3994 passed and 257
  ignored as `main`; clippy with warnings as errors and `cargo fmt --check` are
  green; `cargo doc --no-deps` resolves the moved doc links with no new warning.
  Outside the three `entities` files and `app/events.rs` — whose re-exports keep
  every `crate::app::events::X` in `app/` compiling — only `screens/` paths
  changed.
- **The gate**, on the moved tree: 340 files, 71 of them test files, 1 376
  cross-layer paths, every one downward, in 2.9 s. On `main` at `f5599770`:
  exactly the five production lines of §1. Its `--self-test`: 27 arms, both
  directions. Thirteen plants in a scratch copy of the moved tree — an upward
  `use` at the top of a file in each of the five layers, a type named in a
  signature, the same `use` inside a screen's test module, a line and a block
  comment, `shared → entities` — red and green exactly as §4.2 says.
- **The Sonar snippet analyzer** on the script before the push: two MINOR
  `python:S7519` (a dict comprehension that is `dict.fromkeys`), fixed; nothing
  else — the two largest functions had been split in advance, since the quality
  gate reads cognitive complexity.
- Documents updated as §4.3 lists; the journal entry is in
  [journal/quality.md](../journal/quality.md) (*the FSD layer rule gets a gate,
  and the eleven types that broke it move down*).
