# ADR 0001 — UI crates under ratatui 0.30 (textarea, markdown, scroll)

**Status:** accepted (2026-06-14); the markdown part refined by
[ADR 0003](0003-own-markdown-renderer.md) (post-M9: `tui-markdown` replaced with
our own renderer for tables/math/theming). Closes the `[R]` "UI crates" from
[plan.md §M3](../history/plan.md).
**Context:** the project is pinned to `ratatui 0.30.1`. spec §11.4–11.5 proposes
`tui-textarea` (input) and `ratatui-markdown` (markdown rendering), with a
fallback to `tui-markdown`. We need to verify compatibility with the actual
ratatui version.

## Facts (verified empirically: `cargo add` + `cargo tree` + `cargo check`)

ratatui 0.30 split its core into the crate `ratatui-core 0.1`. Ecosystem crates
divide into those that moved to `ratatui-core ^0.1` (compatible with 0.30) and
those still holding `ratatui ^0.29` (incompatible — they pull in a second copy
of ratatui).

| Crate | Version | Dependency | Verdict |
|---|---|---|---|
| `tui-textarea` | 0.7.0 (10.2024) | `ratatui ^0.29` | ❌ outdated, locks 0.29 |
| `ratatui-textarea` | — | — | ❌ does not exist on crates.io |
| `ratatui-markdown` | 0.3.6 | `ratatui ^0.29` | ❌ locks 0.29 |
| **`tui-markdown`** | 0.3.7 (12.2025) | `ratatui-core ^0.1` | ✅ builds with 0.30.1 |
| **`tui-scrollview`** | 0.6.5 | `ratatui-core ^0.1` | ✅ builds with 0.30.1 |
| `edtui` | 0.11.3 (04.2026) | `ratatui-core ^0.1` | ✅ builds, but modal (vim) |

All compatible candidates unify on a single `ratatui-core 0.1.1` (no ratatui
duplicates), `cargo check` green.

## Decision

1. **Markdown → `tui-markdown` 0.3.7** (instead of `ratatui-markdown`, which
   locks 0.29). It supports essentially everything needed: markdown →
   `ratatui::text::Text`, code highlighting (syntect). Used in
   `shared/markdown.rs`; on top of it — our unicode approximation of LaTeX and
   collapsible CoT/tool blocks (our code, spec §11.4).
2. **Feed scrolling → `tui-scrollview` 0.6.5**.
3. **Input → our own minimal multiline widget** (`widgets/input_box.rs`), NOT an
   external crate. Rationale:
   - `tui-textarea`/`ratatui-textarea` are unavailable under 0.30 (see the table).
   - `edtui` is compatible, but its modal (vim) model does not match the UX of a
     simple chat field and pulls in extra dependencies (clipboard/image/x11rb).
   - `tui-textarea` has **no** API at all for diagnostics/underlines of arbitrary
     ranges — that is a separate open `[R]` for spellcheck (spec §11.5). Our own
     widget closes both questions at once: full control over `Shift+Enter` vs
     `Enter`, scrolling, and highlighting of misspelled words (per-span style).
   - The cost — ~250 lines of well-testable code with no new dependencies.

## Consequences

- spec §11.4–11.5 is amended by this ADR: `ratatui-markdown` → `tui-markdown`,
  `tui-textarea` → our own widget.
- The open `[R]` "Spellcheck rendering in textarea" (spec §11.5) is **removed**:
  we draw it ourselves with a per-span `UNDERLINED` style over our own input
  widget.
- Revisit when an update of `tui-textarea`/`ratatui-markdown` for 0.30 ships (if
  a richer one appears — migration is not mandatory, our widget is
  self-sufficient).
