//! Capture recipes for the generated screenshots (docs/demo-screenshots.md).
//!
//! Each recipe builds a screen from the demo fixture (`features/demo`), renders
//! it into a `TestBackend` and serializes the frame (`shared/shot`). The
//! `#[ignore]` test at the bottom is the regenerator: it (re)writes the
//! committed dumps under `artwork/screenshots/dumps/`, which
//! `tools/screenshots.py` then turns into images. Ordinary tests keep the
//! recipes honest — deterministic, grid-covering, and with the showcase
//! content actually inside the frame.

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::features::demo;
use crate::screens::chat::ChatScreen;
use crate::shared::config::{AppConfig, Theme};
use crate::shared::i18n::Lang;
use crate::shared::shot::{self, ShotFrame};

/// Stage-1 frame size: wide enough for the table and the flowchart to sit
/// comfortably, tall enough that the final exchange — thoughts, table,
/// flowchart, tool card — fits the feed in one screen.
pub const SHOT_W: u16 = 116;
pub const SHOT_H: u16 = 44;

fn theme_slug(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
        // Never captured (design plan §5): Auto's colors belong to a terminal
        // that isn't there.
        Theme::Auto => unreachable!("Auto theme is not capturable"),
    }
}

/// The hero shot: the chat screen over the showcase conversation, thoughts and
/// tool details expanded, English UI.
pub fn chat_frame(theme: Theme) -> ShotFrame {
    let mut screen = ChatScreen::new();
    let mut config = AppConfig::default();
    config.interface.theme = theme;
    config.interface.language = Lang::En;
    screen.set_settings(
        config,
        Vec::new(),
        Vec::new(),
        Default::default(),
        Vec::new(),
    );
    // The demo depicts a healthy stack: chat and embeddings ready (the memory
    // features are part of the showcase), impersonation not configured (its
    // chip stays hidden).
    screen.set_server_status(crate::shared::server::ServerStatuses {
        chat: crate::shared::server::ServerStatus::Ready,
        embed: crate::shared::server::ServerStatus::Ready,
        impersonation: crate::shared::server::ServerStatus::NotConfigured,
    });
    screen.activate_chat(
        demo::chat_id(),
        demo::CHAT_TITLE.into(),
        &demo::showcase_messages(),
        "",
        demo::feed_view(),
        None,
        None,
    );
    let mut term = Terminal::new(TestBackend::new(SHOT_W, SHOT_H)).unwrap();
    term.draw(|f| screen.render(f)).unwrap();
    shot::capture(
        term.backend().buffer(),
        &screen.palette(),
        "chat",
        theme_slug(theme),
        "en",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn frame_text(frame: &ShotFrame) -> String {
        frame
            .rows
            .iter()
            .map(|row| row.iter().map(|c| c.s.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Byte-stable across runs — the property the drift gate stands on.
    #[test]
    fn chat_frame_is_deterministic() {
        let a = serde_json::to_string(&chat_frame(Theme::Dark)).unwrap();
        let b = serde_json::to_string(&chat_frame(Theme::Dark)).unwrap();
        assert_eq!(a, b);
    }

    /// Every row covers the grid exactly — a hole or an overrun means the
    /// wide-glyph accounting broke.
    #[test]
    fn chat_frame_rows_cover_the_grid() {
        let frame = chat_frame(Theme::Dark);
        assert_eq!(frame.rows.len(), SHOT_H as usize);
        for (y, row) in frame.rows.iter().enumerate() {
            let total: u16 = row.iter().map(|c| c.w as u16).sum();
            assert_eq!(total, SHOT_W, "row {y} does not cover the grid");
        }
    }

    /// The load-bearing showcase content is actually *inside* the frame — a
    /// fixture edit that scrolls the subject out of view must fail here, not
    /// ship a screenshot of nothing.
    #[test]
    fn chat_frame_shows_the_showcase() {
        let frame = chat_frame(Theme::Dark);
        let text = frame_text(&frame);
        // Needles are single words or strings that render on one line: the
        // frame text is joined row-by-row, so a phrase that word-wraps would
        // fail here even while perfectly visible (the joined-rows trap,
        // docs/lessons.md §2).
        for needle in [
            demo::CHAT_TITLE,     // the feed header
            "note_save",          // the tool card
            "Q5_K_M",             // the recommendation (table and/or flowchart)
            "sizing",             // the expanded thoughts block
            "sweet spot",         // the verdict table (one cell, no wrap)
            "Need the full 16k?", // the flowchart's decision node
        ] {
            assert!(
                text.contains(needle),
                "{needle:?} is not on screen:\n{text}"
            );
        }
    }

    /// Regenerator for the committed dumps — run deliberately:
    /// `cargo test dump_demo_frames -- --ignored`
    /// then `python tools/screenshots.py` to re-render the images.
    #[test]
    #[ignore = "writes artwork/screenshots/dumps/"]
    fn dump_demo_frames() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("artwork/screenshots/dumps");
        fs::create_dir_all(&root).unwrap();
        // Stage 2 turns this into a loop over the agreed theme matrix
        // (docs/demo-screenshots.md §5, fork 3).
        let frame = chat_frame(Theme::Dark);
        let name = format!("{}-{}-{}.json", frame.screen, frame.theme, frame.locale);
        let json = serde_json::to_string_pretty(&frame).unwrap();
        fs::write(root.join(&name), json).unwrap();
        eprintln!("wrote {name}");
    }
}
