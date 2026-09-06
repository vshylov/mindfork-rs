//! Chat-file migration steps (ADR 0006; the registry is [`super::schema`]).
//!
//! **3 → 4** (`chat_to_v4`): a run title the old 100-character title cap cut
//! gets its tail back, re-derived from the instruction the run still holds.
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

/// The character ceiling the title normalizer used to apply before it was
/// removed (spec §11.2). A literal, not a constant imported from there: the
/// constant is gone, and a step describes the past.
const OLD_TITLE_CAP: usize = 100;

/// `chats/<id>.json` 3 → 4: a run title the old cap cut gets its tail back.
///
/// Every title used to be shortened to [`OLD_TITLE_CAP`] characters **as it
/// was stored**, with nothing to say it had been — so a sub-agent run named
/// after the first line of its instruction read as its own full name, and the
/// tasks screen drew it ending mid-word with half the row still empty
/// (spec §11.10). Removing the cap fixes what is written from now on and can
/// do nothing for what is already on disk, but the text is not lost: the
/// instruction is the run's own first `User` message. So the step rewrites the
/// title with what `initial_title` would have produced had the cap never
/// existed — the seed rule the language-model history uses, *what the recorder
/// would have written had it existed then*.
///
/// Deliberately narrow. It touches a title only when it is **exactly** the
/// cap's worth of characters, is not the user's own (`renamed_manually`), and
/// the re-derivation **starts with it** — so a rename the flag missed or a
/// hand-edited file can never be overwritten, only extended. Chat titles are
/// left alone: a model-written or user-typed one has no source to re-derive
/// from. Dialogue runs too — their `A ↔ B` title is built from labels that may
/// be a localized default, which a `shared` step cannot reproduce.
pub(super) fn chat_to_v4(mut v: Value) -> Result<Value> {
    if let Some(messages) = v.get_mut("messages").and_then(Value::as_array_mut) {
        restore_run_titles(messages);
    }
    if let Some(deleted) = v.get_mut("deleted").and_then(Value::as_array_mut) {
        for exchange in deleted {
            if let Some(messages) = exchange.get_mut("messages").and_then(Value::as_array_mut) {
                restore_run_titles(messages);
            }
        }
    }
    v["v"] = json!(4);
    Ok(v)
}

/// Every run hanging off these messages' records.
fn restore_run_titles(messages: &mut [Value]) {
    for message in messages.iter_mut() {
        let Some(records) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for record in records.iter_mut() {
            if let Some(run) = record.get_mut("subagent") {
                restore_run_title(run);
            }
        }
    }
}

/// One run's title, under the guards in [`chat_to_v4`]'s doc.
fn restore_run_title(run: &mut Value) {
    let title = run
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let manual = run.get("renamed_manually").and_then(Value::as_bool) == Some(true);
    let dialogue = run
        .get("participants")
        .and_then(Value::as_array)
        .is_some_and(|p| !p.is_empty());
    if title.chars().count() != OLD_TITLE_CAP || manual || dialogue {
        return;
    }
    let Some(whole) = derive_title(run) else {
        return;
    };
    if whole.len() > title.len() && whole.starts_with(&title) {
        run["title"] = json!(whole);
    }
}

