# Research: cargo-mutants beside the hand-run mutation check

**Status:** research, closed (2026-11-22). **The owner's decisions, 2026-11-22** (§7):
F2–F5 as recommended; F1 — whether to adopt it at all — on a measurement of what it
adds to CI on hosted runners: a large increase, and above all a risk of timeouts,
means it is not adopted. **The measurement is in (§10), and by that condition
cargo-mutants is not adopted as a job in CI**: a pull request of the median size
ended its checks three times later than it does today, one of the 90th percentile
did not end in 100 minutes, and one mutant in eight ended in a timeout. What the
probes found in the tests is fixed on its own (§8, stage 1). The tool was installed and run on this repository with the
owner's go-ahead (2026-11-21); the numbers below are measured unless a line says
otherwise.

The owner asked whether [cargo-mutants](https://mutants.rs/) can be applied to this
project and whether it should be. This document measures both.

## 1. Summary

- **It can.** Version 27.1.0 lists the tree's mutants in two seconds and runs them
  with no change to the repository: one binary crate, the pinned toolchain (1.96) is
  above the tool's floor (1.88), and the suite is green in a second checkout.
- **The whole tree is out of reach as a routine.** 22 434 mutants at 108 s each is
  28 machine-days. A mutant costs an incremental build (31 s) and the whole suite
  (75 s); the tool cannot pick the tests a mutant needs.
- **A pull request is in reach.** Of the last 100 merged, 56 produce no mutant; the
  other 44 have a median of 46 (83 minutes on one machine), 179 at the 90th
  percentile (5.4 hours), 551 at the most.
- **What it finds is real.** In a sample of 53 mutants spread over the tree, 8 of the
  47 that compile survived, and 5 of those 8 are behaviours no test holds. Four
  mutants of 76 ended in a timeout, and three of those exposed seven tests that wait
  without a bound — the trap [docs/lessons.md](../lessons.md) §2 records six times.
- **It does not replace the hand-run check.** It has no operator for a deleted
  statement or call or for a changed constant, which is what most hand-written
  mutants are; and a hand-run mutant costs a quarter of its time.
- **Recommendation:** adopt it as a second, mechanical pass over a pull request's
  diff, in CI, sharded, **not a gate** — after a pilot on three pull requests (§8).
- **What decided it** (the owner, 2026-11-22): the time it adds to CI. A pull
  request's checks end in 8.3 minutes at the median today. Measured on hosted runners
  (§10), eight shards end in 25–30 minutes on a pull request of the median size and
  did not end in 100 on one of the 90th percentile; 27 of 228 viable mutants ended in
  a timeout. **Not adopted as a job in CI.**

## 2. What the project does today

Mutation testing is already routine here and is done by hand
([docs/lessons.md](../lessons.md) §2). The author of a change writes a mutant for each
load-bearing line, all of them into the source at once, each behind
`crate::mutant(N)` read from an environment variable; the crate is built once and
the test binary is run once per mutant. The journal records about 75 such checks
that give a count — close to a thousand mutants, nine at the median.

Two properties of that recipe matter for the comparison:

- **The mutants are chosen.** They are semantic — "the check dropped", "the call
  removed", "the floor at 0", "the old chunker" — and they cover what the author
  thought was load-bearing.
- **A mutant is cheap.** One build, then about 25 seconds a mutant with a test
  filter (lessons §2).

## 3. What the tool is

cargo-mutants 27.1.0 (2026-06-02; a million downloads) is a cargo subcommand. It
parses the sources, applies one textual mutation at a time, runs `cargo test
--no-run` and then `cargo test`, and reports each mutant as *caught*, *missed*,
*unviable* (does not compile) or *timeout*. By default it works on a copy of the
tree; `--in-place` mutates the tree it is given.

Its operators, all six of which this tree produces:

| Operator | Mutants here |
|---|---|
| a binary operator replaced (`==`/`!=`, `&&`/`\|\|`, `<`, `+`, `+=` …) | 9 670 |
| a function's body replaced by a value of its return type | 9 195 |
| a unary `!` or `-` deleted | 1 456 |
| a match arm deleted, where the match has a wildcard arm | 1 313 |
| a match guard replaced by `true` and by `false` | 678 |
| a field deleted from a struct literal that has a base expression | 122 |
| **total** | **22 434** |

By layer: `shared` 6 493, `features` 6 204, `app` 3 268, `screens` 3 203, `widgets`
2 293, `entities` 848, `main.rs` 125. Code under `#[cfg(test)]` and test functions
are not mutated.

What it does not have: a deleted statement or call, a changed literal or constant, a
swapped argument. It does not evaluate `cfg`: an item compiled out on Linux is
still mutated and reported as missed (its documentation calls this a bug).

Selection and scale: `--file`, `--re`/`--exclude-re`, `--in-diff <diff>` (the
mutants that overlap a diff's changed lines — for a function-body mutant that is any
line of the function), `--shard k/n`. Suppression: `exclude_re` and `exclude_globs`
in `.cargo/mutants.toml`, or `#[mutants::skip]`, which needs the `mutants` crate as a
regular dependency. It emits GitHub annotations for missed mutants when it runs in
Actions.

## 4. Measured on this repository

Measured on the droplet (4 vCPU, 8 GB) on `main` at `c56d4626`.

### 4.1 One mutant

| | |
|---|---|
| the suite, `cargo test` (5 155 tests, one binary) | 77 s |
| incremental build of the test binary after a one-token edit, by hand | 19–35 s |
| clean build and first run in a fresh checkout | 160 s + 100 s |
| a caught mutant, mean of 57 | 31 s build + 75 s test |
| a missed mutant, mean of 8 | 29 s + 74 s |
| an unviable mutant, mean of 7 | 11 s |
| a timeout, at `--timeout 300` | 31 s + 300 s |
| **mean over the 76 mutants run** | **108 s** |

A caught mutant costs the whole suite too: on stable, libtest runs every test of the
binary whatever fails. The build directory of the second checkout is 5 GB.

### 4.2 The tree, and a pull request

The whole tree: 22 434 × 108 s = **28 machine-days**. On hosted runners (free for a
public repository, six hours a job at most) that is about 115 jobs.

A pull request, by `cargo mutants --list --in-diff` at each of the last 100 merge
commits:

| | Mutants | One machine | Eight shards, less start-up |
|---|---|---|---|
| no mutant at all | 56 of 100 | — | — |
| median of the other 44 | 46 | 83 min | 10 min |
| 75th percentile | 111 | 3.3 h | 25 min |
| 90th percentile | 179 | 5.4 h | 40 min |
| the largest (PR 972) | 551 | 16.6 h | 2.1 h |

Each shard builds the crate itself before its first mutant; with the dependency
cache CI already keeps, that is a few minutes a shard. **Not measured on a runner.**

What it is added to: the `CI` workflow on a pull request from a code branch ends in
**8.3 minutes at the median, 11.2 at the 90th percentile, 12.1 at the most** (the
last 40 green runs on `feat/`, `fix/` and `refactor/` branches). A mutants job beside
it is the longest check of any pull request above the median, and by a wide margin
from the 75th percentile on.

### 4.3 What a run cannot judge

- **Code compiled out on Linux** — 172 mutants (0.8 %) sit in Windows- or macOS-only
  items (`osc11.rs`, `keys.rs`, `secrets.rs` lead). Each is a guaranteed "missed".
- **Lines no unit test reaches.** Line coverage is 90.7 % (SonarQube, `main`);
  among the 13 548 uncovered lines are the paths only the live gate walks. A mutant
  there survives, and coverage already said so.
- **Equivalent mutants.** One of the eight survivors in §5.2 changes a clamp no
  input can reach — the third outcome of lessons §2.

## 5. The probe

Both runs: `--in-place` in a disposable worktree (`git worktree add --detach`), one
job, `--timeout 300`, the whole suite for every mutant.

### 5.1 A module the hand check had covered

`src/shared/api/effort.rs`, 23 mutants, 47 minutes: **21 caught, 1 unviable, 1
timeout, none missed.** The hand-run check of that module left nothing for the tool
to find — except the timeout: with `send_asking_again` returning `Ok(None)`, 69
tests fail and two wait for ever.

### 5.2 A sample across the tree

`--shard 0/374 --sharding round-robin` picks 60 mutants spread over every layer. 53
were run before the session's container restarted and ended the run:

| Outcome | Mutants | Of the 47 viable |
|---|---|---|
| caught | 36 | 77 % |
| timeout | 3 | 6 % |
| missed | 8 | 17 % |
| unviable | 6 | — |

The eight survivors, read one by one:

| Mutant | What it is | Coverage said so? |
|---|---|---|
| `background_runs.rs:385` `==` → `!=` in `cancel_background_runs_of` | **a gap**: no test deletes a chat that has a background run | the loop's body is uncovered |
| `generation.rs:959` `==` → `!=` for `BRANCH_SEARCH_ID` | **a gap**: the flag is never told apart on a woken turn | no — the line is covered |
| `generation.rs:4883` `&&` → `\|\|` in `conversation_brief` | **a gap**: no brief ever passes its budget | the `break` is uncovered |
| `tools/self_model.rs:265` `GetSelfModel::parameters` → `null` | **a gap**: the schema sent to a provider is asserted nowhere | no — the line is covered |
| `screens/chat/input.rs:873` `&&` → `\|\|` in `popup_mouse` | **a gap**: a click on the local-servers picker is untested | the line is uncovered |
| `text_decode.rs:230` the `"es"` arm of `tld_hint` deleted | one row of a table of 33; minor | — |
| `voice_note.rs:190` `len - 1` → `len + 1` in a clamp | equivalent: the index never reaches the clamp | — |
| `runtime/input.rs:69` `normalize` in a `#[cfg(windows)]` function | platform noise | — |

Five of eight are real, and two of the five are on lines coverage reports as
covered. Carried over the tree with the width a sample of 47 allows (8 of 47 is
9–30 % at 95 %), that is somewhere between two and six thousand survivors, more
than half of them real if the eight are typical.

### 5.3 The timeouts

| Mutant | What hung |
|---|---|
| `effort.rs:180` `send_asking_again` → `Ok(None)` | `app::supervisor::tests::a_blank_model_field_sends_no_model_key`, `external_chat_sends_the_configured_model` |
| `reflection.rs:192` `!` deleted in `maybe_auto_reflect` | `orchestrator::tests::silent::a_reflection_round_displaced_by_a_turn_is_made_again` |
| `title.rs:222` `apply_title` → `false` | `orchestrator::tests::silent::the_fan_out_asks_for_the_title_then_the_roll_then_the_loops`, `the_title_displaced_by_a_turn_is_made_again`; `orchestrator::tests::subagent::a_landed_transcript_is_auto_titled_under_both_modes`, `a_requested_title_for_a_transcript_lands_on_it` |
| `input_box.rs:2181` `+=` → `*=` in `styled_line` | the mutant itself loops for ever; nothing to fix |

Seven tests wait without a bound. A regression of the kind each mutant imitates
would hang CI to the job's limit instead of failing with a name. The tool reports
this as *timeout*; the log's `has been running for over 60 seconds` lines name the
tests.

## 6. The tool beside the hand recipe

| | By hand (`crate::mutant(N)`) | cargo-mutants |
|---|---|---|
| who picks the mutants | the author of the change | the tool, every site of six operators |
| a deleted call, a changed constant | yes — most of them | no |
| an operator or a return value the author did not think of | no | yes |
| a mutant | ~25 s | 108 s |
| a typical change | 9 mutants, minutes | 46 mutants, 83 minutes |
| a hanging test | found when it hangs the run | reported, bounded by `--timeout` |
| noise | none — each mutant has its test | equivalents, platform code, live-only paths |

They overlap little. The hand check asks "does the test I wrote for this line fail
without it"; the tool asks "is there a line I wrote no test for".

## 7. Forks

**The owner's decisions, 2026-11-22: F2 (c), F3 (b), F4 (a), F5 — each as
recommended. F1 on a condition:** the time the job adds to CI, measured on hosted
runners (§8, stage 2). If CI grows by much, it is not adopted — the more so if there
is a risk of timeouts. **The measurement (§10) met the condition for a no: F1 is (a)
for a job in CI**, and F2–F5, which describe that job, have nothing left to apply to.

**F1. Whether to adopt it.**
(a) No — the hand-run check stays alone.
(b) **Yes, as a second pass beside it** — recommended. §5.2 found five untested
behaviours in 47 mutants and §5.3 seven unbounded waits, in code that had passed
every existing gate.

**F2. Where it runs.**
(a) The whole tree as a gate — no: 28 machine-days (§4.2).
(b) On the droplet before a pull request — no: 83 minutes at the median, serial, and
it holds the machine's memory against every other build.
(c) **In CI on a pull request's diff (`--in-diff`), sharded** — recommended; the
repository is public and the runners cost nothing.
(d) A scheduled sample of `main`, a different shard each week — a later addition,
not a start: it finds old gaps nobody is looking at.

**F3. Gate or report.**
(a) A gate — no: an equivalent mutant or a Windows-only function would fail a
correct change, and the answer would become a growing exclusion list.
(b) **A report** — recommended: annotations on the diff and the `mutants.out`
artifact; AGENTS.md §3 would ask that every survivor be answered in the pull
request — a test, or the reason it is equivalent or out of a unit test's reach.

**F4. How a mutant is excused.**
(a) **`.cargo/mutants.toml` only** (`exclude_globs`, `exclude_re`) — recommended to
start: no dependency.
(b) `#[mutants::skip]` with a comment, as `#[allow(dead_code)]` is used — needs the
`mutants` crate among the dependencies of a published crate. Only if the pilot shows
item-level noise the file cannot express.

**F5. The version.** Pin it (`cargo-mutants@27.1.0`, the installing action pinned to
a commit as `tools/actions_pin_check.py` requires). The set of mutants changes
between versions, and a tree clean under one is not clean under the next.

## 8. Proposed stages, if F1 is (b)

1. **What the probe found** — the seven waits of §5.3 bounded, a test for each of the
   five gaps of §5.2. Tests only; independent of every fork.
   **The waits: done 2026-11-22** (`fix/bounded-test-waits`) — in the shared helper
   and the shared accept rather than in the seven tests
   ([docs/journal/quality.md](../journal/quality.md)). **The five gaps: done the same
   day** (`fix/mutants-found-gaps`) — nine tests, each red against the mutant that had
   survived.
2. **What it adds to CI, measured on hosted runners** — the owner's condition for
   F1. Merged pull requests of the median and of the 90th-percentile size are
   replayed on a spike branch (`--in-diff`, eight shards, with and without
   `cargo-nextest`), and three things are read off: when the last shard ends, beside
   the 8.3 minutes CI takes today; how many mutants end in a timeout; whether any job
   comes near its own limit. **No-go** if the checks of an ordinary pull request end
   much later than they do now, or if timeouts are more than an occasional mutant.
   **Done 2026-11-22: no-go (§10).** Stages 3 and 4 fall away with it.
3. **A pilot**, on a go — `.cargo/mutants.toml` and a workflow of its own, started by
   hand (`workflow_dispatch`) on the next three pull requests that change `src/`.
   **Go** if at least half of the survivors are real gaps.
4. **On every pull request that changes `src/**`**, not a gate; the rule in
   AGENTS.md §3, a line in lessons §2.

## 9. Not measured

- **A `[profile.mutants]` with `debug = "none"`**, which the tool's documentation
  recommends for the build half.
- **The default copy mode** — both runs were `--in-place`.
- **The seven mutants of the sample the interrupted run did not reach.**
- **`cargo-schemata`** (0.5.0, on crates.io since 2026-11-09, 419 downloads): it
  claims to build every mutant once and switch them at run time, which is this
  project's own recipe automated. Too young to rely on; worth a look in a few months.

## 10. What it adds to CI — measured on hosted runners (2026-11-22)

The owner's condition for F1. A workflow on a spike branch (`spike/mutants-ci-probe`,
not merged) replayed two merged pull requests on `ubuntu-latest`: the merge commit
checked out, its own diff, and for each of eight shards

