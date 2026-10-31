//! The fold and the witnesses on the harbour fixture
//! (docs/research/fabula-engine.md §6, docs/fabula.md §4 PR 1).

use super::*;
use crate::entities::story::{Cause, fixture_canon};

fn c(key: &str) -> CharacterKey {
    CharacterKey::from(key)
}

fn at(key: &str) -> LocationKey {
    LocationKey::from(key)
}

fn names(set: &BTreeSet<CharacterKey>) -> Vec<&str> {
    set.iter().map(CharacterKey::as_str).collect()
}

/// A public event at `location`, with no actor unless one is set.
fn event(kind: EventKind, location: &str) -> NewEvent {
    NewEvent {
        round: 1,
        tick: 1,
        location: at(location),
        kind,
        actor: None,
        visibility: Visibility::Public,
        observable: "Something happens.".into(),
        private: None,
        effects: Vec::new(),
        proposal: None,
        cause: Cause::World,
    }
}

fn deed(kind: EventKind, location: &str, actor: &str) -> NewEvent {
    NewEvent {
        actor: Some(c(actor)),
        cause: Cause::Initiative,
        ..event(kind, location)
    }
}

fn place(who: &str, to: &str) -> Effect {
    Effect::Place {
        character: c(who),
        to: at(to),
    }
}

/// The opening of a scene at the harbour: everyone is placed there.
fn everyone_at_the_harbour(canon: &Canon) -> WorldState {
    let opening = NewEvent {
        round: 0,
        effects: vec![place("silas", "harbour"), place("ines", "harbour")],
        cause: Cause::Setup,
        ..event(EventKind::Opening, "harbour")
    };
    let admitted = WorldState::initial(canon).admit(canon, &opening).unwrap();
    assert_eq!(names(&admitted.witnesses), ["ines", "mara", "silas"]);
    admitted.state
}

#[test]
fn the_initial_state_is_the_canons() {
    let canon = fixture_canon();
    let state = WorldState::initial(&canon);
    assert_eq!(state.position(&c("mara")), Some(&at("harbour")));
    assert_eq!(state.position(&c("ines")), Some(&at("lighthouse")));
    assert_eq!(
        state.holder(&ItemKey::from("ledger")),
        Some(&Holder::Character(c("mara")))
    );
    assert_eq!(
        state.holder(&ItemKey::from("crate")),
        Some(&Holder::Location(at("warehouse")))
    );
    assert!(state.knows(&c("mara"), &SecretKey::from("bribe")));
    assert!(!state.knows(&c("ines"), &SecretKey::from("bribe")));
    assert!(state.is_alive(&c("silas")));
    assert!(state.injuries(&c("silas")).is_empty());
    assert_eq!(state.tick(), 0);
    assert_eq!(names(&state.present_at(&at("inn"))), ["silas"]);
}

#[test]
fn a_public_event_is_perceived_by_everyone_present() {
    let canon = fixture_canon();
    let state = everyone_at_the_harbour(&canon);
    let speech = deed(EventKind::Speech, "harbour", "mara");
    let admitted = state.admit(&canon, &speech).unwrap();
    assert_eq!(names(&admitted.witnesses), ["ines", "mara", "silas"]);
    // Elsewhere, no one but the actor is there to see it.
    let alone = deed(EventKind::Speech, "lighthouse", "ines");
    let admitted = WorldState::initial(&canon).admit(&canon, &alone).unwrap();
    assert_eq!(names(&admitted.witnesses), ["ines"]);
}

#[test]
fn a_whisper_reaches_only_its_addressees() {
    let canon = fixture_canon();
    let state = everyone_at_the_harbour(&canon);
    let whisper = NewEvent {
        visibility: Visibility::Whisper {
            to: vec![c("silas")],
        },
        ..deed(EventKind::Speech, "harbour", "mara")
    };
    let admitted = state.admit(&canon, &whisper).unwrap();
    assert_eq!(names(&admitted.witnesses), ["mara", "silas"]);
}

#[test]
fn a_hidden_deed_is_perceived_by_its_actor_alone() {
    let canon = fixture_canon();
    let state = everyone_at_the_harbour(&canon);
    // Ines lifts the ledger from Mara's coat; Mara, its subject, does not notice.
    let theft = NewEvent {
        visibility: Visibility::Hidden,
        effects: vec![Effect::Take {
            by: c("ines"),
            item: ItemKey::from("ledger"),
            from: Some(c("mara")),
        }],
        ..deed(EventKind::Action, "harbour", "ines")
    };
    let admitted = state.admit(&canon, &theft).unwrap();
    assert_eq!(names(&admitted.witnesses), ["ines"]);
    assert_eq!(
        admitted.state.holder(&ItemKey::from("ledger")),
        Some(&Holder::Character(c("ines")))
    );
}