/// The title the run was named with: the persona's name, else the first
/// non-empty line of the instruction — which is the run's first `User`
/// message (`CallSubagentArgs::initial_title`, spec §9.3.2).
fn derive_title(run: &Value) -> Option<String> {
    let named = run
        .get("name")
        .and_then(Value::as_str)
        .and_then(crate::shared::title::sanitize_title);
    named.or_else(|| {
        run.get("messages")?
            .as_array()?
            .iter()
            .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))?
            .get("text")?
            .as_str()?
            .lines()
            .find(|l| !l.trim().is_empty())
            .and_then(crate::shared::title::sanitize_title)
    })
}

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

    /// A chat file as it was stored while the title cap was in force: `v = 3`,
    /// a run whose title is exactly the cap's 100 characters of the
    /// instruction's first line, the same title marked as the user's own, a
    /// run named after its persona, and a fourth cut run in the deleted
    /// archive.
    const V3_CHAT: &str = include_str!("fixtures/chat_v3_cut_run_title.json");

    /// The instruction's first line, whole — what the title would have been
    /// had the cap never existed.
    const WHOLE: &str = concat!(
        "Read the changelog of every dependency we bumped this quarter and list ",
        "the ones whose breaking changes we have not yet handled anywhere in the tree"
    );

    fn v4() -> Value {
        chat_to_v4(serde_json::from_str(V3_CHAT).unwrap()).unwrap()
    }

    fn run_of(out: &Value, call: usize) -> Value {
        out["messages"][1]["tool_calls"][call]["subagent"].clone()
    }

    #[test]
    fn the_v3_fixture_is_what_the_cap_stored() {
        let v: Value = serde_json::from_str(V3_CHAT).unwrap();
        assert_eq!(v["v"], json!(3));
        let title = v["messages"][1]["tool_calls"][0]["subagent"]["title"]
            .as_str()
            .unwrap();
        assert_eq!(title.chars().count(), OLD_TITLE_CAP);
        assert!(WHOLE.starts_with(title), "the cap cut this line: {title}");
        // And it still parses as a chat today.
        let chat: Chat = serde_json::from_value(v).unwrap();
        assert_eq!(chat.v, 3);
    }

    #[test]
    fn a_cut_run_title_gets_its_tail_back_and_stamps_v4() {
        let out = v4();
        assert_eq!(out["v"], json!(4));
        assert_eq!(run_of(&out, 0)["title"], json!(WHOLE));
        // The archive is walked too — a taken-back exchange keeps its runs.
        assert_eq!(
            out["deleted"][0]["messages"][0]["tool_calls"][0]["subagent"]["title"],
            json!(WHOLE)
        );
    }

    #[test]
    fn the_step_leaves_every_other_title_alone() {
        let out = v4();
        // The user's own title, even at exactly the cap's length.
        let manual = run_of(&out, 1);
        assert_eq!(manual["title"].as_str().unwrap().chars().count(), 100);
        assert_ne!(manual["title"], json!(WHOLE));
        // A title that was never cut.
        assert_eq!(run_of(&out, 2)["title"], json!("Reviewer"));
        // The chat's own title has no source to re-derive from.
        assert_eq!(out["title"], json!("Dependency sweep"));
    }

    /// The guard, not the length, is what makes the step safe: a title that is
    /// the cap's length but **not** a prefix of the instruction is somebody's
    /// own text and stays.
    #[test]
    fn a_title_that_is_not_a_prefix_is_never_rewritten() {
        let mut out: Value = serde_json::from_str(V3_CHAT).unwrap();
        let title: String = "z".repeat(OLD_TITLE_CAP);
        out["messages"][1]["tool_calls"][0]["subagent"]["title"] = json!(title);
        let out = chat_to_v4(out).unwrap();
        assert_eq!(run_of(&out, 0)["title"], json!(title));
    }

    /// A dialogue's title is built from labels that may be a localized
    /// default, which a `shared` step cannot reproduce — so it is left alone
    /// whatever its length.
    #[test]
    fn a_dialogue_run_is_left_alone() {
        let mut out: Value = serde_json::from_str(V3_CHAT).unwrap();
        let run = &mut out["messages"][1]["tool_calls"][0]["subagent"];
        run["kind"] = json!("dialogue");
        run["participants"] = json!([{"name": "A"}, {"name": "B"}]);
        let out = chat_to_v4(out).unwrap();
        assert_eq!(run_of(&out, 0)["title"].as_str().unwrap(), &WHOLE[..100]);
    }

    /// Re-running the step changes nothing: the restored title is no longer
    /// the cap's length, so the first guard already stops it (a step lands on
    /// a restored backup exactly as on live data, ADR 0006).
    #[test]
    fn the_step_is_idempotent() {
        let once = v4();
        let twice = chat_to_v4(once.clone()).unwrap();
        assert_eq!(once, twice);
    }

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
