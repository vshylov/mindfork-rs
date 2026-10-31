use std::collections::VecDeque;

use serde_json::{Value, json};

use super::{
    Answer, Attempt, Attempts, CallAddress, LlmPort, PortError, Prompt, Role, decide,
    prompt_sha256, run_json, run_text,
};
use crate::entities::story::{BranchId, Proposal, RunId, SceneId, fixture_canon};
use crate::features::story::schema::{CanonKeys, Property, Schema, StorySchema};
use crate::shared::storage::story::CallStatus;

/// Attempts answered from a script, every request kept.
struct Script {
    answers: VecDeque<Answer>,
    seen: Vec<Attempt>,
}

impl Script {
    fn new(answers: impl IntoIterator<Item = Answer>) -> Self {
        Self {
            answers: answers.into_iter().collect(),
            seen: Vec::new(),
        }
    }
}

#[async_trait::async_trait]
impl Attempts for Script {
    async fn attempt(&mut self, attempt: &Attempt) -> Result<Answer, PortError> {
        self.seen.push(attempt.clone());
        Ok(self
            .answers
            .pop_front()
            .expect("the policy asked for more attempts than scripted"))
    }
}

fn answer(status: CallStatus, text: &str) -> Answer {
    Answer {
        status,
        text: text.to_owned(),
        error: None,
        transient: false,
        cancelled: false,
    }
}

fn ok(text: &str) -> Answer {
    answer(CallStatus::Ok, text)
}

fn failure(message: &str, transient: bool) -> Answer {
    Answer {
        error: Some(message.to_owned()),
        transient,
        ..answer(CallStatus::Error, "")
    }
}

/// A one-field schema.
pub(crate) fn schema() -> Schema {
    Schema::Object(vec![Property::new(
        "mood",
        Schema::Enum(vec!["calm".to_owned(), "grim".to_owned()]),
    )])
}

/// A two-line prompt.
pub(crate) fn prompt() -> Prompt {
    Prompt {
        system: "You are Mara.".to_owned(),
        user: "The gate closes.".to_owned(),
    }
}

async fn json_call(script: &mut Script) -> Result<Value, PortError> {
    run_json(
        script,
        "ch1/sc1/r0/k0/round1/mara/propose",
        &schema(),
        &prompt(),
    )
    .await
}

#[tokio::test]
async fn a_valid_reply_is_read_leniently_at_the_first_attempt() {
    let mut script = Script::new([ok("```json\n{\"mood\": \"Grim\"}\n```")]);
    assert_eq!(
        json_call(&mut script).await.unwrap(),
        json!({"mood": "grim"})
    );
    let [first] = script.seen.as_slice() else {
        panic!("{:?}", script.seen)
    };
    assert_eq!(first.number, 1);
    assert!(
        first
            .system
            .starts_with("You are Mara.\n\nReply with one JSON object")
    );
    assert!(first.system.ends_with(&schema().to_json()));
    assert_eq!(first.turns.len(), 1);
    assert_eq!(first.turns[0].content, "The gate closes.");
}

#[tokio::test]
async fn an_invalid_reply_is_repaired_once_quoting_the_validator() {
    let mut script = Script::new([ok("{\"mood\": \"sly\"}"), ok("{\"mood\": \"calm\"}")]);
    assert_eq!(
        json_call(&mut script).await.unwrap(),
        json!({"mood": "calm"})
    );
    let repair = &script.seen[1];
    assert_eq!(repair.number, 2);
    assert_eq!(repair.system, script.seen[0].system);
    let turns: Vec<(&str, &str)> = repair
        .turns
        .iter()
        .map(|t| (t.role, t.content.as_str()))
        .collect();
    assert_eq!(
        turns,
        [
            ("user", "The gate closes."),
            ("assistant", "{\"mood\": \"sly\"}"),
            (
                "user",
                "That reply does not match the schema:\n\
                 - $.mood: \"sly\" is not one of \"calm\", \"grim\"\n\
                 Reply again with only the corrected JSON object."
            ),
        ]
    );
}

#[tokio::test]
async fn a_second_invalid_reply_fails_the_call() {
    let mut script = Script::new([ok("Mara frowns."), ok("{\"mood\": 1}")]);
    let error = json_call(&mut script).await.unwrap_err();
    assert!(
        script.seen[1].turns[2]
            .content
            .contains("$: no JSON object was found in the reply")
    );
    match error {
        PortError::Invalid {
            address,
            violations,
        } => {
            assert_eq!(address, "ch1/sc1/r0/k0/round1/mara/propose");
            assert_eq!(violations, "- $.mood: expected text, found a number");
        }
        other => panic!("{other}"),
    }
}

#[tokio::test]
async fn a_cut_or_a_transient_failure_is_tried_once_more_with_the_same_request() {
    let mut script = Script::new([answer(CallStatus::Cut, "{\"mo"), ok("{\"mood\": \"calm\"}")]);
    assert!(json_call(&mut script).await.is_ok());
    assert_eq!(script.seen[1].number, 2);
    assert_eq!(script.seen[1].turns, script.seen[0].turns);

    let mut script = Script::new([failure("overloaded", true), ok("{\"mood\": \"calm\"}")]);
    assert!(json_call(&mut script).await.is_ok());

    // The retry and the repair are separate allowances.
    let mut script = Script::new([
        answer(CallStatus::Cut, ""),
        ok("{}"),
        ok("{\"mood\": \"calm\"}"),
    ]);
    assert!(json_call(&mut script).await.is_ok());
    assert_eq!(script.seen.len(), 3);
}

