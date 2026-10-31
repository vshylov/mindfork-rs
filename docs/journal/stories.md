# Journal — Fabula, the story engine

Fabula, the multi-agent story engine used from the chat: its database and event log, the
simulation (characters and the World Master), the Author's loop and its reviewers, the story
tools with their pauses, and the evaluation that decides whether the simulation stays.

**Reference documents for this area:** [docs/fabula.md](../fabula.md) (the track plan),
[docs/research/fabula-engine.md](../research/fabula-engine.md) (the design),
[ADR 0014](../decisions/0014-fabula-story-engine.md), architecture.md §3

Entries are in chronological order. They record what was done, why, what was measured and
what was rejected — the reasoning behind the code, not its current shape. For the current
shape read the reference documents named above; for the traps that recur across areas read
[lessons.md](../lessons.md).

## Entries (1)

- Post-M9: Fabula — PR 1, the story database (done)

### Post-M9: Fabula — PR 1, the story database (done)

The first code of the track (docs/fabula.md §4, M0): the domain types, a story's database
behind one actor, and the fold of the world over a branch path. Nothing in the application
calls it yet — the background story run is PR 8 — so both new modules carry an
`allow(dead_code)` that goes when it lands, and nothing here is user-visible (no CHANGELOG
item; no released build writes a story).

- **`entities/story/`** — keys the canon fixes (`CharacterKey`, `LocationKey`, `ItemKey`,
  `SecretKey`) and ids a run derives from position (`ch1/sc2/r0/k1/e3`, `…/p4`, `run2`), never
  drawn at random, so a replayed run sends the same prompts (spec §9). `Canon` with
  `deny_unknown_fields` and `check()`, which returns every problem at once — keys (1–64
  characters, no whitespace, `/` or control characters, unique per kind without regard to
  case; an item placed twice is a repeated item key), routes (to a real place, not to
  itself, at least one tick, listed once), starts, empty names, descriptions, secrets and
  goals. `SceneSetup`, `Proposal` with the thought first, `Action`, `Visibility`, `Effect`
  (move, place, give, take, drop, reveal, injure), `NewEvent`/`Event`, `Cause`; every union
  tagged by `type`, for the schema dialect of spec §8.
- **The one gate, `WorldState::admit`.** It checks an event against the state it applies to —
  the location and every character it names exist, the living ones are at the event's
  location, time does not run back, a speech, action or movement has an actor and a world
  event has none, a whisper has addressees present — then applies the effects in order, each
  against what the earlier ones left, and computes the witnesses on the state before:
  everyone alive at the location (and at a move's destination) for a public event, the
  addressees for a whisper, the actor alone for a hidden deed; an effect's subjects perceive
  it unless it is hidden; the dead perceive nothing. `Place` exists for the time between
  scenes: only in opening and settle events, anywhere reachable along routes. `fold` replays
  a path through the same gate, so a stored event that no longer folds names itself.
- **The fold lives in `entities`, not `features`** as the plan's §3 first had it: the database
  actor runs it for every commit, so a commit's check and its append see one state with
  nothing between them, and the layer gate lets `shared` name `entities` but not `features`.
  The plan's table now says so.
- **`shared/storage/story/`** — `StoryDb` is a cloneable handle on an actor: a thread of its
  own owns the only connections to the story file and its call-log sibling; every read and
  write is a job awaited through a oneshot. The capabilities `BranchWriter`,
  `ReflectionWriter`, `AuthorWriter` and `RunLogWriter` are not `Clone`, one per store at a
  time, checked by a token on every write and given back on drop — a resumed run takes its
  branch back that way. A second open of a file already open in this process is refused; the
  same path for both files too. A branch's path is its ancestry with a limit per segment
  (the fork's `seq`, the minimum going up), so a parent that goes on after a fork does not
  leak into the child.
- **Where the build differs from the spec's sketch** (recorded in `schema.rs`'s header and in
  the spec): reads go through the actor as writes do, so the files keep the **default rollback
  journal**, not WAL — WAL's concurrent readers buy nothing behind one thread, and a backup can
  take the file as `data.db`'s is taken, with no `-wal` to handle (PR 7); **append-only
  triggers on every table but `run` and `step`** (a failed step is finished by a resume, in
  place); `event` keeps `observable` and `private` as text, its visibility and cause as JSON
  columns of their own; `canon_doc` keeps the author's order (`position`) and marks a removed
  document by a version with a null body; `llm_call.cost_usd` for T1's dollar cap; routes and
  secrets are arrays of closed objects rather than a map and plain strings. The version is
  read **before** any DDL, so a newer file is refused untouched; a file that is not a database
  is refused by its header before SQLite opens it (docs/lessons.md §8). The canon is frozen
  once the story has an event — M4's canon deltas lift that.
- **A finding for PR 2.** The test that expected `{"type":"public","to":[]}` to be refused
  failed: serde reads a stray field on a variant without fields in silence, even under
  `deny_unknown_fields` (a struct variant does refuse). A whisper-shaped reply would read as a
  public line. The test now pins that behaviour, and the rule passes to PR 2: a reply is
  validated against its closed schema before it is parsed (docs/lessons.md §8).
- **The compile-time assertion was checked against its mutant.** With an `impl Clone for
  BranchWriter` added, `cargo check` fails with E0283 at the assertion; the file was restored
  from its bytes afterwards.
- **No live run**: storage and pure logic only, no engine, model or tool path (AGENTS.md §3).
- **Tests** (+49, 4524): the canon's checks on the fixture and on a canon with every problem
  at once; the unions' tags; the gate's 21 refusal cases, witnesses for public, whispered, hidden
  and moving events, items changing hands, a reveal, death, effects in order, a fold that no
  longer folds; `UPDATE` and `DELETE` refused on the event log and on every append-only table
  (both triggers on each, none on `step`); a second writer refused for every store until the
  first is dropped; a branch never sees a sibling's events, a fork nothing its parent wrote
  after it; commits checked against their branch, their proposal's branch and their reaction's
  target; the canon read back as imported and versioned per document; the main line following
  the picks; reflections along the path; steps and calls by address, the call id an
  independently computed UUIDv5; both files stamped; a newer file and a non-database refused
  untouched.

**Gates**: 4524 unit tests green, 308 `#[ignore]`; clippy `-D warnings`, fmt, `cyrillic_scan`,
`link_check`, `doc_index_check`, `layer_check`, `list_scroll_check` clean.
