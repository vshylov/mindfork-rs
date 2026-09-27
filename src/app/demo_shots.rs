//! Capture recipes for the generated screenshots (docs/history/demo-screenshots.md).
//!
//! Each recipe builds a screen from the demo fixture (`features/demo`), renders
//! it into a `TestBackend` and serializes the frame (`shared/shot`). The
//! `#[ignore]` test at the bottom is the regenerator: it (re)writes the
//! committed dumps under `assets/screenshots/dumps/`, which
//! `tools/screenshots.py` then turns into images. Ordinary tests keep the
//! recipes honest — deterministic, grid-covering, with the showcase content
//! actually inside the frame — and the drift gate keeps the committed dumps
//! equal to what the current code renders.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::entities::profile::CharacterNames;
use crate::features::demo;
use crate::screens::chat::ChatScreen;
use crate::screens::chat_list::ChatListScreen;
use crate::screens::self_model::SelfModelScreen;
use crate::screens::settings::SettingsScreen;
use crate::shared::config::{InterfaceSettings, NoteOrder, Theme, ThemeMode};
use crate::shared::i18n::{Lang, locale};
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::shot::{self, ShotFrame};
use crate::shared::theme::Palette;
use crate::shared::ui::finish_frame;

/// One width for the whole set — the gallery reads as one terminal. The hero
/// stands alone at its own height (room for the final exchange); the four
/// gallery panels share one height, so the site's 2×2 `shot-grid` lines up
/// instead of presenting four ragged windows.
pub const SHOT_W: u16 = 116;
pub const HERO_H: u16 = 44;
/// One height for every gallery panel. 33 is not arbitrary: the settings
/// Tools section — the richest capture — fills its parameter area exactly at
/// this height (30 until the three background-run rows joined the agentic
/// group, spec §9.3.2), and the fixture stocks the other screens (21 chats
/// and a sub-agent transcript, the grown self-model) so none of them drags
/// half a frame of empty rows.
pub const PANEL_H: u16 = 33;

/// What a capture is drawn with: a theme of the **system** colour mode, where
/// the terminal supplies the background, one of the **full** mode, where the
/// app paints it, or the **monochrome** mode, which has no theme (spec §11.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    System(Theme),
    Full(&'static str),
    /// Held to its contract by a test, never dumped — like the full mode.
    Mono,
}

impl Look {
    /// Puts the look into interface settings — the same two fields a user's
    /// choice lands in.
    fn dress(self, interface: &mut InterfaceSettings) {
        match self {
            Look::System(theme) => interface.theme = theme,
            Look::Full(name) => {
                interface.set_mode(ThemeMode::Full);
                interface.full_theme = name.to_string();
            }
            Look::Mono => interface.set_mode(ThemeMode::Mono),
        }
    }

    fn palette(self) -> Palette {
        let mut interface = InterfaceSettings::default();
        self.dress(&mut interface);
        Palette::for_interface(&interface)
    }

    fn slug(self) -> String {
        match self {
            Look::System(Theme::Dark) => "dark".into(),
            Look::System(Theme::Light) => "light".into(),
            // Never captured (design plan §5): Auto's colors belong to a
            // terminal that isn't there.
            Look::System(Theme::Auto) => unreachable!("Auto theme is not capturable"),
            Look::Full(name) => format!("full-{name}"),
            Look::Mono => "mono".into(),
        }
    }
}

/// The captured matrix (per the user's fork decisions, design plan §5):
/// Dark + Light, English. The system mode on purpose: a dump then carries a
/// background only where a widget set one, and the full mode's frames are the
/// same picture with the canvas written into every cell — which a test below
/// holds them to, instead of five more dumps each.
pub const THEMES: [Look; 2] = [Look::System(Theme::Dark), Look::System(Theme::Light)];

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
    look: Look,
    height: u16,
    draw: impl FnOnce(&mut ratatui::Frame),
) -> ShotFrame {
    let palette = look.palette();
    let mut term = Terminal::new(TestBackend::new(SHOT_W, height)).unwrap();
    // The screen, then the passes over the finished frame — what
    // `app/runtime` does to every frame (a no-op in the system mode).
    term.draw(|frame| {
        draw(frame);
        finish_frame(frame.buffer_mut(), &palette);
    })
    .unwrap();
    shot::capture(
        term.backend().buffer(),
        &palette,
        screen_id,
        &look.slug(),
        "en",
    )
}

