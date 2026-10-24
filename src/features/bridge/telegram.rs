//! Telegram's messages read into the bridge's own [`Incoming`] (ADR 0013): only
//! private chats, never a bot, a forward named by its origin, a command split
//! from its argument.

use crate::shared::messenger::telegram::{TgMessage, forward_name};

use super::{Author, Body, Incoming, Picture, Voice, split_command};

/// `m` as the bridge reads it; `None` for what is never answered — a group, a
/// channel, another bot, a message with no sender.
pub fn read_message(m: &TgMessage) -> Option<Incoming> {
    if m.chat.kind != "private" {
        return None;
    }
    let from = m.from.as_ref().filter(|u| !u.is_bot)?;
    let author = match &m.forward_origin {
        Some(origin) => Author::Forwarded(forward_name(origin)),
        None => Author::Person,
    };
    let body = match (&m.text, picture(m)) {
        _ if let Some(voice) = &m.voice => Body::Voice {
            voice: Voice::File {
                id: voice.file_id.clone(),
                seconds: voice.duration,
            },
            caption: m.caption.clone().unwrap_or_default(),
            author,
        },
        (_, Some(picture)) => Body::Images {
            pictures: vec![picture],
            caption: m.caption.clone().unwrap_or_default(),
            author,
        },
        (Some(text), None) => match author {
            Author::Forwarded(from) => Body::Forwarded {
                from,
                text: text.clone(),
            },
            Author::Person => match split_command(text) {
                Some((name, arg)) => Body::Command { name, arg },
                None => Body::Text(text.clone()),
            },
        },
        (None, None) => Body::Unsupported,
    };
    Some(Incoming {
        account: from.id.to_string(),
        name: from.handle(),
        body,
        album: m.media_group_id.clone(),
    })
}

/// The picture a message carries: a photo's largest size, or a file whose
/// type says it is an image.
fn picture(m: &TgMessage) -> Option<Picture> {
    if let Some(largest) = m.photo.as_ref().and_then(|sizes| sizes.last()) {
        return Some(Picture::File {
            id: largest.file_id.clone(),
            name: None,
        });
    }
    let doc = m.document.as_ref()?;
    doc.mime_type
        .as_deref()
        .is_some_and(|t| t.starts_with("image/"))
        .then(|| Picture::File {
            id: doc.file_id.clone(),
            name: doc.file_name.clone(),
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
    fn a_photo_is_its_largest_size_with_its_caption_and_album() {
        let got = read_message(&private(serde_json::json!({
            "photo": [
                {"file_id": "small", "width": 90, "height": 60},
                {"file_id": "large", "width": 1280, "height": 853}
            ],
            "caption": "our trip",
            "media_group_id": "g1"
        })))
        .unwrap();
        assert_eq!(
            got.body,
            Body::Images {
                pictures: vec![Picture::File {
                    id: "large".into(),
                    name: None
                }],
                caption: "our trip".into(),
                author: Author::Person,
            }
        );
        assert_eq!(got.album.as_deref(), Some("g1"));
    }

    #[test]
    fn an_image_file_is_a_picture_and_any_other_file_is_not() {
        let png = read_message(&private(serde_json::json!({
            "document": {"file_id": "d1", "file_name": "chart.png", "mime_type": "image/png"}
        })))
        .unwrap();
        assert!(matches!(
            png.body,
            Body::Images { ref pictures, .. }
                if pictures == &[Picture::File { id: "d1".into(), name: Some("chart.png".into()) }]
        ));
        let pdf = read_message(&private(serde_json::json!({
            "document": {"file_id": "d2", "file_name": "a.pdf", "mime_type": "application/pdf"}
        })))
        .unwrap();
        assert_eq!(pdf.body, Body::Unsupported);
    }

    #[test]
    fn a_forwarded_photo_keeps_its_origin() {
        let got = read_message(&private(serde_json::json!({
            "photo": [{"file_id": "p", "width": 10, "height": 10}],
            "forward_origin": {"type": "hidden_user", "sender_user_name": "Eve", "date": 0}
        })))
        .unwrap();
        assert!(matches!(
            got.body,
            Body::Images { author: Author::Forwarded(Some(ref n)), .. } if n == "Eve"
        ));
    }

    #[test]
    fn a_voice_note_is_its_file_and_length_and_a_forwarded_one_keeps_its_origin() {
        let own = read_message(&private(serde_json::json!({
            "voice": {"file_id": "v1", "file_unique_id": "u1", "duration": 9, "mime_type": "audio/ogg"}
        })))
        .unwrap();
        assert_eq!(
            own.body,
            Body::Voice {
                voice: Voice::File {
                    id: "v1".into(),
                    seconds: 9
                },
                caption: String::new(),
                author: Author::Person,
            }
        );
        let fwd = read_message(&private(serde_json::json!({
            "voice": {"file_id": "v2", "file_unique_id": "u2", "duration": 3},
            "forward_origin": {"type": "hidden_user", "sender_user_name": "Eve", "date": 0}
        })))
        .unwrap();
        assert!(matches!(
            fwd.body,
            Body::Voice { author: Author::Forwarded(Some(ref n)), .. } if n == "Eve"
        ));
        let audio = read_message(&private(serde_json::json!({
            "audio": {"file_id": "a", "file_unique_id": "u", "duration": 200}
        })))
        .unwrap();
        assert_eq!(
            audio.body,
            Body::Unsupported,
            "an audio file is not a voice note"
        );
    }

    #[test]
    fn a_sticker_is_unsupported_and_groups_and_bots_are_never_read() {
        let sticker =
            read_message(&private(serde_json::json!({"sticker": {"file_id": "s"}}))).unwrap();
        assert_eq!(sticker.body, Body::Unsupported);
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
