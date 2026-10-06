//! The small-window gate (spec §11.1.1, docs/research/small-terminal.md §4.6).
//!
//! Every screen, and every popup the chat opens, is drawn through the real
//! [`compose_frame`] at window sizes from 0×0 up, in every built-in language.
//! Each frame has to be one of two things:
//!
//! * **the placeholder** — nothing of the screen on it and no cursor — when the
//!   window is smaller than the frame in front needs; or
//! * **the screen, whole** — what the case names is on it, and the cursor is
//!   inside the frame or hidden.
//!
//! What it replaces is a class of frame nobody drew on purpose: an input box
//! whose prompt stood on the status bar, a cursor below the last row, a
//! question whose answering key was cut off at the border.

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::entities::chat::{ChatSummary, FeedView};
use crate::entities::message::Message;
use crate::entities::profile::{CharacterNames, ProfileSummary};
use crate::features::chat_search::{SearchGroup, SearchHit, build_snippet};
use crate::features::demo;
use crate::features::rag_ingest::RagProgress;
use crate::features::workspace_diff::{ChangeSet, DiffKind, DiffLine, FileChange, FileState};
use crate::shared::config::NoteOrder;
use crate::shared::i18n::{Lang, Locale, locale};
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::ui::WayOut;

/// The widths and heights of the sweep: every size up to the smallest window
/// anything is drawn in, both sides of each screen's and each popup's
/// threshold, and the sizes a window really has.
const WIDTHS: [u16; 29] = [
    0, 1, 2, 3, 4, 8, 12, 19, 20, 21, 23, 24, 29, 30, 36, 37, 39, 40, 41, 45, 46, 47, 57, 68, 69,
    72, 80, 100, 116,
];
const HEIGHTS: [u16; 17] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 24, 33];

/// The loop's drawing state, owned in one place.
struct Rig {
    screen: ChatScreen,
    active: ActiveScreen,
    help: HelpOverlay,
}

/// One drawn frame, read back.
struct Shot {
    rows: Vec<String>,
    /// The frame is the placeholder, and these are the keys it names.
    placeholder: Option<WayOut>,
    /// `None` — hidden.
    cursor: Option<(u16, u16)>,
}

