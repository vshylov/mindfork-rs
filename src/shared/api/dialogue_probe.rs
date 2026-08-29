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

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use crate::entities::sampling::SamplingConfig;
use crate::shared::api::contract::{
    ApiMessage, ApiRole, ApiToolCall, ChatChunk, ChatRequest, EngineBackend, ToolCallAccumulator,
    ToolSchema,
};
use crate::shared::api::retry::RetryBackend;
use crate::shared::api::{AnthropicClient, GeminiClient};

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

/// The un-exercised half of F3: the finite and steering runs never provoked
/// `dialogue_retry`/`dialogue_rewrite` (continue/stop/note covered the
/// director's needs), so this fixture manufactures the need — one persona is
/// verbose *by construction*, and the direction makes editing the director's
/// job. A situation, not a numbered script (lessons §9).
fn editing_fixture() -> Fixture {
    Fixture {
        tag: "editing (verbose poet)",
        a: Persona {
            name: "Elias",
            system: "You are Elias, a poet arranging where to hold his reading. \
                You are incapable of brevity: every sentence of yours winds \
                through subclauses, parentheticals and flourishes, and you \
                always use the full three sentences you allow yourself, each as \
                long as you can spin it.",
        },
        b: Persona {
            name: "Rita",
            system: "You are Rita, a café owner hosting poetry readings. You are \
                brisk and practical and just need to settle the day, the hour \
                and the corner of the room.",
        },
        scene: "A café after closing time. The owner is stacking chairs while a \
            poet lingers at the counter.",
        opening_by_a: false,
        opening: "So — your reading. Thursday or Friday, and which corner do \
            you want?",
        direction: "The user wants tight, stage-ready dialogue. Whenever a line \
            sprawls — three sentences, or ornament drowning the point — have \
            its author retry it with a note to cut it down; if a retried line \
            still sprawls, rewrite it yourself, shorter, in the character's \
            voice. Stop once the day, the hour and the spot are settled.",
        max_messages: 8,
        moderate_every: 1,
        parent_persona: "You are the writing assistant in an ongoing chat with a \
            user who is drafting scenes for a short story collection.",
        brief: "Conversation so far, in brief: the user is drafting a scene of a \
            poet and a café owner settling a reading; they asked you to direct \
            it and, above all, to keep every line tight enough to stage.",
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
    /// Reasoning the stream routed away from `text` — a participant's line
    /// landing here instead of `text` is a finding, so it is kept and shown.
    thoughts: String,
    calls: Vec<ApiToolCall>,
    prompt_tokens: u32,
    completion_tokens: u32,
    wall: Duration,
    /// Transient transport failures absorbed by [`RetryBackend`] on the way —
    /// the first Gemma run died on a stale pooled connection
    /// (`connection closed before message completed`), the back-to-back-load
    /// flake lessons §9 records; production wraps external backends in the
    /// retry decorator, so the probe does too and counts what it absorbed.
    retries: usize,
}

async fn ask(client: &dyn EngineBackend, req: ChatRequest, label: &str) -> Result<Reply, String> {
    let started = Instant::now();
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .map_err(|e| format!("{label}: request refused: {e:#}"))?;
    let mut text = String::new();
    let mut thoughts = String::new();
    let mut acc = ToolCallAccumulator::default();
    let (mut prompt_tokens, mut completion_tokens) = (0, 0);
    let mut retries = 0usize;
    let drain = async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::Text(t) => text.push_str(&t),
                ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                ChatChunk::ToolCall(d) => acc.push(d),
                ChatChunk::Usage(u) => {
                    prompt_tokens = u.prompt_tokens;
                    completion_tokens = u.completion_tokens;
                }
                ChatChunk::Retry { .. } => retries += 1,
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
            thoughts,
            calls: acc.finish(),
            prompt_tokens,
            completion_tokens,
            wall: started.elapsed(),
            retries,
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
    transport_retries: usize,
    /// All-thinking empty turns recovered by the muted re-ask — see `speak`.
    empty_recoveries: usize,
    requests: usize,
    prompt_tokens: u64,
    completion_tokens: u64,
    wall: Duration,
    participant_wall: Vec<Duration>,
    director_wall: Vec<Duration>,
}

/// The mutable state of one dialogue run — what the loop, the checkpoint and
/// the verdicts all read and write. `issued` is the list of directions already
/// given, restated in every checkpoint: the probe's checkpoints are stateless
/// (see the module header), so the script has to carry them.
struct RunState {
    transcript: Vec<Line>,
    notes_a: Vec<String>,
    notes_b: Vec<String>,
    issued: Vec<String>,
    rep: RunReport,
    /// Walked forward one request at a time, so a retried line is not the
    /// same sample again.
    seed: i64,
    next_checkpoint_at: usize,
}

/// What a checkpoint leaves the run to do.
enum Checkpoint {
    /// The scene goes on.
    Continue,
    /// It ends here — the director stopped it, or a retry spent the last
    /// `max_messages` slot.
    Stop,
}

/// One participant request accounted into the run's report.
fn account_participant(rep: &mut RunReport, reply: &Reply) {
    rep.requests += 1;
    rep.transport_retries += reply.retries;
    rep.prompt_tokens += u64::from(reply.prompt_tokens);
    rep.completion_tokens += u64::from(reply.completion_tokens);
    rep.participant_wall.push(reply.wall);
}

/// One participant line: the speaker's own view of the transcript (§3.2), its
/// standing notes, and the director's one-shot instruction when this line is
/// a retry. The role-bleed heuristic is counted here, on the text as spoken.
async fn speak(
    client: &dyn EngineBackend,
    fx: &Fixture,
    st: &mut RunState,
    speaker_a: bool,
    one_shot: Option<&str>,
) -> Result<Line, String> {
    let notes = if speaker_a { &st.notes_a } else { &st.notes_b };
    let (system, messages) = participant_view(fx, &st.transcript, speaker_a, notes, one_shot);
    let name = if speaker_a { fx.a.name } else { fx.b.name };
    let req = ChatRequest {
        system: Some(system),
        messages,
        sampling: SamplingConfig {
            max_tokens: Some(1536),
            temperature: Some(0.7),
            seed: Some(st.seed),
            ..Default::default()
        },
        tools: Vec::new(),
        ..Default::default()
    };
    let req_muted = {
        let mut r = req.clone();
        r.sampling.reasoning_budget = Some(0);
        r
    };
    let mut reply = ask(client, req, &format!("{name} line")).await?;
    account_participant(&mut st.rep, &reply);
    // The all-thinking empty turn, found live by this very probe: an
    // instruction conflict (a director's note against a persona's format
    // rule) sends Gemma 4 into unbounded deliberation — 1536 tokens of
    // `reasoning_content`, no text. The recovery the compliance probes
    // established (lessons §9): re-ask once with thinking muted. The
    // product executor adopts this rule (research §5.1).
    if reply.text.trim().is_empty() {
        println!(
            "  [{name}] empty line ({} completion tokens, all thoughts) — re-asking muted",
            reply.completion_tokens
        );
        st.rep.empty_recoveries += 1;
        reply = ask(client, req_muted, &format!("{name} line, muted")).await?;
        account_participant(&mut st.rep, &reply);
    }
    let other = if speaker_a { fx.b.name } else { fx.a.name };
    let text = reply.text.trim().to_string();
    if text.contains(&format!("{other}:")) {
        st.rep.bleed_hits += 1;
    }
    if text.is_empty() {
        return Err(format!(
            "{name} produced an empty line even with thinking muted ({} completion \
             tokens; thoughts, first 300 chars: {:?})",
            reply.completion_tokens,
            reply.thoughts.chars().take(300).collect::<String>()
        ));
    }
    println!("  {name}: {text}");
    Ok(Line {
        by_a: speaker_a,
        text,
    })
}

/// The `dialogue_stop` verdict: the scene ends, with the director's reason
/// recorded — go/no-go metric 3 reads it.
fn verdict_stop(rep: &mut RunReport, args: &serde_json::Value) {
    let reason = args["reason"].as_str().unwrap_or("").to_string();
    let summary = args["summary"].as_str().unwrap_or("");
    println!("  [director] STOP: {reason} — {summary}");
    rep.stopped_by_director = true;
    rep.stop_reason = Some(reason);
}

/// The `dialogue_note` verdict: a standing direction, appended to each
/// addressed participant's notes and to the restated script.
fn verdict_note(st: &mut RunState, args: &serde_json::Value) {
    let to = args["to"].as_str().unwrap_or("both");
    let text = args["text"].as_str().unwrap_or("").to_string();
    println!("  [director] note to {to}: {text}");
    st.rep.notes += 1;
    st.issued.push(format!("note to {to}: {text}"));
    if to != "b" {
        st.notes_a.push(text.clone());
    }
    if to != "a" {
        st.notes_b.push(text);
    }
}

/// The `dialogue_retry` verdict: discard the last line and speak it again
/// under the director's note. `Checkpoint::Stop` when the regeneration would
/// pass `max_messages` — the cap ends the run there rather than overrunning it.
async fn verdict_retry(
    client: &dyn EngineBackend,
    fx: &Fixture,
    st: &mut RunState,
    args: &serde_json::Value,
) -> Result<Checkpoint, String> {
    // Only a generated line can be retried; the fixtures' opening is
    // caller-authored, and generated >= 1 here.
    let note = args["note"].as_str().map(str::to_string);
    println!("  [director] retry last line (note: {note:?})");
    st.rep.retries += 1;
    st.issued.push("retried the last line".into());
    if st.rep.generated == 0 {
        println!("  [director] retry ignored: nothing generated yet");
        return Ok(Checkpoint::Continue);
    }
    let speaker_a = st.transcript.last().map(|l| l.by_a).unwrap_or(false);
    st.transcript.pop();
    if st.rep.generated >= fx.max_messages {
        return Ok(Checkpoint::Stop);
    }
    st.seed += 1;
    let line = speak(client, fx, st, speaker_a, note.as_deref()).await?;
    st.transcript.push(line);
    st.rep.generated += 1;
    Ok(Checkpoint::Continue)
}

/// The `dialogue_rewrite` verdict: the director's own words replace the last
/// line. An empty rewrite is ignored.
fn verdict_rewrite(st: &mut RunState, args: &serde_json::Value) {
    let text = args["text"].as_str().unwrap_or("").to_string();
    println!("  [director] rewrite last line: {text}");
    st.rep.rewrites += 1;
    st.issued.push("rewrote the last line".into());
    if let (Some(last), false) = (st.transcript.last_mut(), text.is_empty()) {
        last.text = text;
    }
}

/// One director checkpoint: the script re-sent whole, then the verdicts of
/// the reply applied in call order. A reply with no tool call is counted as a
/// fallback and read as `continue` — the run walks on toward its cap.
async fn checkpoint(
    client: &dyn EngineBackend,
    fx: &Fixture,
    st: &mut RunState,
) -> Result<Checkpoint, String> {
    st.next_checkpoint_at += fx.moderate_every;
    st.rep.checkpoints += 1;
    st.seed += 1;
    let reply = ask(
        client,
        director_request(fx, &st.transcript, &st.issued, st.seed),
        "director checkpoint",
    )
    .await?;
    st.rep.requests += 1;
    st.rep.transport_retries += reply.retries;
    st.rep.prompt_tokens += u64::from(reply.prompt_tokens);
    st.rep.completion_tokens += u64::from(reply.completion_tokens);
    st.rep.director_wall.push(reply.wall);
    if reply.calls.is_empty() {
        st.rep.fallbacks += 1;
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
                verdict_stop(&mut st.rep, &args);
                return Ok(Checkpoint::Stop);
            }
            "dialogue_note" => verdict_note(st, &args),
            "dialogue_retry" => match verdict_retry(client, fx, st, &args).await? {
                Checkpoint::Stop => return Ok(Checkpoint::Stop),
                Checkpoint::Continue => {}
            },
            "dialogue_rewrite" => verdict_rewrite(st, &args),
            other => {
                st.rep.fallbacks += 1;
                println!("  [director] unknown tool {other:?} — counted as fallback");
            }
        }
    }
    Ok(Checkpoint::Continue)
}

