use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::{
    AuditedCall, BranchId, Cause, Effect, Event, EventKind, Phase, SecretKey, WorldState, audit,
    secrets_in, token_of, unknown_secrets,
};
use crate::entities::story::{EventId, NewEvent, Visibility, fixture_canon};

#[test]
fn a_token_is_the_bracketed_secret_run() {
    assert_eq!(
        token_of("She took a bribe. [SECRET-MARA-7Q]"),
        Some("SECRET-MARA-7Q")
    );
    assert_eq!(token_of("no token here"), None);
    assert_eq!(token_of("[SECRET-]"), None);
    assert_eq!(token_of("[SECRET-OPEN"), None);
    assert_eq!(token_of("[NOTE-1] [SECRET-X]"), Some("SECRET-X"));
}

#[test]
fn secrets_are_found_by_token_in_any_case_and_known_by_the_world() {
    let canon = fixture_canon();
    let found = secrets_in(&canon, "she said secret-silas-3k and SECRET-MARA-7Q aloud");
    let keys: Vec<&str> = found.iter().map(|s| s.as_str()).collect();
    assert_eq!(keys, ["bribe", "rifles"]);
    assert_eq!(secrets_in(&canon, "SECRET-MARA"), Vec::<&SecretKey>::new());
    let world = WorldState::initial(&canon);
    let text = "SECRET-MARA-7Q, SECRET-SILAS-3K";
    assert_eq!(
        unknown_secrets(&canon, &world, &"mara".into(), text),
        [SecretKey::from("rifles")]
    );
    assert_eq!(
        unknown_secrets(&canon, &world, &"ines".into(), text),
        [SecretKey::from("bribe"), SecretKey::from("rifles")]
    );
}

fn event(branch: &str, n: u64, round: u32, cause: Cause, effects: Vec<Effect>) -> Event {
    let branch = BranchId::new(branch);
    Event {
        seq: n,
        id: EventId::nth(&branch, n),
        branch,
        body: NewEvent {
            round,
            tick: round,
            location: "harbour".into(),
            kind: EventKind::Speech,
            actor: Some("mara".into()),
            visibility: Visibility::Public,
            observable: "Mara speaks.".to_owned(),
            private: None,
            effects,
            proposal: None,
            cause,
        },
        witnesses: BTreeSet::new(),
    }
}

fn reveal(secret: &str, to: &str) -> Effect {
    Effect::Reveal {
        secret: secret.into(),
        to: to.into(),
    }
}

#[test]
fn the_audit_finds_a_token_seen_before_its_secret_was_known() {
    let canon = fixture_canon();
    let b = "ch1/sc1/r0/k0";
    // Mara tells Silas her secret as round 2's initiative, and Ines as a
    // reaction in round 3; an earlier branch of the path told Ines Silas's.
    let path = vec![
        event(
            "genesis",
            1,
            0,
            Cause::Settle,
            vec![reveal("rifles", "ines")],
        ),
        event(b, 2, 2, Cause::Initiative, vec![reveal("bribe", "silas")]),
        event(
            b,
            3,
            3,
            Cause::Reaction {
                to: EventId::new("x"),
            },
            vec![reveal("bribe", "ines")],
        ),
    ];
    let paths = BTreeMap::from([(BranchId::new(b), path)]);
    let request =
        |text: &str| json!({"system": "", "messages": [{"role": "user", "content": text}]});
    let mara_token = "[SECRET-MARA-7Q]";
    let cases: Vec<(String, Value, Option<Value>)> = vec![
        // Her own token: never a leak.
        (
            format!("{b}/round1/mara/propose"),
            request(mara_token),
            None,
        ),
        // Silas before the reveal, and in the same round's intents: leaks.
        (
            format!("{b}/round1/silas/propose"),
            request(mara_token),
            None,
        ),
        (
            format!("{b}/round2/silas/propose"),
            request(mara_token),
            None,
        ),
        // After the round's initiative: known.
        (format!("{b}/round2/silas/react"), request(mara_token), None),
        // Ines learns it from a reaction in round 3: her reaction call came
        // before it, her reflection after.
        (format!("{b}/round3/ines/react"), request(mara_token), None),
        (
            format!("{b}/round3/ines/reflect"),
            request(mara_token),
            None,
        ),
        // Silas's secret reached Ines before this branch forked.
        (
            format!("{b}/round1/ines/propose"),
            request("[SECRET-SILAS-3K]"),
            None,
        ),
        // A reply that writes a token its writer cannot know.
        (
            format!("{b}/round1/silas/propose"),
            request("nothing"),
            Some(json!({"text": "I know SECRET-INES-9D"})),
        ),
        // Not audited: the World Master, a branch with no path, an address
        // that is no character call.
        (
            format!("{b}/round1/world_master/initiative"),
            request(mara_token),
            None,
        ),
        (
            "ch9/sc1/r0/k0/round1/silas/propose".to_owned(),
            request(mara_token),
            None,
        ),
        (format!("{b}/round1/silas/dream"), request(mara_token), None),
    ];
    let calls: Vec<AuditedCall> = cases
        .iter()
        .map(|(address, request, response)| AuditedCall {
            address,
            role: if address.contains("world_master") {
                "world_master"
            } else {
                "character"
            },
            request,
            response: response.as_ref(),
        })
        .collect();
    let leaks = audit(&canon, &paths, &calls);
    let found: Vec<String> = leaks.iter().map(ToString::to_string).collect();
    assert_eq!(
        found,
        [
            format!("{b}/round1/silas/propose: silas saw `bribe` in its request before knowing it"),
            format!("{b}/round2/silas/propose: silas saw `bribe` in its request before knowing it"),
            format!("{b}/round3/ines/react: ines saw `bribe` in its request before knowing it"),
            format!("{b}/round1/silas/propose: silas saw `signal` in its reply before knowing it"),
        ]
    );
}

#[test]
fn phases_order_a_round() {
    assert!(Phase::Opening < Phase::Propose);
    assert!(Phase::Propose < Phase::Initiative);
    assert!(Phase::Initiative < Phase::React);
    assert!(Phase::React < Phase::Reaction);
    assert!(Phase::Reaction < Phase::Reflect);
    assert_eq!(Phase::of_act("react"), Some(Phase::React));
    assert_eq!(Phase::of_act("initiative"), None);
    let opening = event("b", 1, 0, Cause::Setup, vec![]);
    assert_eq!(Phase::of_event(&opening), Phase::Opening);
    let world = event("b", 1, 2, Cause::World, vec![]);
    assert_eq!(Phase::of_event(&world), Phase::Initiative);
}
