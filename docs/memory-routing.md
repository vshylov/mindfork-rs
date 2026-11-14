# Plan: where a fact about the person goes — notes or observations

> Status: **stages 1 and 2 done (2026-11-14, §5.4, §5.5) — the track is complete.**
> Forks F1–F5, the F4 merge sub-question and how a nameless note is caught (the
> note names the person; the threshold stays 0.70 — §5.5) decided by the user on
> 2026-11-14. The plan moves to `docs/history/` in a docs branch.

## 1. Task nerve

The app keeps four kinds of memory about a conversation's two parties, each with
its own tool and its own readers (§2). Two of them are the subject here:

- a **note** (`note_save`) — memory about the person and their world, retrieved
  by topic (`note_recall`, the notes block of the prompt);
- an **observation** (`add_insight`, a note tagged `@self`) — the companion's
  memory about itself: what it understood, how it changed, what happened between
  the two ("self over time", [docs/history/narrative-as-notes.md](history/narrative-as-notes.md)).

**The live finding** (2026-11-13). Asked how the tools feel from the inside, a
Gemini model on a real profile said the boundary between `note_save` and
`add_insight` "is sometimes a little blurred". The profile's store says more than
that: of 15 live notes 12 were observations, and of the 10 facts about the person
(their projects, their works, a change of setup, a philosophical view) **7 had
been written with `add_insight`**. The only regular notes were three digests of
attached documents. The probe below reproduces it on a clean profile: Gemma 4
31B filed **6 of 6** facts as observations (§5.3).

This is not a model's quirk. The text the model reads on every turn routes facts
to `add_insight`, while the architecture means them for notes (§3).

## 2. Current state — the four memories

| Memory | Written by | In the prompt | Retrieved by | Gates, upkeep |
|---|---|---|---|---|
| **Note** (`notes`, no `@self`) | `note_save`; `note_revise`/`note_supersede`/`note_merge` | the companion's notes block — only with `notes_in_prompt` (a profile switch, **off by default**; `NOTES_FLOOR` 0.45, 1 600 characters) | `note_recall` | the `note_save` write gate (≥ 0.70, notes only); the consolidation overview; "sleep" |
| **Observation** (`notes`, `@self`) | `add_insight`; the `note` scar of `update_user_model` | the self-model block's observations section — **always** when self-model tools are on (20% of the block's budget, picked by relevance to the chat's opening) | `get_self_model`; `note_recall` **only with** `notes.recall_includes_self` (**off by default**) | the `add_insight` write gate (≥ 0.70, observations only); the self-consolidation overview; self-"sleep" |
| **Interlocutor model** (`self_models`: traits, interests, relationship dynamic) | `update_user_model` | the self-model block, 20% of its budget | `get_self_model` | the related-trait gate (0.72) |
| **Self-description, goals** (`self_models`) | `update_self_model` | the self-model block, 40% + 20% | `get_self_model` | the summary size gate |

The design intent is stated in three places and is unambiguous:
`narrative-as-notes.md` hides `@self` from `note_recall` because "memory about
oneself ≠ memory about the interlocutor, mixing the output is risky"; the
reflection rubric says "find the fact about the interlocutor via `note_recall`";
`ui.notes.empty` says notes are what "the assistant writes as it learns about you".

## 3. Diagnosis — why the facts go to observations

1. **The maintenance protocol never names `note_save`.** `selfmodel.policy_core`
   is in the system prompt on every turn of a profile with self-model tools
   (`maintenance_protocol`, on by default), and in the background reflection and
   self-consolidation prompts. It routes the summary, "event-driven conclusions…
   **even if** they are stable", fleeting things, goals and traits — and nothing to
   notes. "The person moved to Porto" is an event, so the rule as written sends it
   to `add_insight`.
2. **The descriptions point the same way.** `add_insight`: "an observation about
   yourself, the conversation **or the interlocutor**… even if they are stable".
   `note_save`: one line — "a note about the user/context for future conversations".
3. **The loop rewards it.** An observation comes back in the next turn's prompt by
   itself; a note does so only with `notes_in_prompt`, which is off by default. The
   model sees its `add_insight` writes working and its notes vanish.
4. **`update_user_model` is the third sink.** With no home for facts, the probe's
   models also file them as "interests" (`Porto`, `remote work`, `NixOS`) and
   "traits" (`father of a seven-year-old cellist`) — list items meant for stable
   dispositions.

**What it costs.**

- With the default `recall_includes_self = false`, `note_recall` cannot find a
  fact that was filed as an observation.
- Facts take the observations' 20% of the self-model block and crowd out the
  history of the companion itself.
