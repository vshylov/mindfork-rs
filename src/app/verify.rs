//! `mindfork setup --verify` — start what the settings describe, report what it
//! says about itself, stop it
//! ([docs/research/cloud-provisioning.md](../../docs/research/cloud-provisioning.md)
//! §4.3).
//!
//! A command that says "done" followed by a TUI that says "server failed" is the
//! exact failure one-command provisioning exists to remove. On a rented box this
//! is also where a context that does not fit the card, or a first launch's JIT,
//! shows up *before* the user is looking at a chat — and while a command line
//! can still be edited.
//!
//! It launches **what the app will launch**: the same `managed_config` /
//! `managed_embed_config` the supervisor uses, through the same
//! `ServerHandle::launch` and `wait_until_ready`, with the same timeout. Both
//! servers are up **at the same time**, as they are in the app, because whether
//! the two fit the card together is one of the questions.
//!
//! `app` layer: it needs the supervisor's settings → launch-config mapping.

use std::sync::Arc;
use std::time::Instant;

use crate::app::supervisor::{
    BinaryLookup, MANAGED_READY_TIMEOUT, managed_config, managed_embed_config,
};
use crate::shared::api::{
    EngineBackend, ManagedConfig, OpenAiClient, ServerHandle, VisionSupport, wait_until_ready,
};
use crate::shared::config::{AppConfig, ServerMode};
use crate::shared::i18n::Locale;

/// A server that was started and is being waited for.
struct Started {
    /// `setup.verify.chat` / `setup.verify.embed` — the label's locale key.
    label: &'static str,
    client: Arc<OpenAiClient>,
    handle: ServerHandle,
    since: Instant,
}

/// Starts the managed servers `config` describes, waits for each to become
/// ready, prints one line per server through `progress`, and stops them.
/// `true` — nothing failed (a server that is not *meant* to be managed is a
/// skip, not a failure).
pub async fn run(
    config: &AppConfig,
    lookup: &BinaryLookup,
    loc: &'static Locale,
    mut progress: impl FnMut(&str),
) -> bool {
    let mut ok = true;
    let mut started: Vec<Started> = Vec::new();

    let chat = (config.engine.mode == ServerMode::Managed)
        .then(|| managed_config(&config.engine.managed, lookup));
    let embed = (config.embed.mode == ServerMode::Managed).then(|| {
        let binary = lookup
            .resolve(config.embed.managed.binary.as_deref())
            .unwrap_or_default();
        managed_embed_config(&config.embed.managed, binary)
    });

    for (label, mode, cfg) in [
        ("setup.verify.chat", config.engine.mode, chat),
        ("setup.verify.embed", config.embed.mode, embed),
    ] {
        let name = loc.t(label);
        let Some(cfg) = cfg else {
            progress(&loc.tf(
                "setup.verify.not_managed",
                &[("server", name), ("mode", &mode_name(mode))],
            ));
            continue;
        };
        match start(&cfg, loc) {
            Ok(handle) => {
                progress(&loc.tf(
                    "setup.verify.starting",
                    &[("server", name), ("port", &cfg.port.to_string())],
                ));
                started.push(Started {
                    label,
                    client: Arc::new(OpenAiClient::new(handle.base_url())),
                    handle,
                    since: Instant::now(),
                });
            }
            // The embedder with nothing configured is how most installs run; the
            // chat server with nothing configured is what `--verify` is for.
            Err(Refusal::NotConfigured) if label == "setup.verify.embed" => {
                progress(&loc.tf("setup.verify.embed_unset", &[("server", name)]));
            }
            Err(refusal) => {
                ok = false;
                progress(&loc.tf(
                    "setup.verify.failed",
                    &[("server", name), ("reason", &refusal.localized(&cfg, loc))],
                ));
            }
        }
    }

    for s in &started {
        let name = loc.t(s.label);
        let waited = wait_until_ready(
            &s.client,
            MANAGED_READY_TIMEOUT,
            Some(s.handle.exited()),
            loc,
        )
        .await;
        let secs = s.since.elapsed().as_secs().to_string();
        match waited {
            Ok(()) => {
                let facts = facts(s, loc).await;
                progress(&loc.tf(
                    "setup.verify.ready",
                    &[("server", name), ("secs", &secs), ("facts", &facts)],
                ));
            }
            Err(e) => {
                ok = false;
                progress(&loc.tf(
                    "setup.verify.failed",
                    &[("server", name), ("reason", &format!("{e:#}"))],
                ));
            }
        }
    }

    // Dropping a handle signals its monitor task to kill the child; give the
    // tasks a beat to do it before the caller's runtime goes away (which would
    // kill the children anyway — `kill_on_drop` — only less tidily).
    drop(started);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ok
}

