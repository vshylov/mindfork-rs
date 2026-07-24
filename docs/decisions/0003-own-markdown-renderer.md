# ADR 0003 — Own markdown renderer (tables + LaTeX + theming)

**Status:** accepted (2026-06-16). Refines [ADR 0001](0001-ui-crates-ratatui-030.md)
regarding markdown rendering.
**Context:** `tui-markdown` 0.3.7 (chosen in ADR 0001) turned out to be narrow
in practice:

1. **Tables are not supported at all** — the parser doesn't enable
   `ENABLE_TABLES`, and the `Tag::Table*` handlers write
   `warn!("not yet supported")`. GFM tables fell into the feed as raw text
   (`| … |`, `:---`).
2. **LaTeX `$…$` was not recognized** (no `ENABLE_MATH`), and our unicode
   approximation was applied **globally** to all text: the `$ $` delimiters
   weren't stripped (ugly "$→$"), plus false positives in prose/code (`x^2`,
   `a_b`).
3. **Theme was ignored** — `tui-markdown` renders with a hardcoded
   `DefaultStyleSheet` (cyan headings, etc.), our `Palette` (dark/light/auto)
   had no effect on the feed.

There is no ready-made replacement under `ratatui-core 0.1` (ratatui 0.30)
with tables and math (`ratatui-markdown` locks 0.29 — see ADR 0001).

## Decision

Render markdown with our own "writer" (`Writer`) on top of `pulldown-cmark`
0.13 (it was already in the tree transitively). `shared/markdown.rs`:
`render(input, width, palette) -> Text<'static>`.

