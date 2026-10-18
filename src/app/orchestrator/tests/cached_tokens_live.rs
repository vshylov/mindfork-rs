//! Live smokes of the cache share in the token counter —
//! docs/research/cached-tokens.md §7. Part of the [`super`] module.
//!
//! The production path end to end: the real supervisor builds the engine a
//! mode names (its client, `RetryBackend` around it), the orchestrator runs the
//! turns, and what is asserted is the `AppEvent::TokenUsage` the status bar
//! reads. Each arm runs the same script:
//!
//! 1. a profile whose persona is a long stable text — above every documented
//!    cache minimum (4096 tokens on Haiku 4.5 and Gemini 3.x) — with no tools,
//!    so nothing but the persona and the conversation is in the prompt;
//! 2. two turns in one chat: the second request repeats the first one's whole
//!    prompt, so it can be read from the cache;
//! 3. the **control**: the persona's first byte changed and a new chat, so the
//!    prefix differs from its start and nothing past the template can be reused.
//!
//! On llama.cpp the cache is deterministic (one slot, the same prefix), so the
//! second turn must read a share of it and the control almost none — the
//! control is what proves the figure can say "missed" (lessons §2). On a cloud
//! a hit is the provider's routing and best effort, so the arm asserts only that
//! the figure was parsed, and logs it (lessons §2: a provider's optional output
//! cannot be an assertion).

use super::*;
use crate::app::supervisor::LlamaSupervisor;
use crate::shared::config::{CloudSettings, ExternalSettings, ServerMode};

/// Entries of the persona's archive: about 10 000 tokens on Gemini's
/// tokenizer. At 120 entries (4944 tokens, above its documented 4096) Gemini 3.8
/// read nothing from its cache on the second turn, where the probe's 9825-token
/// prompt had read 7782 (docs/research/cached-tokens.md §3.1, §7).
const ENTRIES: usize = 260;

/// The long stable persona; `lead` is its first character — the control
/// changes it, and with it the prompt's prefix from the start. `session` is a
/// nonce of the run: a provider's cache outlives the process, and without one a
/// run's first request read 44% of its prompt from the run before
/// (docs/research/prompt-caching.md §3.2).
fn persona(lead: char, session: Uuid) -> String {
    let mut text = format!(
        "{lead}ou are a terse archivist (session {session}). Answer in one word. \
         The archive follows.\n\n"
    );
    for i in 0..ENTRIES {
        text.push_str(&format!(
            "Entry {i}. In the year {year} the keeper of lamp {lamp} logged the tide at \
             {hour:02}:{minute:02} and repainted railing {rail} in {colour}.\n",
            year = 1700 + i,
            lamp = i % 7,
            hour = (i * 5) % 24,
            minute = (i * 13) % 60,
            rail = i * 3 + 1,
            colour = ["grey", "white", "ochre", "green", "blue"][i % 5],
        ));
    }
    text
}

/// One request's exact counter as the bar receives it.
#[derive(Debug, Clone, Copy)]
struct Figure {
    context: u64,
    cached: Option<u32>,
}

/// A spun-up orchestrator on the production supervisor, and what its events
/// have said so far — every wait goes through [`Self::next`], so a status or a
/// profile list that arrives while something else is awaited is not lost.
struct Live {
    _dir: tempfile::TempDir,
    cmd: UnboundedSender<AppCommand>,
    rx: UnboundedReceiver<AppEvent>,
    handle: tokio::task::JoinHandle<()>,
    ready: bool,
    profile: Option<Uuid>,
    chat: Option<Uuid>,
    /// What the smoke is waiting for, and the last events it saw — said when a
    /// wait times out, so a silent five minutes names its step.
    phase: &'static str,
    seen: std::collections::VecDeque<String>,
}

