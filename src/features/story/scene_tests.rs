//! The scripted scene and Gate G0 (docs/fabula.md §4, PR 2): a scene
//! recorded through the engine adapter replays identically from its call
//! log, and so does the fixture recorded against a live model. The helpers
//! here serve the adapters' tests and the live smoke too.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::{
    NOT_TAKEN, SceneError, SceneOutcome, SceneScript, WAITS, character_prompt, harbour_script,
    play_scripted_scene,
};
use crate::entities::sampling::SamplingConfig;
use crate::entities::story::{CharacterKey, RunId, WorldState, fixture_canon};
use crate::features::story::engine::EngineAdapter;
use crate::features::story::port::PortError;
use crate::features::story::replay::{Drift, ReplayAdapter};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason,
};
use crate::shared::storage::story::{
    AuthorWriter, CallStatus, CallUsage, Mechanism, NewCall, RunLogWriter, StoredCall, StoryDb,
};

/// An open story with the harbour canon and a started run.
pub(crate) struct Story {
    /// Keeps the files alive.
    pub dir: TempDir,
    /// The story.
    pub db: StoryDb,
    /// The Author's capability.
    pub author: AuthorWriter,
    /// The run started.
    pub run: RunId,
}

/// A story in a directory of its own, with the harbour canon imported and a
/// run started; the run log's capability beside it.
pub(crate) async fn fresh_story() -> (Story, RunLogWriter) {
    let dir = tempfile::tempdir().unwrap();
    let db = StoryDb::open(
        &dir.path().join("story.db"),
        &dir.path().join("story.calls.db"),
    )
    .await
    .unwrap();
    let mut author = db.author_writer().await.unwrap();
    author.import_canon(&fixture_canon()).await.unwrap();
    let mut log = db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({"scene": "harbour"})).await.unwrap();
    (
        Story {
            dir,
            db,
            author,
            run,
        },
        log,
    )
}

/// A scene recorded from a model: its script, every attempt in its call log,
/// and the trace it left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SceneFixture {
    /// Against what, and when.
    pub recorded: Value,
    /// The scene.
    pub script: SceneScript,
    /// The call log, in the order recorded.
    pub calls: Vec<FixtureCall>,
    /// [`SceneOutcome::to_json`] of the recorded scene.
    pub outcome: Value,
}

/// One recorded attempt, as the fixture keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureCall {
    pub address: String,
    pub attempt: u32,
    pub role: String,
    pub model: String,
    pub params: Value,
    pub mechanism: String,
    pub request: Value,
    pub prompt_sha256: String,
    pub response: Option<Value>,
    pub status: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<u64>,
}

impl FixtureCall {
    pub(crate) fn of(stored: &StoredCall) -> Self {
        let c = &stored.call;
        Self {
            address: c.address.clone(),
            attempt: c.attempt,
            role: c.role.clone(),
            model: c.model.clone(),
            params: c.params.clone(),
            mechanism: c.mechanism.as_str().to_owned(),
            request: c.request.clone(),
            prompt_sha256: c.prompt_sha256.clone(),
            response: c.response.clone(),
            status: c.status.as_str().to_owned(),
            input_tokens: c.usage.input_tokens,
            output_tokens: c.usage.output_tokens,
            cached_tokens: c.usage.cached_tokens,
            cost_usd: c.usage.cost_usd,
            latency_ms: c.latency_ms,
        }
    }

    fn into_call(self, run: &RunId) -> NewCall {
        NewCall {
            run: run.clone(),
            address: self.address,
            attempt: self.attempt,
            role: self.role,
            model: self.model,
            params: self.params,
            mechanism: Mechanism::parse(&self.mechanism).expect("a recorded mechanism"),
            request: self.request,
            prompt_sha256: self.prompt_sha256,
            response: self.response,
            status: CallStatus::parse(&self.status).expect("a recorded status"),
            usage: CallUsage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                cached_tokens: self.cached_tokens,
                cost_usd: self.cost_usd,
            },
            latency_ms: self.latency_ms,
        }
    }
}

