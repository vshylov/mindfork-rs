use std::collections::BTreeSet;

use super::{
    Again, ChapterContext, Kept, Trace, author_system, continuity, outline, pick, plan, reader,
    revise, story_language, triage, write,
};
use crate::entities::story::{
    Assumption, BranchId, Canon, CharacterKey, Event, EventKind, ReviewNote, SceneSetup, WorldState,
};
use crate::entities::story::{
    Cause, ChapterId, EventId, NewEvent, NewNote, NoteId, NoteKind, NoteSeverity, Reviewer,
    Visibility, fixture_canon,
};
use crate::entities::story::{Decided, NoteDecision};
use crate::features::story::keeper::SoFar;
use crate::features::story::port::Prompt;
use crate::shared::storage::story::Reflection;

fn c(key: &str) -> CharacterKey {
    CharacterKey::from(key)
}

fn trace() -> Trace {
    let branch = BranchId::new("ch1/sc1/r0/k1");
    let event =
        |n: u64, kind, actor: Option<&str>, observable: &str, private: Option<&str>| Event {
            seq: n,
            id: EventId::nth(&branch, n),
            branch: branch.clone(),
            body: NewEvent {
                round: u32::from(kind != EventKind::Opening),
                tick: 0,
                location: "harbour".into(),
                kind,
                actor: actor.map(c),
                visibility: Visibility::Public,
                observable: observable.to_owned(),
                private: private.map(str::to_owned),
                effects: Vec::new(),
                proposal: None,
                cause: Cause::Setup,
            },
            witnesses: BTreeSet::from([c("mara"), c("silas")]),
        };
    Trace {
        branch: branch.clone(),
        events: vec![
            event(1, EventKind::Opening, None, "The ferry is cancelled.", None),
            event(
                2,
                EventKind::Speech,
                Some("mara"),
                "Mara says: \"Gate shuts at sunset.\"",
                Some("If they find the bribe [SECRET-MARA-7Q], I am done."),
            ),
        ],
        reflections: vec![(
            c("mara"),
            Reflection {
                id: "r1".into(),
                branch,
                after_seq: 2,
                body: "Silas knows too much.".into(),
                goal_changes: vec!["Get rid of the ledger.".into()],
            },
        )],
    }
}

fn setup() -> SceneSetup {
    SceneSetup {
        location: "harbour".into(),
        present: vec![c("mara"), c("silas")],
        just_happened: "The ferry was cancelled.".into(),
        pressure: "The gate closes at sunset.".into(),
    }
}

fn ctx<'a>(
    canon: &'a Canon,
    goals: &'a [String],
    summaries: &'a [(u32, String)],
) -> ChapterContext<'a> {
    ChapterContext {
        canon,
        number: 1,
        human_language: "Russian",
        summaries,
        previous: None,
        human_notes: Some("More rain."),
        goals,
        register: &NOTHING_NOTED,
        choices: &[],
        last: false,
        played: true,
    }
}

/// A register with nothing in it, as before the first chapter.
static NOTHING_NOTED: SoFar = SoFar {
    open: Vec::new(),
    facts: Vec::new(),
};

/// No planted token reaches the Author or a reviewer.
fn tokenless(prompt: &Prompt) {
    for part in [&prompt.system, &prompt.user] {
        assert!(!part.contains("[SECRET-"), "a token in {part}");
    }
}

#[test]
fn the_author_sees_the_whole_canon_but_no_planted_token() {
    let mut canon = fixture_canon();
    let system = author_system(&canon, "Russian");
    assert!(system.contains("She took a bribe from the smuggler."));
    assert!(!system.contains("SECRET-"));
    assert!(system.contains(&canon.arc.premise));
    assert!(system.contains("Chapter 1: The shipment's secret reaches the wrong person."));
    assert!(
        system.contains("written in English"),
        "the story's language, English by default"
    );
    assert!(system.contains("is written in Russian"), "the human's");
    canon.style.language = "Russian".into();
    assert_eq!(story_language(&canon), "Russian");
    assert!(author_system(&canon, "English").contains("written in Russian."));
}

