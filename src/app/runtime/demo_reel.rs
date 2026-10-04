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
use crate::entities::self_model::SelfModel;
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

/// How fast the reel streams. A GIF's delay is counted in hundredths of a
/// second, and browsers stretch one under 20 ms to 100 ms, so nothing here goes
/// below 40 ms (design doc §3.4). The typing has a rhythm of its own ([`Hand`]).
const THOUGHT_MS: u32 = 110;
const ANSWER_MS: u32 = 90;
/// Words per streamed piece — the demo engine's own grain (`features/demo`).
const WORDS_PER_PIECE: usize = 3;
/// A `TokenUsage` every this many answer pieces, the way a server reports
/// usage while it streams rather than once at the end.
const USAGE_EVERY: usize = 6;

/// How long the self-model stays on screen — the reel's point, so the longest
/// hold in it (the owner's review of the first reel, 2026-10-03).
const SELF_MS: u32 = 7_000;

/// A hand on a keyboard, for the question the reel types: an even stream of
/// one character every 40 ms read as a machine (the owner's review of the
/// first reel, 2026-10-03). A person types in bursts — a frame shows one to
/// [`Hand::BURST`] new characters — slows between words, sometimes stops
/// before the next one, and stops longer after punctuation. The irregularity
/// comes from a fixed-seed generator, so it is the same on every run.
struct Hand(u64);

impl Hand {
    const SEED: u64 = 0x6d66_5f68_616e_6401;
    /// The most characters one frame adds.
    const BURST: u32 = 3;
    /// A frame of typing stays up this long, plus up to [`Self::KEY_JITTER`].
    const KEY_MS: u32 = 40;
    const KEY_JITTER: u32 = 50;

    fn new() -> Self {
        Self(Self::SEED)
    }

    /// The next number below `n` — an LCG (Knuth's MMIX constants), its high
    /// bits taken, since an LCG's low bits cycle.
    fn below(&mut self, n: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % u64::from(n)) as u32
    }

    /// How many characters the next frame adds.
    fn burst(&mut self) -> usize {
        1 + self.below(Self::BURST) as usize
    }

    /// How long the frame stays up, given the last character it added.
    fn delay_after(&mut self, last: char) -> u32 {
        let key = Self::KEY_MS + self.below(Self::KEY_JITTER);
        let pause = match last {
            // Between words: a beat, and one time in four a pause for the
            // next word.
            ' ' if self.below(4) == 0 => 140 + self.below(120),
            ' ' => 20 + self.below(50),
            ',' | '.' | ';' | ':' | '?' | '!' | '—' => 200 + self.below(120),
            _ => 0,
        };
        key + pause
    }
}

