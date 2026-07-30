//! The user's answer to a dangerous-tool confirmation (spec §9.8).
//!
//! **Why this type lives in `features` and not next to `AppCommand`.** The
//! answer travels UI → orchestrator, so both `app` (the command) and `screens`
//! (the popup that produces it) need it — and `screens` may not import `app`
//! (FSD, docs/architecture.md §3). Same reason `RagProgress` lives in
//! `features::rag_ingest` rather than in the event contract.

/// What the user decided about one dangerous tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDecision {
    /// Run this one call.
    Allow,
    /// Run it, and stop asking about **this tool** for the rest of the turn.
    ///
    /// The turn is the natural unit (fork F4 of docs/history/tool-confirmation.md): it is
    /// the scope of one user request, it ends by itself, and nothing outlives it
    /// — so no standing permission accumulates that the user would later have to
    /// remember granting.
    AllowForTurn,
    /// Do not run it. The loop **continues** and the model is told, so it can
    /// explain itself or try another way (fork F5); ending the turn would throw
    /// away the text already streamed.
    Deny,
}
