//! The model port (docs/research/fabula-engine.md §8, *The model port*): what
//! the engine asks of a model, and the one policy every answer goes through,
//! live or replayed.
//!
//! Two adapters implement it: [`super::engine::EngineAdapter`] over the chat's
//! `EngineBackend`, which records every attempt in the call log, and
//! [`super::replay::ReplayAdapter`], which serves those attempts back by
//! address (spec §9). Both feed their attempts through [`run_json`] and
//! [`run_text`], so a replay walks the policy exactly as the live call did:
//! the same retry after a cut, the same repair after an invalid reply, the
//! same validated value at the end.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::schema::{Schema, Violation, describe, extract_object};
use crate::entities::chat_file::sha256_hex;
use crate::entities::story::{BranchId, CharacterKey, RunId};
use crate::shared::storage::story::{CallStatus, StoryDbError};

/// The role that makes a call (spec §8). Recorded with every call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A cast member: proposes, reacts, reflects.
    Character,
    /// Rules on the proposals and commits the events.
    WorldMaster,
    /// Plans, picks and writes.
    Author,
    /// Reads the published text with a clean context.
    Reader,
    /// Checks the text against the log and the canon.
    Continuity,
    /// Proposes the deltas a chapter makes to the canon.
    CanonKeeper,
}

impl Role {
    /// The name the call log records.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Character => "character",
            Role::WorldMaster => "world_master",
            Role::Author => "author",
            Role::Reader => "reader",
            Role::Continuity => "continuity",
            Role::CanonKeeper => "canon_keeper",
        }
    }
}

/// A call's logical address (spec §9): replay serves a reply by it, never by
/// its prompt, since sibling branches send identical first prompts and get
/// different replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallAddress {
    /// The run the call belongs to.
    pub run: RunId,
    /// Where in the run: `ch1/sc1/r0/k0/round2/mara/propose`.
    pub address: String,
}

impl CallAddress {
    /// A character's call in a round of a branch; `act` says which of its
    /// calls (`propose`, `react`, `reflect`).
    pub fn character(
        run: &RunId,
        branch: &BranchId,
        round: u32,
        who: &CharacterKey,
        act: &str,
    ) -> Self {
        Self {
            run: run.clone(),
            address: format!("{branch}/round{round}/{who}/{act}"),
        }
    }

    /// The World Master's call in a round of a branch; `act` says which
    /// (`initiative`, `reactions`).
    pub fn world_master(run: &RunId, branch: &BranchId, round: u32, act: &str) -> Self {
        Self {
            run: run.clone(),
            address: format!("{branch}/round{round}/world_master/{act}"),
        }
    }
}

/// What a call sends: the stable part first, which providers with prefix
/// caching reuse, then what changes from call to call (spec §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// The role's instructions and the story's stable content.
    pub system: String,
    /// The moment: the scene, the memory, what is asked now.
    pub user: String,
}

/// Why a call gave no usable answer.
#[derive(Debug, thiserror::Error)]
pub enum PortError {
    /// Replay found no recorded attempt where the policy needs one.
    #[error("no reply is recorded for {address} attempt {attempt} of {run}")]
    NotRecorded {
        /// The run.
        run: RunId,
        /// The address.
        address: String,
        /// The attempt the policy asked for.
        attempt: u32,
    },
    /// The reply broke its schema, after the one repair.
    #[error("the reply for {address} does not match its schema:\n{violations}")]
    Invalid {
        /// The address.
        address: String,
        /// The validator's findings on the last attempt.
        violations: String,
    },
    /// The model or a filter refused.
    #[error("the model refused the call for {address}")]
    Refused {
        /// The address.
        address: String,
    },
    /// The reply was cut at its cap, twice.
    #[error("the reply for {address} was cut at its cap")]
    Cut {
        /// The address.
        address: String,
    },
    /// The provider or the transport failed.
    #[error("the call for {address} failed: {message}")]
    Failed {
        /// The address.
        address: String,
        /// The provider's or the transport's words.
        message: String,
    },
    /// The run was cancelled.
    #[error("the call for {address} was cancelled")]
    Cancelled {
        /// The address.
        address: String,
    },
    /// The chapter's budget is spent: the call was not made (docs/fabula.md
    /// §6, T1).
    #[error("the chapter's budget is spent before {address}: {spent}")]
    Budget {
        /// The address of the call not made.
        address: String,
        /// What was spent, against what limit, in words.
        spent: String,
    },
    /// The story database refused or failed.
    #[error(transparent)]
    Store(#[from] StoryDbError),
}

/// A model, as the engine sees one.
#[async_trait::async_trait]
pub trait LlmPort: Send + Sync {
    /// A reply that meets `schema`, validated, its `enum` values in the
    /// schema's spelling.
    async fn json(
        &self,
        call: &CallAddress,
        role: Role,
        schema: &Schema,
        prompt: &Prompt,
    ) -> Result<Value, PortError>;

