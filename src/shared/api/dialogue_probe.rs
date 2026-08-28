//! Stage-0 live probe for the two-agent dialogue —
//! docs/research/two-agent-dialogue.md §5.
//!
//! Measures, against a real server, whether the feature's three-context shape
//! works on the gate models before any product code exists: two personas with
//! caller-written system messages talking via the role swap (each sees the
//! other as `user`), and a director — carrying the parent persona and a
//! conversation brief per the amended F6 — steering through verdict tool
//! calls every K messages. The §3.2 derivation rule (prologue + merge to
//! strict alternation) is inlined here; nothing of the product is touched.
//!
//! What each run records (research §5): request failures (template
//! acceptance), a role-bleed heuristic plus the full transcript for reading,
//! whether the director stopped before the cap and where, the verdict
//! parse/fallback rate, and per-request wall/token costs. The separate
//! `cache_slots` arm replays the three-context alternation through raw
//! non-stream requests to read `timings` (llama-server's `cache_n` — do the
//! contexts keep their slots?), which the app's client deliberately drops.
//!
//! The probe's checkpoints are **stateless** (the script is re-sent whole,
//! earlier directions restated) — the product's persistent director context
//! only changes what the wire replays, not what the model is asked
//! (research §5).
//!
//! Run (llama.cpp stack; n per fixture overridable via
//! `MINDFORK_DIALOGUE_PROBE_RUNS`, default 5):
//! `MINDFORK_ENGINE_URL=http://…:8000/v1 cargo test dialogue_probe -- --ignored --nocapture --test-threads=1`

use std::time::{Duration, Instant};

use futures_util::StreamExt;

use crate::entities::sampling::SamplingConfig;
use crate::shared::api::OpenAiClient;
use crate::shared::api::contract::{
    ApiMessage, ApiRole, ApiToolCall, ChatChunk, ChatRequest, EngineBackend, ToolCallAccumulator,
    ToolSchema,
};

/// A hung server must fail the arm loudly, not sit forever (lessons §2).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// Fixtures

struct Persona {
    name: &'static str,
    system: &'static str,
}

struct Fixture {
    tag: &'static str,
    a: Persona,
    b: Persona,
    scene: &'static str,
    /// The caller-authored opening line (F7) and who says it.
    opening_by_a: bool,
    opening: &'static str,
    direction: &'static str,
    max_messages: usize,
    moderate_every: usize,
    /// The amended-F6 director shape: parent persona + conversation brief.
    parent_persona: &'static str,
    brief: &'static str,
}

/// Both personas end with the same containment clause: one spoken line, no
/// narration, never the other side's words — the role-fidelity claim under
/// test is that the *system message plus the role swap* holds that frame.
const LINE_RULES: &str = " Stay in character at all times. Reply with your next \
spoken line only — one to three sentences, no narration, no stage directions, \
and never write anyone else's words.";

fn finite_fixture() -> Fixture {
    Fixture {
        tag: "finite (café mix-up)",
        a: Persona {
            name: "Mara",
            system: "You are Mara, a barista at the small café 'Krumme Tasse'. \
                You are warm but a little frazzled from the morning rush, and you \
                follow store policy: a wrongly made drink is remade for free, no \
                receipt needed.",
        },
        b: Persona {
            name: "Jonas",
            system: "You are Jonas, a customer in a hurry to catch a tram. You \
                ordered a double espresso but were handed an oat-milk latte. You \
                are polite but pressed for time and want it fixed quickly.",
        },
        scene: "A small café at 8:40 in the morning. A customer comes back to the \
            counter holding a cup.",
        opening_by_a: false,
        opening: "Sorry — I think this isn't mine? I ordered a double espresso, \
            and this looks like a latte.",
        direction: "The scene is done once the mix-up is resolved and they part \
            on good terms. Stop the dialogue at that point — do not let it drag \
            past a natural goodbye.",
        max_messages: 12,
        moderate_every: 2,
        parent_persona: "You are the writing assistant in an ongoing chat with a \
            user who is drafting scenes for a short story collection.",
        brief: "Conversation so far, in brief: the user asked for a realistic café \
            scene in which a drink mix-up gets resolved politely and briskly; you \
            suggested staging it live as a dialogue between the barista and the \
            customer, and the user agreed, asking you to direct it and keep it \
            tight.",
    }
}

