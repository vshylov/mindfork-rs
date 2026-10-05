# The animated demo — a reel generated from code

Status: **design accepted 2026-10-03** (its shape is the promotion plan's §6,
[promotion.md](promotion.md)); **stage 1 — the MVP probe — GO** (the owner,
2026-10-03, after one round of review, §7); **stage 2 — publication —
implemented** (§8); **stage 3 — the Russian variant — implemented** (§9).

## 1. Context and goal

The README and the site show still screenshots; a terminal application is
judged by how it feels in motion. The promotion plan's storefront stage asks
for an animation, and two earlier documents already left room for one: the
demo-screenshots plan's optional stage 4, "replaying the scripted stream frame
by frame into an APNG/GIF" ([demo-screenshots.md §4](../history/demo-screenshots.md)),
and the public-release audit's "a demo recording"
([public-release-readiness.md §2.4](public-release-readiness.md)).

The owner's requirement: **no manual recording.** The application changes
every few days; a recording made by hand would be stale within a week and cost
time every time it was redone. The animation is made the way the screenshots
are — from code, by a script, the same on every run.

## 2. What already exists

- **The showcase conversation** (`features/demo.rs`). Its last exchange is
  already the scenario: the user asks for "the bottom line — a small table plus
  a decision diagram, and save a note about my setup" (m3), and the answer (m4)
  carries thoughts, a GFM table, a Mermaid flowchart, LaTeX and a `note_save`
  tool call. Beside it, the self-model the `F3` capture shows.
- **The still capture**: [demo_shots.rs](../../src/app/demo_shots.rs) builds a
  screen from the fixture, renders it into a `TestBackend`, and
  `shared/shot.rs` serializes the frame cell by cell;
  [tools/screenshots.py](../../tools/screenshots.py) draws PNG and SVG from the
  dumps. Capture is **test-gated on purpose** — the release binary gains no
  surface from it (demo-screenshots plan §5).
- **The runtime's own path, reachable headlessly**: keys go through
  `process_input_batch` → the screen's key handler → `dispatch` (intents become
  `AppCommand`s); the orchestrator's `AppEvent`s go through `apply_event`; a
  frame is `present` → `compose_frame`. The small-window gate
  (`app/runtime/small_window_tests.rs`) already drives this path without a
  terminal.
- **The wall clock** — the one thing that would make frames differ between
  runs. Checked on the chat path of a turn: the only clocks are the spinners of
  the indexing and impersonation banners (neither is in the scenario) and the
  spellcheck debounce (only the real loop runs it). Message timestamps are not
  drawn in the feed.

## 3. Design

### 3.1 The director (Rust, test-gated)

`src/app/runtime/demo_reel.rs`, `#[cfg(test)]`, inside `runtime` because the
loop's pieces are private to it. It holds what `run_loop` holds — the chat
screen, the active screen, the back-stack, the help overlay, a command
channel — and one persistent `TestBackend` terminal at the hero's size
(116×44), and it plays the **orchestrator's** part from a script:

- a key goes through `process_input_batch`, exactly as a typed key does;
- what the interface then asks for is read off the command channel and
  **asserted** — `Enter` must send the typed question, `F3` must request the
  self-model — so an interface that stopped sending on `Enter` fails the reel
  instead of animating a lie;
- the answer is the events the orchestrator emits for such a turn, in its
  order: `UserMessage`, `GenerationStarted`, `Thoughts`…, `ToolCallStarted`,
  `ToolCall`, `Chunk`…, `TokenUsage`, `Finished`; for `F3`, `SelfModelView`.

Every beat draws a frame through `present`, captures it with `shot::capture`
plus the cursor (when the frame shows one), and records it with a duration on
a virtual clock. Consecutive identical frames merge into one longer frame.

### 3.2 The scenario

| Beat | What happens | Time |
|---|---|---|
| Open | the showcase chat with its first exchange, an empty input box | 1.5 s |
| Type | the question (m3), by hand: bursts of one to three characters, a beat between words, now and then a pause, a longer one after a mark | ~6 s |
| Send | `Enter` → the question in the feed, the reply's bubble opens | 0.8 s |
| Think | the thoughts stream in a few words at a time | ~1 s |
| Tool | the `note_save` card — running, then its result | 1.4 s |
| Answer | the reply streams: the table, the flowchart (drawn once its block closes), the formula | ~2.5 s |
| Rest | the finished turn | 2.5 s |
| Fold | `Ctrl+T` folds the thoughts | 1.5 s |
| Self | `F3` — the self-model, whose notes and observations are the point; the longest hold | 7 s |
| Back | `Esc` to the chat | 1.5 s |

