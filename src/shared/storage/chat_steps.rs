//! Chat-file migration steps (ADR 0006; the registry is [`super::schema`]).
//!
//! **1 → 2** (`chat_to_v2`): the sub-agent track (docs/research/subagent-chats.md
//! §3.13, ADR 0010) keeps a `call_subagent` call's transcript on the call's
//! record. A record written before that holds everything such a transcript
//! needs — the persona and the message in `arguments`, the reply in `result` —
//! so the step **synthesizes** one per old record, in `messages` and in the
//! `deleted` archive alike, and stamps `v = 2`. Nothing existing is removed or
//! rewritten: the migrated file is a superset of the old one, and the bump
//! exists so the synthesis runs exactly once, through the backup-then-write
//! path. Pure `Value → Value`, like every step; no I/O.

use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

/// The namespace the synthesized ids are derived under: the same old record
/// always yields the same run id, so re-running the step on a restored backup
/// reproduces the same `chat://` addresses — and the step is idempotent by
/// construction.
const SUBAGENT_NS: Uuid = Uuid::from_u128(0x6d66_5f73_7562_6167_656e_745f_7631_0000);

/// The name of the call this step knows. A literal, not the tool's constant: a
/// step describes the past, and `shared` cannot reach `features` anyway (FSD).
const CALL_SUBAGENT: &str = "call_subagent";

/// `chats/<id>.json` 2 → 3: no shape change. The step exists so every file is
/// stamped with a version an older binary **refuses politely** — a file may
/// now carry a `RunKind::Dialogue` run (spec §9.13), which is a new serde
/// variant, and without the bump an old binary would fail parsing the whole
/// file instead of reporting "data from a newer version" (ADR 0006).
pub(super) fn chat_to_v3(mut v: Value) -> Result<Value> {
    v["v"] = json!(3);
    Ok(v)
}

/// `chats/<id>.json` 1 → 2. See the module doc.
pub(super) fn chat_to_v2(mut v: Value) -> Result<Value> {
    let chat_id = v
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(messages) = v.get_mut("messages").and_then(Value::as_array_mut) {
        synthesize_runs(&chat_id, messages);
    }
    if let Some(deleted) = v.get_mut("deleted").and_then(Value::as_array_mut) {
        for exchange in deleted {
            if let Some(messages) = exchange.get_mut("messages").and_then(Value::as_array_mut) {
                synthesize_runs(&chat_id, messages);
            }
        }
    }
    v["v"] = json!(2);
    Ok(v)
}

/// Gives every old `call_subagent` record in `messages` a run. The reply's
/// timestamp comes from the `Tool` message that answered the call, when the
/// list has one; the call's own timestamp otherwise.
fn synthesize_runs(chat_id: &str, messages: &mut [Value]) {
    // Collected first: the Tool messages are siblings of the assistant message
    // being edited, and the borrow below is exclusive.
    let tool_times: Vec<(String, String)> = messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
        .filter_map(|m| {
            Some((
                m.get("tool_call_id")?.as_str()?.to_string(),
                m.get("timestamp")?.as_str()?.to_string(),
            ))
        })
        .collect();
    for message in messages.iter_mut() {
        let Some(created_at) = message
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let Some(records) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for record in records.iter_mut() {
            if record.get("name").and_then(Value::as_str) != Some(CALL_SUBAGENT)
                || record.get("subagent").is_some()
            {
                continue;
            }
            let call_id = record
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let finished_at = tool_times
                .iter()
                .find(|(id, _)| *id == call_id)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| created_at.clone());
            let run = synthesize_run(chat_id, &call_id, record, &created_at, &finished_at);
            record["subagent"] = run;
        }
    }
}