/// Whether a burst ends after `c` — a person's bursts stop at a word's edge.
fn ends_burst(c: char) -> bool {
    c == ' ' || c.is_ascii_punctuation() || c == '—'
}

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
    fn new(theme: Theme, lang: Lang, title: &str, history: &[Message]) -> Self {
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
            title.into(),
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

    /// A key, the way the loop delivers one: a batch of one event. No frame —
    /// a burst of typing is several keys under one.
    fn press(&mut self, key: KeyEvent) {
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
    }

    /// A key, then the frame it leaves.
    fn key(&mut self, key: KeyEvent, ms: u32) {
        self.press(key);
        self.frame(ms);
    }

    /// Types `text` into whatever has the focus, the way a person does
    /// ([`Hand`]): every character a key of its own, a frame per burst.
    fn type_by_hand(&mut self, text: &str) {
        let mut hand = Hand::new();
        let mut chars = text.chars().peekable();
        while chars.peek().is_some() {
            let mut last = ' ';
            for _ in 0..hand.burst() {
                let Some(c) = chars.next() else { break };
                self.press(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                last = c;
                if ends_burst(c) {
                    // ", " is typed as one: the space after a mark goes with
                    // it, and the pause is the mark's.
                    if c != ' ' && chars.peek() == Some(&' ') {
                        let space = chars.next().unwrap();
                        self.press(KeyEvent::new(KeyCode::Char(space), KeyModifiers::NONE));
                    }
                    break;
                }
            }
            let ms = hand.delay_after(last);
            self.frame(ms);
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

/// The reels a release publishes: the README's dark one, the light one the
/// site shows a visitor whose system is light (design doc §3.6), and a dark
/// Russian one for an article in Russian (stage 3).
pub const LOOKS: [(Theme, Lang); 3] = [
    (Theme::Dark, Lang::En),
    (Theme::Light, Lang::En),
    (Theme::Dark, Lang::Ru),
];

/// What a reel tells, in its language: the chat's title, its conversation and
/// the self-model `F3` opens on. The interface speaks the same language — it
/// is the reel's `lang` — so a Russian reel is Russian through and through.
struct Showcase {
    title: &'static str,
    messages: Vec<Message>,
    self_model: SelfModel,
}

impl Showcase {
    fn of(lang: Lang) -> Self {
        match lang {
            Lang::Ru => Self {
                title: demo::ru::CHAT_TITLE,
                messages: demo::ru::showcase_messages(),
                self_model: demo::ru::self_model(),
            },
            Lang::En | Lang::Ext(_) => Self {
                title: demo::CHAT_TITLE,
                messages: demo::showcase_messages(),
                self_model: demo::self_model(),
            },
        }
    }
}

/// The reel: the showcase chat's last exchange, played live (design doc §3.2).
pub fn reel(theme: Theme, lang: Lang) -> Reel {
    let Showcase {
        title,
        messages,
        self_model,
    } = Showcase::of(lang);
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
    let mut d = Director::new(theme, lang, title, &messages[..2]);

    d.beat("open");
    d.hold(1_500);

    d.beat("type");
    d.type_by_hand(&question.text);
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
            model: Box::new(Some(self_model)),
            names: CharacterNames::default(),
        },
        SELF_MS,
    );

    d.beat("back");
    d.key(plain(KeyCode::Esc), 1_500);
    assert!(d.active.is_chat(), "Esc did not lead back to the chat");

    // The name is the published file's (docs/research/demo-reel.md §3.6).
    let name = format!("mindfork-demo-{}-{}", d.theme, d.locale);
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
        for (theme, lang) in LOOKS {
            let a = serde_json::to_string(&reel(theme, lang)).unwrap();
            let b = serde_json::to_string(&reel(theme, lang)).unwrap();
            assert!(a == b, "{theme:?}: two runs gave two reels");
        }
    }

    /// Every published look tells the same story — the same beats, in order —
    /// on its own canvas and in its own language, under the file name the
    /// release and the site look for.
    #[test]
    fn every_look_tells_the_same_story() {
        let reels: Vec<Reel> = LOOKS.iter().map(|&(t, l)| reel(t, l)).collect();
        let names: Vec<&str> = reels.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "mindfork-demo-dark-en",
                "mindfork-demo-light-en",
                "mindfork-demo-dark-ru"
            ]
        );
        let canvas = |r: &Reel| r.frames[0].frame.canvas_bg.clone();
        assert_ne!(canvas(&reels[0]), canvas(&reels[1]));
        assert_eq!(canvas(&reels[0]), canvas(&reels[2]));
        let beats = |r: &Reel| {
            let mut beats: Vec<&str> = r.frames.iter().map(|f| f.beat).collect();
            beats.dedup();
            beats
        };
        for r in &reels[1..] {
            assert_eq!(beats(r), beats(&reels[0]), "{}", r.name);
        }
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

    /// Each beat's needles are on screen when the beat ends, and the fold
    /// takes the thoughts (`thought`, a piece of them) off the screen.
    /// Needles are strings that render on one line (lessons §2).
    fn assert_beats(reel: &Reel, needles: &[(&str, &[&str])], thought: &str) {
        for (beat, needles) in needles {
            let screen = text(end_of(reel, beat));
            for needle in *needles {
                assert!(
                    screen.contains(needle),
                    "{}: {beat}: {needle:?} is not on screen:\n{screen}",
                    reel.name
                );
            }
        }
        let before = text(end_of(reel, "rest"));
        let after = text(end_of(reel, "fold"));
        assert!(before.contains(thought), "{before}");
        assert!(!after.contains(thought), "{after}");
    }

    /// What each beat is there to show is on screen when the beat ends.
    #[test]
    fn each_beat_shows_what_it_is_for() {
        #[rustfmt::skip]
        let needles: &[(&str, &[&str])] = &[
            ("open", &["Which Gemma 4 12B quant", "Q5_K_M", "sweet spot"]),
            ("type", &["save a note about my setup."]),
            ("think", &["The note should record"]),
            ("tool", &["note_save", "The user's GPU has 12 GB", "Note saved (id="]),
            ("answer", &["How much context?", "KV headroom", "double the cache"]),
            ("self", &["I run locally", "receipts beat repetition"]),
        ];
        assert_beats(&dark(), needles, "The note should record");
    }

    /// The Russian reel tells the same beats in Russian — the conversation
    /// and the interface both: the needles include the interface's own words
    /// (the role labels, the input box's hint, the screen's title), and no
    /// frame carries the English showcase's words or labels.
    #[test]
    fn the_russian_reel_is_russian_through_and_through() {
        let reel = reel(Theme::Dark, Lang::Ru);
        #[rustfmt::skip]
        let needles: &[(&str, &[&str])] = &[
            ("open", &["Какой квант Gemma 4 12B", "золотая середина", "ВЫ", "АССИСТЕНТ"]),
            ("type", &["сохрани заметку о моём железе.", "Enter отправить"]),
            ("think", &["В заметке стоит записать", "мысли"]),
            ("tool", &["note_save", "видеокарта на 12 ГБ", "Заметка сохранена (id="]),
            ("answer", &["Какое окно нужно?", "запас на KV", "кэш вдвое больше"]),
            ("self", &["Модель себя", "Я работаю локально", "Ответы со ссылкой на заметку"]),
        ];
        assert_beats(&reel, needles, "В заметке стоит записать");
        for frame in &reel.frames {
            let screen = text(frame);
            for english in [
                "sweet spot",
                "YOU",
                "ASSISTANT",
                "Self-model",
                "The user's GPU",
                "Note saved",
            ] {
                assert!(!screen.contains(english), "{english:?} in:\n{screen}");
            }
        }
    }

    /// Whether the flowchart's middle leg runs straight from the decision node
    /// to its arrow. The chart is laid out by its labels' widths, and a label
    /// of another length bends that leg into a jog — the Russian labels' first
    /// draft did, and only an eye on the render caught it.
    fn middle_leg_is_straight(frame: &ReelFrame) -> bool {
        // The grid by column: a wide glyph's trailing cell is empty.
        let grid: Vec<Vec<&str>> = frame
            .frame
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .flat_map(|c| std::iter::once(c.s.as_str()).chain((1..c.w).map(|_| "")))
                    .collect()
            })
            .collect();
        let arrows =
            |row: &[&str]| -> Vec<usize> { (0..row.len()).filter(|&x| row[x] == "▾").collect() };
        let Some(arrow_row) = (0..grid.len()).rev().find(|&y| arrows(&grid[y]).len() == 3) else {
            return false;
        };
        let column = arrows(&grid[arrow_row])[1];
        let mut y = arrow_row - 1;
        while y > 0 && grid[y][column] == "│" {
            y -= 1;
        }
        // The walk has to end on the decision node's bottom border.
        grid[y][column] == "─" && grid[y].contains(&"╲")
    }

    #[test]
    fn the_flowchart_s_legs_are_straight_in_every_language() {
        for (theme, lang) in LOOKS {
            let reel = reel(theme, lang);
            assert!(
                middle_leg_is_straight(end_of(&reel, "answer")),
                "{}: the middle leg is bent:\n{}",
                reel.name,
                text(end_of(&reel, "answer"))
            );
        }
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

    /// The question is typed the way a person types it, not at a machine's
    /// even 40 ms: frames add bursts (fewer frames than characters, but more
    /// than the bursts' bound allows at the least), the delays vary, and the
    /// typing stops somewhere to think.
    #[test]
    fn the_typing_has_a_hand_s_rhythm() {
        let reel = dark();
        let question = &demo::showcase_messages()[2].text;
        let chars = question.chars().count();
        let typing: Vec<u32> = reel
            .frames
            .iter()
            .filter(|f| f.beat == "type")
            .map(|f| f.ms)
            .collect();
        // The beat's last frame is the hold after the question, not a key.
        let keys = &typing[..typing.len() - 1];
        assert!(
            keys.len() < chars && keys.len() >= chars / Hand::BURST as usize,
            "{} frames for {chars} characters",
            keys.len()
        );
        let distinct: std::collections::BTreeSet<_> = keys.iter().collect();
        assert!(distinct.len() >= 10, "delays barely vary: {distinct:?}");
        assert!(keys.iter().any(|&ms| ms >= 200), "no pause: {keys:?}");
        assert!(keys.iter().all(|&ms| ms >= Hand::KEY_MS), "{keys:?}");
    }

    /// The self-model is the reel's point: it stays up longer than any other
    /// frame.
    #[test]
    fn the_self_model_is_the_longest_hold() {
        let reel = dark();
        let longest = reel.frames.iter().max_by_key(|f| f.ms).unwrap();
        assert_eq!(longest.beat, "self");
        assert_eq!(longest.ms, SELF_MS);
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
        for (theme, lang) in LOOKS {
            let reel = reel(theme, lang);
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
}