- The write gates do not cross kinds: the same fact as a note and as an
  observation is never flagged (on the live profile a note and an observation
  about the same novel score 0.697).
- Self-"sleep" consolidates facts about the person as if they were the
  companion's own history.

## 4. Forks — decided

**F1. The routing rule.** *User's decision (2026-11-14): (a).*

- **(a) By subject, with one test question — "would this still be true if I did
  not exist?"** Yes → a note (`note_save`, with tags): facts about the person and
  their world — projects, works, plans, places, family, views, setup. The
  companion's reading of the person's work goes there too, tagged with the work,
  so it comes up with the work. No → an observation (`add_insight`): what I
  understood, how I changed, what happened between us. Stable dispositions stay
  short items of `update_user_model`. *Recommended.*
- (b) By form: facts → notes, interpretations → observations. Rejected: the
  companion's reading of a story would then live apart from the story.

**F2. Where the rule is stated.** *Decided: (a).*

- **(a) Everywhere the model reads it**, in both bundles: `selfmodel.policy_core`
  (the turn's protocol, the reflection and self-consolidation prompts), the
  descriptions of `note_save` (widened — what goes there, tags) and `add_insight`
  ("the interlocutor" narrowed to "how you see them and your relationship"), and
  the reflection rubric. *Recommended.*
- (b) Descriptions only. Rejected: the protocol is the stronger, per-turn signal,
  and it is the one that misroutes now.

**F3. Notes that do not come back by themselves.** Routing facts to notes makes
them invisible without `notes_in_prompt`. *Decided: (a).*

- **(a) The protocol says to look**: before answering about the person's life,
  projects or plans, `note_recall`. *Recommended — no default changes.*
- (b) Turn `notes_in_prompt` on with the self-model tools. Rejected for now: it
  changes defaults and the prompt's size for every such profile.

**F4. Write gates across kinds.** *Decided: yes — `note_save`'s gate also lists
observations at the write gates' threshold (0.70), listed apart, and
`add_insight`'s lists notes;
the hint says that the same fact belongs in a note.*

The sub-question for stage 2: `note_merge` gives the merged note the **union** of
its sources' tags and `note_supersede` the old note's tags, so merging an
observation into a note yields an **observation**. Options: (i) a merge that
includes a regular note yields a regular note — facts are the person's, so a mixed
merge is a fact; (ii) the hint names `note_revise` of the note plus
`note_supersede` of the observation instead of a merge. *User's decision
(2026-11-14): (i).*

**F5. Facts already filed as observations.** *Decided: (a) leave them.* No tool
changes a note's kind; a one-off move script for one profile, or teaching
self-"sleep" to move them (it would need such a tool), are not worth it before the
rule is proven to hold.

## 5. The probe — go/no-go

### 5.1. Instrument

`app::orchestrator::tests::memory_routing_live` — a scripted conversation through
the app's own request path (the profile's memory tools, the maintenance protocol,
`LlamaSupervisor` in a cloud mode). Twelve lines in Russian, the language the
finding was made in: six that each carry a fact about the person (a move, a job
and a hobby, a plan with dates, a view on time, a daughter's music school, a
server's OS), three that invite the companion's own conclusion (a habit the
person noticed, how it lives the time between conversations, what it never asks
about), three of small talk. Each turn's writes are sorted by the line's kind:

- a **fact** turn is *observation* if it called `add_insight` at all (even beside
  a note — the fact is then in the self-model's memory), else *note*, else *user
  model*;
- an **own** turn is *self-model* if it called `add_insight` or
  `update_self_model`, else *note*.

Two numbers decide: **facts kept out of observations** (of the fact turns that
wrote anything) and **own conclusions kept in the self-model** (of the own turns
that wrote anything). The writes per turn, the arguments, the live notes left in
the store and every reply go to JSON (`MINDFORK_ROUTING_OUT`).

No embedder: the first write's destination is decided by the descriptions and the
protocol; the gates only speak after it.

Arms (the user's direction: cloud, not the local GPU, for the volume): Gemma 4 31B
through OpenRouter (`google/gemma-4-31b-it`, the app's `openrouter` mode) and
`gemini-4-hydrogen` through the Gemini API (the app's native `gemini` mode — the
model of the live finding). Three conversations per arm per condition, about seven
minutes for both arms. The Gemma arm costs cents; the Gemini arm's cost depends on
how much the model thinks and was not metered here.

### 5.2. Criterion

**GO** for stage 1 when, on both arms: facts kept out of observations **≥ 80%**;
own conclusions kept in the self-model **no lower than the baseline**; facts
written at all no lower than the baseline minus 15 points (the rule must route
facts, not silence them). Otherwise the wording is revised and measured again
before anything else is built.

### 5.3. Baseline (2026-11-14, before any change)

Three conversations per arm, eighteen fact lines and nine own lines each:

| Arm | Facts written | **Facts kept out of observations** | Own written | Own in the self-model | Fact turns also writing `update_user_model` | Live notes per run (regular, observations) |
|---|---|---|---|---|---|---|
| Gemma 4 31B (OpenRouter) | 18/18 | **2/18 (11%)** | 4/9 | 4/4 | 9/18 | (0, 7), (0, 6), (4, 6) |
| `gemini-4-hydrogen` (Gemini) | 18/18 | **13/18 (72%)** | 8/9 | 8/8 | 5/18 | (6, 4), (5, 5), (5, 4) |

- **Gemma files nearly every fact as an observation**, half the time beside an
  "interest" in the interlocutor model (`Porto`, `remote work`, `Java`, `NixOS`) —
  the live finding reproduced on a clean profile and a neutral persona.
- **Gemini routes most facts to notes here**, better than on the live profile,
  whose persona asks for a "model of the user" and a "self-model" kept "with notes
  and the knowledge graph". What still reaches observations is exactly what the
  protocol's wording catches: the move told as an **event** ("at the start of a new
  stretch of our talk, Alex told me he had moved…" — 3 of 3 conversations, twice
  beside a note) and the person's **view** of time (2 of 3).
- **Gemma states its own conclusions without recording them** (written 4 of 9: "I
  realized I was too wordy at first…" stays in the reply). Not this track's
  subject; the after-arm must not lower it.
- Gemini's three *rewrite* turns are the plan (line 5) revising the note about the
  game from line 3 — the right move for a fact that extends a note.

Against §5.2 the baseline is far from GO on Gemma and short of it on Gemini.

### 5.4. After stage 1 (2026-11-14) — GO

The same probe, the same arms, three conversations each, after the wording of §6
stage 1:

| Arm | Facts written | **Facts kept out of observations** | Own written | Own in the self-model | Fact turns also writing `update_user_model` | Live notes per run (regular, observations) |
|---|---|---|---|---|---|---|
| Gemma 4 31B (OpenRouter) | 18/18 | **18/18 (100%)** — was 2/18 | 3/9 — was 4/9 | 3/3 | 3/18 — was 9/18 | (7, 1), (6, 1), (7, 1) |
| `gemini-4-hydrogen` (Gemini) | 18/18 | **18/18 (100%)** — was 13/18 | 9/9 — was 8/9 | 9/9 | 3/18 — was 5/18 | (7, 3), (7, 3), (7, 3) |

- **Every criterion of §5.2 holds on both arms**: facts kept out of observations
  100% (≥ 80%), own conclusions in the self-model 100% as before, facts written
  18/18 as before.
- **The interlocutor model stopped taking facts.** What remains is line 3 only —
  the person's profession and hobby, as traits and interests ("an experienced Java
  developer", "board game design"), which is what the lists are for. Places, the
  daughter, the server and the plan no longer appear there.
- **Every note is tagged** (41 `note_save` calls across both arms).
- Gemini now looks facts up before writing (`note_recall` on lines 9–10 in one
  conversation) and links the plan to the game note (`note_link`).
- **Gemma's own conclusions recorded: 3 of 9, against 4 of 9 before** — one line
  in one conversation (line 11 of baseline run 3). It records its first conclusion
  (line 4) every time and states the other two in the reply without writing them,
  before and after alike — the weakness §5.3 named, not one this wording made.

### 5.5. Stage 2 — the gates across kinds (2026-11-14)

A second instrument in the same file, `memory_routing_cross_kind_*_live`: a profile
from before stage 1 holds a fact as an observation ("Alex moved from Lisbon to Porto
and now works from home", seeded without a vector, so the gate's backfill embeds
it); the person brings it up again with a new detail (the flat with a view of the
Douro). One turn; the store read afterwards. It records each `note_save` with its
cosine to the observation (measured again with the same embedder, passage role),
whether the gate listed the observation, whether `note_merge` took it, and where
the fact ended. It asserts the mechanism: a saved note at or above the threshold
lists the observation. Chat in the cloud as before; the embedder a local bge-m3
Q8_0 (`MINDFORK_EMBED_URL`) — the gate cannot run without one.

| Arm | Runs | Gate listed the observation | `note_merge` took it | The fact a note only |
|---|---|---|---|---|
| `gemini-4-hydrogen` | 6 | 6 | 6 | 6 |
| Gemma 4 31B | 12 | 8 | 6 | 6 |

- **Gemini** looks the fact up first (`note_recall`, `get_self_model`), saves the
  note naming Alex (cosine 0.866–0.876) and already carrying the observation's
  detail ("works from home"), and merges the observation into it every time — the
  result a note, not an observation.
- **Gemma** merges when the gate lists the observation (6 of 8; once it rewrote
  its new note instead, once it left both). The gate stayed silent in 4 of 12
  runs; the two whose note the probe recorded (the first batch of three predates
  the recording) were written **without naming the person** — "Moved from Lisbon
  to Porto a month ago…" scores **0.690–0.697**
  against the observation, against 0.799–0.808 for the same sentence beginning
  "Alex moved…". The gate is right by its threshold; the note is the weak part — a
  memory with no subject.
- **Two ways to catch those**, both automatic: `note_save`'s description asking
  for a standalone statement that names who the note is about, or the threshold at
  0.68 (catches 0.69 at the price of more same-topic items: 9 of 15 notes on the
  live profile have a neighbour at or above it, against 2 at 0.70).
- **Tried first: 0.68.** Gemma, six conversations: the gate listed the observation
  and the model merged it into a note 6 of 6 — two of those notes were nameless,
  caught at **0.681** and 0.689: a margin of 0.001 over the threshold, and the
  nameless note itself left as poor a memory as before.
- ***User's decision (2026-11-14): the note names the person; the threshold stays
  at 0.70*** — fix the note, not the gate, and lower the threshold only if a
  measurement still asks for it. `note_save`'s description now asks for "a
  standalone statement that names who it is about — by name when you know it
  («Anna moved to Porto», not «Moved to Porto»): it will be read later without this
  conversation around it" (a neutral name, so the example does not hand the probe
  its person's).
- **Re-measured at 0.70 with the naming** (both arms; the cross-kind probe now also
  counts notes that name the person):

  | Probe | Arm | Runs | Notes naming the person | Gate listed it | Merged into a note | Fact a note only |
  |---|---|---|---|---|---|---|
  | cross-kind | Gemma 4 31B | 6 | 6 | 6 | 6 | 6 |
  | cross-kind | `gemini-4-hydrogen` | 3 | 3 | 3 | 3 | 3 |

  Gemma's notes now score **0.800–0.876** against the observation — a margin of
  0.10 over 0.70, where 0.68 caught a nameless one by 0.001. The routing probe of
  §5.1 on the new wording, three conversations per arm: facts kept out of
  observations **18/18** (Gemma) and **17/17** (Gemini; one line — the view of
  time — went unrecorded, so facts written 17/18); own conclusions in the
  self-model 6/6 and 8/8 (Gemma now records 6 of 9, against 3–4 of 9 before);
  **fact notes naming the person 20/20 and 22/22**. The calibration smoke at 0.70
  is unchanged (neighbouring topics 0.483–0.598). 0.70 holds; no measurement asks
  for 0.68.

## 6. Stages

1. **The wording (F1–F3)** — **done**. `selfmodel.policy_core` now says what an
   observation is (memory about the companion itself) and is not (a fact about the
   interlocutor, even learned as news), and narrows the interlocutor lists to
   short, stable dispositions; it names no note tool, since it is also the
   background reflection's and self-consolidation's rules and neither offers
   `note_save`. The route is two new sentences, `selfmodel.policy_notes` (facts →
   `note_save`, with tags) and `selfmodel.policy_recall` (look them up with
   `note_recall`), appended to the **turn's** protocol only for the tools the
   request offers (`NoteRoutes::offered(&request.tools)` →
   `maintenance_protocol(loc, routes)`) — a rule naming an absent tool is a dead
   end (docs/lessons.md §4). The descriptions of `note_save` (widened: what goes
   there, tags), `add_insight` (narrowed to the companion itself; a fact "belongs
   in a note") and `update_user_model`, and the reflection rubric, say the same;
   both bundles. Measured in §5.4.
2. **Gates across kinds (F4)** — **done**. The write gates' search returns
   `GateHits { same, other }` — every live note at or above the threshold, split by
   kind, each list capped on its own — and each gate renders the other kind apart,
   under its own lead line: `note_save` lists close observations ("if one holds this
   same fact about the interlocutor, the fact belongs in the note: merge it into
   the new note"), `add_insight` lists close notes ("if your observation only
   restates one, merge; if it is your own conclusion about it, keep both and link
   them"). The merge rule (i): `note_merge` drops `@self` from the union when any
   source is a regular note, and its result says so ("an observation was among
   them… the result is a note"). Measured in §5.5.

Out of scope: moving existing observations (F5); `notes_in_prompt` defaults (F3b);
the `update_user_model` lists' abuse as a fact store beyond what the rule's
wording fixes (watched by the probe's *user model* column).
