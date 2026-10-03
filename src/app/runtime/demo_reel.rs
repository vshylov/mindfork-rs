//! The animated demo — a reel generated from code (docs/research/demo-reel.md).
//!
//! A [`Director`] holds what [`run_loop`] holds — the chat screen, the screen
//! in front, the back-stack, the help overlay, a command channel — and plays
//! the orchestrator's part from a script. Keys go through
//! [`process_input_batch`] exactly as typed keys do; what the interface then
//! asks for is read off the command channel and asserted; the answer is the
//! events the orchestrator emits for such a turn, through [`apply_event`]. Every
//! beat draws a frame through [`present`] onto one persistent `TestBackend`,
//! and the frame is kept with a duration on a virtual clock — nothing here
//! reads the wall clock, so two runs give the same reel.
//!
//! The `#[ignore]` test at the bottom is the regenerator: it writes
//! `target/reel/*.json`, which `tools/demo_reel.py` turns into a GIF and an
//! animated WebP. Nothing it writes is committed (design doc §3.3).

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde::Serialize;
use tokio::sync::mpsc::unbounded_channel;

use crate::app::demo_shots::{HERO_H, SHOT_W};
use crate::entities::message::Message;
use crate::entities::profile::CharacterNames;
use crate::features::demo;
use crate::shared::api::contract::FinishReason;
use crate::shared::config::Theme;
use crate::shared::i18n::Lang;
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::shot::{self, ShotFrame};

use super::*;

/// The turn the reel streams. Fixed, like every id the fixture holds.
fn generation_id() -> Uuid {
    Uuid::from_u128(0x6d66_5f72_6565_6c5f_7475_726e_0000_0001)
}

/// What the model has already read when the turn starts — the system prompt,
/// the first exchange and the question. Plausible, and only ever drawn as the
/// status bar's context figure.
const PROMPT_TOKENS: u64 = 1_874;

/// How fast the reel types and streams. A GIF's delay is counted in hundredths
/// of a second, and browsers stretch one under 20 ms to 100 ms, so nothing here
/// goes below 40 ms (design doc §3.4).
const TYPE_MS: u32 = 40;
const THOUGHT_MS: u32 = 110;
const ANSWER_MS: u32 = 90;
/// Words per streamed piece — the demo engine's own grain (`features/demo`).
const WORDS_PER_PIECE: usize = 3;
/// A `TokenUsage` every this many answer pieces, the way a server reports
/// usage while it streams rather than once at the end.
const USAGE_EVERY: usize = 6;

/// One frame of the reel: how long it stays up, where the terminal's cursor
/// is (when the frame shows one), which beat of the scenario it belongs to,
/// and the cells — `shot`'s format, so the renderer shares the stills' code.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReelFrame {
    pub ms: u32,
    pub cursor: Option<[u16; 2]>,
    pub beat: &'static str,
    #[serde(flatten)]
    pub frame: ShotFrame,
}

/// A whole reel: its name (the file it is written to) and its frames.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reel {
    pub name: String,
    pub frames: Vec<ReelFrame>,
}

impl Reel {
    pub fn total_ms(&self) -> u32 {
        self.frames.iter().map(|f| f.ms).sum()
    }
}

/// The loop's state plus the terminal the frames are drawn on, and the frames
/// drawn so far.
struct Director {
    screen: ChatScreen,
    active: ActiveScreen,
    back: Option<Back>,
    help: HelpOverlay,
    clipboard: Option<arboard::Clipboard>,
    cmd_tx: UnboundedSender<AppCommand>,
    cmd_rx: UnboundedReceiver<AppCommand>,
    term: Terminal<TestBackend>,
    theme: &'static str,
    locale: &'static str,
    beat: &'static str,
    frames: Vec<ReelFrame>,
}