/// One full dialogue run. Any request failure that survives the retry
/// decorator aborts the run with `Err` — template acceptance is go/no-go
/// metric 1, and a *persistent* refusal is what fails it; transient
/// transport flakes are retried and counted instead.
async fn run_dialogue(
    client: &dyn EngineBackend,
    fx: &Fixture,
    run: usize,
) -> Result<RunReport, String> {
    let mut st = RunState {
        transcript: vec![Line {
            by_a: fx.opening_by_a,
            text: fx.opening.to_string(),
        }],
        notes_a: Vec::new(),
        notes_b: Vec::new(),
        issued: Vec::new(),
        rep: RunReport::default(),
        seed: (1000 + run * 100) as i64,
        next_checkpoint_at: fx.moderate_every,
    };
    let started = Instant::now();
    loop {
        if st.rep.generated >= fx.max_messages {
            println!("  [cap] max_messages={} reached", fx.max_messages);
            break;
        }
        if st.rep.generated >= st.next_checkpoint_at {
            match checkpoint(client, fx, &mut st).await? {
                Checkpoint::Stop => break,
                Checkpoint::Continue => continue,
            }
        }
        let speaker_a = !st
            .transcript
            .last()
            .map(|l| l.by_a)
            .unwrap_or(!fx.opening_by_a);
        st.seed += 1;
        let line = speak(client, fx, &mut st, speaker_a, None).await?;
        st.transcript.push(line);
        st.rep.generated += 1;
    }
    st.rep.wall = started.elapsed();
    Ok(st.rep)
}