    /// Free text: chapter prose.
    async fn text(
        &self,
        call: &CallAddress,
        role: Role,
        prompt: &Prompt,
    ) -> Result<String, PortError>;
}

/// A reply read into its type. The value has met its schema already, so a
/// failure here is a schema that does not describe its type, and is reported
/// as an invalid reply.
pub async fn decide<T: DeserializeOwned>(
    port: &dyn LlmPort,
    call: &CallAddress,
    role: Role,
    schema: &Schema,
    prompt: &Prompt,
) -> Result<T, PortError> {
    let value = port.json(call, role, schema, prompt).await?;
    serde_json::from_value(value).map_err(|e| PortError::Invalid {
        address: call.address.clone(),
        violations: format!("- $: {e}"),
    })
}

/// The hash a call's prompt is recorded under: replay compares it with the
/// prompt it would send now, and a difference is drift (spec §9). The schema
/// is part of the prompt.
pub fn prompt_sha256(prompt: &Prompt, schema: Option<&Schema>) -> String {
    let canonical = json!({
        "system": prompt.system,
        "user": prompt.user,
        "schema": schema.map(Schema::to_json),
    });
    sha256_hex(canonical.to_string().as_bytes())
}

// ---------------------------------------------------------------------------
// The schema in the prompt, a lenient parse, the validator, and one repair
// quoting its message (spec §8, *The fallback*).

/// The system part of a JSON call: the schema itself, after the stable
/// content, so it is cached with it. Under schema-constrained decoding as well
/// as under the fallback: llama.cpp's grammar shows the model no description,
/// so the prompt is where a field's meaning is read, and one prompt for both
/// mechanisms leaves M1 comparing the decoding alone.
pub fn system_with_schema(prompt: &Prompt, schema: &Schema) -> String {
    format!(
        "{}\n\nReply with one JSON object that matches this JSON Schema, and nothing else:\n{}",
        prompt.system,
        schema.to_json()
    )
}

/// The message that asks for a repair, quoting the validator.
pub fn repair_note(violations: &[Violation]) -> String {
    format!(
        "That reply does not match the schema:\n{}\nReply again with only the corrected JSON object.",
        describe(violations)
    )
}

/// One message of a call's conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// `user` or `assistant`.
    pub role: &'static str,
    /// What it says.
    pub content: String,
}

/// An attempt's request: what the adapter sends, and what the call log keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// Its number at its address, from 1.
    pub number: u32,
    /// The system part.
    pub system: String,
    /// The conversation after it.
    pub turns: Vec<Turn>,
}

impl Attempt {
    /// The request as the call log records it.
    pub fn to_json(&self) -> Value {
        json!({
            "system": self.system,
            "messages": self.turns.iter()
                .map(|t| json!({"role": t.role, "content": t.content}))
                .collect::<Vec<_>>(),
        })
    }
}

/// How an attempt ended, from the model or from the call log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// How it ended.
    pub status: CallStatus,
    /// The reply's text; empty when there was none.
    pub text: String,
    /// What the provider or the transport said when it failed.
    pub error: Option<String>,
    /// Whether another attempt could succeed where it failed.
    pub transient: bool,
    /// Whether the run was cancelled under it.
    pub cancelled: bool,
}

impl Answer {
    /// The answer as the call log records it.
    pub fn to_json(&self) -> Value {
        json!({
            "text": self.text,
            "error": self.error,
            "transient": self.transient,
            "cancelled": self.cancelled,
        })
    }

