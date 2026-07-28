# Markdown-render refinements: LaTeX approximation, writer, feed cache

> **Status:** implemented (2026-07-11), branch `feat/markdown-refinements` —
> all six stages done as separate commits. See the CLAUDE.md journal,
> "Post-M9: markdown-render refinements."
> **Context:** based on an analysis of `src/shared/markdown/` (an own renderer,
> [ADR 0003](../decisions/0003-own-markdown-renderer.md)) and its call from
> `widgets/message_feed.rs`. Problems found — event-walker defects, gaps in the
> LaTeX unicode approximation on real LLM output, false positives of the
> math extension, and no render cache in the feed.
> **Method:** six focused PRs (stages below). Each is self-contained,
> with tests; order 1→5 is recommended (fewest conflicts in `latex.rs`),
> stage 6 is independent and can run in parallel with any other.

The cross-cutting DoD for every PR is in §7. Deliberate non-goals are in §8.
Existing tests that change (due to a behavior change) are in §9.

---

## 1. Stage 1 — writer: event-stream defects

Branch `fix/markdown-writer-events`. Only `shared/markdown/writer.rs` (+ tests).

### 1.1 Extra blank line before a display formula

**Problem.** `$$…$$` as its own paragraph: `start_paragraph` opens an empty
"current" line (for `push_span`), but `display_math` doesn't use it — it
places content via `push_line`. Two blank lines end up between the text and
the formula instead of one.

**Fix.** In `display_math`: place the formula's first line into the current
line if it's "empty of content" (all spans are whitespace; this preserves
quote prefixes `> `), via `push_span`; otherwise — as now, on a new line.
Subsequent lines — `push_line` + `push_span` (not `Line::from(...)` — so
quote prefixes get added the normal way).

**Tests:** (a) `text\n\n$$E=mc^2$$\n\nend` — exactly one blank line between
"text" and the formula; (b) a formula inside a quote — prefix `> ` on the
formula line; (c) a formula as the message's first element; (d)
`before $$x$$ after` — prior behavior (formula on its own line) isn't broken.

### 1.2 DisplayMath inside a table cell leaks into the feed

**Problem.** `push_span` redirects into the cell, but `display_math` calls
`push_line` with no `in_table_cell()` check — the formula's lines end up in
the main feed (and get drawn *above* the table, since the table renders on
`End(Table)`), the cell loses the content.

**Fix.** At the start of `display_math`: if `in_table_cell()` — convert, join
the lines with `"; "` and hand them off as a single `push_span`, `return`.

**Test:** a table with `$$x+1$$` in a cell — content in the cell, nothing
above the table.

### 1.3 An autolink prints the URL twice

**Problem.** `<https://example.com>` → `Start(Link)` + `Text(url)` +
`End(Link)`; `end_link` appends ` (url)` — "url (url)".