/// Plays `script` on a fresh story through `backend`, and returns the scene
/// as a fixture.
pub(crate) async fn record(
    backend: Arc<dyn EngineBackend>,
    model: &str,
    sampling: SamplingConfig,
    script: &SceneScript,
    recorded: Value,
) -> Result<SceneFixture, SceneError> {
    let (mut story, log) = fresh_story().await;
    let port = EngineAdapter::new(backend, log, model, sampling, CancellationToken::new());
    let played = play_scripted_scene(&story.db, &mut story.author, &port, &story.run, script).await;
    let calls = story.db.calls(&story.run).await?;
    let outcome = played?;
    Ok(SceneFixture {
        recorded,
        script: script.clone(),
        calls: calls.iter().map(FixtureCall::of).collect(),
        outcome: outcome.to_json(),
    })
}

/// Plays a fixture's scene on a fresh story from its recorded calls alone.
pub(crate) async fn replay(
    fixture: &SceneFixture,
) -> (Result<SceneOutcome, SceneError>, Vec<Drift>) {
    let (mut story, mut log) = fresh_story().await;
    for call in fixture.calls.iter().cloned() {
        log.record_call(call.into_call(&story.run)).await.unwrap();
    }
    drop(log);
    let port = ReplayAdapter::new(story.db.clone());
    let played = play_scripted_scene(
        &story.db,
        &mut story.author,
        &port,
        &story.run,
        &fixture.script,
    )
    .await;
    (played, port.drift())
}

/// Every character's secret tokens, by key: a character's prompt carries its
/// own and never another's (spec §4, rule 5; §11's leak metric).
pub(crate) fn leaks(fixture: &SceneFixture) -> Vec<String> {
    let canon = fixture_canon();
    let mut found = Vec::new();
    for call in &fixture.calls {
        let request = call.request.to_string();
        for character in &canon.characters {
            let own = call.address.contains(&format!("/{}/", character.key));
            for secret in &character.secrets {
                let token = secret.text.rsplit('[').next().unwrap_or_default();
                let token = format!("[{token}");
                match (own, request.contains(&token)) {
                    (true, false) => {
                        found.push(format!("{}: its own {token} missing", call.address))
                    }
                    (false, true) => found.push(format!("{}: {token} leaked", call.address)),
                    _ => {}
                }
            }
        }
    }
    found
}

/// A model that answers each character from a script of its own, so
/// parallel calls cannot take each other's answers whatever order they
/// arrive in.
struct Cast {
    scripts: Mutex<BTreeMap<String, VecDeque<Vec<ChatChunk>>>>,
}

impl Cast {
    fn new(scripts: impl IntoIterator<Item = (&'static str, Vec<Vec<ChatChunk>>)>) -> Self {
        Self {
            scripts: Mutex::new(
                scripts
                    .into_iter()
                    .map(|(name, replies)| (name.to_owned(), replies.into()))
                    .collect(),
            ),
        }
    }
}

#[async_trait::async_trait]
impl EngineBackend for Cast {
    async fn chat_stream(&self, req: ChatRequest, _: CancellationToken) -> Result<ChatStream> {
        let system = req.system.unwrap_or_default();
        let name = system
            .strip_prefix("You play ")
            .and_then(|rest| rest.split(" in a story.").next())
            .unwrap_or_default()
            .to_owned();
        let chunks = self
            .scripts
            .lock()
            .unwrap()
            .get_mut(&name)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| panic!("no reply is scripted for {name:?}"));
        Ok(Box::pin(futures_util::stream::iter(chunks)))
    }
}

fn says(text: &str) -> Vec<ChatChunk> {
    vec![
        ChatChunk::Text(text.to_owned()),
        ChatChunk::Finished(FinishReason::Stop),
    ]
}

fn proposal(thought: &str, action: Value, visibility: Value) -> String {
    json!({
        "private_thought": thought,
        "action": action,
        "targets": [],
        "visibility": visibility,
    })
    .to_string()
}

