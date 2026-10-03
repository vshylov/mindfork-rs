# The animated demo — a reel generated from code

Status: **design accepted 2026-10-03** (its shape is the promotion plan's §6,
[promotion.md](promotion.md)); **stage 1 — the MVP probe — implemented**, its
go/no-go (the owner judging the GIF) pending; measurements in §7.

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
interface change, and the animation always shows the released version. The
details — which job runs the regenerator, how the site receives the files —
are stage 2's.

## 4. Stages

- **Stage 1 — MVP probe.** The director, the scenario, its tests, the
  regenerator and `tools/demo_reel.py`; the GIF rendered locally.
  **Go/no-go: the owner judges the GIF** — legible, smooth, representative,
  and of a size a README can carry.
- **Stage 2 — publication.** CI at release, the site, the README's and the
  site's embeds; the Russian variant for Habr.
- **Later, if asked:** the light theme.

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
  stage 2.

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