About 25 seconds, looping. It shows the differentiator — memory and a
self-model — and the renderer's range in one turn.

### 3.3 The output

An `#[ignore]` regenerator, like the stills' `dump_demo_frames`, writes
`target/reel/<name>.json`: the reel's name and grid size, and its frames, each
a duration, a cursor and the cell rows in `shot`'s format. It is **not
committed**: a frame is ~200 KB of JSON, and the reel has well over a hundred.

### 3.4 Rendering

`tools/demo_reel.py`, beside `screenshots.py` and reusing its faces and cell
drawing (refactored so a frame renders to an image rather than to a file):

- **a row cache** — a streaming frame changes a few rows; the rest are drawn
  once;
- **one palette for the whole GIF**, no dithering, so colours do not flicker
  from frame to frame; Pillow writes only the rectangle that changed;
- **GIF** for the README (it plays everywhere), **lossless animated WebP** for
  the site (smaller for the same pixels);
- frame delays of at least 40 ms — browsers stretch a GIF delay under 20 ms to
  100 ms;
- the cursor drawn as a block where the frame placed it.

### 3.5 Tests that hold the reel to the code

- two runs produce the same reel;
- every frame covers the grid;
- the load-bearing content reaches the frames at its beat — the question in
  the input box, the thoughts, the `note_save` card, the table's "sweet spot",
  the flowchart drawn, the self-model's summary;
- the interface asked for what the script answered (the question sent on
  `Enter`, the self-model requested on `F3`);
- the reel's length stays within bounds.

### 3.6 Publication (stage 2)

Rendered in CI at release time and published to mindfork.io; the README embeds
it by absolute URL. Nothing binary is committed, no gate turns red on every
interface change, and the animation always shows the released version.

- **The release draws it.** `release.yml`'s `demo-reel` job checks out the tag,
  runs the regenerator (both looks, [`LOOKS`](../../src/app/runtime/demo_reel.rs))
  and `tools/demo_reel.py`, and hands `mindfork-demo-{dark,light}-en.{gif,webp}`
  to the release job, which puts them among the assets **before** the checksums
  and the attestation — so they are covered like the archives. The job is **not a
  gate** (`continue-on-error`, and the release job's condition names every other
  job but this one): a release without a fresh animation is still a release.
- **The same faces everywhere.** JetBrains Mono is written back out as TTF from
  the woff2 faces the site ships, so a developer's machine and the runner draw
  the primary face identically; the runner installs DejaVu and WenQuanYi Micro
  Hei for the seven glyphs the face lacks (`✦ ⚒ ⟳ ∝ ₖ ᵥ ＋` — checked in an
  `ubuntu:22.04` container; DejaVu alone missed only the fullwidth plus, which
  Noto CJK would have cost sixty megabytes for). A glyph no font covers fails
  the job rather than shipping as tofu.
- **Pinned Python.** Pillow, fontTools and Brotli install from
  `tools/media-requirements.txt` with `--require-hashes`: what draws a release
  asset is pinned the way the workflow's actions and nfpm are.
- **The site takes it from the newest release that has it.** `site.yml`'s deploy
  asks the API for the newest published, non-prerelease release whose assets
  include `mindfork-demo-*` and downloads them into `site/static/demo/`; a
  release whose demo job failed falls back to the one before. None found (before
  the first such release) → a notice, and the page keeps its still; a release
  that lists them but cannot be downloaded from fails the deploy.
- **The home page draws the animation only when its build has it**
  (`get_image_metadata(..., allow_missing=true)`), the still SVGs otherwise — so
  a pull request's build and every deploy before the first release with a demo
  look as they did. Both looks are `loading="lazy"`: the theme that is hidden is
  never fetched (measured in the browser: the light WebP was requested only once
  the theme switched). A visitor who asked for less motion gets the still PNG
  (`prefers-reduced-motion`).
- **The bucket's `demo/` has a pass of its own**, run only by a deploy that found
  the files: a deploy without them leaves it alone rather than deleting what the
  README points at; the pages pass excludes it.
