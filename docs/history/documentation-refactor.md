# Documentation refactor — organizing the docs for context economy

Design plan, archived here on completion (AGENTS.md §1: a finished track's plan
moves to `docs/history/`). The track was delivered in one PR.

**Goal:** cut what a session must load before it can start working, without
losing the engineering knowledge the project has accumulated. Today a session
begins with ~450K of its 1M-token window already spent; the target is a
routing layer of a few thousand tokens plus whatever the task at hand actually
needs.

## 1. The problem, measured

`CLAUDE.md` is loaded **in full, automatically, at the start of every
session**. Nothing else in the tree is — `spec.md`, `architecture.md` and the
ADRs are read on request, which is why the observed figure is roughly
"CLAUDE.md + whatever the first instruction asks for".

| Part | Bytes | ≈ tokens |
|---|---|---|
| **CLAUDE.md — total** | **994 562** | **~249K** |
| — guide (L1–125: what/decisions/structure/conventions/commands) | 8 273 | ~2K |
| — "Status" prose paragraph (L126–423) | 23 387 | ~6K |
| — M0–M9 milestone log (L424–645) | 16 225 | ~4K |
| — post-M9 journal, **243 entries** (L646–13325) | **946 677** | **~237K** |
| spec.md | 216 726 | ~54K |
| docs/architecture.md | 144 239 | ~36K |
| CHANGELOG.md | 57 209 | ~14K |
| README.md | 37 306 | ~9K |
| docs/roadmap.md | 36 680 | ~9K |
| docs/history/ (30 files) | 604 108 | ~151K |
| docs/research/ (16 files) | 451 605 | ~113K |
| docs/decisions/ (9 ADRs) | 76 245 | ~19K |
| **Tree total** | **~2.75 MB** | **~690K** |

So: **95% of the automatic load is the journal**, and the guide that a session
genuinely needs every time is 8 KB of it. The remaining ~200K of the observed
450K comes from the standing instruction to also read `architecture.md` and
the ADRs — both read *whole*, though both are chaptered documents where a task
needs two or three sections.

The average journal entry is ~4 KB. A typical task needs two or three of them.
We load 243.

## 2. What the journal is for (before deciding what to do with it)

The entries are not a changelog — `CHANGELOG.md` is. Reading across them, each
has a recognizable anatomy, and the parts have very different reuse profiles:

1. **What and why** — largely duplicated on purpose into CHANGELOG (the
   user-visible half) and spec/architecture (behaviour and structure).
2. **Forks and decisions** — usually also in a design doc under
   `docs/history/` or an ADR.
3. **Measurements** — "bge-m3 calibrated to 0.41317/0.81843", "63 s → 2 s",
   "88 of 186 805 words". 85 entries carry them. Mostly unique to the journal.
