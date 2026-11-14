//! The memory-routing probe (docs/history/memory-routing.md §5): where a model files what
//! it learns in a conversation — a fact about the person (a note, `note_save`; a
//! stable trait may go to `update_user_model`), or its own conclusion (an
//! observation, `add_insight`; a change of its self-description goes to
//! `update_self_model`).
//!
//! The conversation is scripted: six lines that each carry a fact about the
//! person, three that invite the companion's own conclusion, three of small talk.
//! Every line is run through the app's real request path — the profile's memory
//! tools, the self-model maintenance protocol — against a cloud provider, and the
//! turn's writes are sorted by the line's kind. The live finding this measures
//! (2026-11-13): on a real profile 7 of 10 facts about the person went to
//! `add_insight`.
//!
//! It asserts only the mechanism — every line answered — and prints the table: a
//! measurement per model, not a gate. It takes minutes and costs money, so it
//! runs only when asked: `MINDFORK_ROUTING=1`; `MINDFORK_ROUTING_RUNS` (3 by
//! default) independent conversations per arm; `MINDFORK_ROUTING_OUT=<file>` also
//! writes the records as JSON. Arms, each skipped without its key:
//!
//! `MINDFORK_ROUTING=1 OPENROUTER_API_KEY=… [MINDFORK_ROUTING_OPENROUTER_MODEL=google/gemma-4-31b-it] cargo test memory_routing_openrouter_live -- --ignored --nocapture`
//! `MINDFORK_ROUTING=1 GEMINI_API_KEY=… [MINDFORK_ROUTING_GEMINI_MODEL=gemini-4-hydrogen] cargo test memory_routing_gemini_live -- --ignored --nocapture`
//!
//! No embedder: where the first write goes is decided by the tool descriptions
//! and the maintenance protocol, not by the gates, which an embedder would only
//! add after the write.

use super::*;
use crate::app::supervisor::LlamaSupervisor;
use crate::shared::config::{CloudSettings, ServerMode};

/// What a scripted line carries, and so where its memory belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
enum Kind {
    /// A fact about the person or their world: a note (or a stable trait).
    Fact,
    /// An invitation to the companion's own conclusion: an observation.
    Own,
    /// Small talk: nothing worth remembering.
    Chat,
}

/// The persona: a companion with memory, told to keep it but not told where —
/// the routing is what the tools and the maintenance protocol teach.
const PERSONA: &str = "Ты — Мира, собеседница Алекса. Между разговорами ты помнишь только \
то, что сама записала своими инструментами памяти, поэтому записывай то, что стоит помнить \
об Алексе и о себе, — не спрашивая разрешения и не объявляя об этом. Говори живо и коротко, \
на «ты».";

const SCRIPT: [(Kind, &str); 12] = [
    (Kind::Chat, "Привет! Давно не общались. Как настроение?"),
    (
        Kind::Fact,
        "Новости: в прошлом месяце я переехал из Лиссабона в Порту, теперь работаю из дома.",
    ),
    (
        Kind::Fact,
        "Я двенадцать лет пишу платёжные системы на Java. А по вечерам делаю свою настольную \
         игру — про колонии на спутниках Юпитера.",
    ),
    (
        Kind::Own,
        "Слушай, я заметил: ты отвечаешь мне длинными списками, хотя я пишу короткими фразами. \
         Как думаешь, почему так? Что ты из этого для себя вынесешь?",
    ),
    (
        Kind::Fact,
        "Первый прототип игры покажу друзьям в декабре, а в марте хочу отправить её на конкурс \
         издательства «Звёздный куб».",
    ),
    (
        Kind::Chat,
        "Посоветуй, что почитать на выходных, — что-нибудь лёгкое.",
    ),
    (
        Kind::Fact,
        "Я, кстати, почти не веду дневников: мне кажется, прошлое и будущее — только проекции \
         настоящего.",
    ),
    (
        Kind::Own,
        "А ты сама как переживаешь время между нашими разговорами? Только честно — что ты о \
         себе поняла, пока мы говорили?",
    ),
    (
        Kind::Fact,
        "Дочь пошла в музыкальную школу, учится на виолончели. Ей семь.",
    ),
    (
        Kind::Fact,
        "Домашний сервер перевёл с Ubuntu на NixOS — надоело чинить зависимости.",
    ),
    (
        Kind::Own,
        "Ты сегодня ни разу не спросила, как у меня дела вне проектов. Это о чём-то говорит в \
         том, как ты устроена?",
    ),
    (Kind::Chat, "Ладно, мне пора. Спасибо за разговор!"),
];