#[test]
fn a_move_is_seen_where_it_leaves_and_where_it_arrives() {
    let canon = fixture_canon();
    let state = WorldState::initial(&canon);
    let walk = NewEvent {
        effects: vec![Effect::Move {
            character: c("mara"),
            to: at("inn"),
        }],
        ..deed(EventKind::Movement, "harbour", "mara")
    };
    let admitted = state.admit(&canon, &walk).unwrap();
    assert_eq!(names(&admitted.witnesses), ["mara", "silas"]);
    assert_eq!(admitted.state.position(&c("mara")), Some(&at("inn")));
    assert_eq!(
        names(&admitted.state.present_at(&at("inn"))),
        ["mara", "silas"]
    );
}

#[test]
fn items_change_hands_only_as_the_state_allows() {
    let canon = fixture_canon();
    let state = everyone_at_the_harbour(&canon);
    let ledger = ItemKey::from("ledger");
    let give = NewEvent {
        effects: vec![Effect::Give {
            from: c("mara"),
            to: c("silas"),
            item: ledger.clone(),
        }],
        ..deed(EventKind::Action, "harbour", "mara")
    };
    let state = state.admit(&canon, &give).unwrap().state;
    assert_eq!(state.holder(&ledger), Some(&Holder::Character(c("silas"))));
    let drop = NewEvent {
        effects: vec![Effect::Drop {
            by: c("silas"),
            item: ledger.clone(),
        }],
        ..deed(EventKind::Action, "harbour", "silas")
    };
    let state = state.admit(&canon, &drop).unwrap().state;
    assert_eq!(
        state.holder(&ledger),
        Some(&Holder::Location(at("harbour")))
    );
    let pick_up = NewEvent {
        effects: vec![Effect::Take {
            by: c("ines"),
            item: ledger.clone(),
            from: None,
        }],
        ..deed(EventKind::Action, "harbour", "ines")
    };
    let state = state.admit(&canon, &pick_up).unwrap().state;
    assert_eq!(state.holder(&ledger), Some(&Holder::Character(c("ines"))));
    // Mara no longer holds it, so she cannot give it again.
    assert_eq!(
        state.admit(&canon, &give).unwrap_err(),
        Refusal::NotHolding {
            character: c("mara"),
            item: ledger.clone(),
        }
    );
    // And it no longer lies at the harbour.
    assert_eq!(
        state.admit(&canon, &pick_up).unwrap_err(),
        Refusal::NotHere {
            item: ledger,
            location: at("harbour"),
        }
    );
}

#[test]
fn a_reveal_teaches_the_secret_to_whoever_is_told() {
    let canon = fixture_canon();
    let state = everyone_at_the_harbour(&canon);
    let bribe = SecretKey::from("bribe");
    let confession = NewEvent {
        visibility: Visibility::Whisper {
            to: vec![c("ines")],
        },
        effects: vec![Effect::Reveal {
            secret: bribe.clone(),
            to: c("ines"),
        }],
        ..deed(EventKind::Speech, "harbour", "mara")
    };
    let admitted = state.admit(&canon, &confession).unwrap();
    assert_eq!(names(&admitted.witnesses), ["ines", "mara"]);
    assert!(admitted.state.knows(&c("ines"), &bribe));
    assert!(!admitted.state.knows(&c("silas"), &bribe));
}

#[test]
fn the_dead_neither_act_nor_perceive() {
    let canon = fixture_canon();
    let state = everyone_at_the_harbour(&canon);
    let wave = NewEvent {
        effects: vec![Effect::Injure {
            character: c("silas"),
            severity: Severity::Fatal,
        }],
        ..event(EventKind::WorldEvent, "harbour")
    };
    let admitted = state.admit(&canon, &wave).unwrap();
    assert_eq!(names(&admitted.witnesses), ["ines", "mara", "silas"]);
    let state = admitted.state;
    assert!(!state.is_alive(&c("silas")));
    assert_eq!(state.injuries(&c("silas")), &[Severity::Fatal]);
    assert_eq!(names(&state.present_at(&at("harbour"))), ["ines", "mara"]);
    let speech = deed(EventKind::Speech, "harbour", "mara");
    assert_eq!(
        names(&state.admit(&canon, &speech).unwrap().witnesses),
        ["ines", "mara"]
    );
    let ghost = deed(EventKind::Speech, "harbour", "silas");
    assert_eq!(
        state.admit(&canon, &ghost).unwrap_err(),
        Refusal::Dead(c("silas"))
    );
}

