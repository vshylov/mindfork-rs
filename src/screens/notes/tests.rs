//! The notes screen's tests (spec §11.11): keys, the search, the editors, the
//! delete's question, the footer, and what it draws.

use super::*;
use crate::entities::note::{Note, NoteLinkView};
use crate::entities::self_model::NarrativeSegment;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn en() -> &'static Locale {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::En)
}

fn note(text: &str, tags: &[&str], days: i64) -> Note {
    let mut n = Note::new(
        Uuid::new_v4(),
        text,
        tags.iter().map(|t| t.to_string()).collect(),
    );
    n.created_at = Utc::now() - chrono::Duration::days(days);
    n.updated_at = n.created_at;
    n
}

fn card(n: Note) -> NoteCard {
    NoteCard {
        note: n,
        links: Vec::new(),
        supersedes: Vec::new(),
    }
}

/// Three notes, newest first as the orchestrator sends them; the interview one
/// is in this conversation's prompt.
fn view() -> NotesView {
    let interview = note("Interview at Lumen Pay on Monday.", &["work"], 3);
    let cat = note("Has a cat called Pixel.", &["pets", SELF_NOTE_TAG], 20);
    let peanuts = note("Allergic to peanuts.", &["health"], 40);
    NotesView {
        shown: vec![interview.id],
        notes: vec![card(interview), card(cat), card(peanuts)],
        observations: Vec::new(),
        notes_in_prompt: true,
    }
}

fn screen() -> NotesScreen {
    let mut s = NotesScreen::new(Palette::default(), en());
    s.set_view(view());
    s
}

fn id_at(s: &NotesScreen, i: usize) -> Uuid {
    s.view.as_ref().unwrap().notes[i].note.id
}

fn press(s: &mut NotesScreen, code: KeyCode) -> Option<NotesIntent> {
    s.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn typed(s: &mut NotesScreen, text: &str) {
    for c in text.chars() {
        press(s, KeyCode::Char(c));
    }
}

fn keys(s: &NotesScreen) -> Vec<&'static str> {
    s.hints().iter().map(|(k, _, _)| *k).collect()
}