```
cargo mutants --in-diff pr.diff --shard k/8 --sharding round-robin \
  --in-place --baseline=skip --timeout 240
```

with the dependencies restored from a cache, as a pull request's job has them. Runs
[45027661046](https://github.com/vshylov/mindfork-rs/actions/runs/45027661046) and
[45038456895](https://github.com/vshylov/mindfork-rs/actions/runs/45038456895).

### 10.1 The time

| Replayed | CI today | The last of eight shards ends | A mutant, mean |
|---|---|---|---|
| PR 997, 51 mutants — the median | 8.3 min | **29.6 min** | 173 s |
| the same, `cargo-nextest` as the test tool | | 32.5 min | 181 s |
| the same, incremental compilation on | | **24.9 min** | 123 s |
| PR 993, 201 mutants — the 90th percentile | 11.2 min | **over 100 min** | 191 s |

- **The 90th percentile ran into the job's limit.** Seven shards took 68–91 minutes;
  the eighth was cancelled at the 100 the probe allowed it, one mutant short.
- **A runner is slower than the droplet at both halves**: a caught mutant's test run
  is 107 s against 75, and the unmutated suite took 169 s.
- **rust-cache turns incremental compilation off** (`CARGO_INCREMENTAL=0`), so in the
  first run every mutant rebuilt the whole crate — 99 s. Turned back on, a mutant's
  build is 43 s. The third row is the job at its measured best.
- **`cargo-nextest` does not help.** It stops at the first failure, but it starts a
  process for each of 5 155 tests: the unmutated suite took 265 s against 169, a caught
  mutant 130 s against 107.
- **A shard spends two to two and a half minutes before its first mutant**, and the
  shards end far apart — 13 to 30 minutes in the first run — because the timeouts
  fall where they fall.

To end with today's CI at the measured best, a pull request of the median size would
need about 18 shards and one of the 90th percentile about 70 — computed, for an even
split with no timeout — against the 20 jobs the account runs at once.

### 10.2 The timeouts

| Replayed | Viable | Timeout |
|---|---|---|
| PR 997 | 36 | 5 |
| PR 993 | 192 | 22 |

One viable mutant in eight, each costing the whole 240 seconds. Twenty-five of the 27
are an orchestrator handler replaced by `()` or a constant — `spawn_title`,
`handle_regenerate`, `switch_to`, `handle_command` — and what hangs is the tests'
shared `wait_for` (`app/orchestrator/tests/mod.rs`), which drains the event channel
until it closes: 519 call sites, against 4 of its bounded variant. The other two
(`MessageFeed::render`, `scroll_down`) loop in the widget's own tests.

A bound in that helper was tried on the droplet (60 seconds, free on a paused clock):
the 789 orchestrator tests stay green, and the four tests that hung for ever under
`apply_title → false` (§5.3) fail in a minute, by name. It does not make a mutant
that stops the whole loop cheap — under `handle_command → false` hundreds of tests
each wait their bound out.

### 10.3 What the condition makes of it

**A no.** The checks of an ordinary pull request would end three times later than
they do now at the job's measured best; a large one's did not end in 100 minutes in
the probe, a job reaching its limit there (about three quarters of that with
incremental builds — computed, not run); and one mutant in eight is a timeout.
cargo-mutants is not adopted as a job on pull requests.

What the measurement does not touch, and nobody has asked for: the tool run by hand
on the droplet over one module (`cargo mutants -f <file>`, 108 s a mutant, §5.1) when
a module is worth it, and a scheduled run that no pull request waits for. Both add
nothing to CI's time; neither is proposed here.

**Stage 1 of §8 stands whatever F1 is** — the unbounded waits and the five gaps are
defects in the tests the probes happened to find. The 22 survivors of PR 993's replay,
18 of them arithmetic in `widgets/message_feed.rs` and `feed_selection.rs`, were
read the same day (`fix/feed-landing-gaps`): all 22 still survived on `main`; 3 are
equivalent, and the other 19 were ten behaviours of the feed's landing and of the
orchestrator's refresh that no test held — nine tests and an assertion in a tenth,
after which the 19 are caught
([docs/journal/quality.md](../journal/quality.md)).
