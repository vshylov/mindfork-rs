//! The memory-routing probe (docs/memory-routing.md §5): where a model files what
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
async fn conversation(
    arm: &str,
    run_no: usize,
    config: AppConfig,
) -> (Vec<Record>, (usize, usize)) {
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
/// probe's two numbers (docs/memory-routing.md §5): facts kept out of the
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
