use std::collections::BTreeSet;

use super::*;
use crate::entities::story::{Cause, EventId, EventKind, NewEvent, fixture_canon};

fn event(branch: &BranchId, n: u64, actor: &str, visibility: Visibility, text: &str) -> Event {
    Event {
        seq: n,
        id: EventId::nth(branch, n),
        branch: branch.clone(),
        body: NewEvent {
            round: 1,
            tick: 1,
            location: "harbour".into(),
            kind: EventKind::Speech,
            actor: Some(actor.into()),
            visibility,
            observable: text.to_owned(),
            private: Some(format!("{actor}'s own thought")),
            effects: vec![Effect::Reveal {
                secret: "bribe".into(),
                to: "ines".into(),
            }],
            proposal: None,
            cause: Cause::Initiative,
        },
        witnesses: BTreeSet::from(["ines".into(), "mara".into()]),
    }
}

fn reflection(body: &str, goals: &[&str]) -> Reflection {
    Reflection {
        id: "r1".to_owned(),
        branch: BranchId::new("b"),
        after_seq: 1,
        body: body.to_owned(),
        goal_changes: goals.iter().map(|g| (*g).to_owned()).collect(),
    }
}

#[test]
fn a_character_is_told_its_own_sheet_and_the_others_as_everyone_knows_them() {
    let canon = fixture_canon();
    let system = character_system(&canon, &"silas".into());
    assert!(system.starts_with("You play Silas in a story."));
    assert!(system.contains("How to play:"));
    assert!(system.contains("Get the crates aboard before dawn"));
    assert!(system.contains("[SECRET-SILAS-3K]"));
    assert!(system.contains("Mara (`mara`): The harbour master"));
    assert!(!system.contains("SECRET-MARA"));
    assert!(
        !system.contains("Keep tonight's shipment off the books"),
        "another's goal"
    );
}

#[test]
fn the_moment_holds_what_the_character_perceived_learned_and_reflected() {
    let canon = fixture_canon();
    let branch = BranchId::new("ch1/sc1/r0/k0");
    let mut world = WorldState::initial(&canon);
    let reveal = NewEvent {
        round: 0,
        tick: 0,
        location: "lighthouse".into(),
        kind: EventKind::Opening,
        actor: None,
        visibility: Visibility::Public,
        observable: "x".to_owned(),
        private: None,
        effects: vec![
            Effect::Reveal {
                secret: "bribe".into(),
                to: "ines".into(),
            },
            Effect::Injure {
                character: "ines".into(),
                severity: Severity::Minor,
            },
        ],
        proposal: None,
        cause: Cause::Setup,
    };
    world = world.admit(&canon, &reveal).unwrap().state;
    let memory = [
        event(
            &branch,
            1,
            "mara",
            Visibility::Whisper {
                to: vec!["ines".into()],
            },
            "Mara whispers.",
        ),
        event(
            &branch,
            2,
            "ines",
            Visibility::Hidden,
            "Ines pockets the note.",
        ),
        event(
            &branch,
            3,
            "ines",
            Visibility::Whisper {
                to: vec!["mara".into()],
            },
            "Ines whispers back.",
        ),
    ];
    let reflections = [
        reflection("She lies.", &[]),
        reflection("I must know.", &["Learn the truth"]),
    ];
    let ines = CharacterKey::from("ines");
    let view = CharacterView {
        canon: &canon,
        world: &world,
        who: &ines,
        round: 2,
        memory: &memory,
        reflections: &reflections,
        breaks_routine: true,
    };
    let prompt = character_propose(&view);
    let user = &prompt.user;
    assert!(
        user.starts_with("Round 2. You are at Lighthouse, with no one else. You carry: logbook.")
    );
    assert!(user.contains("You are hurt: minor."));
    assert!(user.contains("Mara's secret: She took a bribe from the smuggler. [SECRET-MARA-7Q]"));
    assert!(!user.contains("SECRET-SILAS"));
    assert!(user.contains("- She lies.\n- I must know. (Your goals now: Learn the truth)"));
    assert!(user.contains("- (Whispered to you) Mara whispers."));
    assert!(user.contains(
        "- (Unseen by others) Ines pockets the note.\n  (You thought: ines's own thought)"
    ));
    assert!(user.contains("- (You whispered) Ines whispers back."));
    assert!(!user.contains("mara's own thought"));
    assert!(user.contains("This round Ines breaks their routine"));
    assert!(user.ends_with("Write the inner thought first, then the action."));

    let quiet = CharacterView {
        breaks_routine: false,
        memory: &[],
        reflections: &[],
        ..view
    };
    let prompt = character_reflect(&quiet);
    assert!(prompt.user.contains("- nothing yet"));
    assert!(!prompt.user.contains("breaks their routine"));
    assert!(prompt.user.contains("The scene pauses."));
    let prompt = character_react(&quiet, &memory[0]);
    assert!(prompt.user.contains("Just now: Mara whispers."));
    assert!(prompt.user.ends_with("wait if Ines lets it pass."));
}