fn text_at(s: &mut NotesScreen, width: u16, height: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn it_says_it_is_waiting_until_the_first_snapshot() {
    let mut s = NotesScreen::new(Palette::default(), en());
    assert!(text_at(&mut s, 100, 20).contains("Reading the notes"));
    // Nothing to act on yet: no key that needs a note is offered or answers.
    assert!(!keys(&s).contains(&"Enter") && !keys(&s).contains(&"Del"));
    assert_eq!(press(&mut s, KeyCode::Delete), None);
    assert_eq!(press(&mut s, KeyCode::Delete), None);
}

#[test]
fn the_list_marks_what_the_prompt_carries_and_hides_the_reserved_tag() {
    let mut s = screen();
    let text = text_at(&mut s, 110, 20);
    assert!(
        text.contains("Notes of this list in this conversation's prompt: 1, marked ◆."),
        "{text}"
    );
    assert!(text.contains("All notes (3)"), "{text}");
    assert!(
        text.contains("◆ (3d) Interview at Lumen Pay on Monday.  [work]"),
        "{text}"
    );
    assert!(
        text.contains("[pets]") && !text.contains(SELF_NOTE_TAG),
        "{text}"
    );
}

#[test]
fn the_prompt_line_says_why_nothing_is_marked() {
    let mut s = screen();
    let mut v = view();
    v.shown.clear();
    s.set_view(v.clone());
    assert!(text_at(&mut s, 120, 20).contains("none is near enough"));
    v.notes_in_prompt = false;
    s.set_view(v.clone());
    assert!(text_at(&mut s, 140, 20).contains("Notes in the prompt is off"));
    // The observations the prompt carries are listed too — F3's, not edited here.
    v.observations = vec![NarrativeSegment {
        id: Uuid::new_v4(),
        text: "I ask before advising.".into(),
        created_at: Utc::now(),
    }];
    s.set_view(v);
    let text = text_at(&mut s, 140, 20);
    assert!(
        text.contains("observations of the self-model (F3)"),
        "{text}"
    );
    assert!(text.contains("(today) I ask before advising."), "{text}");
}

#[test]
fn enter_opens_a_note_and_esc_walks_back_out() {
    let mut s = screen();
    press(&mut s, KeyCode::Down);
    assert_eq!(press(&mut s, KeyCode::Enter), None);
    assert_eq!(s.mode, Mode::Note(id_at(&s, 1)));
    assert!(keys(&s).contains(&"E") && !keys(&s).contains(&"/"));
    assert_eq!(press(&mut s, KeyCode::Esc), None);
    assert_eq!(s.mode, Mode::List);
    assert_eq!(press(&mut s, KeyCode::Esc), Some(NotesIntent::Close));
}

#[test]
fn an_opened_note_shows_its_links_and_what_it_replaced() {
    let mut v = view();
    let other = note("Has a sister, Mia.", &[], 10);
    let observation = note("I ask about family gently.", &[SELF_NOTE_TAG], 5);
    let old = note("Interview on Tuesday.", &[], 6);
    v.notes[0].links = vec![
        NoteLinkView {
            relation: "relates".into(),
            outgoing: true,
            other,
        },
        NoteLinkView {
            relation: "supports".into(),
            outgoing: false,
            other: observation,
        },
    ];
    v.notes[0].supersedes = vec![old];
    let mut s = NotesScreen::new(Palette::default(), en());
    s.set_view(v);
    press(&mut s, KeyCode::Enter);
    let text = text_at(&mut s, 100, 24);
    assert!(text.contains("Tags: work"), "{text}");
    assert!(text.contains("◆ In this conversation's prompt"), "{text}");
    assert!(text.contains("Links (2)"), "{text}");
    assert!(text.contains("→ relates: Has a sister, Mia."), "{text}");
    assert!(
        text.contains("← supports: [observation] I ask about family gently."),
        "{text}"
    );
    assert!(text.contains("Replaced by this note (1)"), "{text}");
    assert!(text.contains("(6d) Interview on Tuesday."), "{text}");
}

#[test]
fn the_search_narrows_by_text_and_tags_and_esc_drops_it() {
    let mut s = screen();
    press(&mut s, KeyCode::Char('/'));
    assert!(s.search.is_some());
    typed(&mut s, "PEANUT");
    assert_eq!(s.visible().len(), 1);
    // `Enter` keeps it in force once the field is closed.
    press(&mut s, KeyCode::Enter);
    assert!(s.search.is_none() && s.query == "PEANUT");
    assert_eq!(s.visible().len(), 1);
    let text = text_at(&mut s, 100, 20);
    assert!(text.contains("Notes matching «PEANUT»: 1 of 3"), "{text}");
    // A tag matches too.
    press(&mut s, KeyCode::Char('/'));
    for _ in 0.."PEANUT".len() {
        press(&mut s, KeyCode::Backspace);
    }
    typed(&mut s, "pets");
    assert_eq!(s.visible().len(), 1);
    press(&mut s, KeyCode::Enter);
    // `Esc` on the list drops the search before it closes anything.
    assert_eq!(press(&mut s, KeyCode::Esc), None);
    assert!(s.query.is_empty());
    assert_eq!(s.visible().len(), 3);
}

#[test]
fn the_search_opens_on_the_russian_layouts_slash_key() {
    let mut s = screen();
    press(&mut s, KeyCode::Char('.'));
    assert!(s.search.is_some());
}

#[test]
fn e_edits_the_text_and_an_emptied_text_is_no_edit() {
    let mut s = screen();
    press(&mut s, KeyCode::Char('e'));
    let editor = s.editor.as_ref().expect("the text editor");
    assert_eq!(editor.input.text(), "Interview at Lumen Pay on Monday.");
    typed(&mut s, " It went well.");
    let id = id_at(&s, 0);
    assert_eq!(
        press(&mut s, KeyCode::Enter),
        Some(NotesIntent::Edit(NoteEdit::SetText {
            id,
            text: "Interview at Lumen Pay on Monday. It went well.".into(),
        }))
    );
    assert!(s.editor.is_none());
    // Emptied, the text saves nothing: deleting is `Del`'s job.
    press(&mut s, KeyCode::Char('e'));
    s.editor.as_mut().unwrap().input.set_text("   ");
    assert_eq!(press(&mut s, KeyCode::Enter), None);
    // `Esc` cancels.
    press(&mut s, KeyCode::Char('e'));
    assert_eq!(press(&mut s, KeyCode::Esc), None);
    assert!(s.editor.is_none());
}

#[test]
fn t_edits_the_tags_without_showing_the_reserved_one() {
    let mut s = screen();
    press(&mut s, KeyCode::Down);
    press(&mut s, KeyCode::Char('t'));
    let editor = s.editor.as_ref().expect("the tags editor");
    assert_eq!(editor.input.text(), "pets");
    s.editor
        .as_mut()
        .unwrap()
        .input
        .set_text("pets, family ,, cat");
    let id = id_at(&s, 1);
    assert_eq!(
        press(&mut s, KeyCode::Enter),
        Some(NotesIntent::Edit(NoteEdit::SetTags {
            id,
            tags: vec!["pets".into(), "family".into(), "cat".into()],
        }))
    );
}

#[test]
fn del_asks_and_only_a_second_del_deletes() {
    let mut s = screen();
    assert_eq!(press(&mut s, KeyCode::Delete), None);
    assert!(text_at(&mut s, 100, 20).contains("Delete this note for good?"));
    // Any other key keeps it.
    assert_eq!(press(&mut s, KeyCode::Down), None);
    assert!(s.confirm_delete.is_none());
    let id = id_at(&s, 0);
    press(&mut s, KeyCode::Up);
    press(&mut s, KeyCode::Delete);
    assert_eq!(
        press(&mut s, KeyCode::Delete),
        Some(NotesIntent::Edit(NoteEdit::Delete(id)))
    );
}

#[test]
fn a_deleted_open_note_returns_to_the_list_and_the_selection_follows_its_id() {
    let mut s = screen();
    press(&mut s, KeyCode::Down);
    press(&mut s, KeyCode::Down);
    let kept = id_at(&s, 2);
    press(&mut s, KeyCode::Up);
    press(&mut s, KeyCode::Enter);
    let mut v = view();
    // The opened one (index 1) is gone; the third keeps its place by id.
    v.notes = vec![
        s.view.as_ref().unwrap().notes[0].clone(),
        s.view.as_ref().unwrap().notes[2].clone(),
    ];
    s.set_view(v);
    assert_eq!(s.mode, Mode::List);
    press(&mut s, KeyCode::Down);
    assert_eq!(s.selected_id(), Some(kept));
}

#[test]
fn an_empty_profile_offers_nothing_to_act_on() {
    let mut s = NotesScreen::new(Palette::default(), en());
    s.set_view(NotesView {
        notes_in_prompt: true,
        ..NotesView::default()
    });
    let text = text_at(&mut s, 110, 20);
    assert!(text.contains("No notes yet"), "{text}");
    let k = keys(&s);
    for absent in ["Enter", "E", "T", "Del"] {
        assert!(!k.contains(&absent), "{absent}: {k:?}");
    }
    assert_eq!(press(&mut s, KeyCode::Char('e')), None);
    assert!(s.editor.is_none());
}

#[test]
fn quit_works_from_the_list() {
    let mut s = screen();
    assert_eq!(press(&mut s, KeyCode::F(10)), Some(NotesIntent::Quit));
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        Some(NotesIntent::Quit)
    );
}

#[test]
fn every_hinted_key_has_a_help_row() {
    // The footer names only keys `handle_key` answers; each of them is in the
    // F1 section (AGENTS.md §3), whatever the state.
    let rows: Vec<&str> = HELP_SECTION.rows.iter().map(|(k, _)| *k).collect();
    let mut s = screen();
    let mut states = vec![keys(&s)];
    press(&mut s, KeyCode::Enter);
    states.push(keys(&s));
    for k in states.concat() {
        let k = k.replace("↑↓", "↑/↓");
        if ["F1", "Ctrl+Q"].contains(&k.as_str()) {
            continue;
        }
        assert!(rows.contains(&k.as_str()), "{k} has no F1 row");
    }
}

#[test]
fn it_draws_at_the_smallest_size_without_a_panic() {
    let mut s = screen();
    let min = s.min_size();
    press(&mut s, KeyCode::Enter);
    let _ = text_at(&mut s, min.width, min.height);
    press(&mut s, KeyCode::Char('e'));
    let _ = text_at(&mut s, min.width, min.height);
}
