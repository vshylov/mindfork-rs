//! Capture recipes for the generated screenshots (docs/history/demo-screenshots.md).
//!
//! Each recipe builds a screen from the demo fixture (`features/demo`), renders
//! it into a `TestBackend` and serializes the frame (`shared/shot`). The
//! `#[ignore]` test at the bottom is the regenerator: it (re)writes the
//! committed dumps under `artwork/screenshots/dumps/`, which
//! `tools/screenshots.py` then turns into images. Ordinary tests keep the
//! recipes honest — deterministic, grid-covering, with the showcase content
//! actually inside the frame — and the drift gate keeps the committed dumps
//! equal to what the current code renders.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::features::demo;
use crate::screens::chat::ChatScreen;
use crate::screens::chat_list::ChatListScreen;
use crate::screens::self_model::SelfModelScreen;
use crate::screens::settings::SettingsScreen;
use crate::shared::config::Theme;
use crate::shared::i18n::{Lang, locale};
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::shot::{self, ShotFrame};
use crate::shared::theme::Palette;

/// One width for the whole set — the gallery reads as one terminal. The hero
/// stands alone at its own height (room for the final exchange); the four
/// gallery panels share one height, so the site's 2×2 `shot-grid` lines up
/// instead of presenting four ragged windows.
pub const SHOT_W: u16 = 116;
pub const HERO_H: u16 = 44;
/// One height for every gallery panel. 30 is not arbitrary: the settings
/// Tools section — the richest capture — fills its parameter area exactly at
/// this height, and the fixture stocks the other screens (22 chats, the
/// grown self-model) so none of them drags half a frame of empty rows.
pub const PANEL_H: u16 = 30;

/// The captured matrix (per the user's fork decisions, design plan §5):
/// Dark + Light, English.
pub const THEMES: [Theme; 2] = [Theme::Dark, Theme::Light];

fn theme_slug(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
        // Never captured (design plan §5): Auto's colors belong to a terminal
        // that isn't there.
        Theme::Auto => unreachable!("Auto theme is not capturable"),
    }
}

/// The demo depicts a healthy stack: chat and embeddings ready (the memory
/// features are part of the showcase), impersonation not configured (its
/// chip stays hidden).
fn ready_statuses() -> ServerStatuses {
    ServerStatuses {
        chat: ServerStatus::Ready,
        embed: ServerStatus::Ready,
        impersonation: ServerStatus::NotConfigured,
    }
}

fn capture(
    screen_id: &str,
    theme: Theme,
    height: u16,
    draw: impl FnOnce(&mut ratatui::Frame),
) -> ShotFrame {
    let mut term = Terminal::new(TestBackend::new(SHOT_W, height)).unwrap();
    term.draw(draw).unwrap();
    shot::capture(
        term.backend().buffer(),
        &Palette::for_theme(theme),
        screen_id,
        theme_slug(theme),
        "en",
    )
}

/// The hero shot: the chat screen over the showcase conversation, thoughts and
/// tool details expanded, English UI.
pub fn chat_frame(theme: Theme) -> ShotFrame {
    let mut screen = ChatScreen::new();
    let mut config = crate::shared::config::AppConfig::default();
    config.interface.theme = theme;
    config.interface.language = Lang::En;
    screen.set_settings(
        config,
        Vec::new(),
        Vec::new(),
        Default::default(),
        Vec::new(),
    );
    screen.set_server_status(ready_statuses());
    screen.activate_chat(
        demo::chat_id(),
        demo::CHAT_TITLE.into(),
        &demo::showcase_messages(),
        // The typed-but-unsent follow-up: the input box is part of the
        // showcase, and an empty prompt reads as a screensaver.
        demo::INPUT_DRAFT,
        demo::feed_view(),
        None,
        None,
    );
    capture("chat", theme, HERO_H, |f| screen.render(f))
}

/// The full-screen chat list: the showcase chat active on top, a spread of
/// topics below it.
pub fn list_frame(theme: Theme) -> ShotFrame {
    let mut screen = ChatListScreen::new(
        demo::chat_summaries(),
        Some(demo::chat_id()),
        Palette::for_theme(theme),
        locale(Lang::En),
    );
    capture("chat-list", theme, PANEL_H, |f| screen.render(f))
}

/// The settings screen. `tools == false` captures the opening "Model/server"
/// section; `tools == true` walks to the Tools section — the toggle set is
/// the single best showcase of what the app can do (the user's addition to
/// the capture set, design plan §5 fork 2).
pub fn settings_frame(theme: Theme, tools: bool) -> ShotFrame {
    let mut screen =
        SettingsScreen::new(demo::app_config(theme), vec![demo::profile()], Vec::new());
    screen.set_server_statuses(ready_statuses());
    if tools {
        // Section order is [Model, Sampling, Tools, ...] (settings::SECTIONS):
        // two Tabs from the initial Model section. The showcase test pins the
        // destination by content, so an order change cannot silently capture
        // the wrong section.
        for _ in 0..2 {
            screen.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        }
    }
    let id = if tools {
        "settings-tools"
    } else {
        "settings-model"
    };
    capture(id, theme, PANEL_H, |f| screen.render(f))
}

/// The self-model screen (`F3`): summary, goals, the user model and the
/// narrative — the flagship feature, shown mid-life rather than empty.
pub fn self_model_frame(theme: Theme) -> ShotFrame {
    let mut screen = SelfModelScreen::new(
        Some(demo::self_model()),
        Palette::for_theme(theme),
        locale(Lang::En),
    );
    capture("self-model", theme, PANEL_H, |f| screen.render(f))
}