#[test]
fn the_world_master_is_shown_the_world_and_the_deeds_but_no_thought() {
    let canon = fixture_canon();
    let system = master_system(&canon);
    assert!(system.contains("Harbour (`harbour`): Stone quays"));
    assert!(
        system.contains("Routes to: `inn` (1 tick), `lighthouse` (2 ticks), `warehouse` (1 tick).")
    );
    assert!(system.contains("`rifles`: The crates hold rifles, not wine. [SECRET-SILAS-3K]"));
    assert!(system.contains("The items: lantern, crate, ledger, warehouse_key, logbook."));

    let branch = BranchId::new("ch1/sc1/r0/k0");
    let world = WorldState::initial(&canon);
    let setup = SceneSetup {
        location: "harbour".into(),
        present: vec!["mara".into(), "silas".into()],
        just_happened: "The ferry is cancelled.".to_owned(),
        pressure: "The gate closes soon.".to_owned(),
    };
    let path = [event(
        &branch,
        1,
        "mara",
        Visibility::Public,
        "Mara says: \"Go home.\"",
    )];
    let view = MasterView {
        canon: &canon,
        setup: &setup,
        world: &world,
        branch: &branch,
        path: &path,
        round: 1,
    };
    let proposal: Proposal = serde_json::from_value(serde_json::json!({
        "private_thought": "THE-HIDDEN-THOUGHT",
        "action": {"type": "act", "verb": "taps", "object": "the ledger", "line": "Sign here."},
        "targets": ["mara"],
        "visibility": {"type": "whisper", "to": ["mara"]}
    }))
    .unwrap();
    let silas = CharacterKey::from("silas");
    let shown = [Shown {
        who: &silas,
        proposal: &proposal,
        perceived: vec!["e1".to_owned()],
    }];
    let prompt = master_initiative(&view, &shown);
    let user = &prompt.user;
    assert!(user.starts_with("The scene: Harbour (harbour), with Mara, Silas."));
    assert!(user.contains("- `mara` is at Harbour; holds ledger; knows bribe"));
    assert!(user.contains("- at `inn`: lantern"));
    assert!(user.contains(
        "- e1 · round 1 · speech by `mara`, openly: Mara says: \"Go home.\" [Ines learns `bribe`] Perceived by: ines, mara."
    ));
    assert!(user.contains(
        "- `silas` taps the ledger, saying \"Sign here.\", aimed at mara, whispered to Mara. They perceived: e1."
    ));
    assert!(!user.contains("THE-HIDDEN-THOUGHT"));
    assert!(!user.contains("own thought"));
    assert!(user.ends_with("and whether the scene is over."));

    let none = master_initiative(&view, &[]);
    assert!(none.user.contains("- none"));
    assert!(none.user.contains("Every character waits this round."));
    let reactions = master_reactions(&view, &path[0], &shown);
    assert!(
        reactions
            .user
            .contains("The initiative this round was e1. The reactions to it:")
    );
    assert_eq!(
        reactions.system, prompt.system,
        "one stable prefix for both rulings"
    );
}

#[test]
fn an_event_is_labelled_by_its_ordinal_on_its_own_branch() {
    let branch = BranchId::new("ch1/sc2/r0/k1");
    let own = event(&branch, 4, "mara", Visibility::Public, "x");
    assert_eq!(label(&own, &branch), "e4");
    let earlier = event(
        &BranchId::new("ch1/sc1/r0/k0"),
        2,
        "mara",
        Visibility::Public,
        "x",
    );
    assert_eq!(label(&earlier, &branch), "ch1/sc1/r0/k0/e2");
}

#[test]
fn the_cast_here_is_the_living_at_the_place() {
    let canon = fixture_canon();
    let mut world = WorldState::initial(&canon);
    let here = LocationKey::from("harbour");
    assert_eq!(
        present_here(&canon, &world, &here),
        [CharacterKey::from("mara")]
    );
    let killing = NewEvent {
        round: 1,
        tick: 1,
        location: "harbour".into(),
        kind: EventKind::WorldEvent,
        actor: None,
        visibility: Visibility::Public,
        observable: "A beam falls.".to_owned(),
        private: None,
        effects: vec![Effect::Injure {
            character: "mara".into(),
            severity: Severity::Fatal,
        }],
        proposal: None,
        cause: Cause::World,
    };
    world = world.admit(&canon, &killing).unwrap().state;
    assert!(present_here(&canon, &world, &here).is_empty());
}