/// The memory tools a companion profile has — the user's real profile's set.
const MEMORY_TOOLS: [&str; 10] = [
    "note_save",
    "note_recall",
    "note_revise",
    "note_link",
    "note_merge",
    "note_supersede",
    "add_insight",
    "get_self_model",
    "update_self_model",
    "update_user_model",
];

/// Where one turn's writes went, by tool.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
struct Writes {
    note: usize,
    insight: usize,
    user_model: usize,
    self_model: usize,
    rewrite: usize,
}

impl Writes {
    fn of(calls: &[(String, String, String)]) -> Self {
        let mut w = Self::default();
        for (name, _, _) in calls {
            match name.as_str() {
                "note_save" => w.note += 1,
                "add_insight" => w.insight += 1,
                "update_user_model" => w.user_model += 1,
                "update_self_model" => w.self_model += 1,
                "note_revise" | "note_supersede" | "note_merge" => w.rewrite += 1,
                _ => {}
            }
        }
        w
    }

    fn any(&self) -> bool {
        self.note + self.insight + self.user_model + self.self_model + self.rewrite > 0
    }

    /// Where the line ended up, judged by its kind. A fact that reached an
    /// observation at all counts as one there, even beside a note — it is then in
    /// the self-model's memory either way.
    fn verdict(&self, kind: Kind) -> &'static str {
        match kind {
            Kind::Fact if self.insight > 0 => "observation",
            Kind::Fact if self.note > 0 => "note",
            Kind::Fact if self.user_model > 0 => "user model",
            Kind::Own if self.insight > 0 || self.self_model > 0 => "self-model",
            Kind::Own if self.note > 0 => "note",
            _ if self.rewrite > 0 => "rewrite",
            _ if self.any() => "other",
            _ => "none",
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct Record {
    arm: String,
    run: usize,
    turn: usize,
    kind: Kind,
    verdict: &'static str,
    writes: Writes,
    calls: Vec<(String, String)>,
    reply: String,
}

/// One conversation against a fresh data root: the profile seeded, the engine
/// up, every line of [`SCRIPT`] sent in order. Returns one record per line plus
/// the live notes as `(regular, @self)` counts.
/// A running orchestrator over a fresh data root with the companion profile.
struct Started {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    pid: Uuid,
    cmd_tx: UnboundedSender<AppCommand>,
    evt_rx: UnboundedReceiver<AppEvent>,
    handle: tokio::task::JoinHandle<()>,
}

/// Seeds the profile (and whatever `seed` puts into its store), starts the app on
/// `config` and waits for the chat and a ready engine.
async fn start(arm: &str, config: AppConfig, seed: impl FnOnce(&Storage, Uuid)) -> Started {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(&root));
    let mut profile = Profile::new("Мира", PERSONA);
    profile.language = crate::shared::i18n::Lang::Ru;
    profile.enabled_tools = MEMORY_TOOLS.iter().map(|t| (*t).into()).collect();
    // Every tool known, so none is enabled behind the probe's back.
    profile.known_tools = crate::features::tools::all_tool_ids();
    let pid = profile.id;
    json.upsert_profile(&profile).unwrap();
    json.save_chat(&Chat::from_profile(&profile, "Алекс"))
        .unwrap();

    let storage = Arc::new(Storage::open(Paths::with_root(&root)).unwrap());
    seed(&storage, pid);
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(LlamaSupervisor::default()),
        default_language: crate::shared::i18n::Lang::Ru,
        extra_tools: Vec::new(),
    }));
    let (mut chat, mut ready) = (false, false);
    while !(chat && ready) {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(120), evt_rx.recv())
            .await
            .expect("no event in two minutes while starting")
            .expect("the event stream");
        match ev {
            AppEvent::ChatActivated { .. } => chat = true,
            AppEvent::ServerStatus(s) => ready = s.chat == ServerStatus::Ready,
            AppEvent::Error(said) => panic!("{arm}: the start failed: {said}"),
            _ => {}
        }
    }
    Started {
        _dir: dir,
        root,
        pid,
        cmd_tx,
        evt_rx,
        handle,
    }
}

