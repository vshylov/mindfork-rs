//! What the engine calls the model it is running, and who outranks whom
//! (`model_name::ModelDiscovery`, docs/research/external-model-name.md §4).

use super::*;
use crate::shared::config::ServerMode;

/// An `external` engine with the "Model (opt.)" field blank — the configuration
/// this whole mechanism exists for, and the one every `MINDFORK_ENGINE_URL` run
/// lands in.
fn external_unnamed(orch: &mut Orchestrator) {
    orch.config.engine.mode = ServerMode::External;
    orch.config.engine.external.url = Some("http://127.0.0.1:9/v1".into());
    orch.config.engine.external.model_name = None;
}

#[test]
fn a_name_in_settings_outranks_what_the_engine_says() {
    let (_d, mut orch) = bare_orch();
    external_unnamed(&mut orch);
    orch.config.engine.external.model_name = Some("qwen-3.6-27b".into());
    let epoch = orch.model.epoch();
    orch.handle_model_result(epoch, Some("gemma-4-31B_q4_0-it".into()));
    assert_eq!(
        orch.effective_model_name().as_deref(),
        Some("qwen-3.6-27b"),
        "a name the user typed is never overruled by the server's opinion"
    );
}

/// The point of the feature: a blank field stops meaning a blank header.
#[test]
fn a_blank_field_falls_back_to_the_engines_own_answer() {
    let (_d, mut orch) = bare_orch();
    external_unnamed(&mut orch);
    assert_eq!(orch.effective_model_name(), None, "nothing known yet");
    let epoch = orch.model.epoch();
    orch.handle_model_result(epoch, Some("gemma-4-31B_q4_0-it".into()));
    assert_eq!(
        orch.effective_model_name().as_deref(),
        Some("gemma-4-31B_q4_0-it")
    );
}

/// An engine that cannot say leaves the caption and the metadata exactly as they
/// were before it was ever asked — nothing, never a placeholder.
#[test]
fn an_engine_that_cannot_say_changes_nothing() {
    let (_d, mut orch) = bare_orch();
    external_unnamed(&mut orch);
    let epoch = orch.model.epoch();
    orch.handle_model_result(epoch, None);
    assert_eq!(orch.effective_model_name(), None);
}

/// The answer reaches the screen, which owns the caption — and it carries only
/// the *discovered* half, so the screen's own preference for the configuration
/// stays the single rule.
#[test]
fn the_discovered_name_is_sent_to_the_ui() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    external_unnamed(&mut orch);
    let epoch = orch.model.epoch();
    orch.handle_model_result(epoch, Some("gemma-4-31B_q4_0-it".into()));
    let sent = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|e| match e {
            AppEvent::EngineModel(m) => Some(m),
            _ => None,
        })
        .last();
    assert_eq!(sent, Some(Some("gemma-4-31B_q4_0-it".into())));
}

/// The epoch is what makes switching servers mid-question safe: a late answer
/// about the previous engine must not label the new one's messages.
// Spawns: re-asking the engine for its model starts a task.
#[tokio::test]
async fn an_answer_about_a_replaced_engine_is_dropped() {
    let (_d, mut orch) = bare_orch();
    external_unnamed(&mut orch);
    let stale = orch.model.epoch();
    orch.refresh_model_name();
    orch.handle_model_result(stale, Some("the-previous-server".into()));
    assert_eq!(
        orch.effective_model_name(),
        None,
        "the late answer belonged to an engine that is gone"
    );
}

/// Changing the engine drops the name **immediately**, rather than leaving the
/// previous server's model on screen until the new one answers.
// Spawns: re-asking the engine for its model starts a task.
#[tokio::test]
async fn changing_the_engine_forgets_the_name_and_says_so() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    external_unnamed(&mut orch);
    let epoch = orch.model.epoch();
    orch.handle_model_result(epoch, Some("gemma-4-31B_q4_0-it".into()));
    while rx.try_recv().is_ok() {}

    orch.refresh_model_name();
    assert_eq!(orch.effective_model_name(), None);
    let sent = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|e| match e {
            AppEvent::EngineModel(m) => Some(m),
            _ => None,
        })
        .last();
    assert_eq!(sent, Some(None), "the caption must be cleared at once");
}

/// A configuration that names a model is not second-guessed: no request goes
/// out, because its answer could never be read.
// Spawns: would start a task if the engine were asked.
#[tokio::test]
async fn a_named_engine_is_never_asked() {
    let (_d, mut orch) = bare_orch();
    external_unnamed(&mut orch);
    orch.config.engine.external.model_name = Some("qwen-3.6-27b".into());
    orch.refresh_model_name();
    assert!(!orch.model.pending(), "settings already answered");

    // …and a managed or cloud mode is answered by settings too, for the same
    // reason: `active_model_name` has the GGUF / the required cloud model.
    orch.config.engine.mode = ServerMode::Claude;
    orch.config.engine.claude.model_name = Some("claude-opus-4-8".into());
    orch.refresh_model_name();
    assert!(!orch.model.pending());
}