- **The README's embed waited for the first release that carries the files.**
  Merged before, it would have been a broken image on the repository's front
  page; it landed with the README's rework (promotion plan §4, stage 0) on
  2026-10-05, after 0.15.0: the dark GIF from mindfork.io in either theme, as
  the README's stills are, and the dark still for a reader who asked for less
  motion.

Locally: `cargo test dump_demo_reel -- --ignored`, `python tools/demo_reel.py`,
then copy the two WebP files into `site/static/demo/` (gitignored) and `zola
serve` shows the page as a deploy would.

## 4. Stages

- **Stage 1 — MVP probe.** The director, the scenario, its tests, the
  regenerator and `tools/demo_reel.py`; the GIF rendered locally.
  **Go/no-go: the owner judges the GIF** — legible, smooth, representative,
  and of a size a README can carry.
- **Stage 2 — publication.** CI at release, the site's embed; the light look
  came with it, since the site follows the visitor's theme. The README's embed
  follows the first release that carries the files (§3.6).
- **Stage 3 — the Russian variant**, for Habr. More than the interface's
  language: the showcase conversation itself is English, so a Russian reel needs
  the fixture's exchange and self-model in Russian (test-only data). Published
  with the others as `mindfork-demo-dark-ru.{gif,webp}`; the site does not show
  it (it is English), but the deploy fetches every `mindfork-demo-*`, so it is
  served from mindfork.io all the same.

## 5. Forks

- **F1 — where the events come from.** (a) scripted, through the real runtime
  path; (b) the real orchestrator with the mock engine. (b) has the real event
  order for free, but other tasks interleave (titles, reflection), and the
  self-model would need a real reflection to change. **Chosen: (a)** — the
  accepted shape says the reply arrives by script, with no model and no
  network. A later guard is possible: a test comparing the reel's event kinds
  with what the orchestrator emits for the same scripted turn.
- **F2 — generated where.** (a) in CI at release, not committed; (b) committed
  with a drift gate, as the stills are. **Owner's decision 2026-10-03: (a).**
- **F3 — the entry point.** (a) an `#[ignore]` regenerator test that CI runs;
  (b) a `mindfork demo --reel` flag in the shipped binary. **Chosen: (a)** —
  capture stays test-gated, as the demo-screenshots plan decided (§5 there).
- **F4 — formats.** GIF for the README, animated WebP for the site.
- **F5 — the scenario.** §3.2. **Owner's decision 2026-10-03.**
- **F6 — size and language.** The hero's 116×44; English first, Russian in
  stage 3.
- **F7 — how the site receives the files.** (a) release assets, fetched by the
  deploy; (b) the release workflow's artifacts — they expire, and a second
  workflow has to find the run; (c) the site's own workflow builds the reel —
  a Rust test build on every content deploy, and from `main`, which is not the
  released version. **Chosen: (a)** — permanent, versioned, covered by the
  release's checksums and attestation.

## 6. Risks

- **The wall clock** — checked (§2); the determinism test is the guard.
- **The GIF's size** — measured in stage 1; the row cache and the
  changed-rectangle frames are the levers, then the frame count.
- **Fonts on CI** — the faces the GIF is drawn with differ between this
  machine and a Linux runner; stage 2 pins them (the site's own woff2 faces,
  written back out as TTF).

## 7. Stage 1 — what was built and measured

- `app/runtime/demo_reel.rs`: the director and the scenario of §3.2; 8 tests
  (determinism, the grid, each beat's content, the cursor, length and frame
  delays, the typing's rhythm, the self-model's hold, the piece splitter) and the `#[ignore]` regenerator `dump_demo_reel`.
  The determinism test held on the first run — §2's survey of the clocks was
  right.
- `tools/demo_reel.py`: the renderer of §3.4. `screenshots.py`'s raster path was
  split into `metrics` and `draw_row` for it; the ten committed PNGs re-render
  byte-identical after the split.
- The first reel: **134 frames, 21 s**, 1122×966 px; rendered in 17 s on this
  machine. **GIF 1 137 KB**; **lossless WebP 955 KB** at `method=6` with
  `minimize_size` (1 344 KB at Pillow's default effort — larger than the GIF;
  lossy at quality 90 was 1 756 KB, at 80 529 KB).
