//! Messenger clients for the bridge (ADR 0013): the wire of each channel and
//! nothing above it. What a message means — who may send one, which chat it
//! reaches, what is answered — is decided in `features::bridge` and the
//! orchestrator.

pub mod telegram;
#[cfg(test)]
pub(crate) mod telegram_fake;