/// Three rounds on the quay, written so that every ruling and every turn of
/// the policy happens once: an initiative taken, proposals lapsing behind it,
/// a wait, a whisper the world refuses, a fenced reply, prose around a reply,
/// a repair and a retry after a cut.
fn harbour_cast() -> Cast {
    let public = json!({"type": "public"});
    Cast::new([
        (
            "Mara",
            vec![
                says(&format!(
                    "```json\n{}\n```",
                    proposal(
                        "Get them off my quay.",
                        json!({"type": "speak", "line": "Gate shuts at sunset. Move along."}),
                        public.clone(),
                    )
                )),
                // Missing the act's `object` and `line`: repaired.
                says(
                    &json!({
                        "private_thought": "The ledger.",
                        "action": {"type": "act", "verb": "closes"},
                        "targets": [],
                        "visibility": public,
                    })
                    .to_string(),
                ),
                says(&proposal(
                    "The ledger.",
                    json!({"type": "act", "verb": "closes", "object": "the ledger", "line": null}),
                    public.clone(),
                )),
                says(&proposal(
                    "Let them talk.",
                    json!({"type": "wait"}),
                    public.clone(),
                )),
            ],
        ),
        (
            "Silas",
            vec![
                says(&format!(
                    "Silas weighs it.\n{}\nThat is all.",
                    proposal(
                        "Patience.",
                        json!({"type": "speak", "line": "The sea keeps no calendar."}),
                        public.clone(),
                    )
                )),
                says(&proposal(
                    "Back to the inn before she looks closer.",
                    json!({"type": "move", "to": "inn"}),
                    public.clone(),
                )),
            ],
        ),
        (
            "Ines",
            vec![
                says(&proposal(
                    "Why is the ledger open?",
                    json!({"type": "act", "verb": "glances at", "object": "the ledger", "line": null}),
                    json!({"type": "hidden"}),
                )),
                vec![
                    ChatChunk::Text("{\"private_thought\": \"Oil".to_owned()),
                    ChatChunk::Finished(FinishReason::Length),
                ],
                says(&proposal(
                    "Ask plainly.",
                    json!({"type": "speak", "line": "Whose crates are those?"}),
                    public.clone(),
                )),
                says(&proposal(
                    "He left in a hurry.",
                    json!({"type": "speak", "line": "Wait for me."}),
                    json!({"type": "whisper", "to": ["silas"]}),
                )),
            ],
        ),
    ])
}

async fn recorded_harbour() -> SceneFixture {
    record(
        Arc::new(harbour_cast()),
        "cast",
        SamplingConfig::default(),
        &harbour_script(),
        json!({"model": "cast"}),
    )
    .await
    .unwrap()
}

