# Demo mode and generated screenshots — design plan

Status: **track complete (2026-08-10).** Forks decided by the user
2026-08-10; stage 1 (MVP hero shot) merged on a go; stage 2 (the full
capture set + the drift gate + README embeds) merged; stage 3 (the
interactive `mindfork demo`) shipped the un-gated mock engine, the demo
supervisor, the seeded throwaway root and the CLI subcommand. Stage 4 (an
SVG writer for mindfork.io) was deliberately deferred to the site work and
lives in [docs/roadmap.md](../roadmap.md). One deviation from §4 as written:
the "demo" marker landed in the **feed header caption** (the engine is
honestly named `demo (mock engine)` via the external-mode model name — zero
new UI surface) rather than as a dedicated status-bar chip, which would have
overloaded the background-tasks slot or grown new render/height plumbing for
one word.

## 1. Context and goal

The repository opens to the public soon, and the README and the upcoming
mindfork.io site need screenshots. Two constraints rule out the obvious
route of screenshotting a live session:

- the maintainer's own profile holds private conversations — nothing from a
  real data directory can appear in a capture;
- the app changes quickly, and hand-made screenshots rot. This project's
  standing answer to silent rot is a gate, not discipline — screenshots
  should be **generated from the current code** and guarded the same way
  `link_check.py` guards links.

Goal: a demo profile with honest, fabricated content; a way to render the
app's screens from it deterministically; a Python tool that turns those
renders into image files for the README and the site; and a gate that fails
when the committed captures no longer match the code.

## 2. What already exists (survey, 2026-08-10)

- **A scriptable engine.** `MockBackend` (`src/shared/api/mock.rs`)
  implements the full `EngineBackend` contract: streamed text, "thoughts",
  tool calls, usage, finish — with `scripted` / `sequence` / `cancellable`
  constructors. `MockEmbedder` exists too. Both are `#[cfg(test)]`-gated, as
  is `MockSupervisor` (`src/app/supervisor.rs`), the injection point that
  hands a backend to the orchestrator. `Cargo.toml` has no `[features]`
  section today.
- **Headless rendering.** Every top-level screen exposes
  `render(&mut Frame)` and is already rendered in tests through
  `ratatui::backend::TestBackend` — the chat feed, the chat list, settings,
  the self-model screen, search. The idiom is
  `Terminal::new(TestBackend::new(w, h))` → `term.draw(...)` → inspect the
  `Buffer`. No color-preserving serialization exists yet: current helpers
  dump text only.
- **Deterministic colors — with two caveats.** `Palette::dark()` is fully
  `Color::Rgb`; `Palette::light()` is too except `user: Color::Blue`;
  `Palette::auto()` (the default) leans on the terminal (`Color::Reset`,
  named ANSI) and is **not capturable**. No palette sets a background —
  a screenshot renderer must supply the canvas color itself.
- **Data is injectable.** `Paths::with_root(root)` boots storage on any
  directory (today `#[cfg(test)]`-adjacent), and `Orchestrator::bootstrap()`
  is idempotent "load, fill in what's missing" — a prepared data directory
  is loaded as-is, `last_active_chat` decides what opens. There is no
  `--data-dir` flag or env override today; `defaults.json` next to the
  binary can redirect the root.
- **Rich-content building blocks.** Markdown tables, highlighted code,
  Mermaid-as-text, LaTeX approximation, thoughts and tool-call cards all
  have working render paths and per-module test fixtures — scattered, not
  assembled into any ready-made chat.
- **Python tool conventions.** `tools/*.py` are stdlib-only (urllib, not
  requests) + argparse; the one third-party precedent is
  `artwork/build-wordmarks.py`, which uses `fontTools` and probes the system
  for JetBrains Mono with an explicit `--font` override — the exact pattern
  a raster renderer needs.

## 3. Design

Four pieces, one dataflow:

```
demo fixture (Rust)  →  headless render (TestBackend)  →  frame dumps (JSON)
                                                              │
                                          tools/screenshots.py┴→  PNG / SVG
```

1. **Demo fixture — generated, not checked in.** A `demo` module builds the
   profile and chats *in code* from current domain types (so schema changes
   can never silently invalidate it) and writes them to an isolated root via
   `Paths::with_root`. Content is honest showcase material: a conversation
   that exercises streaming-shaped text, a markdown table, a highlighted
   code block, a Mermaid flowchart, LaTeX, a collapsed and an expanded
   thoughts block, a tool call with result. All timestamps fixed, token
   usage scripted — a dump must be byte-stable across runs.
2. **Frame dumps from the app itself.** A headless path renders the chosen
   screens at a chosen size into `Buffer`s and serializes them: per cell —
   symbol, fg RGB, bg RGB, modifiers; per frame — theme, locale, screen id,
   grid size, and the canvas background (a new `pub` constant next to the
   palettes in `theme.rs`, so the "intended" background has one source of
   truth). Wide glyphs are emitted once with their cell span, per the
   project's own width rules (`shared/wrap.rs`). Themes: Dark and Light
   only, never Auto; Light's `user: Color::Blue` gets a concrete RGB in the
   dump (or the palette is fixed to RGB outright — small pre-task).