4. **Traps and lessons** — 128 entries carry trap-shaped language ("worth
   recording", "caught by", "mutation-tested", "a hypothesis the measurement
   killed"). This is the part with genuine cross-task value, and the entries
   know it: ten of them cite an earlier entry's trap, and three separately
   re-record the same `git checkout <file>` revert because it bit again.
5. **Provenance** — test counts, live-run stacks, gate results. Rarely reused
   except as a baseline.

The durable, cross-task value is concentrated in (4) and part of (3) — call it
5–10% of the bytes. The rest is per-track narrative, needed only by someone
touching that subsystem.

**The one thing the current shape buys** that a split would lose if we are
careless: because everything is loaded, an agent working on the settings screen
*happens* to know about the ratatui trailing-cell trap. That serendipity is
real and is what §4.3 exists to preserve.

## 3. Why the current shape defeats its own purpose

- **The unit of loading is the file; the unit of need is the entry.** There is
  no way to ask for "the three entries about embeddings".
- **`docs/` is organized by lifecycle, not by subject.** `research/` (before a
  decision) and `history/` (after a track) describe *when* a document was
  written, not *what it is about*. Neither answers "what must I read to work on
  the settings screen?".
- **The "Status" paragraph is stale by construction.** It is a single run-on
  sentence that has been prepended to for ~40 tracks; it now describes the last
  twenty changes in decreasing detail, which is the journal again, in miniature.
- **The M0–M9 log is superseded.** `docs/history/plan.md` holds the completed
  plan, and `architecture.md` describes what those milestones built. What is
  left in CLAUDE.md is a second copy that still mentions `xinfer` (7 times) — an
  engine the project abandoned.

## 4. Proposal

### 4.1 CLAUDE.md becomes a router (~12 KB)

Keeps: what the project is; the key architectural decisions in condensed form;
the FSD structure; conventions; commands; a **short** current status (version,
test counts, the two or three most recent tracks); and — the new part — a
**document map with trigger conditions**, saying not just what each document is
but *when to read it*:

```
| When you are… | Read |
|---|---|
| working on the engine / a provider | architecture §6, spec §6–§8, docs/journal/engine.md |
| working on memory (notes/self-model/RAG) | architecture §9, spec §9.5/§17, docs/journal/self-model.md |
| about to implement anything | docs/lessons.md |
```

Removes: the journal, the M0–M9 log (a pointer to `docs/history/plan.md`), and
the "Status" run-on (replaced by three lines plus a pointer to the journal
index).

### 4.2 The journal splits by subsystem into `docs/journal/`

Not by date — nobody retrieves by date. The split axis is **the chapters of
`architecture.md`**, so the map is self-evident and mechanically checkable
("working on area X → architecture §N → `docs/journal/<X>.md`"):

| File | architecture § | Covers |
|---|---|---|
| `engine.md` | §5, §6 | providers, wire formats, sampling, streaming, servers, health, compaction |
| `storage.md` | §7 | JSON/SQLite, migrations, backup, `cache.db`, search index |
| `tools.md` | §8 | tool system, MCP, sandbox, web/fetch/YouTube, TTS, confirmation |
| `self-model.md` | §9 | the self-model: summary, goals, traits, narrative, reflection |
| `notes.md` | §9 | notes, their graph, semantic recall, cross-organ edges |
| `rag.md` | §9 | the knowledge base, chat attachments, the embedding stack |
| `ui-feed.md` | §10 | feed, markdown renderer, syntax, terminal quirks |
| `ui-input.md` | §10 | input box, keys, spellcheck, selection/undo/mouse |
| `ui-screens.md` | §10 | settings, chat list, self-model screen, search screens |
| `i18n.md` | — | both axes, CLI, external locales |
| `release.md` | §12 | packaging, installers, the release pipeline, branding |
| `ci.md` | §12 | workflows, jobs, caches, minutes, the live-test gate |
| `quality.md` | §12 | static analysis and the repository's own gates |
| `refactors.md` | §3 | god objects, SOLID, Sonar backlog, documentation work |

Entries move **verbatim**, chronological within each file, with a heading index
at the top of each file. A task reads one. Memory was delivered as one
`memory.md` and split again into `self-model` / `notes` / `rag` before the PR
closed — 180 KB and 45 entries is past the point where a file is read rather
than grepped, and "the three memory organs" is the project's own vocabulary.

### 4.3 `docs/lessons.md` — the extraction that makes the split safe (~20 KB)

The recurring traps, deduplicated and stated once, with a pointer to the entry
that found each: `git checkout <file>` reverting *all* uncommitted work in that
file (recorded three times); a poisoned `target/` from a second build of the
same crate (twice); `cmd | tail` reporting the pipe's exit code (twice);
ratatui's trailing cell and `prime_full_redraw`; `is_ok()` cannot assert
anything on a path built to degrade gracefully; dynamically-built i18n keys are
invisible to the key gate; an agent finishing a file after you edited it
silently reverts your edit; a surviving mutation indicts the mutation as often
as the test; measure before assuming.

This is the piece that replaces the serendipity of loading everything. It is
small enough to be read at the start of any implementation task, and it is
where §2's item (4) — the journal's highest-value content — becomes *available*
without reading 237K tokens.

### 4.4 Read `spec.md` and `architecture.md` by section

Both have tables of contents and stable numbering. The guide states the rule
("read the section, not the file") and the map says which § answers what. On a
typical task this alone is ~45K tokens.

### 4.5 A CI gate so the structure cannot rot

`tools/doc_index_check.py`, in the house style of `link_check.py` and
`cyrillic_scan.py`: every entry heading appears in its file's index; every
journal file appears in CLAUDE.md's map; no entry is orphaned; CLAUDE.md stays
under a byte ceiling. Runs in the `lint` job, which already runs on docs-only
PRs — which is exactly when documentation structure breaks.

### 4.6 AGENTS.md §4 updated

The row "Any completed task → CLAUDE.md journal entry" becomes "→ an entry in
the matching `docs/journal/<area>.md`", plus a new row: "a trap that will bite
again → a line in `docs/lessons.md`". The §1 "read before working" list points
at the map rather than at the whole of CLAUDE.md.

## 5. Expected result

| | Now | After |
|---|---|---|
| Automatic load | ~249K | ~4K |
| Typical task ("work on the settings screen") | ~450K | ~40–60K |
| Knowledge reachable | all, at once | all, on demand + a 20 KB lessons digest |

Roughly an **85–90% cut** in what a session spends before its first useful
action, with nothing deleted.

## 6. Costs and risks

- **The refactor is large and mechanical**: 243 entries to classify and move.
  Mitigated by doing it with parallel agents over disjoint files and by a
  verification script that compares the set of entry headings before and after
  — the move is lossless or the check fails.
- **Serendipity loss** — §4.3 is the mitigation, and it is the part to get
  right; if in doubt, `lessons.md` should be over-inclusive rather than terse.
- **Index rot** — §4.5.
- **43 files link to CLAUDE.md** (AGENTS.md, architecture.md, CHANGELOG,
  ADRs, history docs, PR template, two workflows). Most links are to the file
  as an orientation document and stay valid; the ones naming "the journal" need
  re-pointing. `tools/link_check.py` catches broken relative links but **not**
  a link that still resolves while naming the wrong thing — so this needs a
  deliberate pass, not just the gate.
- **A journal entry's home is sometimes ambiguous** (a track that touched
  storage *and* the UI). Rule: file it where the *change* lives, and
  cross-reference from the other file's index.

