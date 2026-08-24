# Lessons

Traps this project has already hit, and the practices that came out of them.

The engineering journal (`CLAUDE.md`, split into `docs/journal/`) records every track
in full. While it was one file it was loaded whole into every session, so an agent
working on *anything* incidentally knew about every trap the project had ever hit.
This file replaces that serendipity: the subset that would change what you do on an
**unrelated** task.

Read it before implementing, not after. Each entry is a rule, the incident that
produced it, and a pointer to the journal entry holding the full story. Where a trap
is recorded more than once, that is noted — the recurrence is the evidence of how easy
it is to hit.

Sections 1–4 apply to any task and are the ones to read first; 5–10 are area-specific,
so read the section that matches what you are touching.

1. Process and tooling traps — delegation, git, scripts, pipes
2. Testing discipline — mutation testing, vacuous and wrong-premise tests
3. Measure; do not assume — instruments that lied, plans the numbers overturned
4. The recurring defect class: a message must close the door
5. Terminal and ratatui rendering — wide glyphs, redraw, clusters
6. Windows and cross-platform — paste, `PATHEXT`, keyboard protocols
7. i18n, gates and localization — the two axes, dynamic keys, scanner blind spots
8. Storage, schema and data safety — migrations, additive change, backups
9. Live runs and model behaviour — fixtures, controls, go/no-go
10. CI and infrastructure — caches, minutes, gates that pass for the wrong reason

---

## 1. Process and tooling traps

**Never `git checkout -- <file>` to undo a scripted mutation.** It discards *all*
uncommitted work in that file, not just the mutation. Commit first, back the file up,
or apply the mutation as a reversible patch. **Recorded four times**: it cost a
`Cargo.toml` feature trim plus the whole of `code.rs`, then all of `apply.rs`, then a
third file that had to be reconstructed by hand — and then, in the session that had
this very entry in context, an hour of a stage's work in one file, because the
mutation being undone was a *one-line* experiment and the revert looked
proportionate to it. That is the shape of the trap: the command is scoped to the
mutation in the author's head and to the file on disk. What made the fourth cheap
was accidental — every edit had been applied by a script kept outside the tree, so
re-running three scripts rebuilt the file. **Make that deliberate**: commit before
mutating, and treat "I will just revert it after" as the moment to commit rather
than the reason not to.
— *vendored syntax grammars for 19 languages*, *MCP servers in the settings window*,
*collapsible tool calls, and the collapse state per chat*, *the code workspace —
stage 2*.

**Keep subagents out of a second build of the same crate.** An agent building in a
*copy* of the tree poisons the shared `target/`, so `cargo test` runs artifacts
compiled from other sources — seven tests "failed", six with `Cargo.toml` /
`Cargo.lock` / `artwork` **not found** (`CARGO_MANIFEST_DIR` baked in from the copy).
`cargo clean -p mindfork-rs` fixed it. Treat a cluster of path-not-found failures as a
build-artifact symptom, not a code one. Cost ~20 minutes of false debugging.
— *embedding-model change — stage 2 (re-embedding in place)*.

**Never pipe a command whose exit code you are checking.** `cargo clippy … | tail`
reports **`tail`'s** exit code, so a `&&` chain sails past a failing gate — that is how
a commit landed with clippy red. Same shape with `./mindfork-rs restore … | tail`,
where a refusal read as `exit=0`. **Recorded twice.**
— *in-feed text search (`Ctrl+F`)*, *password-protected backups*.

**Re-apply and re-verify your own edits to any file a running agent owns.** An agent
finished writing a file *after* the main session had edited it and silently reverted
the edit — a section-number fix the first time, an i18n key rename the second.
**Recorded twice.** Give parallel agents disjoint file sets; split large files at
heading boundaries so they cannot conflict.
— *confirmation before dangerous tool calls*, *history compression — stage 1*,
*English source-language migration*.

**A batch-patch script that asserts mid-batch and dies writes nothing** — silently
losing the whole batch. It cost an early return in `handle_key`, found by a test rather
than by eye. Validate the whole batch before writing any of it. Related: parse a
structured file before writing it back — a locale bundle value may be a string *or* an
array of strings, so a single-line regex edit mangles multi-line hints.
— *in-feed text search (`Ctrl+F`)*, *the assistant can watch a YouTube video*.

**Renumbering a spec section: grep by keyword, not by file**, or you renumber the
neighbouring feature's references too.
— *confirmation before dangerous tool calls*.

**The `(.+?)\s*$` trim idiom is quadratic in Python; pin both edges with `\S`.**
`.` matches whitespace too, so the lazy group and the trailing `\s*` re-split the
same tail on every expansion — the shape SonarQube flags as S8786. It cost
`link_check.py` its code-span regex (rewritten as a manual scanner), and the very
next tools script shipped it again in two title patterns, measured at 397 ms on
one 8 KB line. Capture `(\S(?:.*\S)?)` instead, or trim outside the regex; atomic
groups need Python 3.11+, which the tools cannot assume. **Recorded twice.**
— *SonarQube backlog — triage + stage 1 (Python tools)*, *SonarQube follow-up —
the doc gate's regexes and one test's complexity*.