impl Live {
    fn start(config: AppConfig) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let (cmd, cmd_rx) = unbounded_channel();
        let (evt_tx, rx) = unbounded_channel();
        let handle = tokio::spawn(run(OrchestratorDeps {
            cmd_rx,
            evt_tx,
            storage,
            config,
            supervisor: Arc::new(LlamaSupervisor::default()),
            default_language: crate::shared::i18n::Lang::default(),
            extra_tools: Vec::new(),
        }));
        Self {
            _dir: dir,
            cmd,
            rx,
            handle,
            ready: false,
            profile: None,
            chat: None,
            phase: "the start",
            seen: std::collections::VecDeque::new(),
        }
    }

    /// The next event, with what it says about the session recorded.
    async fn next(&mut self) -> AppEvent {
        let Ok(ev) =
            tokio::time::timeout(std::time::Duration::from_secs(300), self.rx.recv()).await
        else {
            panic!(
                "no event in five minutes while waiting for {} (ready={}); the last ones: {:#?}",
                self.phase, self.ready, self.seen
            );
        };
        let ev = ev.expect("the event stream");
        if self.seen.len() == 8 {
            self.seen.pop_front();
        }
        self.seen
            .push_back(format!("{ev:?}").chars().take(160).collect());
        match &ev {
            AppEvent::ServerStatus(s) => self.ready = s.chat == ServerStatus::Ready,
            AppEvent::ProfileList(list) => self.profile = list.first().map(|p| p.id),
            AppEvent::ChatActivated { id, .. } => self.chat = Some(*id),
            _ => {}
        }
        ev
    }

    async fn until(&mut self, done: impl Fn(&Self) -> bool) {
        while !done(self) {
            self.next().await;
        }
    }

    /// Gives the profile `persona` and no tools, opens a new chat on it, and
    /// waits until the engine is ready.
    async fn fresh_chat(&mut self, persona: String) {
        self.phase = "the profile list";
        self.until(|s| s.profile.is_some()).await;
        self.cmd
            .send(AppCommand::UpdateProfile {
                id: self.profile.unwrap(),
                edit: Box::new(ProfileEdit {
                    system_message: Some(persona),
                    enabled_tools: Some(Vec::new()),
                    ..Default::default()
                }),
            })
            .unwrap();
        self.chat = None;
        self.cmd
            .send(AppCommand::NewChat {
                profile_id: self.profile,
            })
            .unwrap();
        self.phase = "the new chat and a ready engine";
        self.until(|s| s.chat.is_some() && s.ready).await;
    }

    /// One turn to its end: the exact counters of its rounds, in order. A
    /// turn the engine refuses fails the smoke in the app's words.
    async fn turn(&mut self, text: &str) -> Vec<Figure> {
        self.phase = "a turn to finish";
        self.cmd.send(AppCommand::SendMessage(text.into())).unwrap();
        let mut figures = Vec::new();
        loop {
            match self.next().await {
                AppEvent::TokenUsage {
                    context: Some(context),
                    context_exact: true,
                    cached,
                    ..
                } => figures.push(Figure { context, cached }),
                AppEvent::Error(said) => panic!("the turn failed: {said}"),
                AppEvent::Finished { .. } => return figures,
                _ => {}
            }
        }
    }

    async fn stop(self) {
        self.cmd.send(AppCommand::Quit).unwrap();
        self.handle.await.unwrap();
    }
}

/// The three requests of the script: turn 1, turn 2 of the same chat, and the
/// control — each the first round's exact counter.
struct Run {
    first: Figure,
    second: Figure,
    control: Figure,
}

