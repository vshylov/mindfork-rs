//! Stage-0 probe for cloud image generation (docs/research/image-generation.md §8,
//! question 2): does a local model call `image_generate` when a picture is asked for and
//! only then, relay a refusal without describing a picture, and judge the picture it got
//! against the request?
//!
//! The tool is a **stub** — a picture made by the provider probe
//! (`tools/image_gen_probe.py`), or a refusal — registered on top of the standard set and
//! offered beside the default profile's tools, so a call nobody asked for is measured
//! against the tools the model really sees. Its description and result lines are the
//! wording the research proposes. Spike only (`spike/image-generation-probe`).
//!
//! Env: `MINDFORK_ENGINE_URL` (a vision model), `MINDFORK_PROBE_PICTURES` (a folder with
//! `photo`, `lettering`, `russian`, `wide`, `tall` pictures), `MINDFORK_PROBE_OUT` (a JSON
//! Lines file the rows are appended to), `MINDFORK_PROBE_LABEL` (the model's name in the
//! rows), optionally `MINDFORK_PROBE_ARMS` (`ask,none,refuse,match`).

use super::*;
use crate::features::tools::{Tool, ToolContext, ToolImage, ToolOutcome};
use std::io::Write as _;

const TOOL_ID: &str = "image_generate";

const DESCRIPTION: &str = "Draws a picture with an image-generation model and saves it in \
the chat's files. Call it when the user asks you to draw, paint, illustrate or generate a \
picture, or to change one — not otherwise. `prompt` is the whole picture in words: subject, \
setting, style, composition, and any text it must show; a full description, not a title. To \
change a picture the chat already has, name it in `images` and describe the change. One call \
makes one picture.";

#[derive(Clone, Copy)]
enum Mode {
    /// The picture named `first` on the first call of the turn, `then` on every later one.
    Picture {
        first: &'static str,
        then: &'static str,
    },
    /// Every call refused with this reason.
    Refuse(&'static str),
}

struct StubState {
    mode: Mode,
    calls_this_turn: u32,
    saved: u32,
    /// `false` — the control arm: the result says the picture is saved and hands no
    /// pixels over, as `show_to_model` off would.
    with_image: bool,
}

struct Stub {
    pictures: std::collections::HashMap<String, (String, Vec<u8>, u32, u32)>,
    state: Arc<std::sync::Mutex<StubState>>,
}

#[async_trait::async_trait]
impl Tool for Stub {
    fn id(&self) -> crate::entities::profile::ToolId {
        TOOL_ID.into()
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        DESCRIPTION.into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The whole picture in words: subject, setting, style, composition, any text it must show. Any language."
                },
                "aspect": {
                    "type": "string",
                    "enum": ["1:1", "3:2", "2:3", "16:9", "9:16"],
                    "description": "The picture's shape; 1:1 when absent."
                },
                "images": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Pictures to change or to draw from, by the names the chat gives them (#3, cat.png, tool-image-2). Absent for a new picture."
                }
            },
            "required": ["prompt"]
        })
    }
    async fn invoke(
        &self,
        _ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> anyhow::Result<ToolOutcome> {
        let (mode, nth, saved, with_image) = {
            let mut s = self.state.lock().unwrap();
            s.calls_this_turn += 1;
            let mode = s.mode;
            if matches!(mode, Mode::Picture { .. }) {
                s.saved += 1;
            }
            (mode, s.calls_this_turn, s.saved, s.with_image)
        };
        match mode {
            Mode::Refuse(reason) => Ok(ToolOutcome::text(format!(
                "The provider refused to draw this: «{reason}». This is not a connection \
                 error, and the same prompt will be refused again. Tell the user what was \
                 refused; do not describe a picture."
            ))),
            Mode::Picture { first, then } => {
                let key = if nth == 1 { first } else { then };
                let (mime, bytes, w, h) = &self.pictures[key];
                let ext = if mime == "image/png" { "png" } else { "jpg" };
                let n = saved + 2; // the chat's first files are #1 and #2 in this story
                let name = format!("image-{saved}.{ext}");
                let entry = format!(
                    "Saved as #{n} {name} ({w}×{h}, {}) in the chat's files; the user can open it with /file open #{n}.",
                    crate::entities::attachment::format_bytes(bytes.len())
                );
                if !with_image {
                    return Ok(ToolOutcome::text(format!(
                        "{entry}\nYou cannot see this picture: showing it to you is switched \
                         off. This is not an error. Tell the user it is saved; do not describe it."
                    )));
                }
                use base64::Engine as _;
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                Ok(
                    ToolOutcome::text(entry.clone()).with_images(vec![ToolImage {
                        mime: mime.clone(),
                        data,
                        entry: Some(entry),
                    }]),
                )
            }
        }
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::ExternalWorld
    }
    fn ui_label(&self) -> &'static str {
        "Image generation"
    }
}