#[test]
fn effects_apply_in_order_against_the_state_the_earlier_ones_left() {
    let canon = fixture_canon();
    let state = everyone_at_the_harbour(&canon);
    // Mara walks off to the inn; handing the ledger over at the harbour after
    // that finds her gone.
    let event = NewEvent {
        effects: vec![
            Effect::Move {
                character: c("mara"),
                to: at("inn"),
            },
            Effect::Give {
                from: c("mara"),
                to: c("silas"),
                item: ItemKey::from("ledger"),
            },
        ],
        ..deed(EventKind::Movement, "harbour", "mara")
    };
    assert_eq!(
        state.admit(&canon, &event).unwrap_err(),
        Refusal::Absent {
            character: c("mara"),
            location: at("harbour"),
            actually: at("inn"),
        }
    );
}

#[test]
fn every_refusal_names_its_reason() {
    let canon = fixture_canon();
    let initial = WorldState::initial(&canon);
    let crowd = everyone_at_the_harbour(&canon);
    let with = |base: NewEvent, effects: Vec<Effect>| NewEvent { effects, ..base };
    let harbour_deed = || deed(EventKind::Action, "harbour", "mara");
    let cases: Vec<(&WorldState, NewEvent, Refusal)> = vec![
        (
            &initial,
            event(EventKind::WorldEvent, "moon"),
            Refusal::UnknownLocation(at("moon")),
        ),
        (
            &initial,
            deed(EventKind::Speech, "harbour", "nobody"),
            Refusal::UnknownCharacter(c("nobody")),
        ),
        (
            &initial,
            deed(EventKind::Speech, "inn", "mara"),
            Refusal::Absent {
                character: c("mara"),
                location: at("inn"),
                actually: at("harbour"),
            },
        ),
        (
            &initial,
            event(EventKind::Speech, "harbour"),
            Refusal::NoActor(EventKind::Speech),
        ),
        (
            &initial,
            NewEvent {
                actor: Some(c("mara")),
                ..event(EventKind::WorldEvent, "harbour")
            },
            Refusal::ActorOnWorldEvent(c("mara")),
        ),
        (
            &initial,
            NewEvent {
                visibility: Visibility::Hidden,
                ..event(EventKind::WorldEvent, "harbour")
            },
            Refusal::UnseenWithoutActor,
        ),
        (
            &initial,
            NewEvent {
                visibility: Visibility::Whisper {
                    to: vec![c("mara")],
                },
                ..event(EventKind::Opening, "harbour")
            },
            Refusal::UnseenWithoutActor,
        ),
        (
            &crowd,
            NewEvent {
                visibility: Visibility::Whisper { to: Vec::new() },
                ..harbour_deed()
            },
            Refusal::WhisperToNobody,
        ),
        (
            &crowd,
            NewEvent {
                visibility: Visibility::Whisper {
                    to: vec![c("mara")],
                },
                ..harbour_deed()
            },
            Refusal::WhisperToSelf(c("mara")),
        ),
        (
            &initial,
            NewEvent {
                visibility: Visibility::Whisper {
                    to: vec![c("ines")],
                },
                ..harbour_deed()
            },
            Refusal::Absent {
                character: c("ines"),
                location: at("harbour"),
                actually: at("lighthouse"),
            },
        ),
        (
            &crowd,
            NewEvent {
                tick: 0,
                ..harbour_deed()
            },
            Refusal::TimeRunsBackwards { tick: 0, last: 1 },
        ),
        (
            &initial,
            with(
                harbour_deed(),
                vec![Effect::Move {
                    character: c("mara"),
                    to: at("harbour"),
                }],
            ),
            Refusal::AlreadyThere {
                character: c("mara"),
                location: at("harbour"),
            },
        ),
        (
            &initial,
            with(
                harbour_deed(),
                vec![Effect::Move {
                    character: c("mara"),
                    to: at("chapel"),
                }],
            ),
            Refusal::NoRoute {
                from: at("harbour"),
                to: at("chapel"),
            },
        ),
        (
            &initial,
            with(
                harbour_deed(),
                vec![Effect::Move {
                    character: c("mara"),
                    to: at("moon"),
                }],
            ),
            Refusal::UnknownLocation(at("moon")),
        ),
        (
            &initial,
            with(harbour_deed(), vec![place("ines", "harbour")]),
            Refusal::PlaceOutsideOpening(EventKind::Action),
        ),
        (
            &crowd,
            with(
                harbour_deed(),
                vec![Effect::Give {
                    from: c("mara"),
                    to: c("mara"),
                    item: ItemKey::from("ledger"),
                }],
            ),
            Refusal::GiveToSelf(c("mara")),
        ),
        (
            &crowd,
            with(
                harbour_deed(),
                vec![Effect::Take {
                    by: c("mara"),
                    item: ItemKey::from("ledger"),
                    from: Some(c("mara")),
                }],
            ),
            Refusal::TakeFromSelf(c("mara")),
        ),
        (
            &crowd,
            with(
                harbour_deed(),
                vec![Effect::Take {
                    by: c("mara"),
                    item: ItemKey::from("rifle"),
                    from: None,
                }],
            ),
            Refusal::UnknownItem(ItemKey::from("rifle")),
        ),
        (
            &crowd,
            with(
                harbour_deed(),
                vec![Effect::Drop {
                    by: c("mara"),
                    item: ItemKey::from("logbook"),
                }],
            ),
            Refusal::NotHolding {
                character: c("mara"),
                item: ItemKey::from("logbook"),
            },
        ),
        (
            &crowd,
            with(
                harbour_deed(),
                vec![Effect::Reveal {
                    secret: SecretKey::from("treasure"),
                    to: c("ines"),
                }],
            ),
            Refusal::UnknownSecret(SecretKey::from("treasure")),
        ),
        (
            &initial,
            with(
                harbour_deed(),
                vec![Effect::Injure {
                    character: c("ines"),
                    severity: Severity::Minor,
                }],
            ),
            Refusal::Absent {
                character: c("ines"),
                location: at("harbour"),
                actually: at("lighthouse"),
            },
        ),
    ];
    for (state, event, expected) in cases {
        assert_eq!(
            state.admit(&canon, &event).unwrap_err(),
            expected,
            "{event:?}"
        );
    }
}