**Fix.** In `start_tag`, for `Tag::Link { link_type, dest_url, .. }`, don't
store `self.link` for `LinkType::Autolink | LinkType::Email` (the suffix
won't be added; `end_link` is already a no-op on `None`).

**Tests:** an autolink — the URL once; a regular `[t](u)` — the previous
look `t (u)`.

### 1.4 Losing code lines on a highlighting error

**Problem.** In `Writer::text` (the `code_highlighter` path), the
`filter_map(..ok())` chain silently drops the line if `highlight_line`/
`into_text` returned `Err`. A reference for correct degradation already
exists — `highlight_code` in `mod.rs` (error → a flat line).

**Fix.** Replace the chain with an explicit loop that falls back to a flat
line (modeled on `highlight_code`); where possible — a shared private helper
`highlight_or_plain(line, hl) -> Vec<Line>` for both spots.

**Test:** a syntect error can't be triggered artificially — covered by the
smoke "code-block content isn't lost" (already exists) + a unit test on the
helper in the flat path. This item is mainly about robustness.

### 1.5 Treat `<br>` as a line break

**Problem.** `Event::Html`/`InlineHtml` are ignored wholesale. The practical
damage — `<br>` (models love it in table cells): the tag drops out, words
run together with no space.

**Fix.** In `handle_event`: `InlineHtml(s) | Html(s)`, where `s.trim()`
case-insensitively ∈ {`<br>`, `<br/>`, `<br />`} — behave like `HardBreak`
(in a cell → a space, otherwise → a new line). Other HTML — the previous
ignoring (content between tags already arrives as separate `Text` events).

**Tests:** `a<br>b` in a paragraph → two lines; in a table cell → `a b`.

### 1.6 The code-block info-string language tag

**Problem.** ` ```rust,no_run ` / ` ```python title=x ` — `resolve_syntax`
gets the whole info string and misses, the block goes unhighlighted.

**Fix.** In `start_codeblock`, resolve by the first token
(`lang.split([',', ' ', '\t']).next()`); print the original tag on the
fence line (it's data).

**Test:** ` ```rust,no_run ` gives RGB highlighting spans.

### 1.7 A horizontal rule at the panel's width

**Problem.** `rule()` draws `───` (3 characters) — next to full-width tables
it looks stubby.

**Fix.** `"─".repeat(self.width.max(3))` — the panel width is already
available on `Writer` (used by tables); the line ≤ the width, so the feed's
second wrap pass is a no-op. There are no existing tests on `───` (checked),
but run the `message_feed` tests (the rail on DIM lines).

**Test:** the line spans the width `width`.

### 1.8 Images: preserve the URL

**Problem.** `Tag::Image` isn't handled: the alt text prints (the `Text`
events inside), the URL is lost.

**Fix.** A separate field `image: Option<String>` (don't reuse `self.link` —
an image can be nested inside a link) + `End(Image)` → a suffix ` (url)` in
link style, as with `end_link`.

**Test:** `![alt](http://x/i.png)` → `alt (http://x/i.png)`.

---

## 2. Stage 2 — latex: degrading unknowns + symbol tables

Branch `feat/latex-tables`. Only `shared/markdown/latex.rs` (+ `mod.rs` item 2.7).

### 2.1 Don't merge unknown brace-commands with their arguments

**Problem.** The final brace strip in `latex_to_unicode` is global:
`\overbrace{a+b}` → `\overbracea+b`, `\dfrac{a}{b}` → `\dfracab`. The
degradation is unreadable and worse than leaving it as-is.

**Fix.** In `apply_brace_commands`, the "name not recognized, followed by
`{`" branch: copy `\name` and **all consecutive groups** with protected
braces (the same `LBRACE`/`RBRACE` sentinels used for literal `\{`/`\}` —
hoist them into module constants); group contents — recursively. The final
brace strip puts `{`/`}` back: `\foo{a}{b}` stays `\foo{a}{b}`, with `a`/`b`
inside converted (`\foo{\alpha}` → `\foo{α}`).

### 2.2 Frequent commands that were missing

- **`\dfrac`/`\tfrac`/`\cfrac`** — aliases of `\frac` (models emit them often).
- **`\binom{n}{k}`** → `C(n, k)`.
- **`\sqrt[n]{x}`**: recognize the optional `[n]` after `sqrt`; `3` → `∛(x)`,
  `4` → `∜(x)`, otherwise — if `n` maps fully to a superscript → `ⁿ√(x)`,
  fallback `√[n](x)`. Currently it comes out as `√ [3](x)`-like mush.
- **`\left.` / `\right.`**: after stripping the modifier, swallow the
  delimiter dot (currently a dangling `.` is left). In `replace_commands`: if
  `""` was substituted for `left`/`right` and the next character is `.` —
  skip it.
- **`\overset{a}{b}` / `\underset{a}{b}` / `\stackrel{a}{b}`** → the base
  argument `b` (the annotation is dropped, like the diacritic on accents;
  `\overset{def}{=}` → `=`).
- **Text wrappers** (`is_text_command`): + `mbox`, `emph`, `textnormal`,
  `texttt`, `textsf`, `textsl`, `textup`, `overrightarrow`, `overleftarrow`,
  `overbrace`, `underbrace`.

### 2.3 Growing the symbol table (`command_symbol`)

Selection rule: **BMP only**, frequency in model output. Mandatory list:

| Commands | Glyph |
|---|---|
| `langle` / `rangle` | ⟨ ⟩ |
| `lfloor` / `rfloor` / `lceil` / `rceil` | ⌊ ⌋ ⌈ ⌉ |
| `mid` | \| |
| `parallel` / `Vert` | ‖ |
| `setminus` / `smallsetminus` | ∖ |
| `cong` / `simeq` | ≅ ≃ |
| `vdash` / `dashv` / `models` / `vDash` | ⊢ ⊣ ⊨ ⊨ |
| `triangleq` / `coloneqq` / `coloneq` | ≜ ≔ |
| `Longleftarrow` / `longleftrightarrow` / `Longleftrightarrow` | ⟸ ⟷ ⟺ (symmetric with existing ones) |
| `hookrightarrow` / `hookleftarrow` / `twoheadrightarrow` / `rightsquigarrow`, `leadsto` | ↪ ↩ ↠ ⇝ |
| `nearrow` / `searrow` / `nwarrow` / `swarrow` | ↗ ↘ ↖ ↙ |
| `vartheta` / `varsigma` / `varrho` / `varkappa` / `varpi` | ϑ ς ϱ ϰ ϖ |
| `subsetneq` / `supsetneq` / `nsubseteq` / `nsupseteq` | ⊊ ⊋ ⊈ ⊉ |
| `sqsubseteq` / `sqsupseteq` / `sqcap` / `sqcup` / `uplus` | ⊑ ⊒ ⊓ ⊔ ⊎ |
| `bigcup` / `bigcap` / `coprod` / `iint` / `iiint` | ⋃ ⋂ ∐ ∬ ∭ |
| `odot` / `ominus` / `oslash` | ⊙ ⊖ ⊘ |
| `asymp` / `doteq` | ≍ ≐ |
| `dagger` / `ddagger` / `diamond` / `Box`, `square` / `blacksquare` | † ‡ ⋄ □ ■ |
| `triangleleft` / `triangleright` / `ni`, `owns` / `checkmark` | ◁ ▷ ∋ ✓ |

Extending beyond the list is at the implementer's discretion (same BMP
rule). Test — selectively by group, not every glyph.

### 2.4 `normalize_delimiters`: tilde fences and an unclosed backtick

- **`~~~` fences**: a run of ≥3 tildes is treated as a fence (skip to a
  closing run ≥ the same length), by analogy with backtick runs. The ≥3
  threshold doesn't affect `~~strikethrough~~`. Currently `\(…\)` inside a
  `~~~` block gets converted — **code corruption** (the RAG chunker already
  accounts for `~~~`, the renderer doesn't). No anchor to line start (KISS:
  runs of ≥3 tildes don't occur in prose).
- **An unclosed short backtick run (1–2)**: currently it "swallows" the
  rest of the message (copied verbatim to the end) — a lone `` ` `` in
  prose disables formula normalization after it. Per CommonMark an unclosed
  run is a literal: copy the run itself and **continue** normalizing. For
  runs of ≥3 (fences), keep the previous "verbatim to the end" — that's the
  correct behavior during streaming (an unclosed ` ``` ` mid-generation is
  normal, normalizing inside it isn't safe).

**Tests:** `\(x\)` inside a `~~~` block is left untouched; `~~strike~~`
isn't treated as a fence; a formula after a lone backtick is normalized; a
formula inside an unclosed ` ``` ` — isn't.

### 2.5 A recursion-depth ceiling for `apply_brace_commands`

A pathological input (thousands of nested `\frac`) can overflow the stack —
unbounded recursion. An internal function with a `depth` parameter,
`MAX_DEPTH ≈ 64`; deeper — copy the group verbatim. Test: 10,000 nested
`\frac{` doesn't panic.

### 2.6 (in passing) A typo in a doc comment

`mod.rs:1` — a doc-comment typo: a stray Latin `l` where a Cyrillic `л` was
intended (a mixed-script glitch in the Russian source text at the time) —
fix it.

---

## 3. Stage 3 — latex: letter sub/superscripts, `\mathbb`, a bracket fallback

Branch `feat/latex-scripts`. Only `latex.rs`.

### 3.1 Letter super-/subscripts

Currently `superscript`/`subscript` only know digits and signs (+ `n`, `i`
for superscript), so the most common forms don't convert: `x_i`, `a_n`,
`x^T`, `\sum_{i=1}^{n}` (gives the asymmetric `∑_i=1ⁿ`).

Add:

- superscript, lowercase: `ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ` (Unicode has no `q`);
- superscript, uppercase: `ᴬᴮᴰᴱᴳᴴᴵᴶᴷᴸᴹᴺᴼᴾᴿᵀᵁⱽᵂ` (no C F Q S X Y Z);
- subscript: `ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ`;
- (optional) Greek, where available: sup `ᵝᵞᵟᶿᵠᵡ`, sub `ᵦᵧᵨᵩᵪ`.

The "all-or-nothing" semantics per group is preserved (`x_b` stays `x_b`).

### 3.2 Fallback for an unmapped group — parentheses

Currently, on a mapping failure `_{i=1}` becomes `_i=1` (the global strip
removes the braces — grouping is lost). New behavior: a group that couldn't
be mapped converts to `_(…)` / `^(…)` (the content has already gone through
command substitution). A single character with no group — as before
(`x^q` → `x^q`).

### 3.3 `\mathbb{…}` → real double-struck (BMP)

A special branch in `apply_brace_commands` **before** the general
`is_text_command`: a group of a single uppercase letter with a BMP
counterpart → a glyph: `ℂ ℍ ℕ ℙ ℚ ℝ ℤ`. Otherwise — the previous fallback
(content as-is): `\mathbb{XY}` → `XY`. Optionally, the same mechanism for
`\mathcal` → `ℬ ℰ ℱ ℋ ℐ ℒ ℳ ℛ` and `\mathfrak` → `ℭ ℌ ℑ ℜ ℨ`.
Supplementary-plane characters (`𝔸…`, lowercase) are **not** used —
terminal support is uneven (see the past supplementary-plane input
pitfalls), and the payoff is small.

**Tests:** `x_i`→`xᵢ`, `a_n`→`aₙ`, `x^T`→`xᵀ`, `\sum_{i=1}^{n}`→`∑ᵢ₌₁ⁿ`,
`x \in \mathbb{R}`→`x ∈ ℝ`, `\mathbb{XY}`→`XY`, fallback `x^{q+}`→`x^(q+)`.

---

## 4. Stage 4 — latex + writer: environments and formula mode

Branch `feat/latex-environments`. `latex.rs` + call sites in `writer.rs`.
The most valuable stage for quality: `\begin{aligned}…\end{aligned}` is the
main source of "mush" from models.

### 4.1 Formula mode (Inline / Display)

The internal function gets a `MathMode { Inline, Display }` parameter;
externally — two thin wrappers `latex_to_unicode` (Inline, the previous
signature — stages 2–3 tests aren't touched) and `latex_to_unicode_display`.
Call sites: `InlineMath` → Inline, `display_math` → Display.

### 4.2 A new `strip_environments` pass

In the pipeline — after protecting literal `\{`/`\}`, before
`apply_brace_commands`. The pass scans `\` + character pairs (it sees
escaped `\&`, `\\`, etc. as a pair, not one character at a time):

- `\begin{name}` / `\end{name}` → remove together with the name group
  (including `align*` etc.). Wrapping matrices in brackets by environment
  type — **not in the MVP** (just strip them; recorded in §8).
- `\\` (+ an optional vertical spacer `[6pt]` — swallow it) → a line
  separator: Display → `\n`, Inline → `"; "`.
- a lone unescaped `&` (alignment) → remove; `\&` stays.
- `\hline`, `\notag`, `\nonumber` → remove; `\label{…}`, `\cline{…}` →
  remove together with the group.

### 4.3 Sanitizing Inline output

In Inline mode, on output `\n` → a space: ratatui spans must not contain
line breaks (the source can be either `\\` or line breaks inside `$…$` in
the source text).

### 4.4 `\backslash`

Into the command table: `\backslash` → `\` — a literal backslash in math is
now written this way (the semantics of `\\` changes to "line separator",
see §9).

**Tests:** display `\begin{aligned} x &= y \\ z &= w \end{aligned}` → two
lines `x = y` / `z = w`; `cases`; `pmatrix` (content line by line); inline
`$a \\ b$` → `a; b`; `\label{eq:1}` disappears; `\\[4pt]`; `\&` is
preserved; `\backslash` → `\`; a `$$` formula with an environment inside a
quote (together with 1.1).

---

## 5. Stage 5 — writer: a "price range isn't math" heuristic

Branch `fix/math-price-guard`. Only the `InlineMath` handler in `writer.rs`.

**Problem.** `ENABLE_MATH` is global: "$5-$10" → `InlineMath("5-")` — the
dollar signs disappear, "5-10" turns into "5-" + "10". (pulldown doesn't
match lone prices like "$5 costs" anyway — the closing `$` can't sit right
after a space.)

**Fix.** Before conversion: if the content consists only of
`[0-9 . , space - – /]` **and ends in `-` / `–` / `/`** (the signature of a
price range/fraction, not a legitimate number) — re-emit it literally as
`$…$` in the current style. Legitimate `$3.14$`, `$2+2$`, `$n$`, `$O(n)$`
aren't affected (they end in a digit/letter or contain math symbols).
`DisplayMath` is untouched.

**Tests:** `$5-$10`, `$5/$7` → verbatim with the dollar signs; `$3.14$`,
`$2+2$`, `$x^2$` → the previous conversion.

---

## 6. Stage 6 — message_feed: a render cache (performance)

Branch `perf/feed-render-cache`. Only `widgets/message_feed.rs`. Independent
of stages 1–5, the biggest systemic win.

**Problem.** `build_lines` re-runs `markdown::render` (including syntect)
over **every** message in the chat + a repeated `wrap`, on every dirty
frame. During streaming, dirty is raised on every chunk (coalesced by the
50ms tick → up to ~20 full history re-renders per second); every wheel step
is also a full re-render. Yet only the tail message actually changes.

**Design.**

- Fields on `MessageFeed`: `cache: Vec<CachedBlock>` + `cache_key:
  Option<CacheKey>`.
  - `CacheKey { width: usize, palette: Palette, show_thoughts: bool }` —
    `Palette` is already `Copy + Hash + Eq` (the syntect-theme cache key). A
    key change (resize, theme/compat, `Ctrl+T`) → `cache.clear()`.
  - `CachedBlock { fingerprint: u64, lines: Vec<Line<'static>> }` — **the
    exact contribution of a message** to `build_lines`: the body after
    markdown, wrapping into `inner`, the rail, and the decision about a
    trailing separator (the separator is a function of the block itself,
    see `body_ends_blank`).
- `fingerprint` — `std::hash::DefaultHasher` over all `FeedMessage` fields
  (`role`, `text`, `thoughts`, `streaming`, `tools[]`: name/arguments/
  result/text_offset). A hash is O(len) against a render at
  O(len·markdown+syntect) — orders of magnitude cheaper. A streaming
  message changes `text` on every chunk → the fingerprint doesn't match →
  only it gets recomputed. Reactivating a chat rebuilds the
  `Vec<FeedMessage>`, but the fingerprints of prior messages still match —
  the cache survives it for free; `cache.truncate(messages.len())` handles
  truncation.
- Serving from the cache — a clone of `Vec<Line<'static>>` (MVP: a memcpy
  of strings is orders of magnitude cheaper than markdown+syntect). A
  borrowing variant (`Line<'a>` from `Cow::Borrowed` with no string copies)
  — a possible follow-up, not done in the MVP (it complicates signatures).
- The outer `flat_map(wrap_line)` in `render()` stays (a safety-net no-op
  pass over already-fitted lines — cheap).
- The empty-feed placeholder — outside the cache. A u64 hash collision is
  theoretical (the consequence — a stale frame until the next mutation),
  accepted.

**Tests.**

- **Golden equivalence** — the main one: for various scenarios (a regular
  chat; streaming one chunk at a time; tool blocks; thoughts
  collapsed/expanded; a width change; a palette change; a tail rewrite),
  the output of `build_lines` from a warm cache is line-for-line equal to
  the output of a fresh `MessageFeed::new()` on the same data.
- Mutating a message's text invalidates the cache (the new text shows up in
  the output).
- `toggle_thoughts` / a palette/width change produce new, correct output.
- (optional) a counter of real render calls under `#[cfg(test)]` — that on
  a repeat frame only the changed block is recomputed.

---

## 7. Cross-cutting requirements (DoD for every PR)

- `cargo fmt` / `cargo clippy --all-targets -- -D warnings` / `cargo test`
  — green; tests live next to the code (`mod tests` in the submodule — the
  module's convention).
- An entry in the CLAUDE.md journal (post-M9), modeled on existing ones.
- A behavior change → edit spec §11.4 and, if needed, a note in ADR 0003's
  "Consequences" (affected: `rule` at full width, `<br>`, autolinks, `\\`
  semantics, the price heuristic, environments).
- Commits — with a `Co-Authored-By` trailer (see CLAUDE.md "Conventions").

## 8. Out of scope (deliberate non-goals)

- **Full LaTeX / rendering formulas as an image** — as before (spec, ADR 0003).
- **Indented (4-space) code blocks in `normalize_delimiters`**: recognizing
  them requires block context (list continuations are also indented) — the
  risk of false skips of conversion outweighs the benefit; `\(` inside
  indented code remains a known limitation (a rare case, models emit
  fenced blocks).
- **Wrapping matrices by environment type** (`pmatrix` → `(…)` etc.) — the
  MVP just strips the wrappers; revisit if live output shows a need.
- **A compat gate for math glyphs** (a WGL4-safe subset of the table under
  `terminal_compat`): math is content, not chrome; `⁴…⁹`/arrows already
  render as tofu in conhost. Groundwork: `render` already gets a `Palette`
  with the flag — thread it into `latex_to_unicode` if desired.
- **A borrowing feed cache** (no string clones) — after the MVP cache,
  based on measurements.
- **Italics/styling for math spans** — a matter of taste, a separate
  micro-PR if desired.
- **Horizontal scroll for wide tables** — existing groundwork from
  ADR 0003, untouched.

## 9. Existing tests that change

The behavior change is deliberate; tests are updated in their own stage:

| Test (`latex.rs`) | Before | After | Stage |
|---|---|---|---|
| `escaped_backslash_and_brace` | `a \\ b` → `a \ b` | `\\` is a line separator (Inline: `a ; b`); a literal backslash is `\backslash` | 4 |
| `unmappable_script_strips_group_braces` | `x^{ab}` → `x^ab` | `ab` now maps (`xᵃᵇ`); the example changes to something unmappable: `x^{q+}` → `x^(q+)` | 3 |
| `frac_sqrt_and_text_wrappers` | `\mathbb{R}` → `R` | `ℝ` | 3 |

## 10. Order and dependencies

```
Stage 1 (writer defects)         — independent, first (small targeted fixes)
Stage 2 (latex tables)           — independent
Stage 3 (latex scripts)          — after 2 (shared spots in latex.rs, fewer conflicts)
Stage 4 (environments, MathMode) — after 2–3 (changes the latex_to_unicode pipeline)
Stage 5 (price heuristic)        — any time (an isolated InlineMath handler)
Stage 6 (feed cache)             — independent, can run in parallel with any other
```

Stages 2 and 3 can be merged into one PR if desired (both are local edits
to `latex.rs` tables); 1, 4, 5, 6 — strictly separate.