#[test]
fn a_plan_is_shown_the_kept_scenes_and_offered_the_end_after_the_first() {
    let canon = fixture_canon();
    let goals = vec!["The secret surfaces.".to_owned()];
    let summaries = Vec::new();
    let view = ctx(&canon, &goals, &summaries);
    let world = WorldState::initial(&canon);
    let first = plan(&view, &[], &world, (1, 4), None);
    assert!(first.user.contains("Plan scene 1 of at most 4"));
    assert!(!first.user.contains("end the chapter"));
    assert!(first.user.contains("More rain."), "the human's notes");
    assert!(first.user.contains("Mara (`mara`) is at Harbour"));
    let t = trace();
    let setup = setup();
    let kept = [Kept {
        scene: 1,
        setup: &setup,
        intent: "Squeeze Mara.",
        trace: &t,
        played: true,
    }];
    let second = plan(&view, &kept, &world, (2, 4), None);
    assert!(second.user.contains("Or end the chapter here"));
    assert!(
        second.user.contains("Squeeze Mara."),
        "the Author's own intent"
    );
    assert!(
        second.user.contains("Silas knows too much."),
        "the reflections"
    );
    tokenless(&second);
    let last = plan(&view, &kept, &world, (4, 4), None);
    assert!(last.user.contains("the chapter's last"));
    let again = plan(
        &view,
        &[],
        &world,
        (1, 4),
        Some(Again::Failed("Put Ines in it.")),
    );
    assert!(
        again
            .user
            .contains("asked for it to be planned again: Put Ines in it.")
    );
    // A scene asked for again is played: the chapter cannot end at it.
    let failed = plan(&view, &kept, &world, (2, 4), Some(Again::Failed("x")));
    assert!(failed.user.contains("Plan scene 2 of at most 4"));
    assert!(!failed.user.contains("end the chapter"));
}

/// A scene the human asked at P2 to be played again is planned with what
/// they asked and its old setup beside it, and may not end the chapter
/// (docs/fabula.md §4, PR 10).
#[test]
fn a_scene_asked_for_again_is_planned_with_its_old_setup_and_the_request() {
    let canon = fixture_canon();
    let goals = vec!["The secret surfaces.".to_owned()];
    let summaries = Vec::new();
    let view = ctx(&canon, &goals, &summaries);
    let world = WorldState::initial(&canon);
    let old = setup();
    let asked = Again::Asked {
        notes: "The human asked for these changes:\n- Mara keeps quiet.",
        before: Some((&old, "Squeeze Mara.")),
    };
    let prompt = plan(&view, &[], &world, (2, 4), Some(asked));
    let user = &prompt.user;
    assert!(
        user.contains("asked for this scene to be played again"),
        "{user}"
    );
    assert!(
        user.contains("The scene as it was set before: at Harbour"),
        "{user}"
    );
    assert!(user.contains("Your intent for it then: Squeeze Mara."));
    assert!(
        user.contains("What they asked:\nThe human asked for these changes:\n- Mara keeps quiet.")
    );
    assert!(user.contains("Plan scene 2 of at most 4"));
    assert!(!user.contains("end the chapter"));
    tokenless(&prompt);
    let bare = plan(
        &view,
        &[],
        &world,
        (1, 4),
        Some(Again::Asked {
            notes: " ",
            before: None,
        }),
    );
    assert!(
        bare.user
            .contains("They asked for no change in particular.")
    );
    assert!(!bare.user.contains("as it was set before"));
}

#[test]
fn a_pick_reads_every_version_and_says_when_no_rerun_is_left() {
    let canon = fixture_canon();
    let goals = vec!["g".to_owned()];
    let summaries = Vec::new();
    let view = ctx(&canon, &goals, &summaries);
    let t = trace();
    let setup = setup();
    let shown = vec![("k0".to_owned(), &t), ("k2".to_owned(), &t)];
    let with = pick(&view, (1, &setup, "Squeeze Mara."), &shown, 2, None);
    assert!(with.user.contains("Version k0:") && with.user.contains("Version k2:"));
    assert!(with.user.contains("played it 2 times"));
    assert!(with.user.contains("(2 reruns left)"));
    assert!(
        with.user
            .contains("Mara thought: If they find the bribe, I am done.")
    );
    tokenless(&with);
    assert!(!with.user.contains("asked of this chapter"));
    let last = pick(&view, (1, &setup, "x"), &shown[..1], 0, None);
    assert!(last.user.contains("No reruns are left"));
    assert!(last.user.contains("played it 1 time "));
    // In a take, the pick weighs what the human asked of the chapter.
    let take = pick(
        &view,
        (2, &setup, "x"),
        &shown[..1],
        0,
        Some("Mara keeps quiet."),
    );
    assert!(
        take.user
            .contains("Your intent for it: x\nWhat the story's human author asked of this chapter:\nMara keeps quiet.")
    );
}