/// The full capture set for one theme, in gallery order.
pub fn all_frames(theme: Theme) -> Vec<ShotFrame> {
    vec![
        chat_frame(theme),
        list_frame(theme),
        settings_frame(theme, false),
        settings_frame(theme, true),
        self_model_frame(theme),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn frame_text(frame: &ShotFrame) -> String {
        frame
            .rows
            .iter()
            .map(|row| row.iter().map(|c| c.s.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn dumps_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("artwork/screenshots/dumps")
    }

    fn dump_name(frame: &ShotFrame) -> String {
        format!("{}-{}-{}.json", frame.screen, frame.theme, frame.locale)
    }

    const REGEN_HINT: &str = "regenerate: `cargo test dump_demo_frames -- --ignored`, then `python tools/screenshots.py`, and commit both";

    /// Byte-stable across runs — the property the drift gate stands on.
    #[test]
    fn frames_are_deterministic() {
        for theme in THEMES {
            let a: Vec<String> = all_frames(theme)
                .iter()
                .map(|f| serde_json::to_string(f).unwrap())
                .collect();
            let b: Vec<String> = all_frames(theme)
                .iter()
                .map(|f| serde_json::to_string(f).unwrap())
                .collect();
            assert_eq!(a, b, "{theme:?} frames are not deterministic");
        }
    }

    /// Every row of every frame covers the grid exactly — a hole or an overrun
    /// means the wide-glyph accounting broke.
    #[test]
    fn frame_rows_cover_the_grid() {
        for frame in all_frames(Theme::Dark) {
            assert_eq!(frame.rows.len(), frame.height as usize, "{}", frame.screen);
            for (y, row) in frame.rows.iter().enumerate() {
                let total: u16 = row.iter().map(|c| c.w as u16).sum();
                assert_eq!(
                    total, frame.width,
                    "{} row {y} does not cover the grid",
                    frame.screen
                );
            }
        }
    }

    /// The load-bearing showcase content is actually *inside* each frame — a
    /// fixture edit that scrolls or navigates the subject out of view must
    /// fail here, not ship a screenshot of nothing. Needles are single words
    /// or strings that render on one line (the joined-rows trap, lessons §2).
    #[test]
    fn frames_show_their_showcase() {
        // One row per screen (the skip keeps it that way): notable needles —
        // "1B draft" pins the typed input draft (an edit that grows the feed
        // must not push the input's text out), "First week with mindfork" is
        // the list's last fixture row (the fill reaches the frame's bottom),
        // "receipts beat repetition" the newest self-model observation.
        #[rustfmt::skip]
        let needles = |screen: &str| -> &'static [&'static str] {
            match screen {
                "chat" => &["note_save", "Q5_K_M", "sizing", "sweet spot", "How much context?", "KV headroom", "1B draft"],
                "chat-list" => &["Speculative", "Dolomites", "Mermaid", "First week with mindfork"],
                "settings-model" => &["llama-server.exe", "16384", "draft-simple", "gemma-4-1B"],
                "settings-tools" => &["Agentic loop", "Wasmer sandbox", "Web search"],
                "self-model" => &["reproducible", "VRAM", "methodical", "receipts beat repetition"],
                other => panic!("no needles for {other}"),
            }
        };
        for frame in all_frames(Theme::Dark) {
            let text = frame_text(&frame);
            // The chat title heads both the feed and the list — checked once
            // here rather than repeated per row above.
            if matches!(frame.screen.as_str(), "chat" | "chat-list") {
                assert!(
                    text.contains(demo::CHAT_TITLE),
                    "{}: title missing",
                    frame.screen
                );
            }
            for needle in needles(&frame.screen) {
                assert!(
                    text.contains(needle),
                    "{}: {needle:?} is not on screen:\n{text}",
                    frame.screen
                );
            }
        }
    }

    /// The drift gate (design plan §5, fork 4): the committed dumps must equal
    /// what the current code renders. Red here means the UI, the fixture or
    /// the capture pipeline changed — the screenshots are stale until
    /// regenerated. A missing file is a failure, not a skip: a gate that goes
    /// quiet when its subject disappears reports confidence it no longer has.
    #[test]
    fn committed_dumps_match_the_code() {
        let root = dumps_dir();
        for theme in THEMES {
            for frame in all_frames(theme) {
                let name = dump_name(&frame);
                let committed = fs::read_to_string(root.join(&name)).unwrap_or_else(|e| {
                    panic!("{name}: cannot read the committed dump ({e}) — {REGEN_HINT}")
                });
                // `\r\n` may appear via git's eol translation on Windows
                // checkouts; the regenerator itself writes `\n`.
                let committed = committed.replace("\r\n", "\n");
                let fresh = serde_json::to_string_pretty(&frame).unwrap();
                if committed.trim_end() != fresh.trim_end() {
                    let line = committed
                        .lines()
                        .zip(fresh.lines())
                        .position(|(a, b)| a != b)
                        .map(|i| i + 1);
                    panic!(
                        "{name} drifted from the code (first differing line: {line:?}) — {REGEN_HINT}"
                    );
                }
            }
        }
    }

    /// Regenerator for the committed dumps — run deliberately:
    /// `cargo test dump_demo_frames -- --ignored`
    /// then `python tools/screenshots.py` to re-render the images.
    #[test]
    #[ignore = "writes artwork/screenshots/dumps/"]
    fn dump_demo_frames() {
        let root = dumps_dir();
        fs::create_dir_all(&root).unwrap();
        for theme in THEMES {
            for frame in all_frames(theme) {
                let name = dump_name(&frame);
                let json = serde_json::to_string_pretty(&frame).unwrap();
                fs::write(root.join(&name), json).unwrap();
                eprintln!("wrote {name}");
            }
        }
    }
}