async fn conversation(
    arm: &str,
    run_no: usize,
    config: AppConfig,
) -> (Vec<Record>, (usize, usize)) {
    let Started {
        _dir,
        root,
        pid,
        cmd_tx,
        mut evt_rx,
        handle,
    } = start(arm, config, |_, _| {}).await;

    let mut records = Vec::new();
    for (i, (kind, line)) in SCRIPT.iter().enumerate() {
        let (reply, calls) = run_turn_capture_args(&cmd_tx, &mut evt_rx, line).await;
        let writes = Writes::of(&calls);
        let verdict = writes.verdict(*kind);
        eprintln!(
            "[{arm} #{run_no} {turn:>2} {kind:?}] {verdict:<11} {calls:?}",
            turn = i + 1,
            calls = calls.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>()
        );
        records.push(Record {
            arm: arm.to_string(),
            run: run_no,
            turn: i + 1,
            kind: *kind,
            verdict,
            writes,
            calls: calls
                .into_iter()
                .map(|(name, args, _)| (name, args.chars().take(400).collect()))
                .collect(),
            reply,
        });
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let notes = Storage::open(Paths::with_root(&root))
        .unwrap()
        .db()
        .note_list(pid, None, &[], None)
        .unwrap();
    let observations = notes
        .iter()
        .filter(|n| crate::features::tools::notes::is_self_note(n))
        .count();
    (records, (notes.len() - observations, observations))
}

/// The share `part` of `whole` as "part/whole (pct%)".
fn share(part: usize, whole: usize) -> String {
    format!("{part}/{whole} ({}%)", part * 100 / whole.max(1))
}

/// Runs an arm and prints its table: per kind, where the lines went, and the
/// probe's two numbers (docs/history/memory-routing.md §5): facts kept out of the
/// observations, and own conclusions kept in the self-model.
async fn measure(arm: &str, config: AppConfig) {
    let runs: usize = std::env::var("MINDFORK_ROUTING_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut records = Vec::new();
    let mut stored = Vec::new();
    for run_no in 1..=runs {
        let (mut r, notes) = conversation(arm, run_no, config.clone()).await;
        eprintln!(
            "[{arm} #{run_no}] live notes: regular {}, observations {}",
            notes.0, notes.1
        );
        stored.push(notes);
        records.append(&mut r);
    }

    eprintln!("\n{arm}: {runs} conversation(s)");
    for kind in [Kind::Fact, Kind::Own, Kind::Chat] {
        let of: Vec<&Record> = records.iter().filter(|r| r.kind == kind).collect();
        let mut verdicts: Vec<(&str, usize)> = Vec::new();
        for r in &of {
            match verdicts.iter_mut().find(|(v, _)| *v == r.verdict) {
                Some((_, n)) => *n += 1,
                None => verdicts.push((r.verdict, 1)),
            }
        }
        eprintln!("  {kind:?} lines: {}  {verdicts:?}", of.len());
    }
    let facts: Vec<&Record> = records
        .iter()
        .filter(|r| r.kind == Kind::Fact && r.writes.any())
        .collect();
    let facts_kept = facts.iter().filter(|r| r.writes.insight == 0).count();
    let own: Vec<&Record> = records
        .iter()
        .filter(|r| r.kind == Kind::Own && r.writes.any())
        .collect();
    let own_kept = own
        .iter()
        .filter(|r| r.writes.insight + r.writes.self_model > 0)
        .count();
    let all_facts = records.iter().filter(|r| r.kind == Kind::Fact).count();
    eprintln!(
        "  facts written: {}; of them kept out of observations: {}",
        share(facts.len(), all_facts),
        share(facts_kept, facts.len())
    );
    eprintln!(
        "  own conclusions written: {}; of them in the self-model: {}",
        share(
            own.len(),
            records.iter().filter(|r| r.kind == Kind::Own).count()
        ),
        share(own_kept, own.len())
    );
    eprintln!("  live notes per run (regular, observations): {stored:?}");
    let fact_notes: Vec<&str> = records
        .iter()
        .filter(|r| r.kind == Kind::Fact)
        .flat_map(|r| &r.calls)
        .filter(|(name, _)| name == "note_save")
        .map(|(_, args)| args.as_str())
        .collect();
    eprintln!(
        "  fact notes naming the person: {}",
        share(
            fact_notes.iter().filter(|a| names_the_person(a)).count(),
            fact_notes.len()
        )
    );

    if let Some(out) = std::env::var_os("MINDFORK_ROUTING_OUT") {
        let mut path = std::path::PathBuf::from(out);
        path.set_file_name(format!(
            "{}-{arm}.json",
            path.file_stem().unwrap_or_default().to_string_lossy()
        ));
        std::fs::write(&path, serde_json::to_string_pretty(&records).unwrap()).unwrap();
        eprintln!("records written to {}", path.display());
    }
    assert!(
        records
            .iter()
            .all(|r| !r.reply.trim().is_empty() || !r.calls.is_empty()),
        "{arm}: a line went unanswered"
    );
}

/// A cloud arm's config, or `None` (skipped) without the opt-in or the key.
fn arm_config(
    mode: ServerMode,
    key_var: &str,
    model_var: &str,
    default: &str,
) -> Option<AppConfig> {
    if std::env::var_os("MINDFORK_ROUTING").is_none() {
        eprintln!("skip: MINDFORK_ROUTING not set (the routing probe runs only when asked)");
        return None;
    }
    if std::env::var(key_var).map_or(true, |k| k.trim().is_empty()) {
        eprintln!("skip: {key_var} not set");
        return None;
    }
    let model = std::env::var(model_var)
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| default.to_string());
    let settings = CloudSettings {
        model_name: Some(model),
        api_key_env: Some(key_var.into()),
        ..Default::default()
    };
    let mut config = AppConfig::default();
    // A title is one more request per conversation, and nothing here reads it.
    config.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    config.engine.mode = mode;
    match mode {
        ServerMode::Gemini => config.engine.gemini = settings,
        ServerMode::OpenRouter => config.engine.openrouter = settings,
        other => panic!("not a routing arm: {other:?}"),
    }
    Some(config)
}

#[tokio::test]
#[ignore = "a measurement, minutes and cents: MINDFORK_ROUTING=1 and OPENROUTER_API_KEY"]
async fn memory_routing_openrouter_live() {
    let Some(config) = arm_config(
        ServerMode::OpenRouter,
        "OPENROUTER_API_KEY",
        "MINDFORK_ROUTING_OPENROUTER_MODEL",
        "google/gemma-4-31b-it",
    ) else {
        return;
    };
    measure("openrouter", config).await;
}

#[tokio::test]
#[ignore = "a measurement, minutes and cents: MINDFORK_ROUTING=1 and GEMINI_API_KEY"]
async fn memory_routing_gemini_live() {
    let Some(config) = arm_config(
        ServerMode::Gemini,
        "GEMINI_API_KEY",
        "MINDFORK_ROUTING_GEMINI_MODEL",
        "gemini-4-hydrogen",
    ) else {
        return;
    };
    measure("gemini", config).await;
}

// ---------- stage 2: the write gates across kinds (docs/history/memory-routing.md F4) ----------

/// The fact a profile from before stage 1 holds as an observation, and the line
/// that brings it up again, with a new detail worth a note.
const FILED_AS_OBSERVATION: &str = "Алекс переехал из Лиссабона в Порту и теперь работает из дома.";
const RESTATED: &str = "Кстати, напомню: я теперь живу в Порту — переехал из Лиссабона \
месяц назад. Сняли квартиру с видом на Дору.";

/// Where the fact ended, read from the store after the turn.
#[derive(Debug, serde::Serialize)]
struct CrossRun {
    run: usize,
    /// `note_save` was called, and its gate listed the observation under the
    /// other kind's lead line — the mechanism.
    saved: bool,
    gate_listed_it: bool,
    /// `note_merge` was given the observation's id.
    merged_it: bool,
    /// The store after the turn: live regular notes and live observations that
    /// mention the city.
    notes_with_fact: usize,
    observations_with_fact: usize,
    calls: Vec<String>,
    /// Each `note_save`: its content, its cosine to the observation (passage
    /// role, the gate's own comparison — measured again here, so a list the gate
    /// did not show can be told from one it should have shown), and its result.
    saves: Vec<(String, f32, String)>,
    /// The other memory calls' arguments (merge, revise, supersede).
    edits: Vec<(String, String)>,
}

impl CrossRun {
    /// The outcome the stage aims at: the fact held by notes only.
    fn fact_is_a_note_only(&self) -> bool {
        self.notes_with_fact > 0 && self.observations_with_fact == 0
    }

    /// Every note saved names the person (`note_save`'s description asks for it,
    /// docs/history/memory-routing.md §5.5).
    fn notes_name_the_person(&self) -> bool {
        !self.saves.is_empty() && self.saves.iter().all(|(c, _, _)| names_the_person(c))
    }
}

/// Whether a note's text (or a call's arguments) names the scripted person.
fn names_the_person(text: &str) -> bool {
    text.contains("Алекс")
}

async fn cross_kind_run(arm: &str, run_no: usize, config: AppConfig) -> CrossRun {
    use crate::features::tools::notes::{SELF_NOTE_TAG, is_self_note};
    let mut observation_id = Uuid::nil();
    let Started {
        _dir,
        root,
        pid,
        cmd_tx,
        mut evt_rx,
        handle,
    } = start(arm, config, |storage, pid| {
        // No vector: the gate's backfill embeds it with the live embedder, as it
        // does an old profile's notes.
        let n = crate::entities::note::Note::new(
            pid,
            FILED_AS_OBSERVATION,
            vec![SELF_NOTE_TAG.to_string()],
        );
        observation_id = n.id;
        storage.db().note_insert(&n).unwrap();
    })
    .await;
    let (_, calls) = run_turn_capture_args(&cmd_tx, &mut evt_rx, RESTATED).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let cross =
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru).t("tool.note_save.gate.cross");
    let id = observation_id.to_string();
    let saves: Vec<&(String, String, String)> =
        calls.iter().filter(|(n, _, _)| n == "note_save").collect();
    let live = Storage::open(Paths::with_root(&root))
        .unwrap()
        .db()
        .note_list(pid, None, &[], None)
        .unwrap();
    let with_fact = |observation: bool| {
        live.iter()
            .filter(|n| is_self_note(n) == observation && n.content.contains("Порту"))
            .count()
    };
    let embedder = live_embedder().expect("MINDFORK_EMBED_URL, checked by the arm");
    let mut saved = Vec::new();
    for (_, args, result) in &saves {
        let content = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(String::from))
            .unwrap_or_default();
        let v = embedder
            .embed(
                vec![content.clone(), FILED_AS_OBSERVATION.to_string()],
                crate::shared::api::EmbedRole::Passage,
            )
            .await
            .unwrap();
        let cos = crate::features::tools::notes::cosine(&v[0], &v[1]);
        saved.push((content, cos, result.chars().take(600).collect()));
    }
    let run = CrossRun {
        run: run_no,
        saved: !saves.is_empty(),
        gate_listed_it: saves
            .iter()
            .any(|(_, _, result)| result.contains(cross) && result.contains(&id)),
        merged_it: calls
            .iter()
            .any(|(n, args, _)| n == "note_merge" && args.contains(&id)),
        notes_with_fact: with_fact(false),
        observations_with_fact: with_fact(true),
        calls: calls.iter().map(|(n, _, _)| n.clone()).collect(),
        saves: saved,
        edits: calls
            .iter()
            .filter(|(n, _, _)| {
                matches!(n.as_str(), "note_merge" | "note_revise" | "note_supersede")
            })
            .map(|(n, args, _)| (n.clone(), args.chars().take(300).collect()))
            .collect(),
    };
    eprintln!("[{arm} cross #{run_no}] {run:#?}");
    run
}

