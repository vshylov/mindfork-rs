//! Every strict enum a stored file holds, each value with its wire form and the
//! schema it arrived in.
//!
//! **A new value of a stored enum is a breaking change** (the bump policy in
//! [`super`]): an older binary fails the parse of the whole file on a value it
//! does not know — the settings refuse the start, a chat is skipped in silence
//! — and only a schema step makes its refusal say "a newer version". Nothing
//! else in the build says so: `max` was added with every test green and no
//! step, and 0.13.0 dropped the chats that held it from its list.
//!
//! So each enum below is a `match` **with no wildcard arm**: a value added to
//! one of them stops the tests compiling until its author names the schema it
//! arrives in — and writes that schema's step. The wire form is pinned beside
//! it, because renaming a stored value breaks an older reader the same way.
//!
//! What is not here, and why: `Lang` reads any code it is given, `RoleLite`
//! (`data_stats`) has a `#[serde(other)]` arm, `CommandSlot` is not stored (a
//! workspace keeps a field per slot), `DataLocation` lives in `defaults.json`,
//! which has no schema, and the wire enums are the providers'.
//!
//! The schemas are read off the releases: a value present in 0.9.0, the first,
//! is `1`; one added later names the schema of its step, or — when it came
//! with an enum or a field of its own, which an older reader skips whole — the
//! schema of the first release that wrote it.

use serde::Serialize;
use serde_json::Value;

use super::{CHAT_SCHEMA, DB_SCHEMA, SETTINGS_SCHEMA};
use crate::entities::attachment::AttachMode;
use crate::entities::chat::DeletedCause;
use crate::entities::chat_file::FileOrigin;
use crate::entities::message::{MessageFinish, MessageRole};
use crate::entities::sampling::{ReasoningEffort, Verbosity};
use crate::entities::self_model::GoalStatus;
use crate::entities::subagent::{RunKind, RunOutcome};
use crate::shared::config::{
    AutoTitleMode, CodeBlockBackground, FlashAttn, ImpersonationMode, MediaResolution, NoteOrder,
    PythonMode, ServerMode, SpecType, SttMode, Theme, ThemeMode, TtsMode, VideoProvider,
    WebProvider,
};
use crate::shared::embed_prefix::EmbedConvention;
use crate::shared::notify::NotifyEscape;
use crate::shared::osc52::Osc52Mode;

/// Which files hold an enum's values — and so whose schema a new value moves.
#[derive(Debug, Clone, Copy)]
pub(super) enum Held {
    /// `settings.json`.
    Settings,
    /// The chat files.
    Chats,
    /// The sampling's values: `settings.json`, `profiles.json` and the chat
    /// files. `profiles.json` has no step of its own; the settings' stamp
    /// answers for it ([`super::settings_to_v5`]).
    Sampling,
    /// The self-model's JSON in `data.db`, versioned by `PRAGMA user_version`.
    SelfModels,
}

impl Held {
    /// The newest schema a value may name: the one every file it lives in is
    /// stamped with today.
    fn newest(self) -> u32 {
        match self {
            Held::Settings => SETTINGS_SCHEMA,
            Held::Chats => CHAT_SCHEMA,
            Held::Sampling => SETTINGS_SCHEMA.min(CHAT_SCHEMA),
            Held::SelfModels => DB_SCHEMA,
        }
    }
}

/// One value of a stored enum.
pub(super) struct Stored {
    pub(super) variant: &'static str,
    /// What the ledger says the value is written as.
    pub(super) pinned: &'static str,
    /// What serde writes today.
    pub(super) wire: Value,
    /// The schema it arrived in.
    pub(super) since: u32,
}

/// One stored enum and every value it has.
pub(super) struct Ledger {
    pub(super) name: &'static str,
    pub(super) held: Held,
    pub(super) values: Vec<Stored>,
}

fn wire<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("a unit variant serializes")
}