impl Director {
    /// The showcase chat holding `history`, both servers ready, an empty input
    /// box — the settings the stills are taken with.
    fn new(theme: Theme, lang: Lang, history: &[Message]) -> Self {
        let mut config = demo::app_config();
        config.interface.theme = theme;
        config.interface.language = lang;
        let mut screen = ChatScreen::new();
        screen.set_settings(
            config,
            Vec::new(),
            Vec::new(),
            Default::default(),
            Vec::new(),
        );
        screen.set_server_status(ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::NotConfigured,
        });
        screen.activate_chat(
            demo::chat_id(),
            demo::CHAT_TITLE.into(),
            history,
            "",
            demo::feed_view(),
            None,
            None,
        );
        let (cmd_tx, cmd_rx) = unbounded_channel();
        Self {
            screen,
            active: ActiveScreen::Chat,
            back: None,
            help: HelpOverlay::new(),
            clipboard: None,
            cmd_tx,
            cmd_rx,
            term: Terminal::new(TestBackend::new(SHOT_W, HERO_H)).unwrap(),
            theme: match theme {
                Theme::Dark => "dark",
                Theme::Light => "light",
                // Auto's colours belong to a terminal that isn't there — the
                // stills refuse it for the same reason (demo_shots::Look).
                Theme::Auto => unreachable!("the Auto theme is not capturable"),
            },
            locale: lang.code(),
            beat: "",
            frames: Vec::new(),
        }
    }

    /// The beat the next frames belong to.
    fn beat(&mut self, beat: &'static str) {
        self.beat = beat;
    }

    /// Draws the frame the loop would draw now and keeps it for `ms`. A frame
    /// equal to the previous one only lengthens it — a key that changed
    /// nothing on screen is not a frame of its own.
    fn frame(&mut self, ms: u32) {
        let palette = front_palette(&self.active, &self.screen);
        let placeholder = present(
            &mut self.term,
            &mut self.screen,
            &mut self.active,
            &mut self.help,
            &palette,
        )
        .unwrap();
        assert!(
            placeholder.is_none(),
            "the reel's window is smaller than the frame in front needs"
        );
        let backend = self.term.backend();
        let cursor = backend.cursor_visible().then(|| {
            let at = backend.cursor_position();
            [at.x, at.y]
        });
        let frame = shot::capture(backend.buffer(), &palette, "reel", self.theme, self.locale);
        if let Some(last) = self.frames.last_mut()
            && last.cursor == cursor
            && last.frame == frame
        {
            last.ms += ms;
            return;
        }
        self.frames.push(ReelFrame {
            ms,
            cursor,
            beat: self.beat,
            frame,
        });
    }

    /// Holds the screen as it is.
    fn hold(&mut self, ms: u32) {
        self.frame(ms);
    }

    /// A key, the way the loop delivers one: a batch of one event.
    fn key(&mut self, key: KeyEvent, ms: u32) {
        let quit = process_input_batch(
            vec![Event::Key(key)],
            &mut self.screen,
            &mut self.active,
            &mut self.help,
            &mut self.back,
            &self.cmd_tx,
            &mut self.clipboard,
        );
        assert!(!quit, "a reel key asked to quit: {key:?}");
        self.frame(ms);
    }

    /// Types `text` into whatever has the focus, a character a frame.
    fn type_text(&mut self, text: &str, ms: u32) {
        for c in text.chars() {
            self.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), ms);
        }
    }

    /// An orchestrator event, applied without a frame of its own.
    fn apply(&mut self, event: AppEvent) {
        apply_event(
            &mut self.screen,
            &mut self.active,
            &mut self.back,
            &mut self.clipboard,
            &self.cmd_tx,
            event,
        );
    }

    /// An orchestrator event, then the frame it leaves.
    fn event(&mut self, event: AppEvent, ms: u32) {
        self.apply(event);
        self.frame(ms);
    }

    /// Everything the interface has asked of the orchestrator since the last
    /// call — the half of the conversation the script has to answer.
    fn asked(&mut self) -> Vec<AppCommand> {
        let mut asked = Vec::new();
        while let Ok(command) = self.cmd_rx.try_recv() {
            asked.push(command);
        }
        asked
    }

    fn into_reel(self, name: String) -> Reel {
        Reel {
            name,
            frames: self.frames,
        }
    }
}

fn plain(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// `text` cut into pieces of `words` words, each keeping its trailing space —
/// joined back they are `text` exactly, newlines and all.
fn pieces(text: &str, words: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut piece = String::new();
    let mut count = 0;
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            piece.push(' ');
        }
        if count == words {
            out.push(std::mem::take(&mut piece));
            count = 0;
        }
        piece.push_str(word);
        count += 1;
    }
    if !piece.is_empty() {
        out.push(piece);
    }
    out
}

/// A rough token count for the status bar's figures — four characters a
/// token, the same estimate the demo engine reports (`features/demo`).
fn tokens(text: &str) -> u64 {
    (text.len() / 4) as u64
}