/// Runs the script on `config` and logs what each request said.
async fn script(label: &str, mut config: AppConfig) -> Run {
    // A short answer is all the script needs; thinking only costs time.
    config.default_sampling.thinking = Some(false);
    let session = Uuid::new_v4();
    let mut live = Live::start(config);
    live.fresh_chat(persona('Y', session)).await;
    let first = live.turn("Reply with the single word: ready.").await;
    let second = live.turn("Reply with the single word: done.").await;
    live.fresh_chat(persona('y', session)).await;
    let control = live.turn("Reply with the single word: ready.").await;
    live.stop().await;
    let head = |figures: &[Figure], which: &str| -> Figure {
        *figures
            .first()
            .unwrap_or_else(|| panic!("{label}: {which} reported no exact usage"))
    };
    let run = Run {
        first: head(&first, "turn 1"),
        second: head(&second, "turn 2"),
        control: head(&control, "the control"),
    };
    for (which, f) in [
        ("turn 1", run.first),
        ("turn 2", run.second),
        ("control", run.control),
    ] {
        eprintln!(
            "{label} {which}: prompt {} cached {:?} ({}%)",
            f.context,
            f.cached,
            f.cached
                .map_or(0, |c| u64::from(c) * 100 / f.context.max(1))
        );
    }
    // What every wire owes, whatever its cache did: the figure is part of the
    // prompt it is a share of (docs/research/cached-tokens.md §4.2).
    for f in [run.first, run.second, run.control] {
        assert!(
            f.cached.is_none_or(|c| u64::from(c) <= f.context),
            "{label}: more cached than the prompt holds: {f:?}"
        );
    }
    run
}

/// A cloud arm: the figure is parsed on every request — a `Some`, `0`
/// included — and the hit is logged, not asserted.
async fn cloud_arm(label: &str, config: AppConfig) {
    let run = script(label, config).await;
    for (which, f) in [
        ("turn 1", run.first),
        ("turn 2", run.second),
        ("control", run.control),
    ] {
        assert!(
            f.cached.is_some(),
            "{label}: {which} carried no cache figure — the wire's field went unread: {f:?}"
        );
    }
}

/// A cloud mode's config: its model (the run's override or `default`) and its
/// key, read from the environment by name as a user without a stored key would
/// have it. `None` — the key is not set (the arm is skipped).
fn cloud_config(
    mode: ServerMode,
    key_var: &str,
    model_var: &str,
    default: &str,
) -> Option<AppConfig> {
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
    let mut config = no_auto_cfg();
    config.engine.mode = mode;
    match mode {
        ServerMode::OpenAi => config.engine.openai = settings,
        ServerMode::Gemini => config.engine.gemini = settings,
        ServerMode::Claude => config.engine.claude = settings,
        ServerMode::OpenRouter => config.engine.openrouter = settings,
        ServerMode::Grok => config.engine.grok = settings,
        other => panic!("not a cloud arm: {other:?}"),
    }
    Some(config)
}

/// The deterministic arm: a llama.cpp (or any OpenAI-compatible server) at
/// `MINDFORK_ENGINE_URL`. The second turn reads part of its prompt from the
/// slot — `cached > 0`, and the prompt still the whole one, larger than the
/// first — and the control, whose prefix differs from its first byte, reads
/// almost nothing.
///
/// `MINDFORK_ENGINE_URL=…/v1 cargo test cached_tokens_e2e_live -- --ignored --nocapture`
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL (a llama.cpp)"]
async fn cached_tokens_e2e_live() {
    let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let mut config = no_auto_cfg();
    config.engine.mode = ServerMode::External;
    config.engine.external = ExternalSettings {
        url: Some(url),
        model_name: std::env::var("MINDFORK_ENGINE_MODEL").ok(),
        // A rented endpoint needs its key; a LAN llama.cpp has none to name.
        api_key_env: std::env::var("MINDFORK_ENGINE_KEY")
            .is_ok()
            .then(|| "MINDFORK_ENGINE_KEY".into()),
        ..Default::default()
    };
    let run = script("external", config).await;
    let (second, control) = (run.second, run.control);
    let cached = second
        .cached
        .expect("the server reported no cache figure (neither prompt_tokens_details nor timings)");
    assert!(
        cached > 0,
        "the second turn read nothing from the slot: {second:?}"
    );
    assert!(
        second.context > run.first.context,
        "the prompt is the whole one, not the uncached part: {:?} then {second:?}",
        run.first
    );
    let control_cached = control.cached.unwrap_or(0);
    assert!(
        u64::from(control_cached) * 20 < control.context,
        "the control's prefix differs from its first byte, yet {control_cached} of {} \
         tokens were read from the cache",
        control.context
    );
}

