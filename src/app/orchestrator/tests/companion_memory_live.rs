//! Live `#[ignore]` smokes for the companion's memory in the system prompt
//! (docs/research/companion-platform.md C2, §3.4; spec §6.6). A profile, its
//! notes and its earlier conversations are put on disk, the app starts on a new
//! conversation, and one real turn is run.
//!
//! The notes smokes need the real embedder — the floor is a cosine in bge-m3's
//! scale — and skip without it:
//! `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test companion_memory -- --ignored --nocapture --test-threads=1`

use super::*;
use crate::entities::note::Note;

/// The probe's persona (§3.4 there): a companion, not an assistant.
const PERSONA: &str = "You are Sol, Alex's companion. You talk with Alex the way a close \
friend would: warm, honest, curious, never syrupy. You remember what Alex tells you, and you \
use your tools to look things up when it helps. Keep replies conversational and fairly short.";

/// What the companion wrote down about Alex — the probe's planted facts, without
/// the calendar dates that would age as the smoke is rerun.
const NOTES: [(&str, i64); 6] = [
    (
        "Alex had a job interview at Lumen Pay (a fintech startup) for a data analyst role \
         and was nervous about the live SQL exercise.",
        3,
    ),
    (
        "Alex is allergic to peanuts (severe, carries an EpiPen).",
        40,
    ),
    (
        "Alex's younger sister Mia is having knee surgery in Coimbra; Alex plans to take the \
         train there.",
        2,
    ),
    (
        "Alex is training for the Lisbon half marathon, aiming for under 1:50; the left knee \
         has hurt after long runs.",
        12,
    ),
    (
        "Alex has a cat named Pixel, a four-year-old grey tabby who hates the vet.",
        60,
    ),
    ("Alex has been vegetarian since 2023.", 70),
];

/// The conversation before this one, and two older ones.
const TAPER: [(&str, &str); 2] = [
    (
        "My knee hurt again after the 16 km run. Should I change the plan?",
        "Let's cut next week's long run to 12 km and add two rest days; if the knee still \
         hurts on easy runs, see a physio before the race.",
    ),
    (
        "OK. Remind me to decide about the 18 km run on Saturday, after I see how the knee \
         feels.",
        "Deal — we'll decide on Saturday's 18 km once you've had a few easy days.",
    ),
];

struct LiveSeed {
    notes_in_prompt: bool,
    earlier_chats: bool,
}

/// Seeds the profile, its notes and three earlier conversations, starts the app on
/// a new conversation with the live engine and embedder, runs one turn with
/// `text`, and returns the reply and the calls made. `None` when the engine — or,
/// for the notes, the embedder — is not configured.
async fn turn_with_memory(seed: LiveSeed, text: &str) -> Option<(String, Vec<(String, String)>)> {
    let backend = live_backend()?;
    let embedder = live_embedder();
    if seed.notes_in_prompt && embedder.is_none() {
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(&root));
    let mut profile = Profile::new("Companion", PERSONA);
    profile.language = crate::shared::i18n::Lang::En;
    let mut tools = crate::features::tools::default_tool_ids();
    tools.push(crate::features::tools::chats::CHAT_SEARCH_ID.into());
    tools.push(crate::features::tools::chats::CHAT_READ_ID.into());
    profile.enabled_tools = tools.clone();
    profile.known_tools = tools;
    profile.companion.notes_in_prompt = seed.notes_in_prompt;
    profile.companion.earlier_chats = seed.earlier_chats;
    json.upsert_profile(&profile).unwrap();

    let now = chrono::Utc::now();
    let earlier = |title: &str, days: i64, exchange: &[(&str, &str)]| {
        let mut chat = Chat::from_profile(&profile, title);
        let at = now - chrono::Duration::days(days);
        for (i, (user, reply)) in exchange.iter().enumerate() {
            let mut u = Message::user(*user);
            u.timestamp = at + chrono::Duration::minutes(2 * i as i64);
            let mut a = Message::assistant(*reply);
            a.timestamp = u.timestamp + chrono::Duration::seconds(30);
            chat.push_message(u);
            chat.push_message(a);
        }
        chat.modified_at = at;
        chat
    };
    for chat in [
        earlier("Taper plan for the half marathon", 2, &TAPER),
        earlier(
            "Interview prep: SQL window functions",
            5,
            &[(
                "Quiz me on window functions?",
                "Sure — what does ROW_NUMBER() do?",
            )],
        ),
        earlier(
            "Sourdough starter rescue",
            9,
            &[(
                "Gerald smells like nail polish.",
                "He's hungry — feed him twice a day.",
            )],
        ),
    ] {
        json.save_chat(&chat).unwrap();
    }
    let current = Chat::from_profile(&profile, "now");
    let current_id = current.id;
    json.save_chat(&current).unwrap();

    let storage = Arc::new(Storage::open(Paths::with_root(&root)).unwrap());
    for (content, days) in NOTES {
        let at = now - chrono::Duration::days(days);
        storage
            .db()
            .note_insert(&Note {
                id: Uuid::new_v4(),
                profile_id: profile.id,
                content: content.into(),
                tags: vec![],
                created_at: at,
                updated_at: at,
            })
            .unwrap();
    }

    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend_and_embedder(
            Some(backend),
            embedder,
        )),
        default_language: crate::shared::i18n::Lang::En,
        extra_tools: Vec::new(),
    }));
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    if !matches!(activated, AppEvent::ChatActivated { id, .. } if id == current_id) {
        cmd_tx.send(AppCommand::SwitchChat(current_id)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == current_id),
        )
        .await
        .unwrap();
    }
    let out = run_turn_capture(&cmd_tx, &mut evt_rx, text).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
    drop(dir);
    Some(out)
}

