# Research: rendering Mermaid diagrams in the feed (ASCII/Unicode)

**Status:** **implemented (2026-07-16)** per the §4 plan — after upstream
shipped `mermaid-text` 0.56.1 with our multibyte fixes
([issue #29](https://github.com/leboiko/markdown-reader/issues/29) +
[PR #30](https://github.com/leboiko/markdown-reader/pull/30); the maintainer,
per our "broader note", ran a full audit and closed three more spots of the
same class). Re-checked with the spike on 0.56.1: **0 panics** on a corpus of
31 cases (previously 1 + 3 repros), the label corruption is gone
(`│ End │` instead of `│ [End] │`, nodes intact); the width budget remains a
soft hint (14/28 overflows) — closed by our own post-check, as planned. Below
is the history of the 2026-07-14 spike (verdict NO-GO on 0.56.0) that led to
the fix.

Related documents: [ADR 0003](../decisions/0003-own-markdown-renderer.md) (our
own markdown renderer — this is where diagram rendering would plug in),
[spec §11.4](../../spec.md) (the feed and markdown),
[docs/roadmap.md](../roadmap.md).

---

## 1. Task

Models regularly answer with mermaid diagrams (a ` ```mermaid ` block).
Currently such a block isn't broken, but it isn't rendered either:
`resolve_syntax("mermaid")` ([shared/markdown/code.rs](../../src/shared/markdown/code.rs))
misses (syntect's default set has no mermaid), and the block is printed as
plain text between the fences — the user sees the **readable source**. So
there's no bug, just a missed opportunity: render the diagram as ASCII/Unicode
graphics, like Cursor does.

Key concern (stated by the user going in): **diagrams vary a lot, and far not
all of them will fit on screen**.

## 2. Landscape (July 2026)

| Candidate | Assessment |
|---|---|
| **`mermaid-text`** 0.56.0 (MIT) | The profile is ideal: pure Rust, only 2 net-new crates (`mermaid-text` + `ascii-dag`; `chrono`/`unicode-width` are already in the tree). The API matches our signature exactly: `render_with_width(src, Some(w))`, `render_ascii_with_width` (for compat mode/conhost), `detect::detect` (for a whitelist), typed errors (`EmptyInput`/`UnsupportedDiagram`/`ParseError`). **Taken into the spike.** |
| `merman` 0.8.0-alpha | A "headless Mermaid.js in Rust", parity-focused (3500+ SVG baselines), but an alpha combine (SVG/raster/FFI, `resvg`). Overkill for lines in a TUI feed. Rejected. |
| `mermaid-ascii` (Go) | A decorative sidecar binary — against the project's spirit (the `wasmer` sidecar is justified by isolation, ADR 0005, not by cosmetics). Rejected. |
| A homegrown renderer | Sequence is a trivial deterministic layout (~600–800 lines). But an arbitrary flowchart is a Sugiyama layout with edge routing: a qualitatively different order of complexity than anything the project has written itself (`calc`, the CLI parser). A homegrown universal renderer — **no**. |

## 3. `mermaid-text` 0.56.0 spike (2026-07-14)

Corpus: 14 synthetic cases (a sequence diagram like the Cursor screenshot,
Cyrillic, a wide flowchart, broken syntax, a truncated stream, `style`/
`classDef`, `subgraph`, emoji, pie/class/state) + **17 real mermaid blocks
from our own [docs/architecture.md](../architecture.md)** — i.e. diagrams an
LLM wrote for this project. Three go/no-go questions were checked: panics,
width-budget compliance, quality.

### 3.1 Summary

| Outcome | Cases |
|---|---|
| **Panic** | 1 (a real diagram from `architecture.md`) |
| Ok, fit the budget | 13 |
| **Ok, width budget ignored** | 14 |
| Err (→ falls back to the source, correctly) | 3 |

### 3.2 Blocker 1 — panic on sequence diagrams with Cyrillic

`parser/common.rs::strip_keyword_prefix` slices `line[..keyword.len()]` by
**byte** index **before** checking the match and without checking the char
boundary. The sequence parser's keywords have different lengths
(`loop`=4, `actor`=5, `activate`=8, `deactivate`=10, `participant`=11), so a
Cyrillic line very likely lands the byte cut in the middle of a multibyte
character:

| source | outcome |
|---|---|
| `participant Оркестратор` | **PANIC** |
| `loop до готовности` | **PANIC** |
| `alt если готов` / `else иначе` | **PANIC** |
| `loop until ready` (Latin) | ok |
| flowchart / stateDiagram with Cyrillic | ok (no panic) |

Crashes a **real** diagram — #14 from our `architecture.md` (the
auto-reflection sequence diagram, the line
`loop до REFLECT_MAX_ROUNDS=6 раундов…`). Practical consequence: a model's
message with a Russian sequence diagram **would crash the application** — a
panic in the feed's render UI thread, the panic hook restores the terminal,
the process exits. This is a content DoS. Wrapping the render in
`catch_unwind` doesn't fix it — blocker 2 remains.

### 3.3 Blocker 2 — silent corruption of flowchart labels (worse than a panic)

`parser/flowchart.rs::try_consume_pipe_label` documents a char count but
returns a **byte** offset; the tokenizer, meanwhile, walks a `Vec<char>` and
advances the **char** cursor by that value (`i += consumed`). The overrun
equals `byte_len − char_len` of the edge label — the cursor eats into the
start of the next node. No error, no panic — a **plausible but wrong**
result. The only difference between the inputs is the edge label (ASCII
nodes in both):

```
A[Start] -->|yes| B[End]   →  │ Start │──────▸│ End │     ✓
A[Старт] -->|да|  B[Конец] →  │ Старт │──────▸│ [Конец] │ ✗  a bracket leaked into the label
```

On a real authorization diagram, the node `D[Форма входа]` **was lost
entirely** (rendered as an empty `D`), while `C[Доступ разрешён]` got
brackets baked into its label. This is the worst failure mode — the fallback
can't detect it.

The byte indexing is **systemic**, not a single line: `as_bytes()[i]`/
`s[..n]`/offsets from `find()` treated as char counts — in 10+ files under
`src/parser/` (common, flowchart, state, class, er, gantt, git_graph,
journey, pie, sankey, architecture). This is a class of bugs, not one bug.

### 3.4 Blocker 3 — the width budget isn't actually honored

`render_with_width(src, Some(w))` exceeded the budget in **14 of 27**
successful renders: `classDiagram` from `architecture.md` → 203 columns at a
90-column budget; `erDiagram` → 169; `sequenceDiagram` → 185; even the
sequence diagram from the Cursor screenshot gives 69 at `Some(60)`. On its
own this isn't a dealbreaker — our width post-check with a fallback (see §4)
handles it; but combined with §3.2–§3.3 it is.

### 3.5 An "ASCII-only gate" as a workaround — doesn't work

The idea of "only render if the source is pure ASCII" would sidestep both the
panic and the corruption (both bugs are about multibyte content). Measured
against the real corpus: **0 of 17** diagrams in `docs/architecture.md` are
pure ASCII (all have Cyrillic). In this project the gate wouldn't let
**any** diagram through. There's no workaround here.

### 3.6 What's already good (why it's worth coming back to)

- Dependencies are minimal (§2), the API shape fits `Writer` without
  friction.
- **Latin sequence diagrams render beautifully** — exactly like the Cursor
  screenshot.
- The crate is young and actively developed (0.56.0 as of 2026-07-12), MIT,
  issues are open, the maintainer is responsive. Both fixes are one-liners
  (`str::get` instead of a slice; `chars().count()` instead of a byte
  offset); a PR was proposed in the issue.

## 4. Implementation plan (validated by the spike; to be executed after the upstream fix)

Key design decision: **fallback instead of clipping**. For tables
([shared/markdown/table.rs](../../src/shared/markdown/table.rs)), when width
is insufficient we horizontally clip with "…" — a truncated table is still
readable. A diagram with clipped-off arrows is **not**, so the rule is
binary: either the whole diagram or the source, as now. Worst case = current
behavior, the user never gets "mush".

1. **Buffer** the ` ```mermaid ` block in `Writer` (modeled on `TableBuilder`:
   accumulate content, don't highlight) —
   [shared/markdown/writer.rs](../../src/shared/markdown/writer.rs).
2. **Whitelist** by `detect::detect`: `flowchart`/`graph` +
   `sequenceDiagram`. `pie`/`gantt`/`mindmap`/`classDiagram` are almost
   always poor in ASCII → always the source.
3. **Width post-check**: `max_line_width ≤ self.width` (the feed's invariant
   — the crate's budget isn't honored, §3.4).
4. **Hard fallback to the source** on any failure: parser `Err`, outside the
   whitelist, width overflow.
5. **Compat mode**: `render_ascii_with_width` when `palette.compat`
   (conhost/WGL4, `GlyphSet`).
6. **Streaming**: a truncated block usually doesn't parse → the source; once
   it's finished it gets replaced (the feed cache already recomputes the
   streaming message on every chunk).
7. Toggle `interface.render_mermaid` (`#[serde(default)]`); with the hard
   fallback it can be default-on.

Scope: ~300 lines (buffering + config/toggle + tests: golden render, the
width invariant, fallback on broken/wide input). No live run needed (a pure
render module with no engine).

## 5. Conditions for revisiting

- Upstream fixes the multibyte handling
  ([issue #29](https://github.com/leboiko/markdown-reader/issues/29)) —
  then re-check with the spike (the corpus is reproducible, see §6) and
  implement §4;
- or another pure-Rust candidate that's UTF-8 safe shows up.

For now, neither has happened — the block stays as the source (current
behavior, and it's correct).

## 6. Spike artifacts

The spike crate (outside the repo, session scratchpad): a run of the 31-case
corpus + two minimal repros (panic; label corruption — Latin vs Cyrillic).
The repros are reproducible from the text of
[issue #29](https://github.com/leboiko/markdown-reader/issues/29) — both are
given there in full, along with root causes and proposed fixes.