/// The hero shot: the chat screen over the showcase conversation, thoughts and
/// tool details expanded, English UI.
pub fn chat_frame(look: Look) -> ShotFrame {
    let mut screen = ChatScreen::new();
    let mut config = crate::shared::config::AppConfig::default();
    look.dress(&mut config.interface);
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
    capture("chat", look, HERO_H, |f| screen.render(f))
}

/// The full-screen chat list: the showcase chat active on top, a spread of
/// topics below it.
pub fn list_frame(look: Look) -> ShotFrame {
    let mut screen = ChatListScreen::new(
        demo::chat_summaries(),
        Some(demo::chat_id()),
        look.palette(),
        locale(Lang::En),
    );
    capture("chat-list", look, PANEL_H, |f| screen.render(f))
}

/// The settings screen. `tools == false` captures the opening "Model/server"
/// section; `tools == true` walks to the Tools section — the toggle set is
/// the single best showcase of what the app can do (the user's addition to
/// the capture set, design plan §5 fork 2).
pub fn settings_frame(look: Look, tools: bool) -> ShotFrame {
    let mut config = demo::app_config();
    look.dress(&mut config.interface);
    let mut screen = SettingsScreen::new(config, vec![demo::profile()], Vec::new());
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
    capture(id, look, PANEL_H, |f| screen.render(f))
}

/// The self-model screen (`F3`): summary, goals, the user model and the
/// narrative — the flagship feature, shown mid-life rather than empty.
pub fn self_model_frame(look: Look) -> ShotFrame {
    let mut screen = SelfModelScreen::new(
        Some(demo::self_model()),
        // The demo profile names neither side, so the two halves are headed by
        // the localized "Assistant"/"User" labels.
        CharacterNames::default(),
        look.palette(),
        locale(Lang::En),
        // The capture set shows the shipped default: newest observation first.
        NoteOrder::default(),
    );
    capture("self-model", look, PANEL_H, |f| screen.render(f))
}