/// `ledger!(Enum in Held { since => [Variant = "wire", …], … })`. It expands
/// to an exhaustive `match` from the enum to its schemas — which is what stops
/// compiling when a variant is added — and to the ledger the tests read.
macro_rules! ledger {
    ($ty:ident in $held:ident {
        $($since:literal => [$($variant:ident = $pinned:literal),+ $(,)?]),+ $(,)?
    }) => {{
        fn since(value: $ty) -> u32 {
            match value {
                $($($ty::$variant)|+ => $since,)+
            }
        }
        Ledger {
            name: stringify!($ty),
            held: Held::$held,
            values: vec![$($(Stored {
                variant: stringify!($variant),
                pinned: $pinned,
                wire: wire(&$ty::$variant),
                since: since($ty::$variant),
            },)+)+],
        }
    }};
}

/// Every stored strict enum. **Adding a value**: name the schema it arrives
/// in — the current one plus one — and write that schema's step in
/// `schema.rs` (a stamp is enough when nothing in the file moves:
/// [`super::settings_to_v4`], [`super::settings_to_v5`]), with its golden
/// fixture and a CHANGELOG *Data* entry.
pub(super) fn ledgers() -> Vec<Ledger> {
    vec![
        // --- settings.json ---------------------------------------------------
        // `grok` came in 0.9.5 with no step, so the schema-1 readers before it
        // (0.9.0–0.9.4) took a file naming it for a broken one. `openrouter` is
        // what the 3→4 stamp is for, `deepseek` the 6→7 one.
        ledger!(ServerMode in Settings {
            1 => [
                Managed = "managed", External = "external", OpenAi = "openai",
                Gemini = "gemini", Claude = "claude", Grok = "grok",
            ],
            4 => [OpenRouter = "openrouter"],
            7 => [DeepSeek = "deepseek"],
        }),
        ledger!(ImpersonationMode in Settings {
            1 => [
                Shared = "shared", Managed = "managed", External = "external",
                OpenAi = "openai", Gemini = "gemini", Claude = "claude", Grok = "grok",
            ],
            4 => [OpenRouter = "openrouter"],
            7 => [DeepSeek = "deepseek"],
        }),
        ledger!(TtsMode in Settings {
            1 => [OpenAi = "openai", Gemini = "gemini", External = "external"],
            4 => [OpenRouter = "openrouter"],
        }),
        // `video.provider` came with its two values in 0.13.0.
        ledger!(VideoProvider in Settings {
            4 => [Gemini = "gemini", OpenRouter = "openrouter"],
        }),
        ledger!(MediaResolution in Settings { 1 => [Low = "low", Medium = "medium"] }),
        ledger!(EmbedConvention in Settings {
            1 => [None = "none", E5 = "e5", E5Instruct = "e5-instruct"],
        }),
        ledger!(FlashAttn in Settings { 1 => [Auto = "auto", On = "on", Off = "off"] }),
        ledger!(SpecType in Settings {
            1 => [
                None = "none", DraftSimple = "draft-simple", DraftEagle3 = "draft-eagle3",
                DraftMtp = "draft-mtp", NgramSimple = "ngram-simple",
                NgramMapK = "ngram-map-k", NgramMapK4v = "ngram-map-k4v",
                NgramMod = "ngram-mod", NgramCache = "ngram-cache",
            ],
        }),
        ledger!(WebProvider in Settings { 2 => [Auto = "auto", FreeOnly = "freeonly"] }),
        ledger!(PythonMode in Settings { 1 => [Wasmer = "wasmer", Local = "local"] }),
        ledger!(Theme in Settings { 1 => [Auto = "auto", Dark = "dark", Light = "light"] }),
        ledger!(ThemeMode in Settings { 3 => [System = "system", Full = "full", Mono = "mono"] }),
        ledger!(CodeBlockBackground in Settings { 5 => [Inverted = "inverted", Normal = "normal"] }),
        ledger!(AutoTitleMode in Settings {
            1 => [
                AfterUserMessage = "after_user_message",
                AfterAssistantReply = "after_assistant_reply",
                Off = "off",
            ],
        }),
        ledger!(NoteOrder in Settings {
            2 => [NewestFirst = "newest_first", OldestFirst = "oldest_first"],
        }),
        ledger!(Osc52Mode in Settings { 1 => [Auto = "auto", Always = "always", Off = "off"] }),
        // Came with a field of its own, which an older reader skips whole.
        ledger!(NotifyEscape in Settings { 7 => [Osc777 = "osc777", Osc9 = "osc9", Off = "off"] }),
        // Came with the `stt` section, which an older reader skips whole.
        ledger!(SttMode in Settings {
            7 => [OpenAi = "openai", OpenRouter = "openrouter", External = "external"],
        }),
        // --- settings.json, profiles.json and the chat files -----------------
        // `max` is what the 4→5 stamps of both are for.
        ledger!(ReasoningEffort in Sampling {
            1 => [
                None = "none", Minimal = "minimal", Low = "low", Medium = "medium",
                High = "high", XHigh = "xhigh",
            ],
            5 => [Max = "max"],
        }),
        ledger!(Verbosity in Sampling { 1 => [Low = "low", Medium = "medium", High = "high"] }),
        // --- the chat files --------------------------------------------------
        // Every reply records the mode it was generated in (`metadata.mode`).
        // `openrouter` came in 0.13.0 with no chat step, its file at 4, where
        // the settings' stamp answered for a root downgraded in place;
        // `deepseek` is what the 5→6 stamp is for (`chat_steps::chat_to_v6`).
        ledger!(ServerMode in Chats {
            1 => [
                Managed = "managed", External = "external", OpenAi = "openai",
                Gemini = "gemini", Claude = "claude", Grok = "grok",
            ],
            4 => [OpenRouter = "openrouter"],
            6 => [DeepSeek = "deepseek"],
        }),
        ledger!(MessageRole in Chats {
            1 => [System = "system", User = "user", Assistant = "assistant", Tool = "tool"],
        }),
        // A message's `finish` came with its four values in 0.9.9.
        ledger!(MessageFinish in Chats {
            4 => [Stop = "stop", Length = "length", Cancelled = "cancelled", Error = "error"],
        }),
        ledger!(AttachMode in Chats { 1 => [Inline = "inline", ByReference = "by_reference"] }),
        ledger!(FileOrigin in Chats {
            4 => [Sandbox = "sandbox", Recovered = "recovered", Attached = "attached"],
        }),
        ledger!(DeletedCause in Chats {
            1 => [
                DeleteExchange = "DeleteExchange", Regenerate = "Regenerate",
                Rewrite = "Rewrite",
            ],
        }),
        // `dialogue` is what the 2→3 step is for.
        ledger!(RunKind in Chats { 2 => [Subagent = "subagent"], 3 => [Dialogue = "dialogue"] }),
        ledger!(RunOutcome in Chats {
            2 => [
                Completed = "completed", Cancelled = "cancelled", TimedOut = "timed_out",
                Failed = "failed", RoundLimit = "round_limit",
            ],
        }),
        // --- data.db ---------------------------------------------------------
        ledger!(GoalStatus in SelfModels {
            1 => [Active = "Active", Completed = "Completed", Abandoned = "Abandoned"],
        }),
    ]
}

