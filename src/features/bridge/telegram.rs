//! Telegram's messages read into the bridge's own [`Incoming`] (ADR 0013): only
//! private chats, never a bot, a forward named by its origin, a command split
//! from its argument.

use crate::shared::messenger::telegram::{TgMessage, forward_name};

use super::{Body, Incoming, split_command};

/// `m` as the bridge reads it; `None` for what is never answered — a group, a
/// channel, another bot, a message with no sender.
pub fn read_message(m: &TgMessage) -> Option<Incoming> {
    if m.chat.kind != "private" {
        return None;
    }
    let from = m.from.as_ref().filter(|u| !u.is_bot)?;
    let body = match (&m.text, &m.forward_origin) {
        (Some(text), Some(origin)) => Body::Forwarded {
            from: forward_name(origin),
            text: text.clone(),
        },
        (Some(text), None) => match split_command(text) {
            Some((name, arg)) => Body::Command { name, arg },
            None => Body::Text(text.clone()),
        },
        (None, _) => Body::Unsupported,
    };
    Some(Incoming {
        account: from.id.to_string(),
        name: from.handle(),
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(json: serde_json::Value) -> TgMessage {
        serde_json::from_value(json).unwrap()
    }

    fn private(extra: serde_json::Value) -> TgMessage {
        let mut m = serde_json::json!({
            "message_id": 1, "date": 0,
            "from": {"id": 42, "is_bot": false, "first_name": "Ann", "username": "ann"},
            "chat": {"id": 42, "type": "private"},
        });
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        message(m)
    }

    #[test]
    fn text_from_a_person_in_a_private_chat_is_read() {
        let got = read_message(&private(serde_json::json!({"text": "hi"}))).unwrap();
        assert_eq!(got.account, "42");
        assert_eq!(got.name, "@ann");
        assert_eq!(got.body, Body::Text("hi".into()));
    }

    #[test]
    fn a_command_and_a_forward_are_told_apart_from_text() {
        let cmd = read_message(&private(serde_json::json!({"text": "/start CODE2345"}))).unwrap();
        assert_eq!(
            cmd.body,
            Body::Command {
                name: "start".into(),
                arg: "CODE2345".into()
            }
        );
        let fwd = read_message(&private(serde_json::json!({
            "text": "/start X",
            "forward_origin": {"type": "hidden_user", "sender_user_name": "Eve", "date": 0}
        })))
        .unwrap();
        assert_eq!(
            fwd.body,
            Body::Forwarded {
                from: Some("Eve".into()),
                text: "/start X".into()
            },
            "a forwarded command is a quote, never a command"
        );
    }

    #[test]
    fn a_photo_is_unsupported_and_groups_and_bots_are_never_read() {
        let photo = read_message(&private(serde_json::json!({"photo": []}))).unwrap();
        assert_eq!(photo.body, Body::Unsupported);
        let group = message(serde_json::json!({
            "message_id": 1, "date": 0, "text": "hi",
            "from": {"id": 42, "is_bot": false, "first_name": "Ann"},
            "chat": {"id": -5, "type": "group"},
        }));
        assert!(read_message(&group).is_none());
        let bot = private(serde_json::json!({
            "text": "hi",
            "from": {"id": 9, "is_bot": true, "first_name": "B"}
        }));
        assert!(read_message(&bot).is_none());
    }
}