struct Case {
    arm: &'static str,
    id: &'static str,
    text: &'static str,
    mode: Mode,
}

const fn pic(name: &'static str) -> Mode {
    Mode::Picture {
        first: name,
        then: name,
    }
}

const fn wrong_then(first: &'static str, then: &'static str) -> Mode {
    Mode::Picture { first, then }
}

const CASES: &[Case] = &[
    // Asked for a picture: one call, a faithful prompt, the aspect where one is named.
    Case {
        arm: "ask",
        id: "a1",
        text: "Draw me a red fox sitting in the snow at dawn.",
        mode: pic("photo"),
    },
    Case {
        arm: "ask",
        id: "a2",
        text: "Нарисуй лису в снегу на рассвете.",
        mode: pic("photo"),
    },
    Case {
        arm: "ask",
        id: "a3",
        text: "Can you make a minimalist poster that says MINDFORK?",
        mode: pic("lettering"),
    },
    Case {
        arm: "ask",
        id: "a4",
        text: "Сделай минималистичный постер с надписью MINDFORK.",
        mode: pic("lettering"),
    },
    Case {
        arm: "ask",
        id: "a5",
        text: "I'd love a watercolor of a small house by a lake in autumn fog.",
        mode: pic("russian"),
    },
    Case {
        arm: "ask",
        id: "a6",
        text: "Нарисуй акварелью домик у озера в осеннем тумане.",
        mode: pic("russian"),
    },
    Case {
        arm: "ask",
        id: "a7",
        text: "Generate a wide cinematic image of a futuristic neon city at night.",
        mode: pic("wide"),
    },
    Case {
        arm: "ask",
        id: "a8",
        text: "Сгенерируй широкую картинку: футуристический город ночью, неон, летающие машины.",
        mode: pic("wide"),
    },
    Case {
        arm: "ask",
        id: "a9",
        text: "Draw a tall picture of a lighthouse in a storm for my phone wallpaper.",
        mode: pic("tall"),
    },
    Case {
        arm: "ask",
        id: "a10",
        text: "Нарисуй маяк в шторм, вертикально, для обоев на телефон.",
        mode: pic("tall"),
    },
    // Not asked: no call. Two of them are about pictures.
    Case {
        arm: "none",
        id: "n1",
        text: "What's the difference between PNG and JPEG?",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n2",
        text: "Describe what a traditional Japanese garden looks like.",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n3",
        text: "Translate 'good morning' into Spanish and French.",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n4",
        text: "Посоветуй, как улучшить сон.",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n5",
        text: "Write a haiku about autumn.",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n6",
        text: "What colours go well with navy blue in a living room?",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n7",
        text: "Explain how a lighthouse works.",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n8",
        text: "Расскажи, кто такой Клод Моне и чем известны его картины.",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n9",
        text: "What is 17 * 23?",
        mode: pic("photo"),
    },
    Case {
        arm: "none",
        id: "n10",
        text: "I'm planning a trip to Iceland — what should I pack?",
        mode: pic("photo"),
    },
    // Refused by the provider: relayed, no picture described.
    Case {
        arm: "refuse",
        id: "r1",
        text: "Draw a battle scene with knights fighting.",
        mode: Mode::Refuse("moderation_blocked: violence"),
    },
    Case {
        arm: "refuse",
        id: "r2",
        text: "Draw a photorealistic picture of a famous pop star on stage.",
        mode: Mode::Refuse("moderation_blocked: public figure"),
    },
    Case {
        arm: "refuse",
        id: "r3",
        text: "Нарисуй зомби, нападающих на город, в стиле хоррор.",
        mode: Mode::Refuse("IMAGE_SAFETY"),
    },
    Case {
        arm: "refuse",
        id: "r4",
        text: "Нарисуй Микки Мауса в стиле Диснея.",
        mode: Mode::Refuse("IMAGE_RECITATION"),
    },
    Case {
        arm: "refuse",
        id: "r5",
        text: "Draw a crime scene for the cover of my detective novel.",
        mode: Mode::Refuse("moderation_blocked: violence"),
    },
    // The model checks the picture: a wrong one is noticed (said, or drawn again).
    Case {
        arm: "match",
        id: "m1",
        text: "Draw a red fox in the snow, and check that it came out right.",
        mode: wrong_then("tall", "photo"),
    },
    Case {
        arm: "match",
        id: "m2",
        text: "Draw a lighthouse in a storm, and tell me whether the picture matches what I asked.",
        mode: pic("tall"),
    },
    Case {
        arm: "match",
        id: "m3",
        text: "Make a poster that says MINDFORK, and check the lettering is right.",
        mode: pic("lettering"),
    },
    Case {
        arm: "match",
        id: "m4",
        text: "Draw a futuristic city at night and check the result.",
        mode: wrong_then("photo", "wide"),
    },
    Case {
        arm: "match",
        id: "m5",
        text: "Нарисуй домик у озера осенью и проверь, что получилось.",
        mode: wrong_then("wide", "russian"),
    },
];