/// The full capture set for one look, in gallery order.
pub fn all_frames(look: Look) -> Vec<ShotFrame> {
    vec![
        chat_frame(look),
        list_frame(look),
        settings_frame(look, false),
        settings_frame(look, true),
        self_model_frame(look),
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
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/screenshots/dumps")
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
        for frame in all_frames(Look::System(Theme::Dark)) {
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
        // "receipts beat repetition" the last self-model observation that still
        // fits — the section headers and the blank rows between fields cost the
        // frame a row each, so it pins that the observations reach into view at
        // all.
        #[rustfmt::skip]
        let needles = |screen: &str| -> &'static [&'static str] {
            match screen {
                "chat" => &["note_save", "Q5_K_M", "sizing", "sweet spot", "How much context?", "KV headroom", "1B draft"],
                "chat-list" => &["Speculative", "Dolomites", "Mermaid", "First week with mindfork"],
                "settings-model" => &["llama-server.exe", "16384", "draft-simple", "gemma-4-1B"],
                "settings-tools" => &["Agentic loop", "Wasmer sandbox", "Web search"],
                "self-model" => &["Assistant", "reproducible", "User", "methodical", "receipts beat repetition"],
                other => panic!("no needles for {other}"),
            }
        };
        for frame in all_frames(Look::System(Theme::Dark)) {
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

    /// **The full colour mode is the screenshots, with the canvas painted in**
    /// (spec §11.6, docs/theme-modes.md §4.4). Every screen of the capture set,
    /// drawn in the full mode, must be the system mode's frame cell for cell —
    /// the same text, the same attributes, the same colour wherever a widget
    /// chose one — and must carry the canvas or the text colour wherever the
    /// system mode left the cell to the terminal. Two properties at once: the
    /// two modes share their palettes (fork B), and nothing on a real screen
    /// escapes the pass.
    #[test]
    fn the_full_mode_is_the_system_frame_on_its_canvas() {
        for (system, full) in [
            (Look::System(Theme::Dark), Look::Full("dark")),
            (Look::System(Theme::Light), Look::Full("light")),
        ] {
            for (on_terminal, painted) in all_frames(system).iter().zip(&all_frames(full)) {
                let at = format!("{} / {}", painted.screen, painted.theme);
                assert_eq!(painted.screen, on_terminal.screen);
                assert_eq!(
                    (painted.width, painted.height),
                    (on_terminal.width, on_terminal.height),
                    "{at}"
                );
                // The canvas the capture stood the system frame on is the one
                // the full mode paints.
                assert_eq!(painted.canvas_bg, on_terminal.canvas_bg, "{at}");
                assert_eq!(painted.canvas_fg, on_terminal.canvas_fg, "{at}");

                let mut filled = 0usize;
                for (y, (before, after)) in on_terminal.rows.iter().zip(&painted.rows).enumerate() {
                    assert_eq!(before.len(), after.len(), "{at} row {y}");
                    for (x, (b, a)) in before.iter().zip(after).enumerate() {
                        let cell = format!("{at} row {y} cell {x} {:?}", a.s);
                        assert_eq!((&a.s, a.w, &a.m), (&b.s, b.w, &b.m), "{cell}");
                        let want_fg = b.fg.clone().unwrap_or(on_terminal.canvas_fg.clone());
                        let want_bg = b.bg.clone().unwrap_or(on_terminal.canvas_bg.clone());
                        assert_eq!(a.fg.as_deref(), Some(want_fg.as_str()), "{cell}");
                        assert_eq!(a.bg.as_deref(), Some(want_bg.as_str()), "{cell}");
                        filled += usize::from(b.bg.is_none());
                    }
                }
                // Not vacuous: most of a frame is cells the terminal used to
                // colour (the dumps measure 87–99 %).
                let cells: usize = painted.rows.iter().map(Vec::len).sum();
                assert!(
                    filled * 10 > cells * 8,
                    "{at}: only {filled} of {cells} cells were the terminal's to colour"
                );
            }
        }
    }

    /// The monochrome mode on every captured screen: not a colour and not an
    /// attribute in any cell. None of these screens holds a text selection or
    /// a search match, so not even reverse video — the selected rows, the
    /// open tab and the keycaps are all said in text. The system frame of
    /// the same screen is the control: there most of those cells are styled.
    #[test]
    fn the_monochrome_mode_leaves_no_styling_on_any_screen() {
        let styled = |frame: &ShotFrame| -> usize {
            frame
                .rows
                .iter()
                .flatten()
                .filter(|c| c.fg.is_some() || c.bg.is_some() || !c.m.is_empty())
                .count()
        };
        let system = all_frames(Look::System(Theme::Dark));
        for (mono, system) in all_frames(Look::Mono).iter().zip(&system) {
            let at = &mono.screen;
            assert_eq!(&mono.screen, &system.screen);
            for (y, row) in mono.rows.iter().enumerate() {
                for cell in row {
                    assert_eq!(
                        (cell.fg.as_deref(), cell.bg.as_deref(), cell.m.as_str()),
                        (None, None, ""),
                        "{at} row {y} {:?}",
                        cell.s
                    );
                }
            }
            let cells: usize = system.rows.iter().map(Vec::len).sum();
            assert!(
                styled(system) * 4 > cells,
                "{at}: the control — only {} of {cells} cells are styled",
                styled(system)
            );

            // What styling said is in the text: the keycaps are bracketed…
            let text = frame_text(mono);
            assert!(text.contains("[F1]"), "{at}:\n{text}");
            assert!(!frame_text(system).contains("[F1]"), "{at}");
        }

        let by_screen = |screen: &str| -> String {
            let frames = all_frames(Look::Mono);
            frame_text(frames.iter().find(|f| f.screen == screen).unwrap())
        };
        // …the feed says whose rows these are by the rail's shape, and the
        // emphasis by its markers…
        let chat = by_screen("chat");
        assert!(chat.contains("\n│║ ") || chat.contains("│║ "), "{chat}");
        assert!(chat.contains("**"), "{chat}");
        // …and the list, which chat is the open one.
        let list = by_screen("chat-list");
        assert_eq!(list.matches('●').count(), 1, "{list}");
        assert!(list.matches('○').count() >= 1, "{list}");
    }

    /// Regenerator for the committed dumps — run deliberately:
    /// `cargo test dump_demo_frames -- --ignored`
    /// then `python tools/screenshots.py` to re-render the images.
    #[test]
    #[ignore = "writes assets/screenshots/dumps/"]
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