#[test]
fn the_draft_is_asked_in_the_storys_language_from_the_kept_traces() {
    let mut canon = fixture_canon();
    canon.style.language = "Russian".into();
    let goals = vec!["g".to_owned()];
    let summaries = vec![(1, "Earlier.".to_owned())];
    let mut view = ctx(&canon, &goals, &summaries);
    view.number = 2;
    view.previous = Some("## One\n\nThe last chapter.");
    let t = trace();
    let setup = setup();
    let kept = [Kept {
        scene: 1,
        setup: &setup,
        intent: "i",
        trace: &t,
        played: true,
    }];
    let draft = write(&view, &kept, (1200, 2500));
    assert!(draft.user.contains("Write in Russian"));
    assert!(draft.user.contains("between 1200 and 2500 words"));
    assert!(draft.user.contains("Chapter 1: Earlier."));
    assert!(draft.user.contains("The last chapter."));
    assert!(draft.user.contains("Goals now: Get rid of the ledger."));
    tokenless(&draft);
}

#[test]
fn the_reader_sees_the_published_text_and_nothing_of_the_making() {
    let first = reader(&[], "## One\n\nText.");
    assert!(first.user.contains("first chapter"));
    let later = reader(&[(1, "## One\n\nOld.".to_owned())], "## Two\n\nNew.");
    assert!(later.user.contains("Chapter 1:\n\n## One\n\nOld."));
    for prompt in [first, later] {
        for banned in ["bribe", "secret", "Secret", "intent", "record"] {
            assert!(
                !prompt.system.contains(banned) && !prompt.user.contains(banned),
                "{banned}"
            );
        }
    }
}

#[test]
fn the_checker_sees_the_record_with_its_ids_and_not_the_authors_intent() {
    let canon = fixture_canon();
    let t = trace();
    let setup = setup();
    let kept = [Kept {
        scene: 1,
        setup: &setup,
        intent: "Squeeze Mara.",
        trace: &t,
        played: true,
    }];
    let prompt = continuity(
        &canon,
        &WorldState::initial(&canon),
        &kept,
        "## One\n\nText.",
    );
    assert!(prompt.user.contains("`ch1/sc1/r0/k1/e2` Round 1 · Mara"));
    assert!(
        !prompt.user.contains("Squeeze Mara."),
        "the intent is the Author's"
    );
    assert!(
        prompt
            .system
            .contains("She took a bribe from the smuggler.")
    );
    assert!(
        prompt.user.contains("holds ledger; knows bribe"),
        "what each knew when it opened"
    );
    tokenless(&prompt);
}

#[test]
fn a_triage_and_a_revision_show_the_notes_by_label() {
    let canon = fixture_canon();
    let goals = vec!["g".to_owned()];
    let summaries = Vec::new();
    let view = ctx(&canon, &goals, &summaries);
    let note = ReviewNote {
        id: NoteId::new("ch1/v1/continuity/n1"),
        chapter: ChapterId::of(1, 1),
        reviewer: Reviewer::Continuity,
        note: NewNote {
            kind: NoteKind::KnowledgeLeak,
            severity: NoteSeverity::High,
            anchor: Some("Ines knew".into()),
            body: "She cannot know of the bribe.".into(),
            evidence: vec![EventId::new("ch1/sc1/r0/k1/e2")],
        },
    };
    let prompt = triage(&view, "## One", &[("n1".to_owned(), &note)]);
    assert!(prompt.user.contains(
        "n1 · continuity · knowledge_leak, high: \"Ines knew\": She cannot know of the bribe. \
         (events: ch1/sc1/r0/k1/e2)"
    ));
    let decided = Decided {
        note: note.id.clone(),
        decision: NoteDecision::Accepted,
        reason: "True.".into(),
    };
    let t = trace();
    let setup = setup();
    let kept = [Kept {
        scene: 1,
        setup: &setup,
        intent: "i",
        trace: &t,
        played: true,
    }];
    let revision = revise(&view, &kept, "## One\n\nOld.", &[(&note, &decided)], (1, 2));
    assert!(revision.user.contains("(your critique: True.)"));
    assert!(revision.user.contains("acting on every one of them"));
    let human = revise(&view, &kept, "## One\n\nOld.", &[], (1, 2));
    assert!(
        human
            .user
            .contains("as the story's human author asked above")
    );
    assert!(human.user.contains("More rain."));
}