/// One run from one old record.
fn synthesize_run(
    chat_id: &str,
    call_id: &str,
    record: &Value,
    created_at: &str,
    finished_at: &str,
) -> Value {
    let args = record.get("arguments").cloned().unwrap_or(Value::Null);
    let arg = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let system_message = arg("system_message").unwrap_or_default();
    let message = arg("message").unwrap_or_default();
    let name = arg("name");
    let result = record
        .get("result")
        .and_then(Value::as_str)
        .map(str::to_string);

    let id = Uuid::new_v5(&SUBAGENT_NS, format!("{chat_id}/{call_id}").as_bytes());
    let msg_id = |tag: &str| Uuid::new_v5(&SUBAGENT_NS, format!("{id}/{tag}").as_bytes());
    let title = name
        .as_deref()
        .and_then(crate::shared::title::sanitize_title)
        .or_else(|| {
            message
                .lines()
                .find(|l| !l.trim().is_empty())
                .and_then(crate::shared::title::sanitize_title)
        })
        .unwrap_or_else(|| CALL_SUBAGENT.to_string());

    let mut messages = vec![json!({
        "id": msg_id("user"),
        "role": "user",
        "text": message,
        "timestamp": created_at,
    })];
    // A record with no result is a call that never came back — the run is an
    // instruction with no reply and no outcome, which is how an interrupted
    // run reads today too.
    let outcome = match result {
        Some(reply) => {
            messages.push(json!({
                "id": msg_id("assistant"),
                "role": "assistant",
                "text": reply,
                "timestamp": finished_at,
            }));
            Some("completed")
        }
        None => None,
    };
    let mut run = json!({
        "id": id,
        "kind": "subagent",
        "title": title,
        "created_at": created_at,
        "finished_at": finished_at,
        "system_message": system_message,
        "messages": messages,
    });
    if let Some(name) = name {
        run["name"] = json!(name);
    }
    if let Some(outcome) = outcome {
        run["outcome"] = json!(outcome);
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::chat::Chat;

    /// A chat file as the tool-less `call_subagent` wrote it: a v1 file (no
    /// `v`), one call in the live messages with its `Tool` answer, one in the
    /// deleted archive, and an ordinary tool call that must be left alone.
    const V1_CHAT: &str = include_str!("fixtures/chat_v1_call_subagent.json");

    fn migrated() -> Value {
        chat_to_v2(serde_json::from_str(V1_CHAT).unwrap()).unwrap()
    }

    #[test]
    fn the_fixture_is_a_v1_file_that_parses_today() {
        let v: Value = serde_json::from_str(V1_CHAT).unwrap();
        assert!(v.get("v").is_none());
        let chat: Chat = serde_json::from_value(v).unwrap();
        assert_eq!(chat.v, 1);
        assert!(chat.messages[1].tool_calls[0].subagent.is_none());
    }

    #[test]
    fn synthesizes_a_run_from_an_old_record_and_stamps_v2() {
        let out = migrated();
        assert_eq!(out["v"], json!(2));
        let run = &out["messages"][1]["tool_calls"][0]["subagent"];
        assert_eq!(run["kind"], json!("subagent"));
        assert_eq!(run["system_message"], json!("Ты — критик."));
        assert_eq!(run["title"], json!("Оцени идею X."));
        assert_eq!(run["outcome"], json!("completed"));
        assert_eq!(run["created_at"], out["messages"][1]["timestamp"]);
        // The reply's time is the Tool message's, not the call's.
        assert_eq!(run["finished_at"], out["messages"][2]["timestamp"]);
        let msgs = run["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], json!("user"));
        assert_eq!(msgs[0]["text"], json!("Оцени идею X.\nПодробно."));
        assert_eq!(msgs[1]["role"], json!("assistant"));
        assert_eq!(msgs[1]["text"], json!("Идея X слаба: …"));
        // The ordinary call beside it is untouched.
        assert!(
            out["messages"][1]["tool_calls"][1]
                .get("subagent")
                .is_none()
        );
        // Everything else in the file is still there.
        assert_eq!(out["title"], json!("Старый чат"));
        assert_eq!(out["messages"][4]["text"], json!("Итого: X слаба."));
    }

    #[test]
    fn the_deleted_archive_is_migrated_too() {
        let out = migrated();
        let run = &out["deleted"][0]["messages"][1]["tool_calls"][0]["subagent"];
        assert_eq!(run["title"], json!("Похвали Y"));
        // No Tool answer in that archive: the call never came back — an
        // instruction with no reply, and no outcome.
        assert_eq!(run["messages"].as_array().unwrap().len(), 1);
        assert!(run.get("outcome").is_none());
        assert_eq!(run["finished_at"], run["created_at"]);
    }

    #[test]
    fn the_migrated_file_control_parses_and_the_run_is_what_the_app_reads() {
        let chat: Chat = serde_json::from_value(migrated()).unwrap();
        assert_eq!(chat.v, 2);
        let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
        assert_eq!(run.final_reply(), Some("Идея X слаба: …"));
        assert_eq!(
            run.outcome,
            Some(crate::entities::subagent::RunOutcome::Completed)
        );
        assert_eq!(run.name, None);
        assert_eq!(run.tokens, 0);
        assert!(!run.renamed_manually);
        let archived = chat.deleted[0].messages[1].tool_calls[0]
            .subagent
            .as_deref()
            .unwrap();
        assert_eq!(archived.outcome, None);
    }

    #[test]
    fn ids_are_deterministic_and_the_step_is_idempotent() {
        let a = migrated();
        let b = migrated();
        assert_eq!(a, b, "the same old file yields the same run ids");
        let twice = chat_to_v2(a.clone()).unwrap();
        assert_eq!(twice, a, "a record that has a run is skipped");
        // A run's id is stable per (chat, call): two calls differ.
        let live = &a["messages"][1]["tool_calls"][0]["subagent"]["id"];
        let archived = &a["deleted"][0]["messages"][1]["tool_calls"][0]["subagent"]["id"];
        assert_ne!(live, archived);
    }

    #[test]
    fn a_record_with_a_run_or_without_arguments_is_handled() {
        // A new-style record (a run already there) is left alone; an old record
        // with no arguments at all still gets a run, titled by the tool's name.
        let v = json!({
            "id": "00000000-0000-0000-0000-000000000009",
            "messages": [{
                "id": "00000000-0000-0000-0000-000000000010",
                "role": "assistant", "text": "", "timestamp": "2026-01-01T00:00:00Z",
                "tool_calls": [
                    {"id": "a", "name": "call_subagent", "arguments": {}, "result": "r",
                     "subagent": {"marker": true}},
                    {"id": "b", "name": "call_subagent", "arguments": {}}
                ]
            }]
        });
        let out = chat_to_v2(v).unwrap();
        assert_eq!(
            out["messages"][0]["tool_calls"][0]["subagent"],
            json!({"marker": true})
        );
        let run = &out["messages"][0]["tool_calls"][1]["subagent"];
        assert_eq!(run["title"], json!("call_subagent"));
        assert_eq!(run["system_message"], json!(""));
        assert_eq!(run["messages"][0]["text"], json!(""));
    }
}