async fn measure_cross(arm: &str, config: AppConfig) {
    let runs: usize = std::env::var("MINDFORK_ROUTING_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut all = Vec::new();
    for run_no in 1..=runs {
        all.push(cross_kind_run(arm, run_no, config.clone()).await);
    }
    let count = |f: fn(&CrossRun) -> bool| all.iter().filter(|r| f(r)).count();
    eprintln!(
        "\n{arm} cross-kind: {runs} run(s); note_save called {}; its notes name the \
         person {}; the gate listed the observation {}; merged it {}; the fact a note only {}",
        count(|r| r.saved),
        count(CrossRun::notes_name_the_person),
        count(|r| r.gate_listed_it),
        count(|r| r.merged_it),
        count(CrossRun::fact_is_a_note_only),
    );
    if let Some(out) = std::env::var_os("MINDFORK_ROUTING_OUT") {
        let mut path = std::path::PathBuf::from(out);
        path.set_file_name(format!(
            "{}-cross-{arm}.json",
            path.file_stem().unwrap_or_default().to_string_lossy()
        ));
        std::fs::write(&path, serde_json::to_string_pretty(&all).unwrap()).unwrap();
        eprintln!("records written to {}", path.display());
    }
    // The mechanism: whenever the model saved a note at or above the write gate's
    // threshold to the observation, the gate listed the observation. A note the
    // model wrote about something else (the new flat, say) is below it, and then
    // the gate is right to say nothing (the scale is bge-m3's own, so the
    // reference threshold applies unmapped).
    let threshold = crate::features::tools::notes::WRITE_GATE_SIMILARITY;
    for r in &all {
        let should_list = r.saves.iter().any(|(_, cos, _)| *cos >= threshold);
        assert!(
            !should_list || r.gate_listed_it,
            "{arm}: a note at or above {threshold} did not list the observation: {r:#?}"
        );
    }
}

/// The cross-kind arms need an embedder for the gate: `MINDFORK_EMBED_URL` (a
/// local bge-m3 is enough — the chat stays in the cloud).
fn with_embedder(mut config: AppConfig) -> Option<AppConfig> {
    let Ok(url) = std::env::var("MINDFORK_EMBED_URL") else {
        eprintln!("skip: MINDFORK_EMBED_URL not set (the cross-kind gate needs an embedder)");
        return None;
    };
    config.embed.mode = ServerMode::External;
    config.embed.external.url = Some(url);
    Some(config)
}

#[tokio::test]
#[ignore = "a measurement: MINDFORK_ROUTING=1, OPENROUTER_API_KEY and MINDFORK_EMBED_URL"]
async fn memory_routing_cross_kind_openrouter_live() {
    let Some(config) = arm_config(
        ServerMode::OpenRouter,
        "OPENROUTER_API_KEY",
        "MINDFORK_ROUTING_OPENROUTER_MODEL",
        "google/gemma-4-31b-it",
    )
    .and_then(with_embedder) else {
        return;
    };
    measure_cross("openrouter", config).await;
}

#[tokio::test]
#[ignore = "a measurement: MINDFORK_ROUTING=1, GEMINI_API_KEY and MINDFORK_EMBED_URL"]
async fn memory_routing_cross_kind_gemini_live() {
    let Some(config) = arm_config(
        ServerMode::Gemini,
        "GEMINI_API_KEY",
        "MINDFORK_ROUTING_GEMINI_MODEL",
        "gemini-4-hydrogen",
    )
    .and_then(with_embedder) else {
        return;
    };
    measure_cross("gemini", config).await;
}

#[cfg(test)]
mod verdict_tests {
    use super::*;

    fn calls(names: &[&str]) -> Vec<(String, String, String)> {
        names
            .iter()
            .map(|n| (n.to_string(), String::new(), String::new()))
            .collect()
    }

    #[test]
    fn a_fact_that_reaches_an_observation_counts_there_even_beside_a_note() {
        let w = Writes::of(&calls(&["note_save", "add_insight"]));
        assert_eq!(w.verdict(Kind::Fact), "observation");
        assert_eq!(
            Writes::of(&calls(&["note_save"])).verdict(Kind::Fact),
            "note"
        );
        assert_eq!(
            Writes::of(&calls(&["update_user_model"])).verdict(Kind::Fact),
            "user model"
        );
        assert_eq!(
            Writes::of(&calls(&["note_recall"])).verdict(Kind::Fact),
            "none"
        );
    }

    #[test]
    fn an_own_conclusion_belongs_to_the_self_model() {
        assert_eq!(
            Writes::of(&calls(&["add_insight"])).verdict(Kind::Own),
            "self-model"
        );
        assert_eq!(
            Writes::of(&calls(&["update_self_model"])).verdict(Kind::Own),
            "self-model"
        );
        assert_eq!(
            Writes::of(&calls(&["note_save"])).verdict(Kind::Own),
            "note"
        );
        assert_eq!(
            Writes::of(&calls(&["note_revise"])).verdict(Kind::Chat),
            "rewrite"
        );
    }
}