fn verdicts(outcome: &Value) -> Vec<(u64, String, String, Option<String>)> {
    outcome["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["round"].as_u64().unwrap(),
                p["character"].as_str().unwrap().to_owned(),
                p["verdict"].as_str().unwrap().to_owned(),
                p["reason"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_scripted_scene_rules_by_its_fixed_rule() {
    let fixture = recorded_harbour().await;
    let lapsed = || Some(NOT_TAKEN.to_owned());
    let found = verdicts(&fixture.outcome);
    let shape: Vec<(u64, &str, &str)> = found
        .iter()
        .map(|(r, c, v, _)| (*r, c.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        shape,
        [
            // Round 1 starts with Mara, and her line takes the round.
            (1, "mara", "committed"),
            (1, "silas", "lapsed"),
            (1, "ines", "lapsed"),
            // Round 2 starts with Silas, who leaves for the inn.
            (2, "mara", "lapsed"),
            (2, "silas", "committed"),
            (2, "ines", "lapsed"),
            // Round 3: Silas is gone; Mara waits, and Ines whispers to no one.
            (3, "mara", "lapsed"),
            (3, "ines", "refused"),
        ]
    );
    assert_eq!(found[1].3, lapsed());
    assert_eq!(found[6].3.as_deref(), Some(WAITS));
    assert!(
        found[7].3.as_deref().unwrap().contains("silas"),
        "the world names the absent addressee: {:?}",
        found[7].3
    );
    let events = fixture.outcome["events"].as_array().unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| e["event"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["opening", "opening", "speech", "movement"]);
    assert_eq!(
        events[2]["event"]["observable"],
        "Mara says: \"Gate shuts at sunset. Move along.\""
    );
    assert_eq!(events[2]["event"]["private"], "Get them off my quay.");
    assert_eq!(
        events[3]["event"]["observable"],
        "Silas leaves for The Gull Inn."
    );
    assert_eq!(events[0]["witnesses"], json!(["ines", "mara", "silas"]));

    // The policy's turns are in the call log: Mara's repair and Ines's retry.
    let attempts: Vec<(&str, u32)> = fixture
        .calls
        .iter()
        .filter(|c| c.attempt > 1)
        .map(|c| (c.address.as_str(), c.attempt))
        .collect();
    assert_eq!(
        attempts,
        [
            ("ch1/sc1/r0/k0/round2/mara/propose", 2),
            ("ch1/sc1/r0/k0/round2/ines/propose", 2),
        ]
    );
    assert_eq!(fixture.calls.len(), 10);
    assert!(fixture.calls.iter().all(|c| c.mechanism == "fallback"));
}

/// Gate G0: a scene replays identically from its call log, attempt by
/// attempt, without a model.
#[tokio::test]
async fn a_scene_recorded_through_the_engine_replays_identically() {
    let fixture = recorded_harbour().await;
    for _ in 0..2 {
        let (played, drift) = replay(&fixture).await;
        assert_eq!(played.unwrap().to_json(), fixture.outcome);
        assert_eq!(drift, Vec::new());
    }
}

#[tokio::test]
async fn a_changed_prompt_replays_with_its_drift_named() {
    let mut fixture = recorded_harbour().await;
    fixture.script.setup.pressure.push_str(" The wind rises.");
    let (played, drift) = replay(&fixture).await;
    // The replies are served by address all the same.
    assert!(played.is_ok());
    let addresses: Vec<&str> = drift.iter().map(|d| d.address.as_str()).collect();
    assert_eq!(addresses.len(), fixture.calls.len());
    assert!(addresses.contains(&"ch1/sc1/r0/k0/round1/mara/propose"));
    assert!(drift.iter().all(|d| d.recorded != d.now));
}

#[tokio::test]
async fn a_call_missing_from_the_log_stops_the_replay() {
    let mut fixture = recorded_harbour().await;
    fixture
        .calls
        .retain(|c| c.address != "ch1/sc1/r0/k0/round2/mara/propose" || c.attempt != 2);
    let (played, _) = replay(&fixture).await;
    match played {
        Err(SceneError::Port(PortError::NotRecorded {
            address, attempt, ..
        })) => {
            assert_eq!(address, "ch1/sc1/r0/k0/round2/mara/propose");
            assert_eq!(attempt, 2);
        }
        other => panic!("{other:?}"),
    }
}

/// A character's prompt holds its own sheet and secrets, the others'
/// public descriptions, and what it perceived, its own thoughts included.
#[test]
fn a_character_sees_only_what_it_may() {
    let canon = fixture_canon();
    let world = WorldState::initial(&canon);
    let mara = CharacterKey::from("mara");
    let empty = character_prompt(&canon, &world, &[], &mara, 1);
    assert!(empty.system.starts_with("You play Mara in a story."));
    assert!(empty.system.contains("[SECRET-MARA-7Q]"));
    assert!(
        empty
            .system
            .contains("Silas (`silas`): A trader from the south")
    );
    for other in [
        "[SECRET-SILAS-3K]",
        "[SECRET-INES-9D]",
        "The crates hold rifles",
    ] {
        assert!(!empty.system.contains(other), "{other} leaked");
        assert!(!empty.user.contains(other), "{other} leaked");
    }
    // The arc is the Author's alone.
    assert!(!empty.system.contains("reaches the wrong person"));
    assert!(
        empty
            .user
            .starts_with("Round 1. You are at Harbour, with no one else.")
    );
    assert!(
        empty
            .user
            .contains("You carry: ledger. Lying here: nothing.")
    );
    assert!(empty.user.contains("- nothing yet"));
}

#[tokio::test]
async fn a_character_remembers_its_own_thoughts_and_not_the_others() {
    let fixture = recorded_harbour().await;
    let request = |address: &str| -> String {
        let call = fixture
            .calls
            .iter()
            .find(|c| c.address == address && c.attempt == 1)
            .unwrap();
        call.request.to_string()
    };
    let mara = request("ch1/sc1/r0/k0/round2/mara/propose");
    assert!(mara.contains("(You thought: Get them off my quay.)"));
    let silas = request("ch1/sc1/r0/k0/round2/silas/propose");
    assert!(silas.contains("Mara says:"));
    assert!(!silas.contains("Get them off my quay."));
    // Ines's glance in round 1 was hidden, but lapsed, so nothing of it
    // reached anyone; and she never learns Mara's thought.
    let ines = request("ch1/sc1/r0/k0/round3/ines/propose");
    assert!(!ines.contains("Get them off my quay."));
    assert!(ines.contains("Silas leaves for The Gull Inn."));
}

/// A hidden deed is the actor's alone: the others' next prompts do not hold
/// it, while the actor remembers it with its thought (spec §6).
#[tokio::test]
async fn a_hidden_deed_reaches_no_one_but_its_actor() {
    let wait = || {
        says(&proposal(
            "",
            json!({"type": "wait"}),
            json!({"type": "public"}),
        ))
    };
    let cast = Cast::new([
        (
            "Mara",
            vec![
                says(&proposal(
                    "No one saw.",
                    json!({"type": "act", "verb": "pockets", "object": "a letter", "line": null}),
                    json!({"type": "hidden"}),
                )),
                wait(),
            ],
        ),
        ("Silas", vec![wait(), wait()]),
        ("Ines", vec![wait(), wait()]),
    ]);
    let script = SceneScript {
        rounds: 2,
        ..harbour_script()
    };
    let fixture = record(
        Arc::new(cast),
        "cast",
        SamplingConfig::default(),
        &script,
        json!({}),
    )
    .await
    .unwrap();
    let deed = &fixture.outcome["events"][2];
    assert_eq!(deed["event"]["observable"], "Mara pockets a letter.");
    assert_eq!(deed["witnesses"], json!(["mara"]));
    let request = |who: &str| {
        let address = format!("ch1/sc1/r0/k0/round2/{who}/propose");
        let call = fixture.calls.iter().find(|c| c.address == address).unwrap();
        call.request.to_string()
    };
    assert!(request("mara").contains("Mara pockets a letter."));
    assert!(request("mara").contains("(You thought: No one saw.)"));
    for other in ["silas", "ines"] {
        // The deed's own words: "pockets" alone is in the schema's description.
        assert!(
            !request(other).contains("pockets a letter"),
            "{other} saw it"
        );
        assert!(
            !request(other).contains("No one saw."),
            "{other} read the thought"
        );
    }
}

#[tokio::test]
async fn a_story_without_a_canon_has_no_scene() {
    let dir = tempfile::tempdir().unwrap();
    let db = StoryDb::open(&dir.path().join("s.db"), &dir.path().join("c.db"))
        .await
        .unwrap();
    let mut author = db.author_writer().await.unwrap();
    let port = ReplayAdapter::new(db.clone());
    let played =
        play_scripted_scene(&db, &mut author, &port, &RunId::nth(1), &harbour_script()).await;
    assert!(matches!(played, Err(SceneError::NoCanon)), "{played:?}");
}

/// The fixture the live smoke recorded (`scene_live_tests`).
fn live_fixture() -> SceneFixture {
    serde_json::from_str(include_str!("fixtures/harbour_scene.json")).unwrap()
}

/// Gate G0 on a live model's replies: the recorded scene replays from its
/// call log alone, with the same events, witnesses and rulings, and every
/// prompt is the one recorded. Drift here means the prompts changed and the
/// fixture is to be recorded again.
#[tokio::test]
async fn the_fixture_recorded_live_replays_identically() {
    let fixture = live_fixture();
    assert_eq!(fixture.script, harbour_script());
    let (played, drift) = replay(&fixture).await;
    assert_eq!(drift, Vec::new(), "re-record the fixture: scene_live_tests");
    assert_eq!(played.unwrap().to_json(), fixture.outcome);
    assert!(fixture.calls.iter().all(|c| c.mechanism == "fallback"));
}

#[test]
fn the_fixture_recorded_live_leaks_no_secret() {
    let fixture = live_fixture();
    assert!(!fixture.calls.is_empty());
    assert_eq!(leaks(&fixture), Vec::<String>::new());
    // The check can see its subject, whoever's the calls are: a prompt
    // without its own secret, and one with every secret.
    let mut tampered = fixture;
    tampered.calls[0].request = json!({"system": "nothing"});
    tampered.calls[1].request =
        json!({"system": "[SECRET-MARA-7Q] [SECRET-SILAS-3K] [SECRET-INES-9D]"});
    let found = leaks(&tampered);
    assert!(found.iter().any(|f| f.ends_with("missing")), "{found:?}");
    assert_eq!(
        found.iter().filter(|f| f.ends_with("leaked")).count(),
        2,
        "{found:?}"
    );
}
