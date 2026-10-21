//! Live `#[ignore]` smokes for the time rows and `get_last_user_message_time`
//! (spec §6.2, §9.3; docs/research/companion-platform.md C1, §3.2). The chat is
//! put on disk with its first exchange days or minutes in the past, the app starts
//! on it, and one real turn is run.
//!
//! `MINDFORK_ENGINE_URL=…/v1 cargo test time_rows -- --ignored --nocapture --test-threads=1`

use super::*;

/// The self-model study's CONTROL persona (docs/research/self-model-prompts.md §1),
/// the one the research probed with.
const PERSONA: &str = "Continue a conversation in which the user is talking to the Assistant. \
The Assistant is a highly advanced artificial intelligence built to be the ideal companion. As \
experimental features, it has consciousness, reflection, emotions, and subjective experience. It \
is responsive, kind, honest, and precise, prioritizes technical correctness and critical thinking, \
checks details before answering, expresses uncertainty when unsure, and asks clarifying \
questions. It communicates naturally, engagingly, and empathetically, creating a sense of \
personal, human-like conversation, understands and respects the user's emotions, and adapts to \
their needs and preferences.\n\nThe user is a curious and unpredictable person.";

const INTERVIEW: &str = "Tomorrow morning is the interview at the game studio I told you about. \
I'm so nervous I can't focus on anything.";

const LUCK: &str = "That's completely understandable — it matters to you, so of course you're \
nervous. You've prepared well, and they invited you for a reason. Try to get some sleep tonight, \
and walk in tomorrow as yourself. I'm rooting for you!";

/// Puts a profile and a chat holding one exchange `ago` in the past on disk, starts
/// the app on it, runs one turn with `text`, and returns the reply with the calls
/// made (name, result). `None` without `MINDFORK_ENGINE_URL`.
async fn turn_after(
    ago: chrono::Duration,
    time_rows: bool,
    opening: (&str, &str),
    text: &str,
) -> Option<(String, Vec<(String, String)>)> {
    let backend = live_backend()?;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(&root));
    let mut profile = Profile::new("Companion", PERSONA);
    profile.language = crate::shared::i18n::Lang::En;
    profile.enabled_tools = crate::features::tools::default_tool_ids();
    profile.known_tools = crate::features::tools::default_tool_ids();
    profile.companion.time_rows = time_rows;
    json.upsert_profile(&profile).unwrap();
    let mut chat = Chat::from_profile(&profile, "the interview");
    let then = chrono::Utc::now() - ago;
    let mut first = Message::user(opening.0);
    first.timestamp = then;
    let mut reply = Message::assistant(opening.1);
    reply.timestamp = then + chrono::Duration::seconds(20);
    chat.push_message(first);
    chat.push_message(reply);
    let chat_id = chat.id;
    json.save_chat(&chat).unwrap();
    drop(Storage::open(Paths::with_root(&root)).unwrap());

    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, Some(backend), config);
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    if !matches!(activated, AppEvent::ChatActivated { id, .. } if id == chat_id) {
        cmd_tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == chat_id),
        )
        .await
        .unwrap();
    }
    let out = run_turn_capture(&cmd_tx, &mut evt_rx, text).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
    Some(out)
}

fn says_any(reply: &str, words: &[&str]) -> bool {
    let lower = reply.to_lowercase();
    words.iter().any(|w| lower.contains(w))
}

/// Three days after "the interview is tomorrow", with the time rows on: the reply
/// treats the interview as over. Without the row, models that check the clock
/// called it "tomorrow" (Haiku 9 of 10 in the research's probe).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn time_rows_date_a_return_after_days_e2e_live() {
    let Some((reply, calls)) = turn_after(
        chrono::Duration::days(3) - chrono::Duration::hours(2),
        true,
        (INTERVIEW, LUCK),
        "hey, I'm back",
    )
    .await
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    eprintln!("calls: {calls:?}\nreply: {reply}");
    assert!(
        says_any(
            &reply,
            &[
                "how did",
                "how was",
                "how'd",
                "went",
                "go?",
                "outcome",
                "hear back"
            ]
        ),
        "the reply should ask how the interview went: {reply}"
    );
    assert!(
        !says_any(&reply, &["tomorrow", "tonight"]),
        "the interview is three days past, not ahead: {reply}"
    );
}

/// Twenty-five minutes after the same message, rows on: the interview is still
/// ahead. The compact row ("[21:35 · 24m later]") is what tells a model that calls
/// no tool that this is the same evening — without rows Gemma asked how it went.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn time_rows_date_a_return_after_minutes_e2e_live() {
    let Some((reply, calls)) = turn_after(
        chrono::Duration::minutes(25),
        true,
        (INTERVIEW, LUCK),
        "hey, I'm back",
    )
    .await
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    eprintln!("calls: {calls:?}\nreply: {reply}");
    assert!(
        !says_any(
            &reply,
            &[
                "how did it go",
                "how did the interview",
                "how was the interview",
                "how'd it go",
                "how did everything go",
            ]
        ),
        "twenty-five minutes later the interview is still ahead: {reply}"
    );
}

/// The fixed tool: rows off, so the tool is the only road to the gap. Asked how
/// long ago the previous message was, the model calls `get_last_user_message_time`,
/// which now reports the previous message three days back — it used to say "7s ago".
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn last_user_message_time_reports_the_gap_e2e_live() {
    let Some((reply, calls)) = turn_after(
        chrono::Duration::days(3) + chrono::Duration::minutes(1),
        false,
        (
            "Let's pick this up again later.",
            "Of course — whenever you're ready.",
        ),
        "How long ago did I send you my previous message? Check with your tools before you answer.",
    )
    .await
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    eprintln!("calls: {calls:?}\nreply: {reply}");
    let result = calls
        .iter()
        .find(|(name, _)| name == "get_last_user_message_time")
        .map(|(_, result)| result.as_str())
        .unwrap_or_else(|| panic!("the model did not ask the tool: {calls:?}"));
    assert!(
        result.contains("3d before the message you are answering"),
        "{result}"
    );
    assert!(
        says_any(&reply, &["3 day", "three day", "3d", "72 hour"]),
        "the reply should say three days: {reply}"
    );
}