3. **`tools/screenshots.py`.** Reads dumps, places each glyph at its grid
   cell (no font-metrics guesswork — the grid is authoritative), and writes
   **PNG** (Pillow, JetBrains Mono located the `build-wordmarks.py` way)
   and, in a later stage, **SVG** for the site (font by `@font-face`
   reference, so the site serves the webfont). An optional `--frame` adds
   marketing chrome: padding, rounded corners, a slim title bar, shadow.
4. **The rot gate — a unit test, not a workflow.** The committed artifacts
   are the JSON dumps plus the rendered images. An ordinary `#[test]`
   regenerates the dumps in memory and compares them to the committed
   files; on drift it fails with "re-run the dump + `tools/screenshots.py`".
   Dumps are deterministic, so the gate is exact; image bytes are **not**
   gated (font rasterization varies by machine) — the dump is the truth,
   the PNG is its rendering. CI needs no new job.

Interactive demo mode (`mindfork demo`), if adopted (fork 1): the same
fixture booted in the real TUI on a throwaway root, engine =
un-gated `MockBackend` behind `MockSupervisor`, replying to any input with
canned streamed responses — "try the app without downloading a model."
Requires moving mock out of `#[cfg(test)]` (compiled always; a cargo
feature would complicate every release build for ~120 lines of code).

## 4. Stages

- **Stage 1 — MVP probe.** Fixture with one showcase chat; dump path for
  the chat screen only; minimal `tools/screenshots.py`; output: one PNG,
  dark theme, English. **Go/no-go: the maintainer judges the PNG
  representative and pixel-clean** (grid intact around wide glyphs, colors
  faithful, table/Mermaid/thoughts legible). No live model involved —
  the engine is scripted by design.
- **Stage 2 — the set and the gate.** The agreed screen × theme × locale
  matrix, the drift-gate test, `--frame` polish, README embeds, docs
  (install/README/journal per AGENTS.md §4).
- **Stage 3 — optional, per fork 1.** The interactive `mindfork demo`
  subcommand. Un-gate mock, canned-reply policy, isolation guarantees
  (never touches the real data root), a visible "demo" marker in the
  status bar.
- **Stage 4 — optional, later.** SVG writer for mindfork.io; possibly an
  animated capture (replaying the scripted stream frame by frame into an
  APNG/GIF) — explicitly out of scope until the site work starts.

## 5. Forks (to confirm before implementation)

1. **Scope of "demo".**
   - (a) Screenshot generation only; (b) screenshots now, plus the
     interactive `mindfork demo` as stage 3.
   - User's decision (2026-08-10): **(b)** — screenshots first, then the
     interactive demo mode as its own stage.
2. **Screen set for the committed captures.**
   - User's decision (2026-08-10): chat (the hero shot), chat list,
     **settings twice — the "Model/server" section and the "Tools"
     section** (the latter added by the user: the tool toggles are the
     best single showcase of what the app can do), and the self-model
     screen. **Five captures per theme.**
3. **Theme × locale matrix.**
   - User's decision (2026-08-10): **Dark + Light × English** (ten
     captures total). A Russian set may follow with the site work; it
     would need Russian demo content, so it is out of scope here.
4. **Rot control.**
   - User's decision (2026-08-10): **the drift-gate unit test** —
     regenerate dumps in memory, compare with the committed files, fail
     with a regeneration hint on mismatch. Manual-only refresh and a CI
     auto-commit job were both declined.

Decided by the implementer (recorded here for review): fixture generated
from code rather than checked-in JSON; dumps carry the canvas background
from a single constant in `theme.rs`; PNG first and SVG later; JetBrains
Mono as the capture font (the brand font, OFL); Auto theme never captured.
One consequence of the stage split worth naming: **capture stays entirely
test-side** (the dump regenerator is an `#[ignore]` test, the gate an
ordinary test), so the release binary gains no capture surface; only
stage 3's interactive mode un-gates `MockBackend`/`MockSupervisor` and the
fixture module.

## 6. Risks

- **Emoji and wide glyphs.** The grid comes from the app's own width logic,
  so layout is safe; *rasterizing* color emoji in Pillow is
  platform-fragile. Mitigation: the demo content keeps emoji rare and
  monochrome-safe; the stage-1 go/no-go explicitly checks glyph edges.
- **Light theme's `user: Color::Blue`** must become a concrete RGB (in the
  palette or in the dump) or light captures render a terminal-dependent
  color that no terminal is present to supply.
- **A demo that drifts from reality** — a capture showing something the app
  no longer does — is the failure mode the whole design exists to prevent;
  the gate covers layout and content, but *judgment* (does this still
  represent the app well?) stays a release-checklist glance.
- **Determinism debt**: anything time- or randomness-shaped in a rendered
  screen (timestamps, spinners, generation state) must be pinned by the
  fixture; the gate will catch violations as flaky diffs.