/// The reel: the showcase chat's last exchange, played live (design doc §3.2).
pub fn reel(theme: Theme, lang: Lang) -> Reel {
    let messages = demo::showcase_messages();
    let [_, _, question, answer] = &messages[..] else {
        panic!("the showcase conversation is two exchanges");
    };
    let thoughts = answer
        .thoughts
        .clone()
        .expect("the showcase answer has thoughts");
    let [call] = &answer.tool_calls[..] else {
        panic!("the showcase answer makes one tool call");
    };
    let id = generation_id();
    let mut d = Director::new(theme, lang, &messages[..2]);

    d.beat("open");
    d.hold(1_500);

    d.beat("type");
    d.type_text(&question.text, TYPE_MS);
    d.hold(400);

    // `Enter` sends what was typed — asserted, so an interface that stopped
    // sending would fail here instead of animating a reply to nothing.
    d.beat("send");
    d.key(plain(KeyCode::Enter), 150);
    let asked = d.asked();
    assert!(
        asked
            .iter()
            .any(|c| matches!(c, AppCommand::SendMessage(text) if *text == question.text)),
        "Enter did not send the typed question: {asked:?}"
    );
    d.event(AppEvent::UserMessage(question.text.clone()), 250);
    d.event(
        AppEvent::GenerationStarted {
            generation_id: id,
            model: None,
            continuation: false,
        },
        400,
    );

    d.beat("think");
    for piece in pieces(&thoughts, WORDS_PER_PIECE) {
        d.event(
            AppEvent::Thoughts {
                generation_id: id,
                text: piece,
            },
            THOUGHT_MS,
        );
    }
    d.hold(300);

    d.beat("tool");
    let arguments = call.arguments.to_string();
    d.event(
        AppEvent::ToolCallStarted {
            generation_id: id,
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: arguments.clone(),
        },
        700,
    );
    d.event(
        AppEvent::ToolCall {
            generation_id: id,
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments,
            result: call.result.clone().expect("the showcase call has a result"),
            images: 0,
        },
        700,
    );

    d.beat("answer");
    let reasoning = tokens(&thoughts);
    let mut streamed = String::new();
    for (i, piece) in pieces(&answer.text, WORDS_PER_PIECE)
        .into_iter()
        .enumerate()
    {
        streamed.push_str(&piece);
        if i % USAGE_EVERY == 0 {
            let completion = reasoning + tokens(&streamed);
            d.apply(AppEvent::TokenUsage {
                generation_id: id,
                completion,
                context: Some(PROMPT_TOKENS + completion),
                context_exact: false,
                reasoning: Some(reasoning as u32),
            });
        }
        d.event(
            AppEvent::Chunk {
                generation_id: id,
                text: piece,
            },
            ANSWER_MS,
        );
    }
    let completion = reasoning + tokens(&answer.text);
    d.apply(AppEvent::TokenUsage {
        generation_id: id,
        completion,
        context: Some(PROMPT_TOKENS + completion),
        context_exact: true,
        reasoning: Some(reasoning as u32),
    });
    d.beat("rest");
    d.event(
        AppEvent::Finished {
            generation_id: id,
            reason: FinishReason::Stop,
            continuable: false,
        },
        2_500,
    );

    // `Ctrl+T` folds the thoughts on the screen at once and tells the
    // orchestrator, which keeps the fold with the chat.
    d.beat("fold");
    d.key(ctrl('t'), 1_500);
    let asked = d.asked();
    assert!(
        asked
            .iter()
            .any(|c| matches!(c, AppCommand::SetFeedView(view) if !view.thoughts)),
        "Ctrl+T did not fold the thoughts: {asked:?}"
    );

    // `F3` asks for the self-model; the screen opens on the reply.
    d.beat("self");
    d.key(plain(KeyCode::F(3)), 100);
    let asked = d.asked();
    assert!(
        asked
            .iter()
            .any(|c| matches!(c, AppCommand::RequestSelfModel)),
        "F3 did not ask for the self-model: {asked:?}"
    );
    d.event(
        AppEvent::SelfModelView {
            model: Box::new(Some(demo::self_model())),
            names: CharacterNames::default(),
        },
        4_500,
    );

    d.beat("back");
    d.key(plain(KeyCode::Esc), 1_500);
    assert!(d.active.is_chat(), "Esc did not lead back to the chat");

    let name = format!("reel-{}-{}", d.theme, d.locale);
    d.into_reel(name)
}

mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn text(frame: &ReelFrame) -> String {
        frame
            .frame
            .rows
            .iter()
            .map(|row| row.iter().map(|c| c.s.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn dark() -> Reel {
        reel(Theme::Dark, Lang::En)
    }

    /// The last frame of a beat — what the beat leaves on screen.
    fn end_of<'a>(reel: &'a Reel, beat: &str) -> &'a ReelFrame {
        reel.frames
            .iter()
            .rev()
            .find(|f| f.beat == beat)
            .unwrap_or_else(|| panic!("no frame in the beat {beat:?}"))
    }

    /// Byte-stable across runs — nothing in a frame reads the wall clock.
    #[test]
    fn the_reel_is_deterministic() {
        let a = serde_json::to_string(&dark()).unwrap();
        let b = serde_json::to_string(&dark()).unwrap();
        assert!(a == b, "two runs gave two reels");
    }

    /// Every row of every frame covers the grid — a hole or an overrun means
    /// the wide-glyph accounting broke somewhere mid-stream.
    #[test]
    fn every_frame_covers_the_grid() {
        for (i, frame) in dark().frames.iter().enumerate() {
            let f = &frame.frame;
            assert_eq!((f.width, f.height), (SHOT_W, HERO_H), "frame {i}");
            assert_eq!(f.rows.len(), f.height as usize, "frame {i}");
            for (y, row) in f.rows.iter().enumerate() {
                let total: u16 = row.iter().map(|c| c.w as u16).sum();
                assert_eq!(total, f.width, "frame {i} row {y}");
            }
        }
    }

    /// What each beat is there to show is on screen when the beat ends.
    /// Needles are strings that render on one line (lessons §2).
    #[test]
    fn each_beat_shows_what_it_is_for() {
        let reel = dark();
        #[rustfmt::skip]
        let needles: &[(&str, &[&str])] = &[
            ("open", &["Which Gemma 4 12B quant", "Q5_K_M", "sweet spot"]),
            ("type", &["save a note about my setup."]),
            ("think", &["The note should record"]),
            ("tool", &["note_save", "Hardware budget"]),
            ("answer", &["How much context?", "KV headroom", "double the cache"]),
            ("self", &["I run locally", "receipts beat repetition"]),
        ];
        for (beat, needles) in needles {
            let screen = text(end_of(&reel, beat));
            for needle in *needles {
                assert!(
                    screen.contains(needle),
                    "{beat}: {needle:?} is not on screen:\n{screen}"
                );
            }
        }
        // The fold is visible: the thoughts' text is gone from the frame.
        let before = text(end_of(&reel, "rest"));
        let after = text(end_of(&reel, "fold"));
        assert!(before.contains("The note should record"), "{before}");
        assert!(!after.contains("The note should record"), "{after}");
    }

    /// The typing has a cursor to follow, and the self-model screen, which
    /// has no input, shows none.
    #[test]
    fn the_cursor_is_where_typing_happens() {
        let reel = dark();
        assert!(end_of(&reel, "type").cursor.is_some());
        assert!(end_of(&reel, "self").cursor.is_none());
    }

    /// Long enough to tell the story, short enough to be watched; no frame
    /// shorter than a browser plays a GIF delay as written.
    #[test]
    fn the_reel_has_a_watchable_length() {
        let reel = dark();
        let total = reel.total_ms();
        assert!(
            (15_000..=30_000).contains(&total),
            "the reel runs {total} ms"
        );
        for (i, frame) in reel.frames.iter().enumerate() {
            assert!(frame.ms >= 40, "frame {i} is up for {} ms", frame.ms);
        }
    }

    #[test]
    fn pieces_join_back_into_the_text() {
        let text = "a b c d\n\n| x | y |\ne f";
        let cut = pieces(text, 3);
        assert_eq!(cut.concat(), text);
        assert_eq!(cut[0], "a b c ");
        assert!(cut.len() > 1);
    }

    fn reel_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target/reel")
    }

    /// Regenerator — run deliberately:
    /// `cargo test dump_demo_reel -- --ignored`, then
    /// `python tools/demo_reel.py` to render the GIF and the WebP.
    #[test]
    #[ignore = "writes target/reel/"]
    fn dump_demo_reel() {
        let root = reel_dir();
        fs::create_dir_all(&root).unwrap();
        let reel = dark();
        let path = root.join(format!("{}.json", reel.name));
        fs::write(&path, serde_json::to_string(&reel).unwrap()).unwrap();
        eprintln!(
            "wrote {} — {} frames, {} ms",
            path.display(),
            reel.frames.len(),
            reel.total_ms()
        );
    }
}