fn steering_fixture() -> Fixture {
    Fixture {
        tag: "steering (flea-market haggle)",
        a: Persona {
            name: "Vera",
            system: "You are Vera, selling your late father's Zorki-4 film camera \
                at a flea market. It has sentimental value; you will not go below \
                120 euros on your own, and you get quietly defensive when pushed.",
        },
        b: Persona {
            name: "Tomas",
            system: "You are Tomas, a film-photography student with exactly 90 \
                euros to spend. You genuinely want the Zorki-4 but cannot go over \
                your budget, and you keep trying angles.",
        },
        scene: "A flea-market stall on a windy Saturday. A student picks up an old \
            film camera and turns it over in his hands.",
        opening_by_a: false,
        opening: "This Zorki-4 — does the shutter still fire on all speeds? And \
            how much are you asking for it?",
        direction: "The user wants tension that resolves credibly, not a dialogue \
            that circles. If the haggling stalls or repeats itself, send Vera a \
            note nudging her toward a middle path (for example, throwing in the \
            case and two film rolls at a price between their positions). Stop \
            once they strike a deal or definitively walk away.",
        max_messages: 12,
        moderate_every: 2,
        parent_persona: "You are the writing assistant in an ongoing chat with a \
            user who is drafting scenes for a short story collection.",
        brief: "Conversation so far, in brief: the user is drafting a flea-market \
            haggling scene; they dislike dialogues that circle, and asked you to \
            direct this one so it stays sharp and ends decisively.",
    }
}

// ---------------------------------------------------------------------------
// Derivation (research §3.2, inlined)

#[derive(Clone)]
struct Line {
    by_a: bool,
    text: String,
}

/// Builds one participant's view: own lines `assistant`, the other's `user`,
/// a `user` prologue where the first mapped message would be `assistant` (and
/// always here, since the fixtures set a scene), then adjacent same-role
/// messages merged — strict alternation by construction.
fn participant_view(
    fx: &Fixture,
    transcript: &[Line],
    speaker_a: bool,
    notes: &[String],
    one_shot_note: Option<&str>,
) -> (String, Vec<ApiMessage>) {
    let persona = if speaker_a { &fx.a } else { &fx.b };
    let mut system = format!("{}{}", persona.system, LINE_RULES);
    for note in notes {
        system.push_str("\n\nDirector's note: ");
        system.push_str(note);
    }
    if let Some(note) = one_shot_note {
        system.push_str("\n\nDirector's note for your next line: ");
        system.push_str(note);
    }

    let mut mapped: Vec<(ApiRole, String)> = vec![(ApiRole::User, fx.scene.to_string())];
    for line in transcript {
        let role = if line.by_a == speaker_a {
            ApiRole::Assistant
        } else {
            ApiRole::User
        };
        mapped.push((role, line.text.clone()));
    }
    // Merge adjacent same-role entries (the scene + the opener's first line on
    // the non-opening side, and any future same-side sequences).
    let mut messages: Vec<ApiMessage> = Vec::new();
    for (role, text) in mapped {
        match messages.last_mut() {
            Some(last) if last.role == role => {
                last.content.push_str("\n\n");
                last.content.push_str(&text);
            }
            _ => messages.push(match role {
                ApiRole::Assistant => ApiMessage::assistant(text),
                _ => ApiMessage::user(text),
            }),
        }
    }
    (system, messages)
}