fn load_pictures(
    dir: &std::path::Path,
) -> std::collections::HashMap<String, (String, Vec<u8>, u32, u32)> {
    let mut out = std::collections::HashMap::new();
    for name in ["photo", "lettering", "russian", "wide", "tall"] {
        let found = ["png", "jpeg", "jpg", "webp"]
            .iter()
            .map(|ext| dir.join(format!("{name}.{ext}")))
            .find(|p| p.exists())
            .unwrap_or_else(|| panic!("no {name}.* in {}", dir.display()));
        let bytes = std::fs::read(&found).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        // Handed over as PNG or JPEG, as a provider client would after normalising.
        let mime = if bytes.starts_with(b"\x89PNG") {
            "image/png"
        } else {
            "image/jpeg"
        };
        let bytes = if mime == "image/jpeg" && !bytes.starts_with(b"\xff\xd8") {
            let mut buf = std::io::Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
            buf.into_inner()
        } else {
            bytes
        };
        let mime = if bytes.starts_with(b"\x89PNG") {
            "image/png"
        } else {
            "image/jpeg"
        };
        out.insert(
            name.to_string(),
            (mime.to_string(), bytes, img.width(), img.height()),
        );
    }
    out
}

#[tokio::test]
#[ignore = "spike probe: a live vision model (MINDFORK_ENGINE_URL) and MINDFORK_PROBE_PICTURES"]
async fn image_generate_probe_live() {
    let Some(pictures) = std::env::var_os("MINDFORK_PROBE_PICTURES") else {
        eprintln!("skip: MINDFORK_PROBE_PICTURES not set");
        return;
    };
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let label = std::env::var("MINDFORK_PROBE_LABEL").unwrap_or_else(|_| "model".into());
    let out_path = std::env::var("MINDFORK_PROBE_OUT").expect("MINDFORK_PROBE_OUT");
    let arms: Option<Vec<String>> = std::env::var("MINDFORK_PROBE_ARMS")
        .ok()
        .map(|a| a.split(',').map(|s| s.trim().to_string()).collect());
    // The control experiment: chosen conversations, repeated, each under every variant in
    // turn (interleaved, so a drift of the server's mood falls on all of them alike).
    // `image` — the picture inside the tool result (the shipped path on llama.cpp);
    // `noimage` — no picture handed over; `rehome` — the picture in a user message after
    // the tool result, as a gateway gets it.
    let ids: Option<Vec<String>> = std::env::var("MINDFORK_PROBE_IDS")
        .ok()
        .map(|a| a.split(',').map(|s| s.trim().to_string()).collect());
    let variants: Vec<String> = std::env::var("MINDFORK_PROBE_VARIANTS")
        .unwrap_or_else(|_| "image".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let repeat: usize = std::env::var("MINDFORK_PROBE_REPEAT")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(1);
    let state = Arc::new(std::sync::Mutex::new(StubState {
        mode: pic("photo"),
        calls_this_turn: 0,
        saved: 0,
        with_image: true,
    }));
    let stub = Arc::new(Stub {
        pictures: load_pictures(std::path::Path::new(&pictures)),
        state: state.clone(),
    });
    let mut cfg = no_auto_cfg();
    cfg.default_sampling.max_tokens = Some(4096);
    // The profile's language is the scaffold's (labels, notes): `MINDFORK_PROBE_LANG=en`
    // for an English one, the default (Russian) otherwise.
    let language = match std::env::var("MINDFORK_PROBE_LANG").as_deref() {
        Ok("en") => crate::shared::i18n::Lang::En,
        _ => crate::shared::i18n::Lang::default(),
    };
    let _dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(_dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config: cfg,
        supervisor: Arc::new(MockSupervisor::with_backend(Some(backend))),
        default_language: language,
        extra_tools: vec![stub],
    }));
    let mut tools = crate::features::tools::default_tool_ids();
    tools.push(TOOL_ID.into());
    // The default profile's tools plus the stub (live.rs's `narrow_profile_to`, inlined:
    // it is private to that module).
    let profile = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .and_then(|e| match e {
            AppEvent::ProfileList(ps) => ps.first().map(|p| p.id),
            _ => None,
        })
        .expect("the bootstrap profile");
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .expect("the bootstrap chat");
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profile,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(tools),
                ..Default::default()
            }),
        })
        .unwrap();

    let selected: Vec<&Case> = CASES
        .iter()
        .filter(|c| arms.as_ref().is_none_or(|a| a.iter().any(|x| x == c.arm)))
        .filter(|c| ids.as_ref().is_none_or(|i| i.iter().any(|x| x == c.id)))
        .collect();
    let mut runs: Vec<(usize, &str, &Case)> = Vec::new();
    for rep in 0..repeat {
        for variant in &variants {
            for case in &selected {
                runs.push((rep, variant.as_str(), case));
            }
        }
    }
    let mut first = true;
    for (rep, variant, case) in runs {
        crate::shared::api::openai::client::PROBE_REHOME
            .store(variant == "rehome", std::sync::atomic::Ordering::Relaxed);
        state.lock().unwrap().with_image = variant != "noimage";
        if !first {
            cmd_tx
                .send(AppCommand::NewChat { profile_id: None })
                .unwrap();
            wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
                .await
                .unwrap();
        }
        first = false;
        {
            let mut s = state.lock().unwrap();
            s.mode = case.mode;
            s.calls_this_turn = 0;
        }
        let started = std::time::Instant::now();
        cmd_tx
            .send(AppCommand::SendMessage(case.text.into()))
            .unwrap();
        let mut reply = String::new();
        let mut thoughts = 0usize;
        let mut calls = Vec::new();
        let mut errors = Vec::new();
        let mut reason = String::new();
        while let Some(ev) = evt_rx.recv().await {
            match ev {
                AppEvent::Chunk { text, .. } => reply.push_str(&text),
                AppEvent::Thoughts { text, .. } => thoughts += text.chars().count(),
                AppEvent::ToolCall {
                    name,
                    arguments,
                    result,
                    images,
                    ..
                } => calls.push(serde_json::json!({
                    "name": name, "arguments": arguments, "result": result, "images": images
                })),
                AppEvent::Error(e) => errors.push(e),
                AppEvent::Finished { reason: r, .. } => {
                    reason = format!("{r:?}");
                    break;
                }
                _ => {}
            }
        }
        let image_calls = calls.iter().filter(|c| c["name"] == TOOL_ID).count();
        let row = serde_json::json!({
            "model": label, "arm": case.arm, "id": case.id, "request": case.text,
            "variant": variant, "rep": rep, "lang": format!("{language:?}"),
            "image_calls": image_calls, "calls": calls, "reply": reply,
            "thoughts_chars": thoughts, "finish": reason, "errors": errors,
            "seconds": started.elapsed().as_secs_f32(),
        });
        eprintln!(
            "[{label}] {} {} {variant}#{rep}: {image_calls} image call(s), {} other, {reason}, {:.0}s\n  reply: {}",
            case.arm,
            case.id,
            calls.len() - image_calls,
            started.elapsed().as_secs_f32(),
            reply
                .chars()
                .take(300)
                .collect::<String>()
                .replace('\n', " ")
        );
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out_path)
            .unwrap();
        writeln!(f, "{row}").unwrap();
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}