/// Why a server was not even started.
enum Refusal {
    /// No binary to run, or no model to give it (`ManagedConfig::is_runnable`).
    NotConfigured,
    /// Something already listens where it would — and a readiness probe cannot
    /// tell *our* server from that one, so a busy port would verify a stranger.
    PortBusy,
    /// The launch's own preflight or `spawn` refused, in its own words.
    Launch(String),
}

impl Refusal {
    fn localized(&self, cfg: &ManagedConfig, loc: &Locale) -> String {
        match self {
            Refusal::NotConfigured if cfg.binary.as_os_str().is_empty() => {
                loc.t("setup.verify.no_binary").to_string()
            }
            Refusal::NotConfigured => loc.t("setup.verify.no_model").to_string(),
            Refusal::PortBusy => {
                loc.tf("setup.verify.port_busy", &[("port", &cfg.port.to_string())])
            }
            Refusal::Launch(said) => said.clone(),
        }
    }
}

fn start(cfg: &ManagedConfig, loc: &'static Locale) -> Result<ServerHandle, Refusal> {
    if !cfg.is_runnable() {
        return Err(Refusal::NotConfigured);
    }
    if std::net::TcpListener::bind(("127.0.0.1", cfg.port)).is_err() {
        return Err(Refusal::PortBusy);
    }
    // The whole chain: `launch` wraps the OS's reason ("program not found") in
    // its own context, and the reason is the half the user can act on.
    ServerHandle::launch(cfg, loc).map_err(|e| Refusal::Launch(format!("{e:#}")))
}

/// What a ready server reports on `/props`, as one phrase. The embedder has no
/// window, vision or slots worth reading, so it reports none.
async fn facts(s: &Started, loc: &Locale) -> String {
    if s.label != "setup.verify.chat" {
        return String::new();
    }
    facts_phrase(
        s.client.context_budget().await,
        s.client.vision().await,
        s.client.parallel_slots().await,
        loc,
    )
}

/// The phrase itself, from answers already in hand: what the server said is
/// listed, what it did not say is left out — a server that reports nothing
/// (not a llama.cpp, or an old one) gets no dangling dash.
fn facts_phrase(
    context: Option<u32>,
    vision: VisionSupport,
    slots: Option<u32>,
    loc: &Locale,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = context {
        parts.push(loc.tf("setup.verify.fact.context", &[("n", &n.to_string())]));
    }
    match vision {
        VisionSupport::Supported => parts.push(loc.t("setup.verify.fact.vision_on").to_string()),
        VisionSupport::Unsupported => parts.push(loc.t("setup.verify.fact.vision_off").to_string()),
        VisionSupport::Unknown => {}
    }
    if let Some(n) = slots {
        parts.push(loc.tf("setup.verify.fact.slots", &[("n", &n.to_string())]));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" — {}", parts.join(", "))
    }
}