- **The owner's review of that GIF (2026-10-03)**: the self-model should stay
  up longer, and the typing read as a machine — a character every 40 ms. Now
  the self-model holds 7 s (from 4.5), the longest frame of the reel by test,
  and the question is typed by a `Hand`: a fixed-seed generator gives bursts
  of one to three characters per frame, 40–90 ms a frame, a beat between words
  and one time in four a pause, and 200–320 ms after a mark (with its space,
  as one would type `, `). A first cut of that rhythm made the typing 7.2 s and
  the reel 27 s; the pauses were trimmed to 5.8 s of typing.
- The reviewed reel: **92 frames, 25.5 s**; **GIF 1 090 KB**, **WebP 946 KB** —
  fewer frames for the bursts, so smaller for a longer reel.
- Found on the way: the folded thoughts pill says "1 lines" — an interface
  fix, left to a task of its own.

Regenerate locally:

```
cargo test dump_demo_reel -- --ignored
python tools/demo_reel.py --font-dir <dir with JetBrainsMono-*.ttf>
```

## 8. Stage 2 — what was built and checked

- `release.yml`: the `demo-reel` job and the release job's condition and copy step
  (§3.6). `site.yml`: the fetch step and the bucket's `demo/` pass.
  `site/templates/index.html`: the animation when the build has it.
  `tools/demo_reel.py`: the site's faces by default. `tools/media-requirements.txt`:
  the hash-pinned packages. `tools/screenshots.py`: WenQuanYi Micro Hei among the
  fallbacks. The regenerator writes both looks; +1 test that they differ only in
  their canvas and are named as the release and the site expect.
- **In an `ubuntu:22.04` container**, as the runner: the packages installed with
  `--require-hashes` on Python 3.10.12; DejaVu alone missed `＋`; with WenQuanYi
  Micro Hei both looks drew in 24 s, exit 0 — dark GIF 1 090 KB, WebP 945 KB; light
  GIF 1 010 KB, WebP 935 KB.
- **The site, built both ways**: without the files, ten still SVGs and no
  `<picture>`; with them, two `<picture>`s in place of the hero's two SVGs. In the
  browser the dark WebP played and the light one was not fetched until the theme
  switched to light.
- `actionlint` (with shellcheck) clean on both workflows; the release-selection
  query run against the API — empty today, `v0.14.1` for a control asking for
  `install.sh`.
- **Not yet seen**: the two workflows running for real. The next release is the
  proof — its `demo-reel` job, its assets, and the deploy that follows it.

## 9. Stage 3 — the Russian variant

- **The fixture** — `features/demo/ru.rs`, test-only: the English showcase with
  its words replaced (the chat's title, both exchanges, the thoughts, the note,
  the self-model's summary, goals, user model and observations), every id,
  timestamp, status and the tool call's shape kept. A test pins that — and that no
  `zip` stopped short, which would leave an English text behind.
- **The reel** takes its showcase by its language (`Showcase::of`), and the
  interface speaks the same one, so `LOOKS` gained `(Dark, Ru)`:
  `mindfork-demo-dark-ru`, 82 frames, 24.1 s; GIF 1 056 KB, WebP 923 KB.
- **The flowchart's labels are the English ones' lengths to the character**
  (17/20/19/20). The first translation bent the middle leg into a jog — exactly
  the trap the English fixture's comment warns of — and it was the eye on the
  rendered frame that caught it; a test now walks the middle leg from the arrow
  up to the decision node in every look's last answer frame, and fails on the
  first draft's labels (checked) and passes on these.
- **Russian through and through** — a test holds each beat's Russian needles,
  the interface's own words among them (the role labels, the thoughts' header,
  the input box's hint, the self-model screen's title), and that no frame
  carries the English showcase's words or labels.
- **The source-language gate**: the Russian texts sit in an explicit
  `cyrillic-ok` block in a test-only file. Checked that the gate is real — without
  the block and without the file's mention of `#[cfg(test)]` (which alone also
  puts the scanner in test mode), 48 lines are flagged; with either, none. A
  control run on the untracked file proved nothing: the scanner reads tracked
  files only.
- **Left as it was**: the note card's arguments (`title`, `text`) and result are
  the English fixture's shape, which is not the real tool's (one `content`; "Note
  saved (id=…)"). Making both honest regenerates the committed stills — a task
  of its own, with the Russian file to change alongside.

