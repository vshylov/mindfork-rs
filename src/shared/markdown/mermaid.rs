//! Markdown — рендер ```mermaid-блоков диаграммой (крейт `mermaid-text`). Часть
//! модуля [`super`]; см. spec §11.4 и docs/research/mermaid-ascii-rendering.md.
//!
//! Философия — **жёсткий фолбэк вместо клипа** (в отличие от таблиц, где при
//! нехватке ширины допустим горизонтальный клип с «…»): диаграмма с оборванными
//! стрелками нечитаема, поэтому правило бинарное — либо целая диаграмма, либо
//! `None`, и вызывающий ([`Writer::end_codeblock`]) печатает исходник код-блоком,
//! байт-в-байт как при выключенном тумблере. Худший случай = прежнее поведение.
//!
//! Требуется `mermaid-text` ≥ 0.56.1: 0.56.0 паниковал и молча портил подписи на
//! многобайтовом (кириллическом) вводе — наш апстрим-фикс
//! (leboiko/markdown-reader#29/#30). Панику здесь сознательно НЕ ловим
//! (`catch_unwind`): panic-hook приложения восстанавливает терминал на любую
//! панику (в том числе пойманную), так что «поймать и продолжить» оставило бы TUI
//! в сломанном состоянии; полагаемся на аудит апстрима + узкий whitelist.

use mermaid_text::detect::{DiagramKind, detect};

use super::*;

/// Рендерит содержимое ```mermaid-блока в строки диаграммы. `None` — «не берёмся»
/// (тип вне whitelist / не распарсилось / не влезло по ширине): вызывающий обязан
/// показать исходник. Ширина проверяется **нашим** пост-чеком: `max_width` крейта —
/// мягкая подсказка, а не бюджет (sequence/pie её игнорируют целиком), тогда как
/// инвариант ленты «строка ≤ ширины панели» жёсткий — иначе повторный перенос в
/// `message_feed` разорвал бы рамки диаграммы.
pub(super) fn render_mermaid_block(
    src: &str,
    width: usize,
    palette: &Palette,
) -> Option<Vec<Line<'static>>> {
    // Whitelist: flowchart/graph + sequence. Остальные типы (pie/gantt/mindmap/
    // class/state/…) в текстовой графике почти всегда убоги — честнее исходник.
    if !matches!(
        detect(src),
        Ok(DiagramKind::Flowchart | DiagramKind::Sequence)
    ) {
        return None;
    }
    // Компат-режим (conhost/WGL4, spec §11.6) — ASCII-глифы вместо box-drawing.
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
            return None; // пост-чек ширины: не влезло → целиком фолбэк
        }
        lines.push(Line::from(Span::styled(
            l.trim_end().to_string(),
            Style::new().fg(palette.text),
        )));
    }
    // Хвостовые пустые строки крейта не нужны — межблочные отступы даёт Writer.
    while lines
        .last()
        .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
    {
        lines.pop();
    }
    if lines.is_empty() {
        return None; // пустой рендер — нечего показывать, пусть будет исходник
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Валидная sequence-диаграмма (латиница — как на типичном «объясни HTTP»).
    const SEQ: &str = "sequenceDiagram\n    participant Client\n    participant Server\n    Client->>Server: GET /api/data\n    Server-->>Client: 200 OK\n";
    /// Валидный кириллический flowchart (основной язык проекта).
    const FLOW_RU: &str = "flowchart TD\n    A[Пользователь] --> B{Есть токен?}\n    B -->|Да| C[Доступ разрешён]\n    B -->|Нет| D[Форма входа]\n";

    #[test]
    fn renders_sequence_diagram() {
        let lines =
            render_mermaid_block(SEQ, 90, &Palette::default()).expect("должна отрендериться");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("Client"), "{joined}");
        assert!(joined.contains('┌'), "нет box-drawing рамок: {joined}");
    }

    #[test]
    fn renders_cyrillic_flowchart_without_mangling() {
        // Регрессия апстрима (0.56.0 портил подписи): узлы целы, скобки не текут.
        let lines = render_mermaid_block(FLOW_RU, 90, &Palette::default()).expect("рендер");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("Форма входа"), "узел потерян: {joined}");
        assert!(
            !joined.contains("[Доступ"),
            "скобка утекла в подпись: {joined}"
        );
    }

    #[test]
    fn too_narrow_width_falls_back() {
        // Диаграмма не влезает в 20 колонок → None (фолбэк на исходник), не клип.
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
        // Обрубок стрима: заголовок есть, тело оборвано — Ok или None, но не паника;
        // если отрендерился огрызок, его заменит полный блок по дописывании.
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
        let lines = render_mermaid_block(SEQ, 90, &p).expect("ascii-рендер");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains('┌') && !joined.contains('│'),
            "в компат-режиме не должно быть box-drawing: {joined}"
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
                        "строка шире бюджета {w}: {s:?}"
                    );
                }
            }
        }
    }

    // Интеграция через полный рендер markdown — см. тесты writer.rs
    // (mermaid_block_renders_diagram / fallback / выключенный флаг).
}