## 7. Forks — **all adopted as recommended (user's decision, 2026-08-08)**

F1(a) split by subsystem along architecture.md's chapters · F2(a) extract
`docs/lessons.md` · F3(a) move entries verbatim · F4(a) drop the M0–M9 log and
the "Status" paragraph from CLAUDE.md · F5(a) add the gate script · F6(a) one
PR · F7(a) leave spec.md and architecture.md whole, read them by section.

- **F1. Journal split axis.**
  (a) **by subsystem, aligned to architecture.md chapters** — retrieval matches
  how tasks arrive; **recommended**.
  (b) chronological chunks (`journal/2026-08.md`) — trivially mechanical, but
  retrieval by date matches nothing.
  (c) one file per entry (243 files) — maximum granularity, unreadable index,
  and Grep already gives granularity.

- **F2. The lessons digest.**
  (a) **extract `docs/lessons.md`, index it from CLAUDE.md** — **recommended**;
  it is what replaces the serendipity of the current shape.
  (b) don't extract; rely on Grep over the split journal. Cheaper now, and it
  makes the traps findable only by someone who already suspects them.

- **F3. Entry text.**
  (a) **move verbatim** — lossless, mechanical, and verifiable by a heading-set
  comparison; **recommended**.
  (b) also compress older entries while moving. Better end state, but it is
  editorial judgement applied 243 times, unverifiable, and destroys exactly the
  measurements and reasoning §2 identifies as the unique content.

- **F4. The M0–M9 log and the "Status" paragraph.**
  (a) **drop both from CLAUDE.md** — the milestone log to a pointer at
  `docs/history/plan.md`, the paragraph to three lines plus the journal index;
  **recommended**.
  (b) keep compressed versions in CLAUDE.md.

- **F5. The gate script** (§4.5): (a) **yes, recommended**; (b) no.

- **F6. Scope of the change.**
  (a) **one PR** — the split, the map, the lessons digest, the gate and the
  AGENTS.md/link updates together; **recommended**, because a half-applied
  split leaves the docs describing a structure that does not exist.
  (b) staged.

- **F7. Should `spec.md` and `architecture.md` be split too?**
  (a) **no — keep them whole, read them by section**; **recommended**. They are
  reference documents with stable numbering that is cited from dozens of places
  (`spec §9.6`, `architecture §11`); splitting would break that vocabulary for
  a saving §4.4 already achieves.
  (b) split them by chapter as well.

## 8. Execution plan (once the forks are decided)

1. Freeze the entry inventory: extract all 243 headings with byte ranges.
2. Classify every entry into a bucket (this is the only judgement-heavy step;
   ambiguous ones get listed for review rather than guessed).
3. Move verbatim into `docs/journal/*.md` with per-file indexes.
4. Write `docs/lessons.md` from the trap-shaped entries.
5. Rewrite CLAUDE.md as the router with the trigger map.
6. Re-point the inbound links (43 files) and update AGENTS.md §1/§4.
7. Add `tools/doc_index_check.py` + the CI step.
8. Verify: heading sets identical before/after; `link_check.py` clean;
   `cyrillic_scan.py` clean; the new gate clean; CLAUDE.md under its ceiling.

## 9. Out of scope

- Deleting or rewriting any historical content (F3a).
- Touching `docs/research/` and `docs/history/` layout — they are already
  on-demand and correctly organized by lifecycle; only the links into them
  change.
- CHANGELOG.md, which already has the right shape and audience.
- No live model run is required (AGENTS.md §3): documentation and one dev
  script; no engine, memory, tool or provider path is touched.