#[tokio::test]
async fn a_cut_twice_a_refusal_or_a_lasting_failure_ends_the_call() {
    let mut script = Script::new([answer(CallStatus::Cut, ""), answer(CallStatus::Cut, "")]);
    assert!(matches!(
        json_call(&mut script).await,
        Err(PortError::Cut { .. })
    ));

    let mut script = Script::new([answer(CallStatus::Refused, "")]);
    assert!(matches!(
        json_call(&mut script).await,
        Err(PortError::Refused { .. })
    ));
    assert_eq!(script.seen.len(), 1);

    let mut script = Script::new([failure("400 bad request", false)]);
    match json_call(&mut script).await {
        Err(PortError::Failed { message, .. }) => assert_eq!(message, "400 bad request"),
        other => panic!("{other:?}"),
    }

    let mut script = Script::new([failure("overloaded", true), failure("overloaded", true)]);
    assert!(matches!(
        json_call(&mut script).await,
        Err(PortError::Failed { .. })
    ));
    assert_eq!(script.seen.len(), 2);
}

#[tokio::test]
async fn a_cancelled_attempt_is_never_retried() {
    let cancelled = Answer {
        cancelled: true,
        ..failure("cancelled", false)
    };
    let mut script = Script::new([cancelled.clone()]);
    assert!(matches!(
        json_call(&mut script).await,
        Err(PortError::Cancelled { .. })
    ));
    let mut script = Script::new([cancelled]);
    assert!(matches!(
        run_text(&mut script, "a", &prompt()).await,
        Err(PortError::Cancelled { .. })
    ));
}

#[tokio::test]
async fn a_text_call_retries_an_empty_reply_once() {
    let mut script = Script::new([ok("Chapter one.")]);
    assert_eq!(
        run_text(&mut script, "a", &prompt()).await.unwrap(),
        "Chapter one."
    );
    assert_eq!(script.seen[0].system, "You are Mara.");

    let mut script = Script::new([ok("  \n"), ok("Chapter one.")]);
    assert!(run_text(&mut script, "a", &prompt()).await.is_ok());
    assert_eq!(script.seen.len(), 2);

    let mut script = Script::new([ok(""), ok("")]);
    match run_text(&mut script, "a", &prompt()).await {
        Err(PortError::Failed { message, .. }) => assert_eq!(message, "the reply was empty"),
        other => panic!("{other:?}"),
    }

    let mut script = Script::new([answer(CallStatus::Refused, "")]);
    assert!(matches!(
        run_text(&mut script, "a", &prompt()).await,
        Err(PortError::Refused { .. })
    ));
}

#[test]
fn an_answer_reads_back_as_recorded() {
    let answer = Answer {
        transient: true,
        cancelled: false,
        ..failure("overloaded", true)
    };
    let back = Answer::from_json(CallStatus::Error, Some(&answer.to_json()));
    assert_eq!(back, answer);
    assert_eq!(Answer::from_json(CallStatus::Ok, None), ok(""));
}

#[test]
fn the_prompt_hash_covers_both_parts_and_the_schema() {
    let base = prompt_sha256(&prompt(), Some(&schema()));
    assert_eq!(base.len(), 64);
    assert_eq!(base, prompt_sha256(&prompt(), Some(&schema())));
    let mut other = prompt();
    other.user.push('!');
    assert_ne!(base, prompt_sha256(&other, Some(&schema())));
    let mut other = prompt();
    other.system.push('!');
    assert_ne!(base, prompt_sha256(&other, Some(&schema())));
    assert_ne!(base, prompt_sha256(&prompt(), None));
    let wider = Schema::Object(vec![Property::new("mood", Schema::String)]);
    assert_ne!(base, prompt_sha256(&prompt(), Some(&wider)));
}

#[test]
fn a_character_call_is_addressed_by_branch_round_and_act() {
    let scene = SceneId::of(1, 2, 0);
    let call = CallAddress::character(
        &RunId::nth(3),
        &BranchId::of_scene(&scene, 1),
        4,
        &"mara".into(),
        "propose",
    );
    assert_eq!(call.run.as_str(), "run3");
    assert_eq!(call.address, "ch1/sc2/r0/k1/round4/mara/propose");
    let roles = [
        Role::Character,
        Role::WorldMaster,
        Role::Author,
        Role::Reader,
        Role::Continuity,
        Role::CanonKeeper,
    ];
    let names: std::collections::BTreeSet<&str> = roles.iter().map(|r| r.as_str()).collect();
    assert_eq!(names.len(), roles.len());
}

/// A port that answers every JSON call with one value.
struct Fixed(Value);

#[async_trait::async_trait]
impl LlmPort for Fixed {
    async fn json(
        &self,
        _: &CallAddress,
        _: Role,
        _: &Schema,
        _: &Prompt,
    ) -> Result<Value, PortError> {
        Ok(self.0.clone())
    }

    async fn text(&self, _: &CallAddress, _: Role, _: &Prompt) -> Result<String, PortError> {
        Ok(String::new())
    }
}

#[tokio::test]
async fn a_value_its_type_cannot_read_is_an_invalid_reply() {
    let call = CallAddress {
        run: RunId::nth(1),
        address: "a".to_owned(),
    };
    let schema = Proposal::schema(&CanonKeys::of(&fixture_canon()));
    let port = Fixed(json!({"mood": "calm"}));
    let error = decide::<Proposal>(&port, &call, Role::Character, &schema, &prompt())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, PortError::Invalid { violations, .. } if violations.starts_with("- $: unknown field `mood`")),
        "{error}"
    );
}