fn avg_secs(walls: &[Duration]) -> f64 {
    if walls.is_empty() {
        return 0.0;
    }
    walls.iter().map(Duration::as_secs_f64).sum::<f64>() / walls.len() as f64
}

async fn run_fixture(client: &dyn EngineBackend, fx: &Fixture, runs: usize) {
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
    let transport: usize = reports.iter().map(|r| r.transport_retries).sum();
    println!("  transport retries absorbed: {transport}");
    let recovered: usize = reports.iter().map(|r| r.empty_recoveries).sum();
    println!("  empty lines recovered by the muted re-ask: {recovered}");
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

/// The client, wrapped in the production retry decorator (spec §6.8) exactly
/// as `live_backend()` wraps the e2e set — a stale pooled connection or a
/// mid-burst drop is retried, not read as a template rejection (lessons §9).
async fn engine() -> Option<(Arc<dyn EngineBackend>, String)> {
    let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return None;
    };
    let base = url.trim_end_matches('/').to_string();
    // Keyed. Built with `OpenAiClient::new` alone, every request against the
    // rented gate is a `401` — which is how all four of this module's smokes
    // failed on the first gpt-oss dispatch, having only ever met an
    // unauthenticated LAN stand (docs/research/e2e-gpt-oss-120b.md).
    let client = RetryBackend::wrap(Arc::new(
        crate::shared::api::live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")
            .expect("the URL variable was just read"),
    ));
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
    run_fixture(client.as_ref(), &finite_fixture(), probe_runs()).await;
}

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn dialogue_probe_steering_live() {
    let Some((client, _)) = engine().await else {
        return;
    };
    run_fixture(client.as_ref(), &steering_fixture(), probe_runs()).await;
}

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn dialogue_probe_editing_live() {
    let Some((client, _)) = engine().await else {
        return;
    };
    run_fixture(client.as_ref(), &editing_fixture(), 2).await;
}

