//! Markdown — rendering ```mermaid blocks as a diagram (the `mermaid-text`
//! crate). Part of module [`super`]; see spec §11.4 and
//! docs/research/mermaid-ascii-rendering.md.
//!
//! Philosophy — **a hard fallback instead of clipping** (unlike tables, where
//! horizontal clipping with "…" is acceptable when width is short): a
//! diagram with cut-off arrows is unreadable, so the rule is binary — either
//! the whole diagram, or `None`, and the caller
//! ([`Writer::end_codeblock`]) prints the source as a code block,
//! byte-for-byte as with the toggle disabled. Worst case = previous behavior.
//!
//! Requires `mermaid-text` ≥ 0.56.1: 0.56.0 panicked and silently corrupted
//! labels on multibyte (Cyrillic) input — our upstream fix
//! (leboiko/markdown-reader#29/#30). 0.57.0 closed our feature request #32 (a
//! hard width budget via `RenderOptions::max_width_strict` →
//! `Error::TooWide`); we deliberately do NOT use it — our post-check via
//! [`wrap::display_width`] is more accurate (accounts for the double width of
//! CJK/emoji and matches the wrap in `message_feed`), and `render_with_width`
//! already tightens the gaps to the width. We deliberately do NOT catch a
//! panic here (`catch_unwind`): the app's panic hook restores the terminal on
//! any panic (including a caught one), so "catch and continue" would leave
//! the TUI in a broken state; we rely on the upstream audit + the narrow
//! whitelist.

use mermaid_text::detect::{DiagramKind, detect};

use super::*;

/// Renders a ```mermaid block's content into diagram lines. `None` — "not
/// taking this on" (a type outside the whitelist / didn't parse / didn't fit
/// the width): the caller must show the source instead. Width is checked by
/// **our own** post-check: the crate's `max_width` is a soft hint, not a
/// budget (sequence/pie ignore it entirely), whereas the feed's invariant
/// "line ≤ panel width" is hard — otherwise the feed's re-wrap
/// (`message_feed`) would break the diagram's frames.
pub(super) fn render_mermaid_block(
    src: &str,
    width: usize,
    palette: &Palette,
) -> Option<Vec<Line<'static>>> {
    // Whitelist: flowchart/graph + sequence. Other types (pie/gantt/mindmap/
    // class/state/…) are almost always poor as text graphics — the source is
    // more honest.
    if !matches!(
        detect(src),
        Ok(DiagramKind::Flowchart | DiagramKind::Sequence)
    ) {
        return None;
    }
    // Compat mode (conhost/WGL4, spec §11.6) — ASCII glyphs instead of box-drawing.
    let rendered = if palette.compat {
        mermaid_text::render_ascii_with_width(src, Some(width))
    } else {
        mermaid_text::render_with_width(src, Some(width))
    }
    .ok()?;

    let mut lines: Vec<Line<'static>> = Vec::new();
    for l in rendered.lines() {
        let chars: Vec<char> = l.chars().collect();
        if wrap::display_width(&chars) > width {
            return None; // width post-check: didn't fit → fall back entirely
        }
        lines.push(Line::from(Span::styled(
            l.trim_end().to_string(),
            Style::new().fg(palette.text),
        )));
    }
    // Trailing empty lines from the crate aren't needed — Writer already gives
    // inter-block spacing.
    while lines
        .last()
        .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
    {
        lines.pop();
    }
    if lines.is_empty() {
        return None; // an empty render — nothing to show, let it fall back to the source
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid sequence diagram (Latin — as in a typical "explain HTTP").
    const SEQ: &str = "sequenceDiagram\n    participant Client\n    participant Server\n    Client->>Server: GET /api/data\n    Server-->>Client: 200 OK\n";
    /// A valid Cyrillic flowchart (the project's main language).
    const FLOW_RU: &str = "flowchart TD\n    A[Пользователь] --> B{Есть токен?}\n    B -->|Да| C[Доступ разрешён]\n    B -->|Нет| D[Форма входа]\n";

    #[test]
    fn renders_sequence_diagram() {
        let lines = render_mermaid_block(SEQ, 90, &Palette::default()).expect("should render");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("Client"), "{joined}");
        assert!(joined.contains('┌'), "no box-drawing frames: {joined}");
    }

    #[test]
    fn renders_cyrillic_flowchart_without_mangling() {
        // Upstream regression (0.56.0 corrupted labels): nodes intact, no bracket leaks.
        let lines = render_mermaid_block(FLOW_RU, 90, &Palette::default()).expect("render");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("Форма входа"), "node lost: {joined}");
        assert!(
            !joined.contains("[Доступ"),
            "a bracket leaked into a label: {joined}"
        );
    }

    #[test]
    fn too_narrow_width_falls_back() {
        // The diagram doesn't fit into 20 columns → None (fall back to the source), not a clip.
        assert!(render_mermaid_block(SEQ, 20, &Palette::default()).is_none());
    }

    #[test]
    fn non_whitelisted_kind_falls_back() {
        let pie = "pie title X\n    \"A\" : 1\n";
        assert!(render_mermaid_block(pie, 90, &Palette::default()).is_none());
        let state = "stateDiagram-v2\n    [*] --> Idle\n";
        assert!(render_mermaid_block(state, 90, &Palette::default()).is_none());
    }

    #[test]
    fn garbage_and_stream_stub_fall_back() {
        assert!(render_mermaid_block("просто текст", 90, &Palette::default()).is_none());
        assert!(render_mermaid_block("", 90, &Palette::default()).is_none());
        // A stream stub: the header is there, the body cut off — Ok or None, but not a panic;
        // if a stub rendered, the complete block will replace it once finished.
        let _ = render_mermaid_block(
            "sequenceDiagram\n    participant Ser",
            90,
            &Palette::default(),
        );
    }

    #[test]
    fn compat_palette_renders_ascii_frames() {
        use crate::shared::config::Theme;
        let p = Palette::for_theme(Theme::Auto).with_compat(true);
        let lines = render_mermaid_block(SEQ, 90, &p).expect("ascii render");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains('┌') && !joined.contains('│'),
            "there should be no box-drawing in compat mode: {joined}"
        );
    }

    #[test]
    fn every_line_fits_width_budget() {
        for w in [60usize, 90, 120] {
            if let Some(lines) = render_mermaid_block(FLOW_RU, w, &Palette::default()) {
                for l in &lines {
                    let s: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                    let chars: Vec<char> = s.chars().collect();
                    assert!(
                        wrap::display_width(&chars) <= w,
                        "line wider than the budget {w}: {s:?}"
                    );
                }
            }
        }
    }

    // Integration via a full markdown render — see the writer.rs tests
    // (mermaid_block_renders_diagram / fallback / flag disabled).
}
