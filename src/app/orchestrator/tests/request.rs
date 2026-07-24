//! Orchestrator tests — mapping domain messages to the engine format. Part of the
//! [`super`] module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

#[test]
fn build_request_puts_system_aside_and_maps_roles() {
    let mut p = Profile::new("X", "Ты — X.");
    p.greeting = Some("Здравствуйте!".into());
    let mut chat = Chat::from_profile(&p, "c");
    chat.push_message(Message::assistant("Здравствуйте!"));
    chat.push_message(Message::user("привет"));

    let req = build_request(&chat, SamplingConfig::default(), vec![]);
    assert_eq!(req.system.as_deref(), Some("Ты — X."));
    assert_eq!(req.messages.len(), 2);
}