impl Shot {
    fn text(&self) -> String {
        self.rows.join("\n")
    }
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn plain(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

impl Rig {
    /// A chat with an exchange in it and both servers ready. The reply cites
    /// another conversation, so the `chat://` picker has something to list;
    /// the draft, when there is one, wraps to several rows in any window.
    fn chat(lang: Lang, draft: &str) -> Self {
        let mut config = demo::app_config();
        config.interface.language = lang;
        config.interface.confirm_destructive_keys = true;
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
        let here = demo::chat_id();
        let mut cited = ChatSummary::fixture("The other conversation");
        cited.profile_id = uuid::Uuid::from_u128(7);
        let mut open = ChatSummary::fixture(demo::CHAT_TITLE);
        open.id = here;
        open.profile_id = cited.profile_id;
        let reply = format!(
            "A short answer, and where it came from: {}",
            crate::features::chat_links::uri(cited.id)
        );
        screen.activate_chat(
            here,
            demo::CHAT_TITLE.into(),
            &[
                Message::user("A question of two lines.\nThe second one."),
                Message::assistant(reply),
            ],
            draft,
            FeedView::default(),
            None,
            None,
        );
        screen.set_chat_list(vec![open, cited]);
        Self {
            screen,
            active: ActiveScreen::Chat,
            help: HelpOverlay::new(),
        }
    }

    /// Another screen in front of an idle chat.
    fn over(lang: Lang, active: impl FnOnce(Palette, &'static Locale) -> ActiveScreen) -> Self {
        let mut rig = Self::chat(lang, "");
        rig.active = active(rig.screen.palette(), locale(lang));
        rig
    }

    fn key(mut self, key: KeyEvent) -> Self {
        let _ = self.screen.handle_key(key);
        self
    }

    /// A frame the way the loop puts one on the screen ([`present`]): the
    /// cursor read back is the terminal's, after the frame was applied.
    fn draw(&mut self, width: u16, height: u16) -> Shot {
        let palette = front_palette(&self.active, &self.screen);
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        let placeholder = present(
            &mut term,
            &mut self.screen,
            &mut self.active,
            &mut self.help,
            &palette,
        )
        .unwrap();
        let backend = term.backend();
        let cursor = backend.cursor_visible().then(|| {
            let at = backend.cursor_position();
            (at.x, at.y)
        });
        Shot {
            rows: crate::shared::ui::tests::buffer_rows(backend.buffer()),
            placeholder,
            cursor,
        }
    }
}

// ---------- the states ----------

fn long_draft() -> String {
    "a draft long enough to wrap in any window the chat is drawn in, several times over".repeat(3)
}

fn chat_idle(lang: Lang) -> Rig {
    Rig::chat(lang, "")
}

/// The tallest the chat's layout gets: the indexing banner, a draft of many
/// rows, and a turn running — the status bar at its two rows.
fn chat_at_its_tallest(lang: Lang) -> Rig {
    let mut rig = Rig::chat(lang, &long_draft());
    rig.screen
        .set_rag_progress(RagProgress::Started { total: 12 });
    let turn = uuid::Uuid::from_u128(1);
    rig.screen.begin_generation(turn, None);
    rig.screen.push_chunk(turn, "A reply on its way.");
    rig
}

fn chat_search(lang: Lang) -> Rig {
    Rig::chat(lang, "").key(ctrl('f'))
}

fn chat_impersonation(lang: Lang) -> Rig {
    let mut rig = Rig::chat(lang, "");
    let turn = uuid::Uuid::from_u128(2);
    rig.screen.begin_impersonation(turn);
    rig.screen
        .push_impersonation_chunk(turn, "A line written for the user.");
    rig
}

fn chat_help(lang: Lang) -> Rig {
    let mut rig = Rig::chat(lang, "");
    rig.help.open_for(help_context(&rig.active));
    rig
}

fn chat_emoji(lang: Lang) -> Rig {
    Rig::chat(lang, "").key(ctrl('b'))
}

fn chat_links(lang: Lang) -> Rig {
    let mut rig = Rig::chat(lang, "");
    // The references the picker lists are collected by a render.
    let _ = rig.draw(80, 24);
    rig.key(ctrl('l'))
}

fn chat_profiles(lang: Lang) -> Rig {
    let mut rig = Rig::chat(lang, "");
    let profile = |name: &str| ProfileSummary {
        id: uuid::Uuid::new_v4(),
        name: name.to_string(),
    };
    rig.screen
        .set_profile_list(vec![profile("Assistant"), profile("Reviewer")]);
    let _ = rig.screen.request_new_chat();
    rig
}

fn chat_suggestions(lang: Lang) -> Rig {
    let mut rig = Rig::chat(lang, "");
    let dict = spellbook::Dictionary::new("SET UTF-8\n", "2\nhello\nworld\n").unwrap();
    rig.screen.set_spellchecker(SpellChecker::new(
        vec![dict],
        std::collections::HashSet::new(),
        None,
    ));
    for c in "helo".chars() {
        rig = rig.key(plain(KeyCode::Char(c)));
    }
    rig.key(ctrl('g'))
}

fn chat_confirm(lang: Lang) -> Rig {
    Rig::chat(lang, "").key(ctrl('r'))
}

/// The dangerous-tool question over a running turn, with code of several
/// lines to approve.
fn chat_tool_confirm(lang: Lang) -> Rig {
    let mut rig = Rig::chat(lang, "");
    rig.screen.request_tool_confirm(
        uuid::Uuid::from_u128(3),
        "call-1".into(),
        "python_exec".into(),
        r#"{"code":"import os\nprint(os.getcwd())"}"#.into(),
        None,
    );
    rig
}

/// The *Local servers* list, as the start of a chat with no engine opens it
/// (docs/research/local-servers.md, stage 2).
fn chat_local_servers(lang: Lang) -> Rig {
    use crate::shared::api::ServerKind;
    use crate::shared::api::local_servers::LocalOffer;
    let mut rig = Rig::chat(lang, "");
    rig.screen.offer_local_servers(vec![
        LocalOffer {
            server: ServerKind::LmStudio,
            url: "http://127.0.0.1:1234/v1".into(),
            model: "google_gemma-4-e4b-it".into(),
            loaded: true,
            embedder: Some("text-embedding-nomic-embed-text-v1.5".into()),
        },
        LocalOffer {
            server: ServerKind::Ollama,
            url: "http://127.0.0.1:11434/v1".into(),
            model: "gemma4:e4b".into(),
            loaded: false,
            embedder: None,
        },
    ]);
    rig
}

fn chat_list(lang: Lang) -> Rig {
    Rig::over(lang, |palette, loc| {
        ActiveScreen::ChatList(Box::new(ChatListScreen::new(
            demo::chat_summaries(),
            Some(demo::chat_id()),
            palette,
            loc,
        )))
    })
}

fn settings_with(lang: Lang, keys: &[KeyEvent]) -> Rig {
    let mut config = demo::app_config();
    config.interface.language = lang;
    let mut settings = SettingsScreen::new(config, vec![demo::profile()], Vec::new());
    for key in keys {
        let _ = settings.handle_key(*key);
    }
    let mut rig = Rig::chat(lang, "");
    rig.active = ActiveScreen::Settings(Box::new(settings));
    rig
}

fn settings(lang: Lang) -> Rig {
    settings_with(lang, &[])
}

/// A field of the first section open in its one-line editor.
fn settings_editor(lang: Lang) -> Rig {
    let down = plain(KeyCode::Down);
    let enter = plain(KeyCode::Enter);
    settings_with(lang, &[enter, down, down, down, enter])
}

fn settings_search(lang: Lang) -> Rig {
    settings_with(lang, &[plain(KeyCode::Char('/'))])
}

fn self_model(lang: Lang) -> Rig {
    Rig::over(lang, |palette, loc| {
        ActiveScreen::SelfModel(Box::new(SelfModelScreen::new(
            Some(demo::self_model()),
            CharacterNames::default(),
            palette,
            loc,
            NoteOrder::default(),
        )))
    })
}

fn search(lang: Lang) -> Rig {
    let hit = |text: &str| SearchHit {
        message_id: uuid::Uuid::new_v4(),
        role: "user".into(),
        ts: "2026-07-29T10:00:00+00:00".into(),
        snippet: build_snippet(text, "marker", 160),
    };
    let groups = vec![SearchGroup {
        parent: None,
        chat_id: uuid::Uuid::new_v4(),
        title: "A chat".into(),
        hits: vec![hit("one message with the MARKER"), hit("a MARKER again")],
    }];
    Rig::over(lang, |palette, loc| {
        ActiveScreen::Search(Box::new(SearchScreen::new(
            "marker".into(),
            groups,
            2,
            palette,
            loc,
        )))
    })
}

fn changes_with(lang: Lang, keys: &[KeyEvent]) -> Rig {
    let file = |path: &str| FileChange {
        path: path.into(),
        state: FileState::Modified,
        added: 12,
        removed: 4,
        lines: vec![
            DiffLine {
                kind: DiffKind::Removed,
                text: "the old line".into(),
            },
            DiffLine {
                kind: DiffKind::Added,
                text: "the new line".into(),
            },
        ],
    };
    let set = ChangeSet {
        root: "D:/proj".into(),
        files: vec![file("src/main.rs"), file("src/lib.rs")],
    };
    Rig::over(lang, |palette, loc| {
        let mut screen = ChangesScreen::new(set, palette, loc);
        for key in keys {
            let _ = screen.handle_key(*key);
        }
        ActiveScreen::Changes(Box::new(screen))
    })
}

fn changes(lang: Lang) -> Rig {
    changes_with(lang, &[])
}

fn changes_revert(lang: Lang) -> Rig {
    changes_with(lang, &[plain(KeyCode::Char('r'))])
}

fn tasks(lang: Lang) -> Rig {
    Rig::over(lang, |palette, loc| {
        ActiveScreen::Tasks(Box::new(TasksScreen::new(palette, loc)))
    })
}

// ---------- what a drawn frame has to show ----------

/// The chat's own parts, top to bottom, none standing on another — what the
/// ladder of spec §11.1.1 keeps at the window's height, stated here as the
/// table it is rather than read off the code: the feed's border with a row
/// of the conversation inside it from eight rows up, and a bare row of the
/// conversation below; the input box with its border from ten rows up, and
/// a bare `❯` row below; the status bar under it from six rows up, two rows
/// deep at most from eleven, one row otherwise, and no row at all below six.
/// The cursor is on the input's rows.
fn chat_is_whole(shot: &Shot, _loc: &'static Locale) -> Result<(), String> {
    let height = shot.rows.len();
    let starts = |row: usize, with: &str| shot.rows.get(row).is_some_and(|r| r.starts_with(with));
    let framed = |part: &str, on: bool| {
        if on {
            format!("{part} has no row inside its border")
        } else {
            format!("{part} is bare")
        }
    };
    let feed_border = height >= 8;
    if feed_border != (starts(0, "╭") && starts(1, "│")) {
        return Err(framed("the feed", feed_border));
    }
    let (input_border, prompt) = if height >= 10 {
        (true, "│❯")
    } else {
        (false, "❯")
    };
    let Some(input) = shot.rows.iter().position(|r| r.starts_with(prompt)) else {
        return Err(framed("the input box", input_border));
    };
    // The input's rows of text end at its `╰` row, and the rows past the box
    // start after it; a bare box's rows of text run to the status bar or the
    // frame's end.
    let (past_text, past_box) = if input_border {
        let Some(closed) = shot.rows[input..].iter().position(|r| r.starts_with('╰')) else {
            return Err("the input box has no bottom border".into());
        };
        (input + closed, input + closed + 1)
    } else {
        let bar = shot.rows[input..].iter().position(|r| r.starts_with('●'));
        let past = bar.map_or(height, |at| input + at);
        (past, past)
    };
    let status = shot.rows[past_box..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let allowed = match height {
        ..=5 => 0..=0,
        6..=10 => 1..=1,
        _ => 1..=2,
    };
    if !allowed.contains(&status) || (status > 0 && !starts(past_box, "●")) {
        return Err(format!(
            "{status} rows of status bar under the input, {allowed:?} allowed"
        ));
    }
    match shot.cursor {
        Some((_, y)) if usize::from(y) >= input && usize::from(y) < past_text => Ok(()),
        other => Err(format!(
            "the cursor is not on the input's rows {input}..{past_text}: {other:?}"
        )),
    }
}

/// A full-screen panel is open and whole: its border with a row inside, and
/// under its bottom border a footer of no more than a third of the window —
/// one row at the least — with `F1` in it, the key that lists whatever the
/// footer shed (spec §11.1.1, docs/research/small-terminal.md §5 F3(b)).
/// Until stage 3 the footer wrapped without end, and at 57 columns the `ru`
/// chat list's took 13 rows (§2.4). What the panel holds is each case's to
/// check.
fn panel_is_open(shot: &Shot, _loc: &'static Locale) -> Result<(), String> {
    let starts = |row: usize, with: &str| shot.rows.get(row).is_some_and(|r| r.starts_with(with));
    if !starts(0, "╭") || !starts(1, "│") {
        return Err("the panel has no row inside its border".into());
    }
    let Some(bottom) = shot.rows.iter().rposition(|r| r.starts_with('╰')) else {
        return Err("the panel has no bottom border".into());
    };
    let footer = &shot.rows[bottom + 1..];
    let allowed = (shot.rows.len() / 3).max(1);
    if footer.len() > allowed {
        return Err(format!(
            "{} rows of footer, {allowed} allowed",
            footer.len()
        ));
    }
    if !footer.iter().any(|r| r.contains("F1")) {
        return Err("the footer has lost `F1`".into());
    }
    Ok(())
}

/// The text inside the box titled `title`, its rows joined by spaces — a
/// wrapped sentence reads as it was written.
fn boxed(shot: &Shot, title: &str) -> String {
    let top = shot
        .rows
        .iter()
        .position(|r| r.contains(title))
        .unwrap_or_else(|| panic!("no box titled {title:?}:\n{}", shot.text()));
    let left = shot.rows[top].find('╭').unwrap();
    shot.rows[top + 1..]
        .iter()
        .map(|r| &r[left..])
        .take_while(|r| r.starts_with('│'))
        .map(|r| {
            let inner = r.trim_start_matches('│');
            inner.split('│').next().unwrap_or(inner).trim()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every one of `needles` is on the frame.
fn shows(shot: &Shot, needles: &[&str]) -> Result<(), String> {
    let text = shot.text();
    match needles.iter().find(|n| !text.contains(**n)) {
        Some(missing) => Err(format!("{missing:?} is not on screen")),
        None => Ok(()),
    }
}

/// A popup's key legend, whole, as its border carries it.
fn legend(shot: &Shot, loc: &'static Locale, key: &str) -> Result<(), String> {
    shows(shot, &[loc.t(key).trim()])
}

/// A question's keys. They may be wrapped across rows of the body, so what is
/// looked for is each key's name — the cut this gate exists for took `Enter`.
fn answer_keys(shot: &Shot, extra: &[&str]) -> Result<(), String> {
    shows(shot, &["Enter", "Esc"]).and_then(|()| shows(shot, extra))
}

/// The impersonation preview, streaming: the tail of its text, and at the
/// left edge of that row what says the preview is at work — the box's
/// border from ten rows up, the spinner in the bare row's prompt column
/// below (spec §11.1.1); the status bar's chip from six rows up.
fn preview_is_streaming(shot: &Shot, loc: &'static Locale) -> Result<(), String> {
    let Some(row) = shot.rows.iter().find(|r| r.contains("user.")) else {
        return Err("the preview's text is not on screen".into());
    };
    let spinner = crate::shared::theme::Palette::default().glyphs().spinner;
    let at_work = if shot.rows.len() >= 10 {
        row.starts_with('│')
    } else {
        // The spinner is on the preview's first row, which is the tail's
        // when the text fits it and the row above otherwise.
        shot.rows.iter().any(|r| {
            let first = r.chars().next().unwrap_or(' ');
            spinner.contains(&first)
                && r[first.len_utf8()..].starts_with(' ')
                && (r.contains("written") || r.contains("user."))
        })
    };
    if !at_work {
        return Err(format!("nothing says the preview is at work: {row:?}"));
    }
    // With its border, the way out is whole on it — the title fitted by
    // whole parts, not cut at the corner (`ui::fit_title`).
    let esc = loc
        .t("ui.impersonation.active")
        .split(" · ")
        .last()
        .unwrap_or("")
        .trim();
    if shot.rows.len() >= 10
        && !shot
            .rows
            .iter()
            .any(|r| r.starts_with('╭') && r.contains(esc))
    {
        return Err(format!("the preview's border lost {esc:?}"));
    }
    let chip = (shot.rows.len() >= 6).then(|| loc.t("ui.status.chip.chat"));
    shows(shot, chip.as_slice())
}

type Build = fn(Lang) -> Rig;
type Check = fn(&Shot, &'static Locale) -> Result<(), String>;

/// Every state the sweep draws, and what a frame of it that is not the
/// placeholder has to show.
const CASES: &[(&str, Build, Check)] = &[
    ("chat", chat_idle, chat_is_whole),
    ("chat at its tallest", chat_at_its_tallest, chat_is_whole),
    ("in-feed search", chat_search, chat_is_whole),
    ("impersonation", chat_impersonation, preview_is_streaming),
    ("help", chat_help, |s, l| {
        // No cursor: the input box under the dialog placed one, and it used
        // to blink through the dialog ([`present`]).
        match s.cursor {
            Some(at) => Err(format!("a cursor at {at:?} under the help")),
            None => legend(s, l, "ui.help.footer.tabs"),
        }
    }),
    ("emoji picker", chat_emoji, |s, l| {
        legend(s, l, "ui.emoji.footer").and_then(|()| shows(s, &["😀", "🎯"]))
    }),
    ("chat:// picker", chat_links, |s, l| {
        legend(s, l, "ui.chat_links.footer")
    }),
    ("local servers", chat_local_servers, |s, l| {
        legend(s, l, "ui.local_servers.footer").and_then(|()| shows(s, &["LM Studio", "Ollama"]))
    }),
    ("profile picker", chat_profiles, |s, l| {
        legend(s, l, "ui.profile_list.footer").and_then(|()| shows(s, &["Reviewer"]))
    }),
    ("spelling suggestions", chat_suggestions, |s, l| {
        legend(s, l, "ui.suggest.footer").and_then(|()| shows(s, &["hello"]))
    }),
    ("destructive confirmation", chat_confirm, |s, l| {
        answer_keys(s, &[l.t("ui.confirm.title")])
    }),
    ("tool confirmation", chat_tool_confirm, |s, _| {
        answer_keys(s, &["python_exec", "import os", "print(os.getcwd())"])
    }),
    ("chat list", chat_list, |s, l| {
        // The selected row: the rail and the chat's dot.
        panel_is_open(s, l).and_then(|()| shows(s, &["▌ ●"]))
    }),
    ("settings", settings, |s, l| {
        // The menu whole: its last section is on screen.
        panel_is_open(s, l).and_then(|()| {
            shows(
                s,
                &[
                    l.t("ui.settings.ui.title").trim(),
                    l.t("ui.settings.section.interface"),
                ],
            )
        })
    }),
    ("settings editor", settings_editor, |s, _| shows(s, &["│❯"])),
    ("settings search", settings_search, |s, _| shows(s, &["│❯"])),
    ("self-model", self_model, |s, l| {
        panel_is_open(s, l).and_then(|()| shows(s, &[l.t("ui.self_model.title")]))
    }),
    ("message search", search, |s, l| {
        // The query the results answer — it used to be squeezed out by a
        // footer of five rows at 30×8.
        panel_is_open(s, l).and_then(|()| shows(s, &["marker"]))
    }),
    ("changes", changes, |s, l| {
        panel_is_open(s, l).and_then(|()| shows(s, &["main.rs", "the old"]))
    }),
    ("revert confirmation", changes_revert, |s, l| {
        answer_keys(s, &[l.t("ui.confirm.title")])
    }),
    ("tasks", tasks, |s, l| {
        panel_is_open(s, l).and_then(|()| shows(s, &[l.t("ui.tasks.title")]))
    }),
];

/// Says which frame a failure — or a panic — belongs to.
struct At(String);

impl Drop for At {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("while drawing {}", self.0);
        }
    }
}

/// Draws one state at every size of the sweep and holds each frame to the
/// module's rule. The same screen object lives through the whole sweep, the
/// way it lives through a resize.
fn sweep(name: &str, build: Build, check: Check) {
    for &lang in Lang::ALL {
        let loc = locale(lang);
        let mut rig = build(lang);
        let (mut drawn, mut refused) = (0usize, 0usize);
        for width in WIDTHS {
            for height in HEIGHTS {
                let at = At(format!("{name}, {lang:?}, {width}×{height}"));
                let shot = rig.draw(width, height);
                let text = shot.text();
                if shot.placeholder.is_some() {
                    refused += 1;
                    assert_eq!(shot.cursor, None, "{}: a cursor on the placeholder", at.0);
                    assert!(
                        !text.contains(['╭', '│', '❯']),
                        "{}: the screen shows through the placeholder:\n{text}",
                        at.0
                    );
                    continue;
                }
                drawn += 1;
                if let Some((x, y)) = shot.cursor {
                    assert!(
                        x < width && y < height,
                        "{}: the cursor is outside the frame at ({x}, {y})",
                        at.0
                    );
                }
                if let Err(why) = check(&shot, loc) {
                    panic!("{}: {why}:\n{text}", at.0);
                }
            }
        }
        // Not vacuous either way: the state was really drawn, and really refused.
        assert!(
            drawn > 0 && refused > 0,
            "{name}, {lang:?}: {drawn} drawn, {refused} refused"
        );
    }
}

/// One thread a state: twenty-one sweeps of 493 sizes in two languages are
/// twenty thousand frames, and none of them shares anything with another.
#[test]
fn every_state_is_whole_or_the_placeholder_at_every_size() {
    std::thread::scope(|scope| {
        for (name, build, check) in CASES {
            scope.spawn(move || sweep(name, *build, *check));
        }
    });
}

// ---------- the placeholder itself, and the reported windows ----------

/// The three windows of the report (docs/research/small-terminal.md §1), in
/// the language they were reported in: each used to be a frame with parts
/// missing. Two are a line that says what size they need; the chat's, 57×5,
/// is the chat — four rows of the conversation over the `❯` row, its cursor
/// on it (stage 2 of the track), where the prompt used to stand on the
/// status bar.
#[test]
fn the_reported_windows_say_what_they_need() {
    for (build, width, height, need) in [
        (chat_idle as Build, 57, 2, "20×3"),
        (settings as Build, 45, 6, "46×12"),
        (chat_list as Build, 45, 2, "24×7"),
    ] {
        let shot = build(Lang::Ru).draw(width, height);
        let text = shot.text();
        assert!(shot.placeholder.is_some(), "{width}×{height}:\n{text}");
        assert!(
            text.contains(&format!("{width}×{height}")) && text.contains(need),
            "{width}×{height}: the size it is and the size it needs — {need}:\n{text}"
        );
    }
    let shot = chat_idle(Lang::Ru).draw(57, 5);
    assert_eq!(shot.placeholder, None, "{}", shot.text());
    assert!(
        shot.rows[4].starts_with("❯ ") && shot.cursor == Some((2, 4)),
        "the `❯` row is the last one, with the cursor:\n{}",
        shot.text()
    );
    assert!(
        shot.rows[..4].iter().all(|r| !r.contains(['╭', '│', '●'])),
        "four bare rows of the conversation above it:\n{}",
        shot.text()
    );
    assert!(
        !shot.rows[3].trim().is_empty(),
        "the row over the prompt is text, not the padding under the last message:\n{}",
        shot.text()
    );
    assert!(
        shot.text().contains("The second one."),
        "the last message's text:\n{}",
        shot.text()
    );
    // The chat's minimum, three rows: a row of it, the `❯` row, nothing else.
    let shot = chat_idle(Lang::Ru).draw(20, 3);
    assert_eq!(shot.placeholder, None, "{}", shot.text());
    assert!(shot.rows[2].starts_with("❯ "), "{}", shot.text());
}

/// The footers of stage 3 (docs/research/small-terminal.md §5 F3(b)), in
/// the case the research measured: the `ru` chat list at 57 columns spent 13
/// rows of a 14-row window on its legend and showed one chat. Within a third
/// of the window it spends four and shows five — and at 80×24, where seven
/// rows are inside a third, nothing changes.
#[test]
fn a_footer_takes_no_more_than_a_third_of_the_window() {
    let footer = |shot: &Shot| -> Vec<String> {
        let bottom = shot.rows.iter().rposition(|r| r.starts_with('╰')).unwrap();
        shot.rows[bottom + 1..].to_vec()
    };
    let chats = |shot: &Shot| shot.rows.iter().filter(|r| r.contains(" ● ")).count();

    let mut rig = chat_list(Lang::Ru);
    let shot = rig.draw(57, 14);
    let rows = footer(&shot);
    assert_eq!(rows.len(), 4, "{}", shot.text());
    assert!(rows.iter().any(|r| r.contains("F1")), "{}", shot.text());
    assert!(rows.iter().any(|r| r.contains("Esc")), "{}", shot.text());
    assert_eq!(chats(&shot), 5, "{}", shot.text());

    let shot = rig.draw(80, 24);
    let rows = footer(&shot);
    assert_eq!(
        rows.len(),
        7,
        "the legend whole, as it was:\n{}",
        shot.text()
    );
    for key in ["Del", "F5", "Ctrl+Q", "Tab"] {
        assert!(
            rows.iter().any(|r| r.contains(key)),
            "{key}:\n{}",
            shot.text()
        );
    }

    // At a screen's minimum the footer is the one row the minimum was counted
    // with, and the panel is whole: the settings' menu to its last section.
    let shot = settings(Lang::Ru).draw(46, 12);
    let rows = footer(&shot);
    assert_eq!(rows.len(), 1, "{}", shot.text());
    assert!(
        rows[0].contains("F1") && rows[0].contains("Esc"),
        "{}",
        shot.text()
    );
    let interface = locale(Lang::Ru).t("ui.settings.section.interface");
    assert!(shot.text().contains(interface), "{}", shot.text());
    // One row more, and the footer takes it: the panel needs eleven rows,
    // not twelve — the minimum's one row of hints is the footer's own.
    let shot = settings(Lang::Ru).draw(46, 13);
    assert_eq!(footer(&shot).len(), 2, "{}", shot.text());
    assert!(shot.text().contains(interface), "{}", shot.text());
}

/// The ladder, frame by frame, in the reported window's 57 columns: from ten
/// rows down each step gives up one thing and the conversation keeps its
/// four rows — the numbers of docs/research/small-terminal.md §5 F1(b).
#[test]
fn the_chat_sheds_its_chrome_as_the_window_shrinks() {
    let mut rig = chat_idle(Lang::En);
    // (rows, the rows of the feed's border, the input's first row, status rows)
    for (height, feed_border, prompt, status) in [
        (11, true, "│❯", 2),
        (10, true, "│❯", 1),
        (9, true, "❯", 1),
        (8, true, "❯", 1),
        (7, false, "❯", 1),
        (6, false, "❯", 1),
        (5, false, "❯", 0),
        (3, false, "❯", 0),
    ] {
        let shot = rig.draw(57, height);
        let text = shot.text();
        assert_eq!(shot.placeholder, None, "{height} rows:\n{text}");
        assert_eq!(
            shot.rows[0].starts_with('╭'),
            feed_border,
            "{height} rows:\n{text}"
        );
        let input = shot.rows.iter().position(|r| r.starts_with(prompt));
        assert!(input.is_some(), "{height} rows, {prompt:?}:\n{text}");
        let bar = shot.rows.iter().filter(|r| r.starts_with('●')).count();
        assert_eq!(
            bar.min(1),
            status.min(1),
            "{height} rows, the status bar:\n{text}"
        );
        let under = shot
            .rows
            .iter()
            .rposition(|r| !r.trim().is_empty())
            .unwrap()
            + 1;
        assert_eq!(
            under,
            usize::from(height),
            "{height} rows: the frame is used to its last row:\n{text}"
        );
    }
    // Four rows of the conversation from six rows up: the `❯` row is the
    // fifth at six, and at ten, with the feed bordered and the input boxed.
    assert!(rig.draw(57, 6).rows[4].starts_with('❯'));
    let ten = rig.draw(57, 10);
    assert!(
        ten.rows[4].starts_with('│') && ten.rows[7].starts_with("│❯"),
        "{}",
        ten.text()
    );
}

/// A popup raises what the window has to hold: a chat that fits its window
/// stops fitting it when the emoji picker — 46 columns of grid — opens, and
/// fits again when it closes. No resize is involved, which is why the
/// placeholder's coming and going is a repaint trigger of its own.
#[test]
fn a_popup_raises_the_minimum_and_lowers_it_back() {
    let mut rig = chat_idle(Lang::En);
    assert_eq!(rig.draw(40, 12).placeholder, None);
    rig = rig.key(ctrl('b'));
    let shot = rig.draw(40, 12);
    assert_eq!(shot.placeholder, Some(WayOut::EscOrQuit), "{}", shot.text());
    assert!(shot.text().contains("46×6"), "{}", shot.text());
    assert!(
        shot.text().contains("Esc"),
        "the way back is named: {}",
        shot.text()
    );
    rig = rig.key(plain(KeyCode::Esc));
    assert_eq!(rig.draw(40, 12).placeholder, None);
}

/// The tool confirmation in the reported 57-column window, in `ru`: its
/// legend is 69 columns, so it used to lose the key that runs the call at the
/// border's corner. It is asked whole — the keys as the last lines of the
/// body — in a window that can hold it, and not asked in one that cannot.
#[test]
fn the_tool_confirmation_is_asked_whole_or_not_at_all() {
    let loc = locale(Lang::Ru);
    let legend = loc.t("ui.confirm.tool.footer").trim();
    let title = loc.t("ui.confirm.tool.title");

    let mut rig = chat_tool_confirm(Lang::Ru);
    let wide = rig.draw(100, 24);
    // The question has the keys, so the input box under it has no cursor —
    // it used to blink through the question's border.
    assert_eq!(wide.cursor, None);
    assert!(
        wide.text().contains(legend) && !boxed(&wide, title).contains(legend),
        "on the border where the border holds it:\n{}",
        wide.text()
    );

    for width in [57, 40, 20] {
        let narrow = rig.draw(width, 24);
        let body = boxed(&narrow, title);
        assert!(
            body.ends_with(legend),
            "{width} columns: every key, as the last lines of the body — {body:?}\n{}",
            narrow.text()
        );
        assert!(body.contains("print(os.getcwd())"), "{width}: {body:?}");
    }

    // Twenty columns wrap the question into eleven rows. A window of ten —
    // tall enough for the chat under it — is too short for the question:
    // nothing of it is drawn, and the line that is says how tall it has to be.
    assert_eq!(rig.draw(20, 11).placeholder, None);
    let shot = rig.draw(20, 10);
    assert!(shot.placeholder.is_some(), "{}", shot.text());
    assert!(shot.text().contains("20×11"), "{}", shot.text());
    assert!(!shot.text().contains("python_exec"), "{}", shot.text());
}

// ---------- the keys under the placeholder ----------

/// One batch of keys, read against `shot` — the frame that was on screen.
/// Returns whether it asked to quit, and the commands it sent.
fn tick(rig: &mut Rig, shot: &Shot, keys: &[KeyEvent]) -> (bool, Vec<AppCommand>) {
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let quit = route_batch(
        shot.placeholder,
        keys.iter().copied().map(Event::Key).collect(),
        &mut rig.screen,
        &mut rig.active,
        &mut rig.help,
        &mut None,
        &cmd_tx,
        &mut None,
    );
    let mut sent = Vec::new();
    while let Ok(command) = cmd_rx.try_recv() {
        sent.push(command);
    }
    (quit, sent)
}

/// In a window too small for the chat itself the placeholder names one key,
/// and only that one works (F2): nothing is typed, nothing is sent, `Esc`
/// goes nowhere, and `Ctrl+Q` — under any keyboard layout — and `F10` end
/// the session.
#[test]
fn a_window_too_small_for_the_chat_takes_only_a_quit_key() {
    let mut rig = chat_idle(Lang::En);
    let shot = rig.draw(57, 2);
    assert_eq!(shot.placeholder, Some(WayOut::Quit));
    assert!(!shot.text().contains("Esc"), "{}", shot.text());

    let typed = [
        plain(KeyCode::Char('h')),
        plain(KeyCode::Enter),
        plain(KeyCode::Esc),
    ];
    for key in typed {
        let (quit, sent) = tick(&mut rig, &shot, &[key]);
        assert!(!quit && sent.is_empty(), "{key:?}: {sent:?}");
    }
    assert!(rig.active.is_chat(), "`Esc` did not open the chat list");
    let roomy = rig.draw(100, 24);
    assert!(
        roomy.rows.iter().any(|r| r.starts_with("│❯  ")),
        "nothing reached the input box:\n{}",
        roomy.text()
    );

    for key in [plain(KeyCode::F(10)), ctrl('q'), ctrl('й')] {
        assert!(tick(&mut rig, &shot, &[key]).0, "{key:?} quits");
    }
    // A quit key is found wherever it is in the batch.
    assert!(tick(&mut rig, &shot, &[plain(KeyCode::Enter), ctrl('q')]).0);

    // The control: the same keys against a frame that shows the chat.
    let (quit, sent) = tick(&mut rig, &roomy, &[plain(KeyCode::Char('h'))]);
    assert!(!quit && sent.is_empty());
    assert!(
        rig.draw(100, 24).text().contains("│❯ h"),
        "typed, this time"
    );
}

/// A question the window cannot hold is not answered by `Enter` — and is not
/// a trap either: it was opened by the turn, not by a resize, so `Esc`, the
/// key that declines, reaches it. One `Esc` a batch: a second would act on
/// the chat the first uncovered, before a frame of it was drawn.
#[test]
fn a_question_that_does_not_fit_can_be_declined_but_not_run() {
    let mut rig = chat_tool_confirm(Lang::Ru);
    let shot = rig.draw(20, 10);
    assert_eq!(shot.placeholder, Some(WayOut::EscOrQuit), "{}", shot.text());
    assert!(shot.text().contains("Esc"), "{}", shot.text());

    for key in [plain(KeyCode::Enter), plain(KeyCode::Char('a'))] {
        let (quit, sent) = tick(&mut rig, &shot, &[key]);
        assert!(!quit && sent.is_empty(), "{key:?} ran nothing: {sent:?}");
    }
    assert!(
        rig.draw(100, 24).text().contains("python_exec"),
        "the question is still waiting"
    );

    let esc = plain(KeyCode::Esc);
    let (quit, sent) = tick(&mut rig, &shot, &[plain(KeyCode::Enter), esc, esc]);
    assert!(!quit);
    assert!(
        matches!(
            sent.as_slice(),
            [AppCommand::ConfirmTool {
                decision: crate::features::tools::confirm::ToolDecision::Deny,
                ..
            }]
        ),
        "one answer, and it is the refusal: {sent:?}"
    );
    assert!(rig.active.is_chat(), "the second `Esc` went nowhere");
    assert_eq!(rig.draw(20, 10).placeholder, None, "the chat is back");
}

/// A screen opened over the chat in a window it does not fit — the settings
/// need 46 columns — is left by `Esc`, not only by quitting: the key that
/// opened it was pressed in a window that showed the chat whole.
#[test]
fn a_screen_that_does_not_fit_is_left_by_esc() {
    let mut rig = settings(Lang::En);
    let shot = rig.draw(45, 12);
    assert_eq!(shot.placeholder, Some(WayOut::EscOrQuit), "{}", shot.text());
    assert!(shot.text().contains("46×12"), "{}", shot.text());
    // `Enter` would have walked into the fields; it does nothing.
    let (quit, _) = tick(&mut rig, &shot, &[plain(KeyCode::Enter)]);
    assert!(!quit && !rig.active.is_chat());

    let (quit, _) = tick(&mut rig, &shot, &[plain(KeyCode::Esc)]);
    assert!(!quit);
    assert!(rig.active.is_chat(), "`Esc` closed the settings");
    assert_eq!(rig.draw(45, 12).placeholder, None, "and the chat fits");

    // The help dialog is the same kind of layer: `F1` in a window narrower
    // than its key legend, and `Esc` takes the notice down with the dialog.
    let mut rig = chat_help(Lang::En);
    let shot = rig.draw(30, 12);
    assert_eq!(shot.placeholder, Some(WayOut::EscOrQuit), "{}", shot.text());
    let (quit, _) = tick(&mut rig, &shot, &[plain(KeyCode::Esc)]);
    assert!(!quit && rig.help.open.is_none(), "`Esc` closed the help");
    assert_eq!(rig.draw(30, 12).placeholder, None);
}

// ---------- the cursor under the help dialog ----------

/// The help dialog covers whatever is in front, and the box under it placed
/// a cursor that kept blinking through the dialog — in the chat's input, the
/// in-feed search, a settings editor or search, the chat list's rename field
/// (docs/research/small-terminal.md §7.3). With the dialog open the frame has
/// no cursor ([`present`]); closed, the box has it back where it was.
#[test]
fn no_cursor_blinks_through_the_help() {
    let chat_list_renaming = |lang: Lang| -> Rig {
        let mut rig = chat_list(lang);
        let shot = rig.draw(80, 24);
        let _ = tick(&mut rig, &shot, &[plain(KeyCode::F(2))]);
        rig
    };
    let states: [(&str, Build); 5] = [
        ("chat", chat_idle),
        ("in-feed search", chat_search),
        ("settings editor", settings_editor),
        ("settings search", settings_search),
        ("chat list, renaming", chat_list_renaming),
    ];
    for (name, build) in states {
        let mut rig = build(Lang::Ru);
        let before = rig.draw(80, 24);
        let at = before
            .cursor
            .unwrap_or_else(|| panic!("{name}: the box has the cursor:\n{}", before.text()));

        rig.help.open_for(help_context(&rig.active));
        let covered = rig.draw(80, 24);
        assert!(rig.help.open.is_some(), "{name}");
        assert_eq!(
            covered.cursor,
            None,
            "{name}: a cursor under the help:\n{}",
            covered.text()
        );

        rig.help.close();
        let after = rig.draw(80, 24);
        assert_eq!(
            after.cursor,
            Some(at),
            "{name}: the cursor back where it was"
        );
    }
}