- **Parser**: `ENABLE_STRIKETHROUGH | TASKLISTS | MATH | TABLES`. Walker over
  events (a port of `tui-markdown`'s core + our extensions): paragraphs,
  headings, lists, quotes, code, rules, links, emphasis.
- **Theme**: colors (headings/links/list markers) are taken from `Palette`;
  modifiers (bold/italic/reversed/dim) are theme-independent. Code-block
  highlighting — via `syntect` + `ansi-to-tui`, with the syntect theme
  **built from `Palette`** (see "Consequences"), not hardcoded.
- **LaTeX — delimiter-scoped** (as in the .NET `LaTeXConverter`):
  `normalize_delimiters` converts `\(…\)`→`$…$`, `\[…\]`→`$$…$$` (skipping
  code spans/blocks), the parser returns `InlineMath`/`DisplayMath`, and only
  their content goes through `latex_to_unicode`: `\frac`→`a/b`, `\sqrt{x}`→
  `√(x)`, `\pmod{n}`→`(mod n)`, text/font wrappers and accents (`\text`/
  `\mathrm`/`\vec`/`\hat`/…) → their content, operator-name functions
  (`\log`/`\sin`/`\lim`/`\max`/…) → as a word, sizing bracket modifiers
  (`\left`/`\right`/`\big…`) are stripped, sub/superscripts, an extended
  symbol table. Delimiters are stripped by the parser; "bare" commands
  outside `$…$` are left alone.
- **Tables** — native: cells accumulate between `Table*` events, column
  widths are picked "water-fill" style to fit `width` (balancing fit ×
  readability), content wraps by word (`shared/wrap`), on space shortage —
  a horizontal clip with "…". The table is guaranteed ≤ panel width → a
  second wrap pass in `message_feed` is safe.

The `tui-markdown` dependency is removed; direct `pulldown-cmark`, `syntect`,
`ansi-to-tui` are added (the same versions that were pulled transitively).

## Consequences

- spec §11.4 is amended by this ADR: rendering is our own, on
  `pulldown-cmark`; LaTeX acts **only** inside math delimiters.
- **Behavior change**: `\alpha`/`x^2` outside `$…$` are no longer converted
  (fewer false positives; wrap formulas in `$…$`/`\(…\)`).
- **Code highlighting — via theme (done).** Previously syntect took the
  hardcoded `base16-ocean.dark`, not aligned with dark/light/auto. Now
  `shared/markdown.rs::build_code_theme(palette)` builds a syntect `Theme`
  from the semantic `Palette`: scopes → roles (`keyword`→accent,
  `string`→success, `constant.numeric`→warning, `entity.name.function`→user,
  `entity.name.type`→assistant, tags→accent), "default" text and comments —
  an absolute gray, light on a dark background and dark on a light one (new
  flag `Palette.dark`: `Auto`/`Dark`→dark, `Light`→light). Named ANSI colors
  of the palette are converted to RGB (Campbell) — the
  `as_24_bit_terminal_escaped(.., false)` pipeline still emits a 24-bit
  **foreground color** (background/bold/italic are not carried over). Themes
  are cached by palette (`Box::leak`, number of palettes finite →
  `HighlightLines<'static>`).
- Horizontal scrolling for wide tables (instead of clipping) — a possible
  refinement; the current clip is sufficient.
- **LaTeX approximation and writer refinements (2026-07, done)** — plan
  [docs/history/markdown-refinements.md](../history/markdown-refinements.md):
  - **Environments** `\begin{…}…\end{…}` (aligned/cases/pmatrix/…) are
    stripped; `\\` → a line break (by mode: block `$$…$$` — a real one,
    inline `$…$` — "; "), `&` (alignment) and service commands
    (`\label{…}`/`\hline`/`\notag`/…) are removed. Introduced
    `MathMode { Inline, Display }` (two wrappers `latex_to_unicode`/`_display`).
  - **Behavior change**: `\\` outside context is now a line separator
    (literal backslash — `\backslash`), not `\` (was `a \\ b`→`a \ b`, now
    `a; b`).
  - An unrecognized brace command **keeps its braces** (`\binom{n}{k}`/
    `\boxed{x}` no longer collapse together); the symbol table grew (~50),
    plus text wrappers, fractions (`\dfrac`/`\binom`/`\sqrt[n]`), letter
    subscripts (`x_i→xᵢ`), `\mathbb{R}→ℝ` (BMP), and a fallback for
    unmapped groups `x^{q+}→x^(q+)`.
  - **Writer defects**: `$$…$$` without a double blank line; DisplayMath in a
    cell stays in the cell; autolinks don't duplicate the URL; a syntect
    error → a plain line (not lost text); `<br>`→line break; the fence's info
    string resolves syntax by its first token; `---` spans the full width;
    image URLs; `~~~` fences and unclosed `` ` `` in normalization; a
    heuristic for "a price range `$5-$10` — not math".
- **Feed render cache (2026-07, done)** — `widgets/message_feed.rs`:
  `build_lines` is called on every dirty frame (streaming/scrolling) and
  re-ran markdown+syntect over the entire history each time. A per-message
  block cache (key `width`/`palette`/`show_thoughts`, checked via a
  fingerprint of `FeedMessage` fields) recomputes only what changed;
  golden-equivalence of a warm cache vs. a fresh render is covered by tests.
  Orthogonal to the renderer itself.
- **Mermaid diagram rendering in the feed — implemented (2026-07-16)** after
  an upstream fix: `mermaid-text` 0.56.1 closed the multibyte issue (our bug
  report + PR —
  [issue #29](https://github.com/leboiko/markdown-reader/issues/29) /
  [PR #30](https://github.com/leboiko/markdown-reader/pull/30); re-checked
  with the probe — 0 panics, label corruption gone). Implementation per the
  research plan: buffering the block in `Writer` (modeled on `TableBuilder`),
  a flowchart/sequence whitelist via `detect`, **our own** post-check on
  width (the crate's `max_width` is a hint, not a budget), a **hard fallback
  to source** byte-for-byte (golden test), an ASCII mode in the compat
  palette, the `interface.render_mermaid` toggle (default-on — the fallback
  makes the worst case equal to prior behavior). Probe history below.
- **Mermaid rendering probe (2026-07-14) — NO-GO verdict on 0.56.0, was
  deferred** —
  [docs/research/mermaid-ascii-rendering.md](../research/mermaid-ascii-rendering.md).
  At the time, a ` ```mermaid ` block printed as source (syntect doesn't know
  it) — that was correct, no pain. Rendering candidate — `mermaid-text`
  0.56.0 (MIT, pure Rust, only 2 net-new crates; the API fits `Writer`
  without stretching, Latin sequence diagrams render great). Blocker: the
  crate **is not multibyte-safe** — on Cyrillic it (a) **panics** in the
  sequence/state parser (a byte slice on a non-char boundary), meaning a
  Russian diagram from the model would crash the app from the UI render
  thread; (b) **silently corrupts** flowchart labels (a byte offset is
  returned as a char count → the tokenizer overruns on multibyte text: a
  diagram such as `A[Start] -->|yes| B[End]` with Cyrillic labels yields a
  truncated label like `[End]`, and nodes get lost). No workaround:
  0 of 17 real diagrams in `docs/architecture.md` are pure-ASCII, so an
  ASCII-only gate wouldn't have passed any of them. Upstream notified (both
  fixes are one-liners):
  [leboiko/markdown-reader#29](https://github.com/leboiko/markdown-reader/issues/29).
  The implementation plan was validated and is ready (buffer the block in
  `Writer` modeled on `TableBuilder` → whitelist by `detect` → width
  post-check → **hard fallback to source**; clipping, as with tables, isn't
  suitable here — a truncated diagram is unreadable, unlike a truncated
  table). Revisit after the upstream fix.
- Revisit when a markdown crate compatible with ratatui 0.30, with tables and
  math, appears (migration is not mandatory — our renderer is self-sufficient).