    /// The answer a recorded attempt gave. A field the record lacks reads as
    /// empty, so a record without a response reads as an empty reply.
    pub fn from_json(status: CallStatus, response: Option<&Value>) -> Self {
        let field = |name: &str| response.and_then(|r| r.get(name));
        Self {
            status,
            text: field("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            error: field("error").and_then(Value::as_str).map(str::to_owned),
            transient: field("transient")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            cancelled: field("cancelled")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        }
    }
}

/// Where a call's attempts come from: a model, or the call log.
#[async_trait::async_trait]
pub trait Attempts: Send {
    /// Runs `attempt` and says how it ended.
    async fn attempt(&mut self, attempt: &Attempt) -> Result<Answer, PortError>;
}

/// What an attempt that did not answer means for the call: `Ok` to try once
/// more with the same request, `Err` to give up.
fn after_failure(answer: &Answer, address: &str, retried: &mut bool) -> Result<(), PortError> {
    let retry = match answer.status {
        CallStatus::Cut => true,
        CallStatus::Error => answer.transient,
        CallStatus::Refused | CallStatus::Ok => false,
    };
    if answer.cancelled {
        return Err(PortError::Cancelled {
            address: address.to_owned(),
        });
    }
    if retry && !*retried {
        *retried = true;
        return Ok(());
    }
    Err(match answer.status {
        CallStatus::Cut => PortError::Cut {
            address: address.to_owned(),
        },
        CallStatus::Refused => PortError::Refused {
            address: address.to_owned(),
        },
        CallStatus::Error | CallStatus::Ok => PortError::Failed {
            address: address.to_owned(),
            message: answer
                .error
                .clone()
                .unwrap_or_else(|| "the reply was empty".to_owned()),
        },
    })
}

/// A JSON call: a cut or a transient failure is tried once more, an invalid
/// reply is repaired once, a refusal fails the call (spec §8, *What still breaks
/// a reply*). The same under either mechanism; the source decides whether the
/// provider constrains the decoding too.
pub async fn run_json(
    attempts: &mut dyn Attempts,
    address: &str,
    schema: &Schema,
    prompt: &Prompt,
) -> Result<Value, PortError> {
    let mut attempt = Attempt {
        number: 1,
        system: system_with_schema(prompt, schema),
        turns: vec![Turn {
            role: "user",
            content: prompt.user.clone(),
        }],
    };
    let (mut retried, mut repaired) = (false, false);
    loop {
        let answer = attempts.attempt(&attempt).await?;
        if answer.status != CallStatus::Ok || answer.cancelled {
            after_failure(&answer, address, &mut retried)?;
        } else {
            let checked = match extract_object(&answer.text) {
                Some(value) => schema.validate(&value),
                None => Err(vec![Violation {
                    path: "$".to_owned(),
                    problem: "no JSON object was found in the reply".to_owned(),
                }]),
            };
            match checked {
                Ok(value) => return Ok(value),
                Err(violations) if !repaired => {
                    repaired = true;
                    attempt.turns.push(Turn {
                        role: "assistant",
                        content: answer.text,
                    });
                    attempt.turns.push(Turn {
                        role: "user",
                        content: repair_note(&violations),
                    });
                }
                Err(violations) => {
                    return Err(PortError::Invalid {
                        address: address.to_owned(),
                        violations: describe(&violations),
                    });
                }
            }
        }
        attempt.number += 1;
    }
}

/// A text call: a cut, a transient failure or an empty reply is tried once
/// more, a refusal fails the call.
pub async fn run_text(
    attempts: &mut dyn Attempts,
    address: &str,
    prompt: &Prompt,
) -> Result<String, PortError> {
    let mut attempt = Attempt {
        number: 1,
        system: prompt.system.clone(),
        turns: vec![Turn {
            role: "user",
            content: prompt.user.clone(),
        }],
    };
    let mut retried = false;
    loop {
        let mut answer = attempts.attempt(&attempt).await?;
        if answer.status == CallStatus::Ok && !answer.cancelled {
            if !answer.text.trim().is_empty() {
                return Ok(answer.text);
            }
            // An empty reply is as good as a transport failure.
            answer.status = CallStatus::Error;
            answer.transient = true;
        }
        after_failure(&answer, address, &mut retried)?;
        attempt.number += 1;
    }
}

// Its schema and prompt serve the engine adapter's tests too.
#[cfg(test)]
#[path = "port_tests.rs"]
pub(crate) mod tests;