/// The effort scale's values with the schema each arrived in, for the
/// downgrade test in `schema.rs`.
pub(super) fn efforts() -> Vec<(ReasoningEffort, u32)> {
    let ledger = ledgers()
        .into_iter()
        .find(|l| l.name == "ReasoningEffort")
        .expect("the effort scale has a ledger");
    ledger
        .values
        .into_iter()
        .map(|v| (serde_json::from_value(v.wire).unwrap(), v.since))
        .collect()
}

/// A value names a schema its files can be stamped with — one above them is a
/// value whose step was never written.
#[test]
fn every_stored_value_arrived_in_a_schema_its_files_have() {
    for ledger in ledgers() {
        let newest = ledger.held.newest();
        for v in &ledger.values {
            assert!(
                (1..=newest).contains(&v.since),
                "{}::{} names schema {}, and {:?} files are at {newest}: write the step",
                ledger.name,
                v.variant,
                v.since,
                ledger.held
            );
        }
    }
}

/// A stored value is written as the ledger says: renaming one breaks an
/// older reader as surely as adding one.
#[test]
fn a_stored_value_keeps_its_wire_form() {
    for ledger in ledgers() {
        for v in &ledger.values {
            assert_eq!(
                v.wire,
                Value::from(v.pinned),
                "{}::{} is written differently: a renamed stored value is a breaking change",
                ledger.name,
                v.variant
            );
        }
    }
}