/// OpenAI Responses.
///
/// `MINDFORK_OPENAI_KEY=… [MINDFORK_CACHED_E2E_OPENAI_MODEL=…] cargo test
/// cached_tokens_openai_e2e_live -- --ignored --nocapture`
#[tokio::test]
#[ignore = "requires MINDFORK_OPENAI_KEY"]
async fn cached_tokens_openai_e2e_live() {
    let Some(config) = cloud_config(
        ServerMode::OpenAi,
        "MINDFORK_OPENAI_KEY",
        "MINDFORK_CACHED_E2E_OPENAI_MODEL",
        "gpt-5.4-mini",
    ) else {
        return;
    };
    cloud_arm("openai", config).await;
}

/// Gemini — where a miss is an absent key, read as `Some(0)`.
///
/// `MINDFORK_GEMINI_KEY=… [MINDFORK_CACHED_E2E_GEMINI_MODEL=…] cargo test
/// cached_tokens_gemini_e2e_live -- --ignored --nocapture`
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY"]
async fn cached_tokens_gemini_e2e_live() {
    let Some(config) = cloud_config(
        ServerMode::Gemini,
        "MINDFORK_GEMINI_KEY",
        "MINDFORK_CACHED_E2E_GEMINI_MODEL",
        "gemini-3.8-flash",
    ) else {
        return;
    };
    cloud_arm("gemini", config).await;
}

/// The OpenRouter mode.
///
/// `MINDFORK_OPENROUTER_KEY=… [MINDFORK_CACHED_E2E_OPENROUTER_MODEL=…] cargo
/// test cached_tokens_openrouter_e2e_live -- --ignored --nocapture`
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn cached_tokens_openrouter_e2e_live() {
    let Some(config) = cloud_config(
        ServerMode::OpenRouter,
        "MINDFORK_OPENROUTER_KEY",
        "MINDFORK_CACHED_E2E_OPENROUTER_MODEL",
        "openai/gpt-5.4-mini",
    ) else {
        return;
    };
    cloud_arm("openrouter", config).await;
}

/// xAI — measured on the wire before this arm existed (cached-tokens.md §9.1):
/// `prompt_tokens_details.cached_tokens`, a block of xAI's own cached even on a
/// cold request, and no write figure.
///
/// `MINDFORK_XAI_KEY=… [MINDFORK_CACHED_E2E_XAI_MODEL=…] cargo test
/// cached_tokens_xai_e2e_live -- --ignored --nocapture`
#[tokio::test]
#[ignore = "requires MINDFORK_XAI_KEY"]
async fn cached_tokens_xai_e2e_live() {
    let Some(config) = cloud_config(
        ServerMode::Grok,
        "MINDFORK_XAI_KEY",
        "MINDFORK_CACHED_E2E_XAI_MODEL",
        "grok-4.5",
    ) else {
        return;
    };
    cloud_arm("xai", config).await;
}

/// Anthropic — nothing asks it to cache, so its figure is a reported zero and
/// its prompt the sum of three fields.
///
/// `MINDFORK_ANTHROPIC_KEY=… [MINDFORK_CACHED_E2E_ANTHROPIC_MODEL=…] cargo
/// test cached_tokens_anthropic_e2e_live -- --ignored --nocapture`
#[tokio::test]
#[ignore = "requires MINDFORK_ANTHROPIC_KEY"]
async fn cached_tokens_anthropic_e2e_live() {
    let Some(config) = cloud_config(
        ServerMode::Claude,
        "MINDFORK_ANTHROPIC_KEY",
        "MINDFORK_CACHED_E2E_ANTHROPIC_MODEL",
        "claude-haiku-4-5",
    ) else {
        return;
    };
    cloud_arm("anthropic", config).await;
}