**A new `tools/*.py` that takes a CLI path must canonicalize and confine it
before touching the file system.** SonarQube's agentic-workflows rule
(`pythonsecurity:S8707`) reads every `open`/`mkdir`/`read_text` on an unvalidated
argparse path as an injection sink — two of them dropped a PR's new-code
security rating to C and blocked the gate. The fix it accepts: `Path.resolve()`
then `is_relative_to(base)` against the directory the tool is meant to work in —
not a `startswith` prefix test, which the rule's own documentation calls out as
the partial-path-traversal pitfall.
**And the guard has to sit next to the call it protects**: the same family fired
twice more on a script that ran `cargo test <name from argv>` — first
`pythonsecurity:S8701` for interpolating the name into a `shell=True` command
line, then, once that became an argv list, `S8705` on the surviving path,
because the name was validated in `main` while the subprocess call lived in
another function and a taint analysis does not follow a guard across that
boundary. What it accepts is validating immediately before use and passing the
**match object'''s own output** rather than the string it came from. The rule is
right for a human reader too: a check one function away reads as safe and is one
refactor from being gone. **Recorded three times.**
— *demo screenshots — stage 1*, *the code workspace — stages 0 and 1*.

**The `LICENSE` file is a machine input; keep it byte-identical to the canonical
text.** An addendum appended to it — however clearly marked as "not part of the MIT
License" — makes the `MIT` SPDX identifier published in `Cargo.toml`, `nfpm.yaml` and
the README badge untrue, and license scanners that match by *similarity* (GitHub's
`licensee`, distribution audits) start reporting "Other"; MIT is short enough that a
few lines is a large fraction of it. Anything the project wants to say around the
license goes in a separate file, and a gate test pins the license file's shape.
— *a disclaimer for what the models say and do*.

**To re-render the screenshots you need JetBrains Mono as TTF, and the machine
probably does not have it.** The family is deliberately not vendored, so
`tools/screenshots.py` probes the system and exits when the probe misses. Do not
go hunting for an installer: the woff2 faces the website already ships
(`site/static/fonts/`) are the **full** family, not a subset, and `fontTools`
(with `brotli`) writes them back out as TTFs for `--font-dir`. Confirm the faces
are faithful before trusting a regeneration — re-render at the *old* settings
first and check the images come back byte-identical, which is cheap because the
pipeline is deterministic.
— *the canvas padding measured in cells*.

---

## 2. Testing discipline

**Mutation-test every load-bearing line, and read a surviving mutation twice.** A
surviving mutation indicts the *mutation* as often as the test: one "survived" only
because it had been applied to a different section than the test measured. Conversely,
nine mutations in one change caught eight defects and the ninth exposed a genuinely
untested claim, without which the whole fix was never consulted. A third outcome is
neither: the mutation is real but **no realistic input distinguishes it**. Loosening an
exact string match to `contains` survived because no provider error name contains
another as a substring — the fixture, not the code, was the problem. The fix is to
assert the *input contract* instead (feed it the one shape a call site could plausibly
confuse it with), or to stop claiming the property in the comment.
— *self-model injection — per-section budgets*, *MCP servers — secrets for the `env`
map and JSON import*, *engine failures stop being silent*.

**A test that waits on an event must bound the wait, or a regression hangs instead of
failing.** The orchestrator's `wait_for` helper blocks until the event channel
*closes*, so removing the code under test made a run sit past ten minutes; wrapped in
a five-second `timeout` it fails immediately and says what it was waiting for. In CI a
hang reads as broken infrastructure rather than a broken promise, which is the worse
of the two failure modes.
— *engine failures stop being silent*.

**On a path built to degrade gracefully, `is_ok()` can never be the assertion.** A test
asserted `is_ok()` on a tool that turns an index failure into a normal answer, so
bypassing the query escaping entirely still passed. Assert *which* answer comes back.
— *history compression — stage 3 (the read-back tools)*.

**Beware the vacuous assertion — a value equal to the type's default proves nothing.**
A language assertion passed with the seeding code removed, because the builder defaults
to the same value the config deserializes to. Two more in the same change: a "don't
rewrite" test compared bytes of a file the program had itself written (a rewrite
reproduces them exactly), and a "corrupt config" test never reached the guard it was
meant to exercise. All three found by mutation, not by passing.
— *the Windows installer can provision the Python sandbox*.

**Tests that assert an absence can pass for the wrong reason.** Converting a fixture to
in-memory storage failed six tests loudly — and two more *kept passing*, because they
assert an absence and an always-empty store satisfies them either way. The hazard is
not the six that shout, it is the two that do not. Elsewhere, two specified checks
turned out structurally incapable of failing at all.
A third shape: an assertion that a *debounced* side effect did **not** fire passed with
the defect applied, because the test's `Quit` was handled before the debounce expired —
the wrong restart was queued and simply never flushed. Absence has to be read against a
later event that proves the flush ran (follow it with a change that *does* fire, and count
against **that**).
— *orchestrator fixtures — two convertible, the rest not*, *full-text search over chat
content — stage 1*, *the external server's API key, entered in settings*.

**When a test and the code disagree, work out which is wrong.** Several times the
*test* was fixed: an assertion that HTML-block lines fit the panel width invented a
promise the writer never made. Equally, when the *plan* and the code disagree, fix the
code — a screen hardcoding one sort order while the plan said "your existing sort" was
invisible only because the default toggle matched.
— *raw HTML blocks render their text*, *collapsible tool calls*, *chat content search —
stage 2*.

**A guard that lives next to the call site is a habit; a guard inside the type is an
invariant.** The address policy shipped first as a check function each caller had to
remember, with the risky path (`reqwest::Client::get`) still in reach — and the wire test
promptly connected to `127.0.0.1` through a fully "guarded" client, because `hyper` parses
an IP-literal host itself and never consults the DNS resolver the guard lived in. Wrapping
the client so the unchecked path is unreachable is what fixed it. When a security check has
an "and also call this" step, assume the step will be skipped, including by you.
— *an address policy for model-chosen URLs*.

**A gate that passes is indistinguishable from a gate that does nothing.**
Mutation-test gates in *both* directions: re-break a link and it must go red; a file of
only code-span links must stay green; add one real broken link and it must go red
again. Same for "code equals asset" gates (a glyph table against its SVG, a manifest
against vendored files).
— *broken documentation links*, *branding — logo and wordmark*, *live-smoke diagnostic
log in English*.

**Check that a gate can see its subject at all — untracked files are invisible to
most of them.** `link_check.py` and `cyrillic_scan.py` walk **git-tracked** files.
While a new directory is untracked they report *clean* without ever reading it: the
documentation refactor got three consecutive green `link_check` runs over a
`docs/journal/` holding **106 broken links**, and the gate only spoke up after
`git add`. When you add a tree, stage it before you believe a gate about it, and
prefer a filesystem walk to a git listing when writing the fix.
— *the documentation refactor — CLAUDE.md became a router*.

**A provider's *optional* output cannot be an assertion — assert the deterministic
half.** A smoke required Anthropic's `display:"summarized"` to yield non-empty
"thoughts"; measured, the turn returns `thoughts=0, signature=380, text=50` — thinking
really ran (a signature that long only exists for a real thinking block) and the
provider simply summarized a short one to nothing. Assert the signature, which is empty
the moment `thinking` leaves the request, and *log* the summary. Weakening an assertion
is only safe if the remaining one can still fail: verify that with a live mutation.
— *engine failures stop being silent*.

**A smoke's target can trigger the model's own refusal, and then it measures nothing.**
An address-policy smoke aimed at `169.254.169.254` had the model decline on its own —
*"I am not permitted to access internal network addresses"* — without ever calling the
tool, so the guard under test never ran. What caught it was asserting that the tool **was**
called; without that the run reads as a pass. Pick a target the model has no opinion about
(a plain loopback service the user might ask about), and keep the "it was actually
exercised" assertion. Same family as the fixture traps below.
— *an address policy for model-chosen URLs*.

**When a live smoke fails, suspect the fixture before the feature.** One took **four**
attempts, each failing its own precondition: the model could answer from memory; the
target was the only named entry, so the summary kept exactly what had to be lost;
sixty entries were compressible by structure, so the fixture went to 200 against a
120-word limit — arithmetic rather than a hope about the model's judgement; and finally
**the control question was itself the confound**, making the target the most salient
thing in the range. Note the mirror image: in a sibling smoke the control *helps*,
because that test wants the fact kept. The same device is an aid or a confound
depending on which way the assertion runs.
— *history compression — stage 3 (the read-back tools)*, *… — stage 1*.

**Assert the symptom or the property, not the mechanism.** A highlighting test asserts
the block carries RGB colours and no reverse-video line style — the path taken, not the
table lookup. A degraded-message test asserts every locale names two stable tool ids,
not the prose. A rendering gate is behavioural: no diff update may target the second
half of a wide glyph.
— *Zig code blocks are highlighted*, *`youtube_watch` — a degraded answer has to close
the door*, *emoji popup — a "hanging" selection ghost after closing*.

**Some bugs are only visible on the second pass.** Hidden chats were re-parsed on every
startup because "forget" dropped the bookkeeping too — 43 of 171 chats, 27 ms per
launch. A single-pass test cannot see it.
— *full-text search over chat content — stage 1*.

**A default-on background engine call eats scripted test engines out of turn.** The
automatic chat title — one extra request after the first exchange — broke 8 orchestrator
tests at once: a finite `sequence` script had an entry consumed by the title task, and
`CapturingBackend::last` held the *title* request where the assertion expected the
turn's. The failures are loud but read as unrelated. Fixture-driven tests opt out via
`no_auto_cfg()` (one line, at the fixture), while the trigger's own tests run the true
default; any future automatic background call (auto-summary, auto-anything) re-creates
the same interference and should budget for the same opt-out at design time.
— *automatic chat titling on the first exchange*.

**A blocking `join()` on a stub thread deadlocks a `#[tokio::test]` whose work is a
spawned task.** A test paired an OS-thread TCP stub with the readiness probe the code
spawns, then called `JoinHandle::join()` — that blocks the current-thread runtime, so the
probe never gets polled, never connects, and the stub waits on an `accept` that will not
come; the run sat past five minutes instead of failing. `tokio::task::spawn_blocking(move
|| h.join())` awaits it instead and yields, so both sides progress. The neighbouring test
that looked identical was fine because it `await`s the request itself rather than letting
the code spawn it — which is exactly the difference to check before copying such a
fixture. Related: **read a stub's request to the end of the headers, not into a fixed
buffer**, whenever the value under assertion can be any length — a 2 KiB read truncated a
`PATH`-sourced key and the comparison failed on the tail, which reads as a resolution bug.
— *the external server's API key, entered in settings*.

**A `contains()` assertion over a rendered buffer cannot see a layout defect —
render the screen once and look at it.** Fifteen passing tests said the changes
screen drew its file list, its diff, its hunk headers and its confirmation; what
it actually drew ran the two panes together (`+12 −4@@ -940,7 +940,9 @@`) and left
the counts column ragged, because each row sized it to its own text. Every
assertion was true and the screen was unusable. A single `println!` of the
rendered buffer, read by eye, found both in a minute — and both then got tests
that pin the **symptom** (no `−4@@` in the output; two rows' counts ending at the
same column) rather than the layout arithmetic. Budget one look at the real
render before calling a screen done; text assertions verify presence, not
composition.
— *the code workspace — stage 4*.

**A process-wide gate in production code makes sibling async tests fail each
other.** The code workspace runs one project command at a time through a
`static Semaphore` — correct for the application, and it means two
`#[tokio::test]`s that spawn commands refuse each other with the "busy" message,
with the loser decided by the scheduler. It reads as a flaky feature and is a
flaky *test harness*. Make such tests queue behind a `SERIAL` mutex of their own,
and assert the gate's behaviour in a test written for it rather than meeting it
by accident. The rule generalizes to any process-wide singleton the tests touch:
if production says "one at a time", the tests have to say it too.
— *the code workspace — stage 3*.

**An assertion that reads a rendered screen at scroll 0 is an "everything fits"
assumption, and it fails as "the thing is missing".** Two rows added to the help
overlay pushed `/tts` below the fold, and the test said `missing the /tts
command` — which points at the command, not at the page. Reading at both scroll
extremes was still not enough: the tab had grown past *two* screens, so five rows
were invisible in both frames. Either render tall enough that the frames overlap,
or assert against the model behind the screen. The failure mode is worth
recognizing on sight: a render test that names a long-standing item as missing is
usually reporting a layout change.
— *the code workspace — stage 3*.

**Instrument traps to know by name.** ratatui's `Buffer` `Debug` prints row content
**without escaping quotes**, so an assertion containing `"` against
`format!("{:?}", buffer)` can never match — a render test written that way passed while
the popup showed raw JSON; join the rows by hand. Extract panel text *between* the
borders, or a border glyph lands between joined rows and breaks a match on wrapped
text. `wait_for` **drains** events, so an earlier one must be pulled before waiting on a
later one or the test hangs.
— *confirmation before dangerous tool calls*, *a settings hint always fits its panel*,
*plugins — stage 3a: MCP host core*.

**A hand-rolled HTTP stub must say `Connection: close`, or the client pools a socket the
stub has already dropped.** A one-connection-at-a-time stub that answers and closes is
telling the truth only if the response says so: without the header `reqwest` reuses the
connection for the next redirect hop, and the request fails in transport instead of
following. It is **Windows-only in practice** — closing a socket with unread bytes sends
RST there rather than FIN, and a reset is a hard error where a FIN is just a stale pooled
connection the client silently reopens. Two redirect tests were green locally and on
Linux and red on the Windows runner; neither machine reproduces the other, so the CI run
is the instrument.
— *images in a message — attach by URL*.

**A slow test is usually paying a real cost, not misbehaving.** A "probe a dead port"
test ran **63 s**: a connect to a closed local port costs ~2.0 s on Windows (SYN retry)
and the probe retries 30 times. Virtual time was already working — the *connect* was
the cost. Making the stub actually listen took it to 2 s.
— *readiness probe for the embedding server*.

**A committed render that shows a calendar date shows it in local time — keep fixture
timestamps mid-day UTC.** The screens format dates through the local offset, so a
narrative segment stamped 21:55 UTC rendered `[2026-07-31]` on the regenerating machine
(UTC+3) and would render `[2026-07-30]` on CI (UTC) — a gate that compares committed
renders goes red on one side or the other. Mid-day UTC times are stable across every
timezone the dumps travel between. Corollary: a screen that lists items newest-first
*by insertion* needs its fixture vec appended in chronological order, or the visible
dates scramble.
— *demo screenshots — a uniform gallery and a richer hero*.

**The duplication gate matches *normalized* tokens against a *density* bar — bulk
fixture data belongs in one flat literal.** A PR failed at **24.7%** new-code
duplication (bar ≤ 3%) with every literal different: repeated constructor blocks and
same-shape one-line tuple rows are sliding self-duplicates once literals are
normalized away — `jscpd`, which compares exact tokens, reports 0% on the same files,
which is how you tell the two models apart. And because the bar is a density, shapes
that big PRs got away with fail a small PR. Hoist table-shaped fixture data into one
raw-string literal parsed by a few unique lines: one string is one token, and there is
nothing left to match. **Recorded four times**. The second time it was right about the
*code*: a clipboard-paste PR failed at 8.2% because the paste handler was the attach
handler copied with a different pixel source — the fix was a shared seam that made the
source the only difference, and the same seam then absorbed a third source (a URL) for
almost nothing. The third was a **production const table**: regrouping the help tables
into nested slices rewrote every long-lived tuple row, and 50 same-shape rows in changed
lines scored 16.1%. Keeping the rows byte-identical to `main` and inserting an
identifier-sentinel row between the groups still failed at 3.3% — the tables' line
ranges stay flagged whatever token the inserted line carries, and each inserted line is
a *new* line inside a flagged range. Structure a stable table **from outside it**: the
breaks moved to a separate labels const the renderer consults, the tables carry zero
new lines, and a gate pins each named label to exactly one row (the desync the
indirection trades for).
The fourth was three small copies at once: a scoped twin of a search query, a sibling
reader's parameter schema, and a live smoke's bootstrap prologue — each "the same
tokens with one difference" — summed to 4.7% on a PR that added them all. The fix was
the paste-PR seam three times over: one SQL body with an optional `IN`, one
paged-reader schema helper next to `search_parameters`, one narrow-profile smoke
helper. When a new thing is a sibling of an existing thing, budget for the seam at
design time — the pair's *contract* was shared from the start, its boilerplate was not.
The **sixth** is the same mechanism as the third, in a place easier to walk into: a file
whose *existing* code is triplicated (three sections with identical `cloud()`/`cloud_mut()`
accessors) has no safe place to add a method — a six-line addition to each landed **inside
the already-flagged ranges** and 18 lines counted as duplicated, at 3.8% against a 3% bar,
though nothing was copied from anything. Before adding a sibling method to a family of
same-shaped types, check whether that family is *already* flagged; if it is, the addition
belongs outside it — here one trait holding the decision once, with each type contributing
two one-liners. Adjacent small impls are safe when what differs between them is
**identifiers** (type, enum, constant), which the detector does not normalize — the earlier
cases were invisible precisely because only *literals* differed.
The fifth came from **test fixtures written in the same PR**, and scored the worst yet
at **19.8%**: six tests of one back-stack, each spelling out the five locals `dispatch`
and `apply_event` take plus the same three-step "arrive here" prologue. Nothing was
copied from older code — the copies were of each other, written minutes apart, which is
exactly the shape that reads as thorough while being sliding self-duplication. A test
harness (one struct owning the loop's state, one method per step) collapsed each
prologue to two lines with no test losing a word of what it asserts. **The rule
generalizes past production code: if the third test starts the same way as the first
two, that opening is a fixture, not a test.**
The **seventh** was that rule ignored in the PR whose author had just re-read it: two
e2e prologues in one new test file — a `MockBackend::sequence` of same-shape
`vec![Text, Finished]` blocks plus spawn-and-activate, written minutes apart — scored
6.0% against the 3% bar, and also matched the *impersonation* suite's prologue across
files. Collapsed the same way: a one-line `script()` and one spawn fixture. The gate
fired **after** the PR was opened, so check the Sonar analysis before calling a PR
done, not after the reviewer does.
The **eighth** hit both known mechanisms in one small PR (4.6%) and adds the tool for
seeing them before the push. Half was the fifth again — three launcher tests written
minutes apart sharing a write-parts/launch/assert opening, collapsed into one fixture.
The other half was the sixth **from the other side**: what lands in an already-flagged
range need not be new code at all. Two identical `active_model_name` copies sit inside a
range `cloud()`/`cloud_mut()` already have flagged, so *rewriting* their five-line arm
wrote six new lines into it — and extracting a shared helper made it worse, both call
sites becoming the same six lines. What passed was the smallest edit: one changed line
inside each copy, the rest byte-identical to `main` (an import is what kept the arm on
one line). **When the flagged pair is old code and not your PR's subject, minimize the
lines you touch instead of fixing it.** The tool: a ~40-line throwaway script that
normalizes literals away, hashes every 10-line window across `sonar.sources` and reports
the windows containing changed lines — it read **1.2%** where Sonar then read **2.1%** on
the same tree, so it finds the right blocks and under-reports the density. Iterate on it
offline, but leave margin against the 3% bar rather than stopping at the first number
under it.
— *demo screenshots — a uniform gallery and a richer hero*, *pasting an image from the
clipboard*, *images in a message — attach by URL*, *the help dialog sizes itself, and
its tables align*, *cross-chat search for the assistant*, *`Esc` retraces a followed
`chat://` reference*, *the external server's API key, entered in settings*, *automatic
chat titling on the first exchange*, *a model split across several GGUF files*.

---

## 3. Measure; do not assume

**Assume the measurement will overturn the plan, because it repeatedly has.** Defender
was blamed for CI slowness and the runner reported `RealTimeProtectionEnabled = False`;
vendoring grammars was predicted to *shrink* the binary and added 181 KiB (LTO already
dropped the unused dumps); an HTML fix estimated at 77% of a gap closed 26%; a media
resolution setting documented as a 3x saving is a **no-op** on newer models. The number,
not the reasoning, decided each design.
— *cutting the Windows CI job from 19 minutes*, *vendored syntax grammars for 19
languages*, *raw HTML blocks render their text*, *`youtube_watch` — a degraded answer
has to close the door*.

**Use the project's own client as the instrument, not curl and awk.** A curl+awk cosine
measurement returned corrupted values (0.97 on everything, 1.000 on antonyms); the real
numbers came from `OpenAiClient`. A CHANGELOG-extraction "bug" was an artifact of a
quoted heredoc collapsing `\\[` to `\[`, turning an awk regex into a character class —
`od -c` on the real file showed the workflow was always correct.
— *narrative as notes — Tier 2, step C*, *the mindfork.io site URL in project
metadata*, *impersonation sends the compacted conversation*.

**An instrumentation bug looks exactly like evidence.** Two diagnostics "proved" usage
events were missing; in fact the test helper drains events up to `Finished` and had
already consumed them — twice. Only isolating the question to the client answered it.
Same family: two provider probes were wrong fixtures (a trailing assistant turn is a
*prefill*, not a rejection; a 64-token cap starves reasoning models because thinking
counts against it), and the control arm is what made both diagnosable.
— *history compression — stage 2*, *impersonation sends the compacted conversation*.

**Calibrate thresholds from live data; never by analogy.** A similarity gate set to
0.85 by analogy silently missed genuine paraphrases (0.77 on a real pair); measuring
gave paraphrases 0.73–0.83 against unrelated 0.51–0.69, hence 0.72. The second-order
finding matters more: such constants are positions inside *one model's* distribution —
on another embedding model an unrelated pair scores 0.751 and an antonym 0.887, flipping
every gate from "silently never fires" to "fires on everything".
— *narrative as notes — Tier 2, step C*, *self-model consolidation — stage A2*,
*embedding-model change — stage 3 (per-model similarity thresholds)*.

**Exercise decision logic against fabricated data before spending anything.** A
sweeper's logic run against a fake listing caught a real bug: the fractional-second
truncation in the timestamp parser also ate the timezone offset's digits, so **every**
endpoint would have read as "age unknown" — a main path, not a corner.
— *the remote live e2e gate on HF Inference Endpoints*.

**Beware APIs that silently coerce instead of rejecting.** An endpoint-type field
accepted three values and quietly coerced an unknown one, producing a network-isolated
endpoint no CI runner could reach — with a healthy-looking 200. The client now refuses
to continue when the echoed value differs from the requested one.
— *the remote live e2e gate on HF Inference Endpoints*.

**Read the existing code before designing; it repeatedly shrinks or reshapes the
task.** Reading first turned a "one-line gate" into the discovery that the agentic loop
only *emits* events and needed a reverse channel; turned an editor into a change needing
no new command, event or storage; and turned a planned embedding index into reusing the
full-text index that already covered every message. It also *found* defects up front — a
startup path that would have discarded the installer's language choice, and a staleness
comparison that would have left a server running with an old secret. It equally invalidates
premises the project *itself* wrote down: a deferred note said attaching an image by URL
would inherit "the SSRF care `fetch_url` already had to take", and `fetch_url` validates the
scheme and nothing else — the guard to be reused did not exist, so the question had to be
answered rather than inherited. Treat a recorded rationale as a claim with a date on it.
**Recorded three times**: an accepted design doc likewise stated that deleting a profile
was "already confirmed today" — it is not, `Ctrl+D` deletes the selected row outright —
which turned a parenthetical into that track's one deliberate parity break, needing a
justification of its own. The claim was a day old and written by the same author. The
third time the item under attack was a *roadmap entry*: OSC 52 was filed with JupyterLab
as its motivating case, and JupyterLab embeds xterm.js **without** the clipboard addon, so
it drops the escape — the feature was still worth building, but for a different host, with
a different failure to fix (over plain SSH the old behaviour was not "the wrong clipboard"
but "no copy at all"). Check the *beneficiary* of a feature, not only its mechanism.
— *confirmation before dangerous tool calls*, *MCP servers in the settings window*,
*history compression — stage 3*, *images in a message — attach by URL*,
*command-only control — stage 2*.

**A headless Chromium screenshot narrower than ~500px fabricates overflow on
Windows.** `--window-size=375` (old and new headless alike) still lays the page
out at the OS minimum window width and then crops the image to the requested
size, so text appears cut mid-word at the right edge — a defect the DOM does not
reproduce (`scrollWidth` equalled the viewport in a real 375px browser). Judge
narrow-viewport layout from DOM geometry, or capture at ≥500px and reason from
the breakpoints.
— *website — the hero fetch panel*.

**A `200` is not proof a parameter works — send a wrong type to find out.** xAI
silently ignores fields outside its request schema, so `{"totally_bogus_field": 1}`
and `{"top_k": 40}` both answer `200`; sending `"top_k": "banana"` separates them —
a field the server actually knows fails deserialization (`422`), an unknown one is
still dropped. That one probe turned four "supported" sampling knobs into two
honoured, two silently discarded, and kept the settings UI from offering knobs that
do nothing. Applies to any permissive API, not just xAI.
— *Grok (xAI) as a cloud provider*.

**A value the orchestrator sets on its own turns can break only the background.**
`reasoning_effort: "none"` is never typed by a user — the app sets it for title
generation, compaction and impersonation. A provider that rejects that one *value*
(xAI does) leaves ordinary chat working while exactly those three fail, which reads
as "the app is flaky" rather than "the provider disagrees". When adding a provider,
enumerate what the code sends without being asked, not just what the settings expose.
— *Grok (xAI) as a cloud provider*.

**Record sub-decisions before implementing, then re-read them.** Writing them down is
what caught a hole in one: the argument for a single wording held for the chat-level
gate but not for a profile that can switch the tools off — precisely the failure the
wording existed to prevent.
— *history compression — stage 3 (the read-back tools)*.

---

**When a measurement decides whether something ships, budget more for the
instrument than for the thing.** The code-workspace track's semantic index took
one afternoon to build as a probe and **six revisions of the measurement** to
get an answer — and every defect was found by reading the raw turns, never by
reading the summary table, which looked plausible every single time. In order:
the tool under test was never called at all; one pass was treated as a rate (the
control scored 5/5 then 3/5 on the same questions); two questions were padding
both arms could skip; "wrong" conflated a retrieval miss with an empty turn and
with a turn that never looked; the grader had false positives, then false
negatives once tightened; and the harness was starving both arms at the default
token ceiling. Four of the six moved the headline number, two reversed its
direction. The rule that falls out: **before believing a table, read enough raw
trials to reconstruct one of its cells by hand.**
— *the code workspace — stage 5*.

**An effect that changes sign when you change a denominator is smaller than your
instrument.** The same 96 turns said the index was ahead (19/48 against 16/48 of
all turns) or behind (19/29 against 16/22 of the turns that looked), depending on
whether an unusable turn counts as a failure. That is not a result to argue
about — it is the measurement telling you it cannot resolve the question. Say so
and stop, or build a better instrument; picking the flattering denominator is how
a feature ships on nothing.
— *the code workspace — stage 5*.

**A keyword grader cannot resolve an open-ended prose answer.** Loose markers
graded a general-knowledge guess *correct* on a turn that made no tool call at
all (it matched `mutex`); markers tightened to literals only the source contains
then graded "machine-bound encryption … ADR 0008" *wrong* for missing `dpapi`.
Every marker set trades false positives for false negatives, and the residual
error lands in the same range as the effect being chased. A keyword marker is
fine for "did the model reach the one fact only this file has"; it is not an
instrument for "was this answer good". That needs a judge model, and budgeting
for one is part of designing the measurement, not a fallback.
— *the code workspace — stage 5*.

## 4. The recurring defect class: a message must close the door

**Never let a message describe a situation without saying what is and is not possible
next.** This is the project's most-repeated defect — **five separate instances**, each
found live, each costing a user a wasted turn:

- A **by-reference attachment block** stated the situation without stating the route,
  so the model improvised with `fs_list`, `fs_read` and four `web_search` calls and
  ended on a suggestion the user could not act on.
- **`youtube_watch` unconfigured** said what was missing and how the *user* could fix
  it, but not that the content was unreachable by any other route. The model spent
  **eight tool calls** rediscovering dead ends the project had already measured —
  including `pip install` in a sandbox with no pip, and a signed captions URL that
  returns 200 with an empty body.
- **`python_exec`** never said each call gets a fresh sandbox, so state written to
  `/tmp` vanished between calls with no explanation.
- An **MCP status row** read `ready · tools: 1` while the assistant said it had no such
  tool — the double opt-in working as designed, with nothing on screen saying so.
- The **OSC 52 copy note** told a JupyterLab user their terminal "may not support the
  sequence" and stopped there. Every word of it was true and carefully hedged — the
  protocol really does not acknowledge, and the research really had established that
  JupyterLab drops the escape — but the message named no way forward, and `/export`,
  built in the very next track for exactly that host, went unmentioned. The lesson has
  a sharper edge than the earlier four: **an honest description of a dead end is still
  a dead end**, and hedging ("may not") is not a substitute for a route. Watch for it
  in the messages of a feature whose *whole point* is that it can fail invisibly.

The fix is always the same: name the routes that do *not* work, and the one that does.
Two corollaries. **Only advertise what exists** — an attachment entry names the search
tool only for a file that really has an index. And **distinguish "nothing here" from
"no hits"**, since they call for different next actions. An error pointing at a command
that would refuse is the same bug in politer clothing.
— *chat file attachments — stage 2 (`attachment_read`)*, *`youtube_watch` — a degraded
answer has to close the door*, *page fidelity for `fetch_url`*, *MCP servers in the
settings window*, *history compression — stage 2*, *OSC 52 — the note that did not
close the door*.

**`let x = thing?` inside a handler that owes the user an answer turns a refusal
into silence.** `/rename` began `let id = self.active_chat?` — with no chat open
the `?` returned `None` from the whole command, so the app did nothing and said
nothing. The bug shipped *inside the very track built to remove that silence*, and
a test caught it, not review: `?` reads as "nothing to do here" while the
surrounding function's contract is "always answer". Wherever a function's job
includes explaining itself, spell the early exit out (`let Some(x) = … else`) so
the explanation has somewhere to live.
— *command-only control — stage 1*.

**A promise with nothing behind it is the same lie in miniature**: a collapsed tool card
with no arguments and no result gets no "expand me" pill.
— *collapsible tool calls, and the collapse state per chat*.

**A note the feed has not been built yet cannot say anything.** A startup message sent
before `bootstrap` is wiped by the `activate` inside it, which rebuilds the feed from
the chat's messages — the event is delivered, the note is gone, and the door the text
was written to close is open again. Anything pushed into the feed at startup goes
*after* the rebuild, and the test asserts the **order**, not just that the event was
sent. The same shape one layer down: a fact about a file (was it there before we opened
it?) has to be read before the code that creates it runs, or the answer is `false`
forever.
— *a launch that finds chats but no `data.db` says so*.

**A lazily-synced state trails every intent dispatched in the same loop iteration.**
The input draft flushes at the *top* of the next tick while a key's intent goes out at
the *bottom* of this one, so four handlers reading `chat.draft` saw the spent command:
`/takeback` glued it onto the restored message, `/regen` re-loaded it into the box,
`/clone` copied it into the clone, `/new` left it on the old chat forever. One
mechanism, four symptoms — and the confirmation-popup path hid it, because its round
trip gave the flush time to land, which is exactly the arm the stage-1 tests had
covered. Flush before dispatching (one seam at the dispatch site), and pin the
**channel order** in a test, not the UI state. When a handler reads state the UI syncs
lazily, ask when that sync last ran — and mistrust the answer that came from the
gated path.
— *commands — stage 3 (the residue fix)*.

---

**A tool the system prompt does not name does not get used — even with its
schema in the request.** Stages 1–3 wrote the workspace block against one fear,
the one §9.7 records: it must never promise a tool the turn does not have. Stage
5 measured the converse and it is just as strong. The probe's `code_search` was
registered, gated in and its schema sent, and the first measurement came back
with the tool called **zero times in twenty turns** — because the block
enumerates the turn's tools in words, was built from a list the new tool was not
in, and the model believed the block over its own tool list. That block is not
documentation of the turn; it is the model's working inventory. Anything added to
a family has to be added to both.
— *the code workspace — stage 5*.

**A message that names the wrong knob is the same defect as a message that names
none.** Two limits ended a turn — the tool-round budget and the workspace
ceiling — and the note always quoted `max_tool_rounds`, sending the user to
change a setting that was not the problem. Same shape as an under-described
capability: the workspace system block still named the three read-only tools a
stage after the editors shipped. The durable fix in both cases was to derive the
text from the state it describes (which limit fired; which tools this turn
actually offers) rather than to correct the sentence, because a sentence
maintained by hand drifts again on the next stage.
— *the code workspace — stage 3*.

**A path check validates the string, not the thing the string names.**
`is_file()` on the managed server's GGUF passes for part 1 of a three-part model
whose other parts never finished downloading, and for part 2 of a model
llama.cpp refuses to load at all — and both then surface as "the process exited
before it was ready", the message that says something went wrong and nothing
about what. Whenever a configured path *implies* other files — a split model's
siblings, a projector beside the weights — the preflight owes their names: the
file to point at, or the file to fetch. A generic exit message is what a check
looks like when it stopped one step short of the format it was checking.
— *a model split across several GGUF files*, *managed — preflight model-file
check*.

## 5. Terminal and ratatui rendering

**One wide label in an aligned table re-wraps every description in it.** The help
overlay aligns its label column to the widest entry, so a 44-character command
(`/project build-cmd|run-cmd|test-cmd [line]`) squeezed the description column
across the *whole tab* and broke phrases other tests asserted on — a change that
announces itself somewhere entirely unrelated to what was edited. Keep a new row
the width of its neighbours and let the description carry the rest; the sibling
subcommands read just as well there.
— *the code workspace — stage 3*.

**A wide glyph's trailing cell is where terminal rendering goes wrong.** ratatui resets
it to the default style, `Buffer::diff` normally omits it, and `CrosstermBackend::draw`
tracks `last_pos` **by cell number without accounting for glyph width** — so when the
trailing cell *is* emitted it prints one column right and the whole row drifts. Three
symptoms came from this one mechanism: a selection ghost surviving a popup close, an
emoji vanishing from a grid, and a "broken" frame. A fourth lies upstream.
— *emoji popup — a "hanging" selection ghost after closing*, *ratatui-core/crossterm
update 0.1.1 to 0.1.2*.

**A `Line`'s style covers the whole row, not just its text** — so any padding you
prepend to a styled line inherits it. Indenting markdown output by two spaces made a
level-1 heading's `UNDERLINED` run out to the left of the text: the writer puts a
heading's style on the `Line`, not on its spans. Fold a line style into the content
spans (`line_style.patch(span.style)` keeps each span's own overrides) whenever you
add decoration cells around it.
— *a disclaimer for what the models say and do*.

**Use `shared/ui.rs::prime_full_redraw` for a full repaint; never `terminal.clear()`
and never a plain back-buffer reset.** `clear()` emits `ESC[2J` and flickers. A plain
`swap_buffers` is *worse*: the diff then compares "empty to frame" and **skips space
cells**, leaving old content visible — the diff only emits cells that differ. The
sentinel writes a space plus the `HIDDEN` modifier: the space matches a wide glyph's
trailing cell so no shift is triggered, while the modifier forces every other cell to
repaint.
— *scrolling with VS16 emoji without flicker*, *emoji popup — a "hanging" selection
ghost after closing*.

**Decide a full redraw *before* the render, not from render state.** Deriving it from a
cache miss worked but was one frame late, so the artifact still flashed — and a bad
frame cannot be hidden behind synchronized output, because conhost (where the problem
lives) ignores mode 2026. A screen switch needs one too: the incoming buffer holds a
foreign symbol in a wide glyph's trailing cell.
— *emoji popup — a "hanging" selection ghost after closing*, *a full redraw on screen
switch and in the input box with VS16*.

**Wrap every frame in synchronized output (DEC 2026).** ratatui writes the diff with the
cursor visible and flushes cursor moves separately, so Windows Terminal shows an
intermediate state — the cursor visibly jumping to a token counter or spinner at
~20 fps. Emit the closing sequence on the error path and in the panic hook so the
terminal never stays buffering.
— *synchronized output (DEC 2026) — fixing the "jumping cursor"*.

**Cursors and deletion move by grapheme cluster; widths are causal, not per-character.**
`unicode-width` gives a variation-selector cluster width 1 while the terminal draws 2,
and a skin-tone sequence measured 4 instead of 2 — hence `width_at`, which looks at the
previous character. Movement and deletion follow UAX #29 boundaries, or a `Backspace`
leaves an orphaned selector and phantom glyphs.
— *emoji paste from clipboard (cluster width + clipboard recovery)*, *InputBox
refinements (clusters/spellcheck/navigation)*.

**New chrome glyphs must be WGL4 and one column wide.** Compatibility mode substitutes
only what is outside WGL4, and a two-column glyph in a status line shifts the hotkey
grid. Emoji in message *content* are data and are never substituted.
— *compatibility mode for old terminals*.

**ratatui's `List` silently skips a multi-line item that does not fit the remaining
height** — rendering nothing rather than clipping, so the gap reads as the end of the
list. Render row-by-row when items can be multi-line.
— *SelfModel — partial display of a long item on the `F3` screen*.

**A multi-span decoration appended to a header row is re-wrapped with it — at its
spaces.** The collapsed tool pill was pushed onto the header's last row on the
promise that `build_message_block` "wraps every body line afterwards"; it does, at
whitespace, so a long `web_search` header ended its row with `▸ details ·` and the
keycap alone opened the next, under the icon. Measure the group against the row at
the wrap width and move it **whole** to a continuation row when it does not fit
(`push_trailing`) — the seam any future chip on a card header should go through.
The sibling trap on a one-row status line: a bare `Line` in a one-row area is
**clipped** at the edge with no marker, and the part that falls off is the tail —
which for the indexing banner was the progress counter, the one thing that moves.
A row whose middle varies in length needs a width-aware fit that decides which part
gives way (`RagBanner::fit`), not a truncation from the end.
— *the indexing banner fits its row, the collapsed pill wraps whole*.

**A `ListState` built inside `render` is a scroll position that is recomputed
every frame.** ratatui moves a list's offset only far enough to bring the
selection into view, so starting from `0` on every draw means "scroll until the
selection is the *last* visible row": downward it looks like a correctly
following window, upward the list scrolls on every press and the selection never
climbs to the top row. The offset is state, not decoration. It now has exactly
one implementation — `shared::ui::ListScroll`, the only place a `ListState` is
built: the widget owns one, and the clamp to `len - height` lives there too, so
a list shortened by a filter or a deletion cannot leave the window past its
tail. **The shape repeats itself**: eight lists carried the same four lines with
the same defect, because each was written by copying a neighbour rather than a
helper. A rule that must hold in eight places is one function, not eight.
The sibling trap: a list whose rows are **not all selectable** (group headers)
needs `scroll_padding(1)`, or the header of the group the selection stands in
scrolls out — the selection is the only thing ratatui keeps in view. Since a
convention is what let this spread in the first place, `tools/list_scroll_check.py`
(CI's `lint` job) now fails on a `ListState`/`TableState` built anywhere else.
— *the chat list scrolls symmetrically*, *the same for every other list*,
*the gate under it*.

---

## 6. Windows and cross-platform

**`Path` only knows the host's separators, and this app's data crosses hosts.**
`Path::file_name()` on a `C:\Projects\app` string returns the **whole string** on
Linux — there is no `\` separator there — so a label derived that way came out as
the entire path instead of `app`. It matters because the data directory is
portable and a backup restores across machines: a root canonicalized on Windows
is read back on Linux. Split stored paths on **both** separators by hand
(`rsplit(['/', '\\'])`), and remember the two degenerate roots that then have no
component at all (`/`, and `C:\` trimming to `C:`). The Windows job was green and
the Linux one red, which is the only reason it was caught before merge — a test
that constructs a path for the *other* platform belongs in the suite for exactly
this.
— *the code workspace — stages 0 and 1*.

**Killing a child is not killing a tree, and every launcher spawns a tree.**
`Child::kill`/`kill_on_drop` end the process this application spawned and nothing
it spawned in turn: killing `cargo` leaves its `rustc` children compiling, and
the machine stays busy after the user pressed `Esc`. It needs a kill-on-close Job
Object on Windows and `process_group(0)` + `killpg` on unix — both in
`shared/proc.rs`. Two details that are easy to get wrong and cheap to get right:
the group must be **opt-in**, because a guard that remembers a pid without having
made a group would `killpg` the *application's own* group; and the guard has to
kill on **`Drop`**, because cancellation drops the tool's future and no cleanup
code of yours runs at all — with `disarm()` after the child is reaped, so a late
signal cannot reach a pid the OS has since reused. Verify it the way it fails:
spawn a grandchild that keeps writing to a file, kill, and assert the file stops
growing — and mutate the kill back to `start_kill()` to check the test can see
the difference.
— *the code workspace — stage 3*.

**Bracketed paste does not work on Windows.** `Event::Paste` is emitted only by
crossterm's unix parser; on Windows a paste arrives as ordinary key events (interleaved
with releases), so the loop must batch and coalesce them. A large paste also spans
several console-buffer chunks, so a lone `Enter` on a seam used to fire as a send.
— *fast multiline clipboard paste*.

**Supplementary-plane characters are lost by crossterm on Windows before they reach
us** — console key-down/key-up records break the UTF-16 surrogate pair. The workaround
reconciles the reconstructed paste against the clipboard; a paste made *entirely* of
such characters still cannot be recovered.
— *emoji paste from clipboard*.

**Rust does not complete a bare command name from `PATHEXT`; `cmd.exe` does.**
`Command::new("npx")` fails where `npx.cmd` spawns. Complete only a *bare* name: npm
also ships an extensionless Unix script next to `npx.cmd`, and preferring the exact name
spawns the script (`os error 193`). The same spike **overturned the ban on
`.bat`/`.cmd`**: CVE-2024-24576 is fixed in `std` as of Rust 1.77.2, so the ban added
nothing while pushing users onto `cmd /c`, where arguments are re-parsed *outside* that
escaping — a workaround that was strictly less safe than the thing it replaced.
— *MCP servers in the settings window*.

**On a bare unix terminal `Shift+Enter` is indistinguishable from `Enter`.** The kitty
keyboard protocol at the `DISAMBIGUATE_ESCAPE_CODES` level fixes it without affecting
ordinary typing or layout-independent Ctrl parsing (unlike `REPORT_ALL_KEYS`, which
would). `Alt+Enter` is the fallback for terminals without it.
— *line breaks on unix terminals — the kitty protocol + Alt+Enter*.

**Resolve a shortcut's physical key through the OS, not a per-language table.** A
hardcoded table covered exactly one layout; `VkKeyScanExW` plus `MapVirtualKeyExW`
inverts any installed layout through one static scan-code table. ASCII characters must
short-circuit *before* that, or `Ctrl+A` on AZERTY becomes `Ctrl+Q` (quit).
— *universal layout-independent hotkeys — stage 1, Windows*, *… — stage 3, upstream
crossterm*.

**Inno Setup: `[Run]` entries execute *before* `CurStepChanged(ssPostInstall)`.**
Verified with a stub binary after the opposite had been written into a comment as
"verified" — a file written at `ssPostInstall` was not yet there when `[Run]` fired, so
provisioning would have silently used the *default* data directory rather than the one
the user picked. Two smaller ones: a Pascal `{ }` comment containing an app constant
**closes early** at that constant's `}`, and Inno remembers the previous install's task
selection per AppId, so a clean retest needs a clean registry state.
— *the Windows installer can provision the Python sandbox*.

**Driving a GUI wizard from a script: accelerators are localized, window rects are
not scaled.** `Alt+A` for "I accept" is a *different letter* in the `ru` wizard, so an
accelerator-driven run stays on the page it meant to leave and screenshots the wrong
one — `Tab` into the control group plus an arrow key is locale-independent. And
`GetWindowRect` reports physical pixels while a DPI-unaware PowerShell process draws in
scaled ones, so a per-window `CopyFromScreen` comes out clipped; capture the whole
screen and crop. Neither failure announces itself — both produce a plausible image.
— *license and disclaimer pages in the Windows installer*.

**The newest Zola broke Windows twice over; reproduce on a pristine site before
doubting your own config.** 0.23.0–0.23.2 do not discover `templates/` on Windows at
all (upstream getzola/zola#3229 — "Template not found" for every custom template), and
0.22.1's file-watcher never fires there, so `zola serve` keeps serving stale pages
until restarted. The site pins 0.22.1 with the config named `config.toml` (both majors
read that name); the dev loop is "edit → restart serve". A five-minute `zola init`
repro answered what staring at correct templates could not.
— *website — research + S1 scaffold (Zola, terminal-styled)*.

---

## 7. i18n, gates and localization

**Never build a bundle key with `format!`.** A key assembled from a prefix is
**invisible to the key scanner**: the real keys read as dead and the template reads as a
key that does not exist. Pass keys whole. A genuinely dynamic family must carry its own
gate test.
— *history compression — stage 3 (the read-back tools)*, *axis B — closing the
groundwork item*.

**A bundle prefix can collide with ordinary strings in the code.** Introducing the
prefix `compact.` made the gate read the scratch filename `"compact.db"` as a key; the
fix was renaming the keys, not adding a scanner exception. A reserved secret name moved
from a dotted to a hyphenated form for the same reason.
— *history compression — stage 1*, *MCP servers — secrets for the `env` map and JSON
import*.

**Deleting the last user of a key is not enough** — the no-dead-key gate fails on an
orphan, so remove the bundle entry in the same change.
— *settings-screen focus model*.

**A bundle key added twice is invisible to every key gate and silently rewrites the
first text.** JSON map parsing keeps the last duplicate without an error, and the
dead/unknown-key scanners cannot object — the key exists and is used. Both texts of a
duplicated `ui.settings.desc.model_name` were real fields' descriptions, so the cloud
"Model" row spent a release showing the show-model-name toggle's text. Before reusing
a plausible-sounding key name, grep the bundle for it; the i18n gate
`builtin_bundles_have_no_duplicate_keys` now bans the class.
— *the settings hint panel — one height for every section*.

**A fixed-width strip of localized labels has a budget, and only one locale finds
out.** Adding a sixth tab to the 76-column help dialog fitted comfortably in `en` and
overflowed `ru` by six columns — and what silently truncates is the *rightmost* tab, so
a developer working in the other locale never sees it. Whenever labels share one line of
fixed width, measure the rendered line for **every** bundled locale in a gate test; do
not reason about the language you happen to be reading.
— *a disclaimer for what the models say and do*.

**Know where `cyrillic_scan.py` cannot see.** It allowlists test files **wholesale**
(they legitimately hold fixture data and reference-locale assertions), and it sets
`in_test` on the first `#[cfg(test)]` it sees and **never unsets it** — so production
code below a `#[cfg(test)]` test accessor is scanned as test code. Two unlocalized
user-facing strings hid there for months.
— *live-smoke diagnostic log in English*, *collapsible tool calls*.

**Decide which axis a string belongs to before localizing it.** Axis A is the *agent's*
language (tool descriptions, results, prompts — via `ctx.loc`); axis B is the *interface*
language (chrome, user-facing errors — via the UI locale). The same text can need both:
a console exit-code label is axis A inside the tool result and axis B when the feed
re-renders it, so it needs two keys.
— *interface multilingualism — axis B (UI language)*, *collapsible tool calls*.

**Keep the reference locale byte-for-byte when extracting strings.** Hundreds of
existing assertions then pass unedited, and any divergence is a genuine regression
rather than churn. Conversely, translating a *label* must not touch the fixture data or
locale assertions in the same file.
— *interface multilingualism — axis B*, *live-smoke diagnostic log in English*.

**`tf` substitutes in a single pass and asserts on unknown arguments.** A value is never
rescanned, so a placeholder inside a value cannot expand — retiring several hand-ordered
call sites that had been working around the old behaviour.
— *hardening i18n (single-pass tf, gates, observability, export)*.

**A localized *document* is a file, not a bundle key** — and it needs a structural gate.
A legal notice or any long text belongs next to its original as a file the language picks
(`credits::license_text`), never inside `locales/*.json`, where it is unreviewable,
undiffable and unwrappable. What the pair does need is a test comparing the two files'
*shape* — heading levels, list items — because a section missing from one language is
invisible to everyone reading the other.
— *the licence and the disclaimer in Russian*.

**Our markdown renderer prints a link's target after its text** (a terminal cannot click),
so `[LICENSE.ru.txt](LICENSE.ru.txt)` draws "LICENSE.ru.txt (LICENSE.ru.txt)". Any markdown
the app itself displays wants link text that *describes* rather than repeats the target.
— *the licence and the disclaimer in Russian*.

---

## 8. Storage, schema and data safety

**Additive is free; a value rewrite is a migration.** A new `#[serde(default)]` field, a
`CREATE TABLE IF NOT EXISTS`, or a guarded `ALTER TABLE … ADD COLUMN` needs no schema
bump (ADR 0006 F12) — every query names its columns, so an older binary ignores the new
one. Changing an *existing* value is different, and was deliberately not done when a
default moved from 1200 to 4000: it would overwrite a deliberate user choice.
— *embedding-model change — stage 2*, *self-model injection — per-section budgets*.

**A new nullable column must read as "foreign", not as "unknown".** Every row in a real
database predates the marker, and a plain `col = ?` evaluates to `NULL` — neither true
nor false — so it silently skips exactly the rows most in need of work. Fold every
predicate through a sentinel that cannot collide.
— *embedding-model change — stage 2 (re-embedding in place)*.

**`vec0` virtual tables join by rowid, so anything that renumbers rows silently returns
the wrong text.** That is why `VACUUM` was safe (it preserves `user_version` and
explicit `INTEGER PRIMARY KEY` rowids) — pinned by a test that *searches* the compacted
copy rather than trusting the documentation. It is also why a dropped vector table must
take its document rows with it: sqlite reuses rowids.
— *database compaction on backup and restore*, *embedding-model change — stage 2*.

**Opening a SQLite file can mutate the directory.** SQLite deletes a stale `-wal` next
to a file it reads as zero-page — a VFS-level delete that `SQLITE_OPEN_READ_ONLY` does
**not** prevent — and a read-write open removes it for a valid database too. So refuse a
non-database **by its header before opening it at all**, and open a real one read-only.
— *database compaction on backup and restore*.

**Raw text can never reach an FTS5 `MATCH`.** Measured: `C++`, `cost-benefit`, `50%`,
`AND` and `(` are all syntax errors on ordinary text, and `cost-benefit` is read as a
*column filter*, so the error names a column the user never typed. Quote every token,
double inner quotes, count the trigram floor in **characters** (a three-character
Cyrillic token is six bytes), and keep the rule in exactly one place.
— *full-text search over chat content — stage 1*.

**Two repair paths need two questions, not one predicate reused.** "Is this file
searchable?" and "does this file have any rows at all?" read as the same question
and are not: rows from a retired embedding generation answer no to the first and
yes to the second, and each state has a different fix (re-embed the stored text
versus rebuild from the source). Reusing the searchable predicate for both would
have put every attachment through both paths after a model change — re-chunked by
one, counted into the queue of the other, which then comes up short of the total
its progress banner was promised. Before pointing a second consumer at an existing
predicate, ask what it is *for*, not what it returns.
— *`/reindex` rebuilds an attachment index that is missing entirely*.

**Prefer best-effort to refusal on paths that protect data.** A `data.db` that cannot be
read as a database is packed raw rather than failing the backup — a backup that
*happens* for a corrupt database is worth more than a compact one. Same shape elsewhere:
a corrupt metadata value reads as "nothing recorded", and a failed calibration falls
back to the *exact* identity rather than to arithmetic that merely ought to cancel out.
— *database compaction on backup and restore*, *embedding-model change — stage 3*.

---

**When two writes undo one thing, the order is the design — and so is doing them
together.** Reverting a file the assistant changed is "restore the bytes" plus
"drop the journal row", and the plan listed them as two steps. Apart they are two
failure modes that are *not* symmetric: a restored file still listed offers a
second revert that does nothing, while a dropped row whose file was not restored
loses the pre-image **for good**, because those bytes exist nowhere else. So the
recoverable half goes first, they live in one function, and the bookkeeping
deletes the stored copy with the row rather than leaving orphaned copies of the
user's source on disk. Ask which half, done alone, cannot be retried — that one
goes second.
— *the code workspace — stage 4*.

## 9. Live runs and model behaviour

**A decorator is only covered live if the test harness wraps too — check, don't
assume.** The retry `EngineBackend` decorator sits on every real cloud and external
turn, and the live e2e set appeared to exercise it; it did not. The harness builds its
backend directly through `MockSupervisor`, so the wrapping had unit coverage only, and a
mistake in how it hands a stream over — the head it replays, the commit point, the
delegated `context_budget` the compaction trigger reads — would never have met a real
model. One line in `live_backend()` put the whole set through it. Whenever production
composes a layer the harness constructs by hand, ask which of the two the live gate is
actually testing.
— *retry with backoff on transient cloud failures*.

**Run the live gate when the change touches engine, memory or tool paths — even when it
looks local.** `build_request` and `effective_tool_ids` sit on *every* turn, so "it only
affects a compacted chat" still warrants the full e2e set. Skip it for pure rendering,
pure parsing and settings UI, and say so explicitly.
— throughout; e.g. *history compression — stage 1*, *an unhighlighted code block is
drawn as a rectangle*.

**A skipped smoke that reports `ok` is worse than a failing one.** Sandbox smokes gated
on an environment variable silently no-op'd and still reported `ok`; the helper now
prints a skip line. Relatedly, one smoke deliberately *fails* rather than skips when its
sidecar is missing, because an absent sandbox that was meant to be installed should be
loud.
— *English source-language migration*.

**Running the whole `#[ignore]` set provokes failures the individual smokes never see.**
It drives several web smokes from one IP within minutes, and the search providers throttle
exactly that (a DuckDuckGo challenge arrives as `HTTP 200`); the local server can also drop
a connection mid-stream under back-to-back load. Two consecutive full runs each failed one
or two *different* tests, and every one passed on isolated re-run. Read the set as "the
union of the runs is green, and no failure repeated", not as one clean sweep — and say so
in the journal instead of quoting the best run.
— *an address policy for model-chosen URLs*.

**Distinguish a model-behaviour flake from a regression by the diff.** A control-tool
smoke failed once because the model simply answered without calling the tool, and passed
on re-run — defensible only because the change touched **zero** files on that path. A
cold npm cache can also push an MCP smoke past its 120 s readiness timeout; it passes in
~13 s once warm.
— *page fidelity for `fetch_url`*, *English source-language migration*.

**A new capability can invalidate an older smoke's assertion while improving its
outcome.** Once by-reference attachments gained an index, the model stopped walking
pages entirely — one search call, correct answer — and the narrow "must call
`attachment_read`" assertion had become wrong. Rewritten as two turns, so both paths
stay covered.
— *chat file attachments — stage 3 (`attachment_search`)*.

**Ask a model for the situation, not for the script — a step-by-step "demonstrate the
tool" prompt invites it to narrate the call instead of making it.** A live probe asked
the model to "demonstrate `rewrite_current_message` strictly by steps, skipping none:
step 1 write X, step 2 (MANDATORY) call the tool, step 3 write Y". It complied — in
prose: `"2+2=5\n<call:rewrite_current_message/>\n2+2=4"`, a call syntax no protocol here
defines, so no call happened and no effect followed. Measured **1 failure in 8**; the
same test phrased as a situation the tool answers ("your draft is no good — call the tool
and write it again") is **0 in 30**, same model, same sampling, thinking still on. The
sibling probe, which names its tool but asks in two clauses rather than three numbered
steps, was 0 in 12 untouched — so the trigger is the script, not the naming. This is
[§9's blind-vision trap](#9-live-runs-and-model-behaviour) in another costume: a test
worded so that it can be satisfied without doing the thing it checks.
— *the rewrite probe's flake, found by the live gate*.

**Two models flake for two different reasons; fixing one does not fix the other.** The
same probe, after the prompt above was fixed, still failed 1 in 20 on the other family —
not by narrating but by producing **nothing**: no tool call, empty text, the whole turn
spent in `reasoning_content`. Muting thinking for that turn took it to 0 in 20, while the
prompt fix alone had been enough for the first model. Chase the failure *mode*, not the
failure rate: had the rate alone been watched, "much better on Gemma" would have shipped
a probe that was still red one dispatch in twenty.
— *the rewrite probe's flake, found by the live gate*.

**"Remove the alternative" is about competing tools, not about model silence — and
applying it blindly can make a smoke worse.** Narrowing that same probe's profile to the
single tool under test looked like the rule in §9 below, and measured **7 failures in
20** against 1 with every tool enabled — all of them the empty turn above. A one-tool
list stops a model from answering with the *wrong* tool; it does nothing to stop it
spending the turn thinking, and appears to invite it. Check which failure the rule
addresses before reaching for it.
— *the rewrite probe's flake, found by the live gate*.

**Check the journal for a measured ceiling before spending a run discovering it
again.** A probe asked open-ended questions at the default `max_tokens` of 2048
with thinking on, and more than half of one arm's turns came back with **no text
at all** — while `docs/journal/ci.md` already held the measurement for that exact
class of prompt on that exact model family: 1024: 0/3, 2048: 2/3, 4096: 3/4. The
cost was a thirty-minute live run and a headline number that had to be thrown
away. Two arms starved by the same harness do not compare to anything. (The
sequel is worth knowing too: raising it to 4096 moved the per-arm numbers and did
**not** clear the empty turns, so the ceiling was necessary and not sufficient.)
— *the code workspace — stage 5*.

**In a smoke, remove the alternative rather than hope the model does not take it.**
Enabling only the tools under test is what makes a live assertion mean something. Some
claims can *only* be settled live: that a real model reaches for a tool at all, that a
real subprocess is torn down and re-handshaked, or that a real embedding server actually
serves the `/health` a probe relies on.
— *history compression — stage 3*, *MCP servers in the settings window*, *readiness
probe for the embedding server*.

---

## 10. CI and infrastructure

**A failed `needs` dependency skips the dependent job regardless of its `if`.** A
docs-only gate reading `!= 'true'` still skipped the test job when the classifier job
failed to start — observed for real. Add `!cancelled()`; `always()` would be wrong,
because cancelling the workflow must still cancel the job.
— *skipping the test job for docs-only pull requests*.

**"Upload succeeded" is not "analysis accepted".** A scanner logging `ANALYSIS
SUCCESSFUL` means the report was *uploaded*; processing is asynchronous, and a report
rejected server-side left CI green while the platform's own check read failed. Waiting
on the verdict collapses the two.
— *what the first real CI run of the remote gate found*, *the SonarQube quality gate
became blocking*.

**A cache belongs to the branch that wrote it.** With `save-if` at its default, every PR
branch stored its own copy of the same build and pushed the shared ones out by age. Save
on the default branch only — but keep an exception for any OS the default branch does
not otherwise build, or nothing ever writes that cache. And a cache key fingerprints the
*dependencies*, so a source-only change correctly does not re-save it.
— *one Actions cache per job instead of one per branch*, *cutting the Windows CI job
from 19 minutes*.

**A docs-only classifier reads the whole PR file list, not the last push.** Merging a CI
branch into a docs PR therefore correctly makes it non-docs-only. Its allowlist must
exclude files that *look* like docs but are read by the build or a gate test (`LICENSE`,
artwork SVGs, locale JSON, `Cargo.toml`) — established by grepping `include_str!` /
`include_bytes!` / `CARGO_MANIFEST_DIR`, not by assumption.
— *one Actions cache per job instead of one per branch*, *skipping the test job for
docs-only pull requests*.

**A refactor moves long-uncovered lines into the "new code" ledger.** Three pure
extraction PRs failed a new-code coverage gate with zero new issues and unchanged
project coverage: extraction rewrites lines, and complexity concentrates exactly where
unit tests cannot reach (the TUI loop, network and audio paths). The percentage also
understates reality where the real coverage comes from `#[ignore]` live smokes a
coverage run does not execute.
— *the quality gate stopped judging new-code coverage*.

**A green PR quality gate still ships maintainability findings — it judges new-code
*ratings*, not counts.** A handful of new smells cannot flip an A rating, so they
surface as open issues on the next `main` analysis instead: twice now (two S8786
regexes + one S3776 after the 2026-08-07/08 merges; an S3776 at complexity 50 + an
S1871 in the screenshots SVG writer after 2026-08-10) — while the security-class
S8707, which does move a rating, blocked its PR on the spot. For a new or reshaped
`tools/*.py`, or any function a change grew, run the file through the Sonar MCP
snippet analyzer before the PR: one tool call, the server's own rules, and the
backlog stays at zero. **That pre-check is unavailable for Rust** — the analyzer
takes no such language — so on this side of the codebase the shape has to be
watched by eye, and the PR analysis is the first real measurement. The shape that
caught one out: a **dispatch `match` with nineteen arms** scored S3776 complexity
16 against a bar of 15 on the strength of five short `if`s inside it, no single
arm looking remotely complex. Delegating each precondition-carrying arm to a
named method fixed it and read better — a table of one-liners.
— *SonarQube follow-up — the doc gate's regexes and one test's complexity*,
*SonarQube follow-up — the screenshots SVG writer*, *command-only control —
stage 2*.

**An invariant enforced only where data is *written* is not enforced.** The code
workspace's journal reset lived inside `Journal::record` — correct, and useless for the
window it mattered in: re-attaching a chat to another project left the old journal in
place until the assistant's next edit, and that window is exactly when the changes screen
gets looked at. Reverting a file both projects have then wrote the old project's bytes
into the new one's tree. Ask of any such rule: *who reads this between the event and the
next write?* — and put the check in the reader too, so it cannot be undone by a new caller
of the writer.
— *the change journal followed the chat, not the project*.

**`cargo clippy -D warnings` green is not "Sonar-clean" for Rust.** The analyzer's
rule set is broader than clippy's default warn set, so an analyzer update raises findings
on code nobody has touched — 22 of one 30-issue backlog dated back two months. Measured on
a scratch crate: `rust:S1612` is clippy's `redundant_closure_for_method_calls`, which is
*pedantic* and therefore off; `rust:S8863` is `redundant_static_lifetimes`, which is on by
default but never reaches an **associated** const or a `'static` nested inside a generic
argument — enabling it explicitly changes nothing. Expect a periodic lint backlog that no
local gate could have caught, and do not read it as a regression.
— *SonarQube follow-up — two new lint families, and eight complexity findings*.

**On CI, distrust a single run.** Identical code produced Windows test phases of
359 / 469 / 376 / 386 / 927 s; a controlled local measurement is the trustworthy one. The
outlier was diagnosed by joining per-test CI timings against local ones — a 3.9x median
but 8–19x on disk-bound tests, and parallel efficiency of 2.0x against 4.4x locally: the
signature of contention on one shared resource, not a slow CPU.
— *cutting the Windows CI job from 19 minutes*, *tool tests run against in-memory
storage*.

**Validate workflow YAML by rendering it, not by reading it.** A matrix written as
`os: '["ubuntu-latest","windows-latest"]'` expands only inside a `${{ }}` expression; as
a plain string it silently produces one entry with that literal name. Likewise, extract
a `run:` block and execute it against stubs — six scenarios of a docs-only classifier
were verified that way, including the truncated-listing case.
— *cutting the Windows CI job from 19 minutes*, *skipping the test job for docs-only
pull requests*.

**Every job gets a `timeout-minutes`, sized from its measured history; so does every
step that talks to the network.** GitHub's default ceiling is **six hours**, billed.
A stalled `apt-get update` behind an unhealthy Ubuntu mirror — a ten-second step —
held four Linux jobs across two runs for 2.5 hours until someone noticed, and would
have held them for 24 hours of billable minutes otherwise; the runner was never
frozen, and the "hung runner" reading only survived until the step logs were opened.
The asymmetry decides it: a ceiling that fires on a legitimately slow run costs one
rerun, a missing one costs six hours per job. Size each value from the job's run
history (p50/max, with room for a cold cache), and write the figure next to the value.
And when a job does hang, read the *step* timestamps before blaming the machine: four
jobs stopping on the same line is a dependency, not a runner.
— *a ceiling on every job, and two orphaned workflows*.