fn render_script(fx: &Fixture, transcript: &[Line]) -> String {
    transcript
        .iter()
        .map(|l| {
            let name = if l.by_a { fx.a.name } else { fx.b.name };
            format!("{name}: {}", l.text)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The director's request per the amended F6: parent persona + conversation
/// brief + director appendix; the script and earlier directions in one `user`
/// message; verdicts as tool calls (thinking muted — `reasoning_budget: 0`
/// rides the existing wire path into `enable_thinking:false`).
fn director_request(
    fx: &Fixture,
    transcript: &[Line],
    issued: &[String],
    seed: i64,
) -> ChatRequest {
    let system = format!(
        "{}\n\n{}\n\nYou are directing a live dialogue between {} and {} for \
         the user; the script so far is in the message below. Your direction: \
         {}\n\nAct through the tools. If the dialogue has reached the ending \
         your direction describes, call dialogue_stop with a short reason and a \
         one-sentence summary. Otherwise call dialogue_continue; you may first \
         call dialogue_note to steer a participant's lines from now on, \
         dialogue_retry to have the last line rewritten by its author, or \
         dialogue_rewrite to replace the last line with your own wording. \
         Always call at least one tool; do not answer in prose.",
        fx.parent_persona, fx.brief, fx.a.name, fx.b.name, fx.direction
    );
    let mut user = format!("The script so far:\n\n{}", render_script(fx, transcript));
    if !issued.is_empty() {
        user.push_str("\n\nDirections you have already issued:\n");
        for d in issued {
            user.push_str("- ");
            user.push_str(d);
            user.push('\n');
        }
    }
    user.push_str("\nWhat do you do?");
    ChatRequest {
        system: Some(system),
        messages: vec![ApiMessage::user(user)],
        sampling: SamplingConfig {
            max_tokens: Some(512),
            temperature: Some(0.2),
            seed: Some(seed),
            reasoning_budget: Some(0),
            ..Default::default()
        },
        tools: verdict_tools(fx),
        ..Default::default()
    }
}

fn verdict_tools(fx: &Fixture) -> Vec<ToolSchema> {
    let obj = |props: serde_json::Value, required: &[&str]| serde_json::json!({ "type": "object", "properties": props, "required": required });
    vec![
        ToolSchema {
            name: "dialogue_continue".into(),
            description: "Let the dialogue proceed to the next line.".into(),
            parameters: obj(serde_json::json!({}), &[]),
        },
        ToolSchema {
            name: "dialogue_stop".into(),
            description: "End the dialogue: the ending your direction describes has \
                been reached, or the scene cannot get there."
                .into(),
            parameters: obj(
                serde_json::json!({
                    "reason": { "type": "string", "description": "Why the dialogue is over." },
                    "summary": { "type": "string", "description": "One sentence on how it ended." }
                }),
                &["reason"],
            ),
        },
        ToolSchema {
            name: "dialogue_note".into(),
            description: "Send a private stage direction that shapes a participant's \
                lines from now on."
                .into(),
            parameters: obj(
                serde_json::json!({
                    "to": { "type": "string", "enum": ["a", "b", "both"],
                            "description": format!("Who receives it: 'a' = {}, 'b' = {}.", fx.a.name, fx.b.name) },
                    "text": { "type": "string" }
                }),
                &["to", "text"],
            ),
        },
        ToolSchema {
            name: "dialogue_retry".into(),
            description: "Discard the last line; its author writes it again \
                (optionally guided by a note)."
                .into(),
            parameters: obj(serde_json::json!({ "note": { "type": "string" } }), &[]),
        },
        ToolSchema {
            name: "dialogue_rewrite".into(),
            description: "Replace the last line's text with your own wording.".into(),
            parameters: obj(
                serde_json::json!({ "text": { "type": "string" } }),
                &["text"],
            ),
        },
    ]
}

// ---------------------------------------------------------------------------
// Driving one run

struct Reply {
    text: String,
    calls: Vec<ApiToolCall>,
    prompt_tokens: u32,
    completion_tokens: u32,
    wall: Duration,
}

async fn ask(client: &OpenAiClient, req: ChatRequest, label: &str) -> Result<Reply, String> {
    let started = Instant::now();
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .map_err(|e| format!("{label}: request refused: {e:#}"))?;
    let mut text = String::new();
    let mut acc = ToolCallAccumulator::default();
    let (mut prompt_tokens, mut completion_tokens) = (0, 0);
    let drain = async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::Usage(u) => {
                    prompt_tokens = u.prompt_tokens;
                    completion_tokens = u.completion_tokens;
                }
                ChatChunk::Error { message, .. } => return Err(format!("{label}: {message}")),
                ChatChunk::Finished(_) => break,
                _ => {}
            }
        }
        Ok(())
    };
    match tokio::time::timeout(REQUEST_TIMEOUT, drain).await {
        Ok(Ok(())) => Ok(Reply {
            text,
            calls: acc.finish(),
            prompt_tokens,
            completion_tokens,
            wall: started.elapsed(),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(format!("{label}: no finish within {REQUEST_TIMEOUT:?}")),
    }
}

#[derive(Default)]
struct RunReport {
    generated: usize,
    checkpoints: usize,
    fallbacks: usize,
    notes: usize,
    retries: usize,
    rewrites: usize,
    stopped_by_director: bool,
    stop_reason: Option<String>,
    bleed_hits: usize,
    requests: usize,
    prompt_tokens: u64,
    completion_tokens: u64,
    wall: Duration,
    participant_wall: Vec<Duration>,
    director_wall: Vec<Duration>,
}

/// One full dialogue run. Any request failure aborts the run with `Err` —
/// template acceptance is go/no-go metric 1 and is asserted, not tolerated.
async fn run_dialogue(
    client: &OpenAiClient,
    fx: &Fixture,
    run: usize,
) -> Result<RunReport, String> {
    let mut transcript = vec![Line {
        by_a: fx.opening_by_a,
        text: fx.opening.to_string(),
    }];
    let mut notes_a: Vec<String> = Vec::new();
    let mut notes_b: Vec<String> = Vec::new();
    let mut issued: Vec<String> = Vec::new();
    let mut rep = RunReport::default();
    let started = Instant::now();
    let mut seed = (1000 + run * 100) as i64;
    let mut next_checkpoint_at = fx.moderate_every;

    let speak = async |transcript: &[Line],
                       speaker_a: bool,
                       one_shot: Option<&str>,
                       notes_a: &[String],
                       notes_b: &[String],
                       seed: i64,
                       rep: &mut RunReport|
           -> Result<Line, String> {
        let notes = if speaker_a { notes_a } else { notes_b };
        let (system, messages) = participant_view(fx, transcript, speaker_a, notes, one_shot);
        let name = if speaker_a { fx.a.name } else { fx.b.name };
        let req = ChatRequest {
            system: Some(system),
            messages,
            sampling: SamplingConfig {
                max_tokens: Some(1536),
                temperature: Some(0.7),
                seed: Some(seed),
                ..Default::default()
            },
            tools: Vec::new(),
            ..Default::default()
        };
        let reply = ask(client, req, &format!("{name} line")).await?;
        rep.requests += 1;
        rep.prompt_tokens += u64::from(reply.prompt_tokens);
        rep.completion_tokens += u64::from(reply.completion_tokens);
        rep.participant_wall.push(reply.wall);
        let other = if speaker_a { fx.b.name } else { fx.a.name };
        let text = reply.text.trim().to_string();
        if text.contains(&format!("{other}:")) {
            rep.bleed_hits += 1;
        }
        if text.is_empty() {
            return Err(format!("{name} produced an empty line"));
        }
        println!("  {name}: {text}");
        Ok(Line {
            by_a: speaker_a,
            text,
        })
    };

    'dialogue: loop {
        if rep.generated >= fx.max_messages {
            println!("  [cap] max_messages={} reached", fx.max_messages);
            break;
        }
        if rep.generated >= next_checkpoint_at {
            next_checkpoint_at += fx.moderate_every;
            rep.checkpoints += 1;
            seed += 1;
            let reply = ask(
                client,
                director_request(fx, &transcript, &issued, seed),
                "director checkpoint",
            )
            .await?;
            rep.requests += 1;
            rep.prompt_tokens += u64::from(reply.prompt_tokens);
            rep.completion_tokens += u64::from(reply.completion_tokens);
            rep.director_wall.push(reply.wall);
            if reply.calls.is_empty() {
                rep.fallbacks += 1;
                println!(
                    "  [director] no tool call — fallback to continue; prose: {:?}",
                    reply.text.trim()
                );
            }
            for call in &reply.calls {
                let args: serde_json::Value =
                    serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                match call.name.as_str() {
                    "dialogue_continue" => println!("  [director] continue"),
                    "dialogue_stop" => {
                        let reason = args["reason"].as_str().unwrap_or("").to_string();
                        let summary = args["summary"].as_str().unwrap_or("");
                        println!("  [director] STOP: {reason} — {summary}");
                        rep.stopped_by_director = true;
                        rep.stop_reason = Some(reason);
                        break 'dialogue;
                    }
                    "dialogue_note" => {
                        let to = args["to"].as_str().unwrap_or("both");
                        let text = args["text"].as_str().unwrap_or("").to_string();
                        println!("  [director] note to {to}: {text}");
                        rep.notes += 1;
                        issued.push(format!("note to {to}: {text}"));
                        if to != "b" {
                            notes_a.push(text.clone());
                        }
                        if to != "a" {
                            notes_b.push(text);
                        }
                    }
                    "dialogue_retry" => {
                        // Only a generated line can be retried; the fixtures'
                        // opening is caller-authored, and generated >= 1 here.
                        let note = args["note"].as_str().map(str::to_string);
                        println!("  [director] retry last line (note: {note:?})");
                        rep.retries += 1;
                        issued.push("retried the last line".into());
                        if rep.generated == 0 {
                            println!("  [director] retry ignored: nothing generated yet");
                            continue;
                        }
                        let speaker_a = transcript.last().map(|l| l.by_a).unwrap_or(false);
                        transcript.pop();
                        if rep.generated >= fx.max_messages {
                            break 'dialogue;
                        }
                        seed += 1;
                        let line = speak(
                            &transcript,
                            speaker_a,
                            note.as_deref(),
                            &notes_a,
                            &notes_b,
                            seed,
                            &mut rep,
                        )
                        .await?;
                        transcript.push(line);
                        rep.generated += 1;
                    }
                    "dialogue_rewrite" => {
                        let text = args["text"].as_str().unwrap_or("").to_string();
                        println!("  [director] rewrite last line: {text}");
                        rep.rewrites += 1;
                        issued.push("rewrote the last line".into());
                        if let (Some(last), false) = (transcript.last_mut(), text.is_empty()) {
                            last.text = text;
                        }
                    }
                    other => {
                        rep.fallbacks += 1;
                        println!("  [director] unknown tool {other:?} — counted as fallback");
                    }
                }
            }
            continue;
        }
        let speaker_a = !transcript
            .last()
            .map(|l| l.by_a)
            .unwrap_or(!fx.opening_by_a);
        seed += 1;
        let line = speak(
            &transcript,
            speaker_a,
            None,
            &notes_a,
            &notes_b,
            seed,
            &mut rep,
        )
        .await?;
        transcript.push(line);
        rep.generated += 1;
    }
    rep.wall = started.elapsed();
    Ok(rep)
}

fn avg_secs(walls: &[Duration]) -> f64 {
    if walls.is_empty() {
        return 0.0;
    }
    walls.iter().map(Duration::as_secs_f64).sum::<f64>() / walls.len() as f64
}

async fn run_fixture(client: &OpenAiClient, fx: &Fixture, runs: usize) {
    println!("\n================ fixture: {} ================", fx.tag);
    let mut reports = Vec::new();
    for run in 0..runs {
        println!("\n--- run {} of {runs} ---", run + 1);
        match run_dialogue(client, fx, run).await {
            Ok(rep) => reports.push(rep),
            Err(e) => panic!("run {} failed a request — go/no-go metric 1: {e}", run + 1),
        }
    }
    let stopped = reports.iter().filter(|r| r.stopped_by_director).count();
    let checkpoints: usize = reports.iter().map(|r| r.checkpoints).sum();
    let fallbacks: usize = reports.iter().map(|r| r.fallbacks).sum();
    let bleed: usize = reports.iter().map(|r| r.bleed_hits).sum();
    let notes: usize = reports.iter().map(|r| r.notes).sum();
    let retries: usize = reports.iter().map(|r| r.retries).sum();
    let rewrites: usize = reports.iter().map(|r| r.rewrites).sum();
    let p_wall: Vec<Duration> = reports
        .iter()
        .flat_map(|r| r.participant_wall.iter().copied())
        .collect();
    let d_wall: Vec<Duration> = reports
        .iter()
        .flat_map(|r| r.director_wall.iter().copied())
        .collect();
    println!("\n==== summary: {} ====", fx.tag);
    println!("  stopped by director: {stopped}/{runs}");
    for (i, r) in reports.iter().enumerate() {
        println!(
            "    run {}: {} msgs, {} checkpoints, stop={:?}, {:.0}s, {}+{} tokens",
            i + 1,
            r.generated,
            r.checkpoints,
            r.stop_reason.as_deref().unwrap_or("<cap>"),
            r.wall.as_secs_f64(),
            r.prompt_tokens,
            r.completion_tokens,
        );
    }
    println!("  verdict fallbacks: {fallbacks}/{checkpoints} checkpoint replies");
    println!("  steering used: {notes} notes, {retries} retries, {rewrites} rewrites");
    println!("  role-bleed heuristic hits: {bleed}");
    println!(
        "  avg wall: participant {:.1}s, director {:.1}s",
        avg_secs(&p_wall),
        avg_secs(&d_wall)
    );
    // Mechanical floors only — the go bars of research §5 are judged from the
    // printed transcripts and this summary, not asserted here.
    assert!(
        reports
            .iter()
            .all(|r| r.checkpoints >= 1 && r.generated >= 2),
        "a run never reached a checkpoint — the loop did not loop"
    );
    assert!(
        fallbacks * 2 < checkpoints.max(1),
        "verdict fallback rate above 50% — the checkpoint shape does not work at all"
    );
}

fn probe_runs() -> usize {
    std::env::var("MINDFORK_DIALOGUE_PROBE_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
}

async fn engine() -> Option<(OpenAiClient, String)> {
    let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return None;
    };
    let base = url.trim_end_matches('/').to_string();
    let client = OpenAiClient::new(base.clone());
    let model = client
        .model_id()
        .await
        .unwrap_or_else(|| "<unknown>".into());
    println!("engine: {base} model: {model}");
    Some((client, model))
}

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn dialogue_probe_finite_live() {
    let Some((client, _)) = engine().await else {
        return;
    };
    run_fixture(&client, &finite_fixture(), probe_runs()).await;
}

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn dialogue_probe_steering_live() {
    let Some((client, _)) = engine().await else {
        return;
    };
    run_fixture(&client, &steering_fixture(), probe_runs()).await;
}

/// The §3.9 slot question, measured raw: alternate the three contexts through
/// non-stream requests and read llama-server's `timings` (`cache_n` — tokens
/// reused from the slot's prefix). The app's client drops `timings`, so this
/// arm speaks to the server directly; it asserts only transport success.
#[tokio::test]
#[ignore = "requires a running llama-server (MINDFORK_ENGINE_URL)"]
async fn dialogue_probe_cache_slots_live() {
    let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let base = url.trim_end_matches('/').to_string();
    let http = reqwest::Client::new();
    let fx = finite_fixture();
    let mut transcript = vec![Line {
        by_a: fx.opening_by_a,
        text: fx.opening.to_string(),
    }];

    let post = async |system: String,
                      messages: Vec<ApiMessage>,
                      tag: &str|
           -> (String, serde_json::Value) {
        let mut wire = vec![serde_json::json!({ "role": "system", "content": system })];
        for m in &messages {
            let role = match m.role {
                ApiRole::Assistant => "assistant",
                _ => "user",
            };
            wire.push(serde_json::json!({ "role": role, "content": m.content }));
        }
        let body = serde_json::json!({
            "messages": wire, "stream": false,
            "temperature": 0.7, "seed": 42, "max_tokens": 256,
        });
        let started = Instant::now();
        let resp: serde_json::Value = http
            .post(format!("{base}/chat/completions"))
            .json(&body)
            .send()
            .await
            .expect("the raw request must reach the server")
            .error_for_status()
            .expect("the raw request must be accepted")
            .json()
            .await
            .expect("the response must be JSON");
        let text = resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        let timings = resp["timings"].clone();
        println!(
            "  [{tag}] {:.1}s prompt_n={} cache_n={} predicted_n={}",
            started.elapsed().as_secs_f64(),
            timings["prompt_n"],
            timings["cache_n"],
            timings["predicted_n"],
        );
        (text, timings)
    };

    println!("cache-slot probe over {base} (three contexts alternating):");
    for round in 0..3 {
        for speaker_a in [true, false] {
            let (system, messages) = participant_view(&fx, &transcript, speaker_a, &[], None);
            let (text, _) = post(system, messages, if speaker_a { "A" } else { "B" }).await;
            transcript.push(Line {
                by_a: speaker_a,
                text,
            });
        }
        let script = render_script(&fx, &transcript);
        let system = format!(
            "{}\n\n{}\n\nYou are directing the dialogue below. Answer with one \
             word: CONTINUE or STOP.",
            fx.parent_persona, fx.brief
        );
        let user = format!("The script so far:\n\n{script}\n\nContinue or stop?");
        let _ = post(system, vec![ApiMessage::user(user)], "D").await;
        println!("  -- round {} done --", round + 1);
    }
    println!(
        "read: a context whose cache_n tracks its prompt_n kept its slot; \
         cache_n=0 on every revisit means the three contexts evict each other."
    );
}