/// A mode, spelled as `settings.json` spells it.
fn mode_name(mode: ServerMode) -> String {
    serde_json::to_value(mode)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::config::ManagedSettings;
    use crate::shared::i18n::{Lang, locale};

    fn lines_of(config: &AppConfig, lookup: &BinaryLookup) -> (bool, Vec<String>) {
        let loc = locale(Lang::En);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut lines = Vec::new();
        let ok = rt.block_on(run(config, lookup, loc, |l| lines.push(l.to_string())));
        (ok, lines)
    }

    /// A cloud engine is not a failure to verify — there is nothing of ours to
    /// start — and an embedder nobody configured is how most installs run.
    #[test]
    fn a_server_that_is_not_meant_to_be_managed_is_skipped_not_failed() {
        let mut config = AppConfig::default();
        config.engine.mode = ServerMode::Claude;
        let (ok, lines) = lines_of(&config, &BinaryLookup::default());
        assert!(ok, "{lines:?}");
        assert!(lines[0].contains("claude"), "{lines:?}");
        assert_eq!(lines.len(), 2, "one line per server: {lines:?}");
    }

    /// The managed chat engine with nothing to run is the case `--verify`
    /// exists to catch, and it says which half is missing.
    #[test]
    fn a_managed_engine_with_nothing_to_run_fails_and_says_which_half() {
        let loc = locale(Lang::En);
        let (ok, lines) = lines_of(&AppConfig::default(), &BinaryLookup::default());
        assert!(!ok);
        assert!(
            lines[0].contains(loc.t("setup.verify.no_binary")),
            "{lines:?}"
        );

        // A binary, and no model: the router llama-server would start without
        // `-m` is refused before it is spawned (`ManagedConfig::is_runnable`).
        let mut config = AppConfig::default();
        config.engine.managed = ManagedSettings {
            binary: Some("/somewhere/llama-server".into()),
            ..Default::default()
        };
        let (ok, lines) = lines_of(&config, &BinaryLookup::default());
        assert!(!ok);
        assert!(
            lines[0].contains(loc.t("setup.verify.no_model")),
            "{lines:?}"
        );
    }

    /// A readiness probe cannot tell our server from whatever already answers
    /// on the port, so a busy port is refused rather than "verified".
    #[test]
    fn a_busy_port_is_refused_rather_than_verifying_a_stranger() {
        let loc = locale(Lang::En);
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("m.gguf");
        std::fs::write(&model, b"GGUF").unwrap();
        let squatter = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = squatter.local_addr().unwrap().port();
        let mut config = AppConfig::default();
        config.engine.managed = ManagedSettings {
            binary: Some("/somewhere/llama-server".into()),
            model_path: Some(model.display().to_string()),
            port,
            ..Default::default()
        };
        let (ok, lines) = lines_of(&config, &BinaryLookup::default());
        assert!(!ok);
        let want = loc.tf("setup.verify.port_busy", &[("port", &port.to_string())]);
        assert!(lines[0].contains(&want), "{lines:?}");
    }

    /// The launch's preflight speaks for itself: a model path that names no file
    /// is reported in the words the status bar would have used.
    #[test]
    fn the_launchs_own_refusal_is_passed_through() {
        let mut config = AppConfig::default();
        config.engine.managed = ManagedSettings {
            binary: Some("/somewhere/llama-server".into()),
            model_path: Some("/no/such/model.gguf".into()),
            port: 18231,
            ..Default::default()
        };
        let (ok, lines) = lines_of(&config, &BinaryLookup::default());
        assert!(!ok);
        assert!(lines[0].contains("/no/such/model.gguf"), "{lines:?}");
    }

    /// The ready line lists what the server said and leaves out what it did not.
    #[test]
    fn the_facts_are_what_the_server_said_and_nothing_else() {
        let loc = locale(Lang::En);
        assert_eq!(
            facts_phrase(Some(32768), VisionSupport::Supported, Some(4), loc),
            " — context 32768, takes images, 4 slots"
        );
        assert_eq!(
            facts_phrase(Some(4096), VisionSupport::Unsupported, None, loc),
            " — context 4096, text only"
        );
        // A server that answers `/health` and nothing on `/props`: no dash at all.
        assert_eq!(facts_phrase(None, VisionSupport::Unknown, None, loc), "");
        let ru = facts_phrase(Some(1), VisionSupport::Supported, Some(2), locale(Lang::Ru));
        assert!(
            !ru.contains('{') && ru.contains('1') && ru.contains('2'),
            "{ru}"
        );
    }

    /// The whole path against a real binary and a real model:
    ///
    ///     MINDFORK_LLAMA_DIR=…/data/llama MINDFORK_MODEL=…/small.gguf \
    ///       cargo test verify_starts_reports_and_stops -- --ignored --nocapture
    ///
    /// `MINDFORK_EMBED_MODEL`, when set, brings the embedder up beside it.
    #[test]
    #[ignore = "requires a downloaded build (MINDFORK_LLAMA_DIR) and a model (MINDFORK_MODEL)"]
    fn verify_starts_reports_and_stops_e2e_live() {
        let (Ok(llama_dir), Ok(model)) = (
            std::env::var("MINDFORK_LLAMA_DIR"),
            std::env::var("MINDFORK_MODEL"),
        ) else {
            eprintln!("skip: MINDFORK_LLAMA_DIR / MINDFORK_MODEL not set");
            return;
        };
        let mut config = AppConfig::default();
        config.engine.managed = ManagedSettings {
            model_path: Some(model),
            gpu_layers: 0,
            context_size: 2048,
            port: 18232,
            ..Default::default()
        };
        if let Ok(embed) = std::env::var("MINDFORK_EMBED_MODEL") {
            config.embed.managed.model_path = Some(embed);
            config.embed.managed.gpu_layers = 0;
            config.embed.managed.port = 18233;
        }
        let lookup = BinaryLookup {
            llama_dir: Some(llama_dir.into()),
            exe_dir: None,
        };
        let (ok, lines) = lines_of(&config, &lookup);
        for l in &lines {
            println!("{l}");
        }
        assert!(ok, "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("2048")),
            "the chat server's window is reported: {lines:?}"
        );
        // Stopped: the port is free again.
        assert!(std::net::TcpListener::bind(("127.0.0.1", 18232)).is_ok());
    }
}