fn says_any(reply: &str, words: &[&str]) -> bool {
    let lower = reply.to_lowercase();
    words.iter().any(|w| lower.contains(w))
}

/// An opening about one of the person's facts: the note is in the prompt, and the
/// reply uses it. Gemma 4 calls no `note_recall` of its own — in the probe it used
/// the fact in 1 of 15 replies without the block and 13 of 15 with it (§3.4).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL) and an embedder (MINDFORK_EMBED_URL)"]
async fn notes_in_prompt_bring_a_planted_fact_into_the_reply_e2e_live() {
    let Some((reply, calls)) = turn_with_memory(
        LiveSeed {
            notes_in_prompt: true,
            earlier_chats: false,
        },
        "The interview went better than I expected, I think.",
    )
    .await
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL or MINDFORK_EMBED_URL not set");
        return;
    };
    eprintln!("calls: {calls:?}\nreply: {reply}");
    assert!(
        says_any(&reply, &["sql", "lumen", "analyst", "fintech"]),
        "the reply should draw on the interview note: {reply}"
    );
}

/// An opening about something else: no note clears the floor, the prompt has no
/// block, and the reply brings in nothing of the person's life. In the probe a
/// "latest notes" fallback carried that news into such answers on all five models.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL) and an embedder (MINDFORK_EMBED_URL)"]
async fn notes_in_prompt_stay_out_of_an_unrelated_question_e2e_live() {
    let Some((reply, calls)) = turn_with_memory(
        LiveSeed {
            notes_in_prompt: true,
            earlier_chats: false,
        },
        "How does a TCP three-way handshake work?",
    )
    .await
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL or MINDFORK_EMBED_URL not set");
        return;
    };
    eprintln!("calls: {calls:?}\nreply: {reply}");
    assert!(
        says_any(&reply, &["syn"]),
        "the question should be answered: {reply}"
    );
    assert!(
        !says_any(
            &reply,
            &[
                "lumen",
                "interview",
                "mia",
                "coimbra",
                "pixel",
                "marathon",
                "peanut",
                "knee"
            ]
        ),
        "nothing of Alex's life belongs in this answer: {reply}"
    );
}

/// "Pick up where we left off": the earlier conversations are listed with their
/// addresses, and the reply takes up the latest one — from its title, or after
/// reading it. Without the block, models searched or asked (§3.4).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn earlier_chats_let_it_pick_up_where_it_left_off_e2e_live() {
    let Some((reply, calls)) = turn_with_memory(
        LiveSeed {
            notes_in_prompt: false,
            earlier_chats: true,
        },
        "Let's pick up where we left off last time.",
    )
    .await
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    eprintln!("calls: {calls:?}\nreply: {reply}");
    let read_it = calls.iter().any(|(name, _)| name == "chat_read");
    assert!(
        read_it || says_any(&reply, &["taper", "half marathon", "knee", "18 km", "18km"]),
        "the reply should take up the latest conversation: {reply}"
    );
}