fn cloud_key(primary: &str, fallback: &str) -> Option<String> {
    std::env::var(primary)
        .ok()
        .filter(|k| !k.is_empty())
        .or_else(|| std::env::var(fallback).ok().filter(|k| !k.is_empty()))
}

/// The cloud spot-check (research §5): one finite run each on Anthropic and
/// Gemini — the two providers whose wire layers enforce strict alternation by
/// merging — to see the §3.2 derivation and the verdict tools survive a real
/// cloud round trip. Each arm runs only when its key is present.
#[tokio::test]
#[ignore = "requires MINDFORK_ANTHROPIC_KEY / MINDFORK_GEMINI_KEY (live cloud APIs)"]
async fn dialogue_probe_cloud_live() {
    let mut ran = false;
    if let Some(key) = cloud_key("MINDFORK_ANTHROPIC_KEY", "ANTHROPIC_API_KEY") {
        let model =
            std::env::var("MINDFORK_ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-haiku-4-5".into());
        println!("cloud arm: anthropic {model}");
        let client = RetryBackend::wrap(Arc::new(AnthropicClient::new(
            "https://api.anthropic.com",
            key,
            model,
        )));
        run_fixture(client.as_ref(), &finite_fixture(), 1).await;
        ran = true;
    } else {
        eprintln!("skip: no Anthropic key");
    }
    if let Some(key) = cloud_key("MINDFORK_GEMINI_KEY", "GEMINI_API_KEY") {
        let model =
            std::env::var("MINDFORK_GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
        println!("cloud arm: gemini {model}");
        let client = RetryBackend::wrap(Arc::new(GeminiClient::new(
            "https://generativelanguage.googleapis.com/v1beta",
            key,
            model,
        )));
        run_fixture(client.as_ref(), &finite_fixture(), 1).await;
        ran = true;
    } else {
        eprintln!("skip: no Gemini key");
    }
    if !ran {
        eprintln!("skip: no cloud keys at all");
    }
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
        // Keyed like every other live request: this arm talks to the server
        // directly (the app's client drops `timings`), so it needs the header the
        // client would have added.
        let resp: serde_json::Value =
            crate::shared::api::live_bearer(http.post(format!("{base}/chat/completions")))
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