/// The chapter's choices so far reach every Author prompt that plans,
/// picks or writes, each question once with its latest answer
/// (docs/fabula.md §4, *A chapter's choices stand*); the triage, which
/// decides notes, goes without.
#[test]
fn the_chapters_choices_reach_every_author_prompt_once_each() {
    let canon = fixture_canon();
    let goals = vec!["g".to_owned()];
    let summaries = Vec::new();
    let mut view = ctx(&canon, &goals, &summaries);
    let world = WorldState::initial(&canon);
    assert!(
        !plan(&view, &[], &world, (1, 3), None)
            .user
            .contains("These choices stand"),
        "no choices, no list"
    );
    let choices = [
        Assumption {
            doubt: "Who signed the order?".into(),
            choice: "The harbour master.".into(),
        },
        Assumption {
            doubt: "Is Ines new in town?".into(),
            choice: "Yes, a month.".into(),
        },
        Assumption {
            doubt: "Who signed the order?".into(),
            choice: "The mayor.".into(),
        },
    ];
    view.choices = &choices;
    let t = trace();
    let setup = setup();
    let kept = [Kept {
        scene: 1,
        setup: &setup,
        intent: "i",
        trace: &t,
        played: true,
    }];
    let shown = vec![("k0".to_owned(), &t)];
    let note = ReviewNote {
        id: NoteId::new("ch1/v1/continuity/n1"),
        chapter: ChapterId::of(1, 1),
        reviewer: Reviewer::Continuity,
        note: NewNote {
            kind: NoteKind::KnowledgeLeak,
            severity: NoteSeverity::High,
            anchor: None,
            body: "Slow.".into(),
            evidence: Vec::new(),
        },
    };
    let listed = "- Who signed the order? \u{2014} The mayor.\n- Is Ines new in town? \u{2014} \
                  Yes, a month.\nThese choices stand: plan, pick and write by them";
    let recorded = "These choices stand: plan, pick and write by them, and do not ask them \
                    again. Only what was played or what the human author asked can change one. \
                    A question they do not answer is a new one: record it among your \
                    assumptions.";
    let written = "These choices stand: plan, pick and write by them. Only what was played or \
                   what the human author asked can change one.";
    // The steps whose replies record assumptions are told to record every
    // other question; the draft and the revisions record none.
    for (prompt, close) in [
        (outline(&view), recorded),
        (plan(&view, &kept, &world, (2, 3), None), recorded),
        (pick(&view, (2, &setup, "i"), &shown, 1, None), recorded),
        (write(&view, &kept, (1, 2)), written),
        (revise(&view, &kept, "## One\n\nOld.", &[], (1, 2)), written),
    ] {
        assert!(prompt.user.contains(listed), "{}", prompt.user);
        assert!(prompt.user.contains(close), "{}", prompt.user);
        assert!(!prompt.user.contains("The harbour master."), "replaced");
        tokenless(&prompt);
    }
    let triaged = triage(&view, "## One\n\nOld.", &[("n1".to_owned(), &note)]);
    assert!(!triaged.user.contains("These choices stand"));
}

#[test]
fn the_craft_reaches_the_plans_the_draft_and_the_revisions() {
    let canon = fixture_canon();
    let system = author_system(&canon, "Russian");
    assert!(system.contains("A scene is a collision."));
    assert!(system.contains("name whose goals collide in it and what it may cost someone"));
    assert!(system.contains("No conflict ends without a price"));
    assert!(system.contains("keep the version where it is paid"));
    assert!(system.contains("Pace the reveals"));
    assert!(system.contains("every secret in one evening"));
    let goals = vec!["g".to_owned()];
    let summaries = Vec::new();
    let view = ctx(&canon, &goals, &summaries);
    let t = trace();
    let setup = setup();
    let kept = [Kept {
        scene: 1,
        setup: &setup,
        intent: "i",
        trace: &t,
        played: true,
    }];
    let lines = "act on a plan or a boundary rather than announce it, open no reply on thanks, \
                 apology or agreement the moment does not call for, and show a want in a deed";
    let draft = write(&view, &kept, (1, 2));
    let revision = revise(&view, &kept, "## One\n\nOld.", &[], (1, 2));
    for prompt in [&draft, &revision] {
        assert!(prompt.user.contains(lines));
        assert!(prompt.user.contains("as long as what happened stays"));
        assert!(prompt.system.contains("A scene is a collision."));
    }
    let world = WorldState::initial(&canon);
    let planned = plan(&view, &[], &world, (1, 3), None);
    assert!(planned.system.contains("No conflict ends without a price"));
    assert!(!planned.user.contains(lines), "the plan sets no lines");
}