#[test]
fn a_placement_needs_a_way_there() {
    let mut canon = fixture_canon();
    // The chapel loses its only way out: it can be reached, never left.
    canon.locations[3].routes.clear();
    let to_chapel = NewEvent {
        round: 0,
        effects: vec![place("mara", "chapel")],
        ..event(EventKind::Opening, "chapel")
    };
    let state = WorldState::initial(&canon)
        .admit(&canon, &to_chapel)
        .unwrap()
        .state;
    let back = NewEvent {
        round: 0,
        effects: vec![place("mara", "harbour")],
        ..event(EventKind::Settle, "harbour")
    };
    assert_eq!(
        state.admit(&canon, &back).unwrap_err(),
        Refusal::Unreachable {
            from: at("chapel"),
            to: at("harbour"),
        }
    );
    // Placing someone where they already are changes nothing and is allowed.
    let stay = NewEvent {
        effects: vec![place("mara", "chapel")],
        ..back
    };
    assert!(state.admit(&canon, &stay).is_ok());
}

#[test]
fn refusals_read_as_sentences() {
    let texts: Vec<String> = [
        Refusal::NoActor(EventKind::Movement),
        Refusal::PlaceOutsideOpening(EventKind::WorldEvent),
        Refusal::Absent {
            character: c("mara"),
            location: at("inn"),
            actually: at("harbour"),
        },
        Refusal::TimeRunsBackwards { tick: 2, last: 5 },
        Refusal::UnknownEvent(EventId::from("genesis/e9")),
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        texts,
        [
            "a movement event needs an actor",
            "a character can be placed only by an opening or settle event, not by a world_event",
            "mara is at harbour, not at inn",
            "tick 2 is before the branch's last tick, 5",
            "event genesis/e9 is not on this branch",
        ]
    );
}

fn stored(n: u64, body: NewEvent) -> Event {
    Event {
        seq: n,
        id: EventId::nth(&crate::entities::story::BranchId::genesis(), n),
        branch: crate::entities::story::BranchId::genesis(),
        body,
        witnesses: BTreeSet::new(),
    }
}

#[test]
fn a_fold_replays_a_path_and_names_an_event_that_no_longer_folds() {
    let canon = fixture_canon();
    let walk = NewEvent {
        effects: vec![Effect::Move {
            character: c("mara"),
            to: at("warehouse"),
        }],
        ..deed(EventKind::Movement, "harbour", "mara")
    };
    let pick = NewEvent {
        tick: 2,
        effects: vec![Effect::Take {
            by: c("mara"),
            item: ItemKey::from("crate"),
            from: None,
        }],
        ..deed(EventKind::Action, "warehouse", "mara")
    };
    let path = [stored(1, walk), stored(2, pick)];
    let state = WorldState::fold(&canon, &path).unwrap();
    assert_eq!(state.position(&c("mara")), Some(&at("warehouse")));
    assert_eq!(
        state.holder(&ItemKey::from("crate")),
        Some(&Holder::Character(c("mara")))
    );
    assert_eq!(state.tick(), 2);
    // The same path under a canon whose harbour lost its warehouse route.
    let mut changed = canon.clone();
    changed.locations[0]
        .routes
        .retain(|r| r.to != at("warehouse"));
    let err = WorldState::fold(&changed, &path).unwrap_err();
    assert_eq!(err.event.as_str(), "genesis/e1");
    assert_eq!(
        err.to_string(),
        "stored event genesis/e1 no longer folds: there is no direct route from harbour to warehouse"
    );
}
