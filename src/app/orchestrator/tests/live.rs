//! Orchestrator tests — live #[ignore] e2e smokes (Gemma/bge-m3 via MINDFORK_*_URL). Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;
use crate::features::tools::confirm::ToolDecision;
use crate::shared::api::EmbedRole;

/// Runs `topics` as ordinary turns, then folds the conversation with `/compact`
/// and returns `(summary, folded)`.
///
/// Shared by the two compaction smokes, which need the same three steps for
/// opposite reasons — one checks that a planted fact **survives** the summary,
/// the other that what the summary **dropped** is still reachable. The filler
/// turns are what push the seed behind the verbatim tail; only their topics
/// differ, which is why they are the parameter.
async fn fill_then_compact(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    topics: &[&str],
) -> (String, usize) {
    for topic in topics {
        let (r, _) = run_turn_capture(cmd_tx, evt_rx, topic).await;
        eprintln!("filler reply: {}", r.chars().take(80).collect::<String>());
    }
    cmd_tx.send(AppCommand::Compact).unwrap();
    let compacted = wait_for(evt_rx, |e| {
        matches!(e, AppEvent::Compacted { .. } | AppEvent::Error(_))
    })
    .await
    .unwrap();
    let AppEvent::Compacted {
        summary, folded, ..
    } = compacted
    else {
        panic!("compaction failed: {compacted:?}");
    };
    eprintln!(
        "folded {folded} messages into {} chars:\n{}",
        summary.chars().count(),
        summary.chars().take(600).collect::<String>()
    );
    (summary, folded)
}

/// Chat attachments, stage 3 go/no-go (docs/file-attachments.md): on a **large**
/// file the model finds the right place **by meaning in one `attachment_search`
/// call**, instead of walking pages. The payload sits deliberately deep — around
/// page 20 of ~25 — so paging to it would take a dozen-plus rounds and blow past
/// `max_tool_rounds`, while search reaches it immediately.
/// Needs both servers: `MINDFORK_ENGINE_URL` **and** `MINDFORK_EMBED_URL` (with no
/// embedder the index isn't built and the smoke has nothing to test).
/// `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL) and embedder (MINDFORK_EMBED_URL)"]
async fn attachment_search_e2e_live() {
    use crate::app::events::FileProgress;
    use crate::shared::config::AttachmentSettings;
    const CODE: &str = "ZARYA-4417";
    if std::env::var("MINDFORK_EMBED_URL").is_err() {
        eprintln!("skip: MINDFORK_EMBED_URL not set (the index needs a real embedder)");
        return;
    }
    let config = AppConfig {
        attachments: AttachmentSettings {
            max_file_tokens: 100, // force by reference
            excerpt_tokens: 60,
            page_tokens: 300,
            ..Default::default()
        },
        ..Default::default()
    };
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Filler around one topically distinctive paragraph: nothing but semantic
    // similarity can single it out.
    let mut body = String::from("Протоколы совещаний отдела эксплуатации.\n\n");
    for i in 1..=120 {
        body.push_str(&format!(
            "Пункт {i}: обсудили график дежурств и порядок передачи смены, решений не приняли.\n"
        ));
    }
    body.push_str(&format!(
        "\nПункт 121: по итогам проверки холодильной установки заменён компрессор; \
         инвентарный код запасной части: {CODE}.\n"
    ));
    for i in 122..=240 {
        body.push_str(&format!(
            "Пункт {i}: рассмотрели заявки на канцелярию и мелкий ремонт, замечаний нет.\n"
        ));
    }
    let path = dir.path().join("protocols.txt");
    std::fs::write(&path, &body).unwrap();

    cmd_tx
        .send(AppCommand::FileAttach {
            path: path.to_string_lossy().into_owned(),
        })
        .unwrap();
    // Wait for indexing to finish (it runs in the background after the attach).
    let indexed = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::FileProgress(FileProgress::Indexed { .. })
                | AppEvent::FileProgress(FileProgress::IndexSkipped { .. })
        )
    })
    .await
    .unwrap();
    eprintln!("index: {indexed:?}");
    assert!(
        matches!(
            indexed,
            AppEvent::FileProgress(FileProgress::Indexed { .. })
        ),
        "the smoke needs a real embedder: {indexed:?}"
    );

    let (answer, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "В прикреплённом файле protocols.txt где-то упомянута замена компрессора \
         холодильной установки. Найди это место и назови инвентарный код запасной части.",
    )
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let names: Vec<&String> = calls.iter().map(|(n, _)| n).collect();
    eprintln!("tool calls: {names:#?}");
    eprintln!("reply: {answer}");
    assert!(
        calls
            .iter()
            .any(|(n, _)| n == crate::features::tools::attachment::ATTACHMENT_SEARCH_ID),
        "the model must find the place by meaning, called: {names:?}"
    );
    assert!(
        answer.contains(CODE),
        "the code sits deep in the file and must be found: {answer}"
    );
}

/// Chat attachments, stage 2 go/no-go (docs/file-attachments.md): a file too big
/// to inline is attached **by reference**, and the model reaches the part it
/// needs — the answer sits on a **late** page, so the excerpt alone cannot
/// produce it. This is the direct regression for the behaviour seen on a live run
/// of stage 1, where the model had no reader tool at all and flailed into
/// `fs_read`/`web_search` instead.
///
/// Two turns, because stage 3 changed what "reaching it" looks like: with an
/// embedder configured the file is also indexed, and the model now legitimately
/// prefers **one** `attachment_search` call over walking pages (observed live —
/// the earlier, `attachment_read`-only assertion started failing on exactly that,
/// with the answer still correct). So turn 1 asserts the outcome and that the
/// model stayed within the attachment tools, and turn 2 asks for a specific page
/// to keep `attachment_read` itself covered live.
/// `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn attachment_read_e2e_live() {
    use crate::shared::config::AttachmentSettings;
    const CODE: &str = "ZARYA-8823";
    let config = AppConfig {
        attachments: AttachmentSettings {
            // Force by-reference, and page the file into a handful of pages.
            max_file_tokens: 100,
            excerpt_tokens: 60,
            page_tokens: 300,
            ..Default::default()
        },
        ..Default::default()
    };
    // No embedder on purpose: an indexed attachment hands the model
    // `attachment_search`, and then whether it pages through the file at all
    // becomes its choice rather than a property of the code. Search is covered
    // by `attachment_search_e2e_live`; this smoke owns the *guaranteed* path.
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_no_embed(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Filler first, the payload last — the excerpt shows only the beginning.
    let mut body = String::from("Технические заметки проекта.\n\n");
    for i in 1..=45 {
        body.push_str(&format!(
            "Заметка {i}: рутинная запись без особого содержания, строка для объёма.\n"
        ));
    }
    body.push_str(&format!("\nВНУТРЕННИЙ КОД СБОРКИ: {CODE}\n"));
    let path = dir.path().join("notes-big.txt");
    std::fs::write(&path, &body).unwrap();

    cmd_tx
        .send(AppCommand::FileAttach {
            path: path.to_string_lossy().into_owned(),
        })
        .unwrap();
    let attached = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::FileProgress(_)))
        .await
        .unwrap();
    eprintln!("attach: {attached:?}");

    let (answer, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "В прикреплённом файле notes-big.txt указан внутренний код сборки. \
         Прочитай файл и назови этот код.",
    )
    .await;
    let names: Vec<&String> = calls.iter().map(|(n, _)| n).collect();
    eprintln!("turn 1 tool calls: {names:#?}");
    eprintln!("turn 1 reply: {answer}");
    assert!(
        answer.contains(CODE),
        "the answer sits on a late page and must be found: {answer}"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // The stage-1 defect, precisely: the model must reach the file through the
    // attachment tools rather than improvising with the filesystem or the web.
    let attachment_tools = [
        crate::features::tools::attachment::ATTACHMENT_READ_ID,
        crate::features::tools::attachment::ATTACHMENT_SEARCH_ID,
    ];
    assert!(
        !calls.is_empty()
            && calls
                .iter()
                .all(|(n, _)| attachment_tools.contains(&n.as_str())),
        "the file must be reached through the attachment tools only, called: {names:?}"
    );
    // And the guaranteed path itself: with nothing indexed, paging is the only
    // way in, so this is now a property of the setup rather than a hope about
    // which route the model picks.
    assert!(
        calls
            .iter()
            .any(|(n, _)| n == crate::features::tools::attachment::ATTACHMENT_READ_ID),
        "a by-reference file with no index must be read page by page, called: {names:?}"
    );
}

/// Chat attachments (docs/file-attachments.md, stage 1 go/no-go): a file attached
/// with `/file attach` actually reaches the model through the real wire path and
/// is used to answer. Two phases in two chats: a **baseline** (no attachment —
/// the model can't know the invented code) and the attached case (it answers with
/// the code). The mirror half — that `/file remove` takes the text back out of the
/// request — is deterministic and covered by a unit test
/// (`attachments::removing_an_attachment_takes_it_out_of_the_request`), so it
/// needs no model. `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn file_attachment_e2e_live() {
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // An invented fact no model can know from pretraining.
    const CODE: &str = "ZARYA-7719";
    const QUESTION: &str =
        "What is the internal build code for project Mindfork? Reply with the code only.";
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Phase 1 — baseline: with nothing attached, the model must not produce the code.
    let (baseline, _) = run_turn_live(&cmd_tx, &mut evt_rx, QUESTION).await;
    eprintln!("baseline reply: {baseline}");

    // Phase 2 — a fresh chat with the file attached.
    cmd_tx
        .send(AppCommand::NewChat { profile_id: None })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let path = dir.path().join("build-notes.md");
    std::fs::write(
        &path,
        format!(
            "# Project Mindfork — internal notes\n\n\
             The internal build code for project Mindfork is {CODE}.\n\
             Do not confuse it with the release tag.\n"
        ),
    )
    .unwrap();
    cmd_tx
        .send(AppCommand::FileAttach {
            path: path.to_string_lossy().into_owned(),
        })
        .unwrap();
    let attached = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::FileProgress(crate::app::events::FileProgress::Attached { .. })
                | AppEvent::FileProgress(crate::app::events::FileProgress::Failed(_))
        )
    })
    .await
    .unwrap();
    eprintln!("attach: {attached:?}");

    let (answer, _) = run_turn_live(&cmd_tx, &mut evt_rx, QUESTION).await;
    eprintln!("attached reply: {answer}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        answer.contains(CODE),
        "the model must answer from the attached file, got: {answer}"
    );
    assert!(
        !baseline.contains(CODE),
        "the baseline must not know the invented code (otherwise the test proves nothing): {baseline}"
    );
}

/// Live e2e for image attachments (spec §9.10): an image staged with `/image attach`
/// reaches a vision-capable model, and **is still seen a turn later**, replayed out of
/// history rather than re-staged.
///
/// The fixture is a generated image, not a photograph, so the assertion is objective and
/// no pretrained knowledge can answer it: a solid blue field with a large white square in
/// the middle. Two turns, because history replay is the half a unit test cannot settle —
/// the first proves the image arrived, the second proves the stored message re-sent it.
///
/// Requires the server to be started with `--mmproj` (`/props` then reports
/// `modalities.vision`); against a text-only server this fails loudly rather than
/// skipping, which is deliberate — a vision smoke that quietly passes on a blind model is
/// worse than none (docs/lessons.md §9). `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a vision-capable OpenAI-compatible server (MINDFORK_ENGINE_URL + --mmproj)"]
async fn image_attachment_e2e_live() {
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // A blue field with a centred white square — two facts to check, both objective.
    let buf = image::ImageBuffer::from_fn(512, 512, |x, y| {
        if (160..352).contains(&x) && (160..352).contains(&y) {
            image::Rgb([255u8, 255, 255])
        } else {
            image::Rgb([20u8, 60, 200])
        }
    });
    let path = dir.path().join("figure.png");
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(buf)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    std::fs::write(&path, &bytes).unwrap();

    cmd_tx
        .send(AppCommand::ImageAttach {
            path: path.to_string_lossy().into_owned(),
        })
        .unwrap();
    let staged = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::ImageProgress(crate::app::events::ImageProgress::Attached { .. })
                | AppEvent::ImageProgress(crate::app::events::ImageProgress::Failed(_))
        )
    })
    .await
    .unwrap();
    eprintln!("attach: {staged:?}");
    assert!(
        matches!(
            staged,
            AppEvent::ImageProgress(crate::app::events::ImageProgress::Attached { .. })
        ),
        "the image was refused before it ever reached the model: {staged:?}"
    );

    // Turn 1 — the image is on the message being sent.
    let (first, _) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "What is the background colour of this image, and what shape is in the centre? \
         Answer in a few words.",
    )
    .await;
    eprintln!("turn 1 reply: {first}");

    // Turn 2 — nothing new is staged; the model can only answer from the replayed
    // history, which is what this half of the test exists to prove.
    let (second, _) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "What colour was the square in the image I sent? One word.",
    )
    .await;
    eprintln!("turn 2 reply: {second}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let first = first.to_lowercase();
    assert!(
        first.contains("blue"),
        "the model must see the background colour, got: {first}"
    );
    assert!(
        first.contains("square"),
        "the model must see the centred shape, got: {first}"
    );
    assert!(
        second.to_lowercase().contains("white"),
        "the image must still be visible a turn later, replayed from history, got: {second}"
    );
}

/// i18n Tier 1 (docs/history/i18n.md, go/no-go): a profile with agent-scaffold language `En` —
/// the auto-title of an English conversation is English, with NO Cyrillic. A fresh profile
/// (the bootstrap profile is locked: it already has a default chat), set it to En, create a
/// chat, run an English turn and auto-titling. `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn i18n_en_profile_title_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // A fresh profile (the bootstrap profile has a default chat → its language is locked).
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "English".into(),
            system_message: "You are a helpful assistant. Reply in English.".into(),
        })
        .unwrap();
    let pl = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(v) if v.len() >= 2),
    )
    .await
    .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v.last().unwrap().id,
        _ => unreachable!(),
    };
    // Scaffold language En (a fresh profile with no data → the change is allowed).
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                language: Some(crate::shared::i18n::Lang::En),
                ..Default::default()
            }),
        })
        .unwrap();
    // A new chat under this profile (will become active).
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let act = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match act {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };
    // An English turn.
    let (reply, _) =
        run_turn_live(&cmd_tx, &mut evt_rx, "Tell me a fun fact about the Moon.").await;
    eprintln!("en reply: {:?}", reply.chars().take(80).collect::<String>());
    // Auto-title from the conversation (digest/system message — in the scaffold language).
    cmd_tx.send(AppCommand::AutoRenameChat(chat_id)).unwrap();
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    let title = match renamed {
        AppEvent::ChatRenamed { title, .. } => title,
        _ => unreachable!(),
    };
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    eprintln!("en auto-title: {title:?}");
    let has_cyr = title
        .chars()
        .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c));
    assert!(!title.trim().is_empty(), "empty title");
    assert!(
        !has_cyr,
        "the English conversation's title contains Cyrillic: {title:?}"
    );
}

/// Live i18n Tier 2 smoke (docs/history/i18n.md, group 2c): an en profile + `current_time`.
/// The subject is Tier 2b — **a tool result is localized in the profile's
/// language** — not the tool's formatting options.
///
/// `current_time` has two legitimate result shapes (`features/tools/datetime.rs`):
/// without a `format` argument it renders the localized label plus UTC
/// ("Local time: … / UTC: …"), and **with** one it renders only the strftime
/// string, which carries no scaffold text in any language. So the label can only
/// be asserted against a call that passed no `format`, and the tool description
/// openly advertises `format` — an assertion that demands the label of *every*
/// call fails whenever the model takes that option, i.e. for the wrong reason
/// (docs/lessons.md §2). The alternative is therefore removed rather than hoped
/// away (§9): only `current_time` is enabled, and both the system message and
/// the prompt ask for an argument-free call, with one explicit re-ask if the
/// model passes `format` anyway. The Cyrillic check still applies to every
/// shape — under a `ru` profile the label, and the bad-format error, are
/// Russian — and the label check applies to the argument-free calls, of which at
/// least one is required, so neither assertion can pass vacuously.
///
/// Needs no network/sandbox. Run:
/// `MINDFORK_ENGINE_URL=…/v1 cargo test utils_en_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn utils_en_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "English".into(),
            system_message: "You are a helpful assistant. Reply in English. \
                 When you call the current_time tool, call it with no arguments \
                 at all — never pass the format parameter."
                .into(),
        })
        .unwrap();
    let pl = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(v) if v.len() >= 2),
    )
    .await
    .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v.last().unwrap().id,
        _ => unreachable!(),
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                language: Some(crate::shared::i18n::Lang::En),
                // Only the tool under test — nothing else can be reached for.
                enabled_tools: Some(vec!["current_time".to_string()]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    /// True when the call passed no usable `format` — i.e. the result is the
    /// localized default shape. Unparsable arguments count as "format passed":
    /// the label cannot be claimed for a shape we could not determine.
    fn no_format(args: &str) -> bool {
        let t = args.trim();
        if t.is_empty() {
            return true;
        }
        match serde_json::from_str::<serde_json::Value>(t) {
            // Mirrors the tool's own reading of the argument (datetime.rs):
            // absent, non-string or blank all fall back to the default shape.
            Ok(v) => v
                .get("format")
                .and_then(|f| f.as_str())
                .is_none_or(|s| s.trim().is_empty()),
            Err(_) => false,
        }
    }
    let current_time = |calls: Vec<(String, String, String)>| -> Vec<(String, String)> {
        calls
            .into_iter()
            .filter(|(n, _, _)| n == "current_time")
            .map(|(_, a, r)| (a, r))
            .collect()
    };

    let (_t, calls) = run_turn_capture_args(
        &cmd_tx,
        &mut evt_rx,
        "What is the current date and time? Call the current_time tool \
         with no arguments (do not pass format).",
    )
    .await;
    let mut ct = current_time(calls);
    if !ct.iter().any(|(a, _)| no_format(a)) {
        // The model took the `format` option despite both instructions; ask once
        // more, as explicitly as the tool contract allows. Two refusals is a
        // model-behaviour failure worth seeing, not a flake to absorb.
        eprintln!("current_time called with format only: {ct:#?} — re-asking");
        let (_t2, calls2) = run_turn_capture_args(
            &cmd_tx,
            &mut evt_rx,
            "Call current_time once more with an empty argument object {}, \
             passing no format, and show me its raw output.",
        )
        .await;
        ct.extend(current_time(calls2));
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    eprintln!("utils calls: {ct:#?}");

    let has_cyr = |s: &str| {
        s.chars()
            .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
    };
    assert!(!ct.is_empty(), "expected a current_time call");
    // Holds for both shapes: the ru default label and the ru bad-format error are Cyrillic.
    for (a, r) in &ct {
        assert!(
            !has_cyr(r),
            "current_time result has cyrillic (args {a:?}): {r:?}"
        );
    }
    // The localized label exists only in the argument-free shape — require one.
    let plain: Vec<&(String, String)> = ct.iter().filter(|(a, _)| no_format(a)).collect();
    assert!(
        !plain.is_empty(),
        "the model passed `format` on every current_time call, so the localized \
         label was never rendered — the i18n claim is untested: {ct:#?}"
    );
    for (_, r) in plain {
        assert!(r.contains("Local time:"), "expected English label: {r:?}");
        assert!(r.contains("UTC:"), "expected the UTC line: {r:?}");
    }
}

/// Live i18n Tier 2 smoke (docs/history/i18n.md, group 2b): an en profile + RAG tools.
/// The model adds a fact (`rag_add`) and searches for it (`rag_search`); **the
/// rag-tool results must be in English** (no Cyrillic) — the 2b criterion
/// (tool results are localized). Needs a real embedder (`MINDFORK_EMBED_URL`).
/// Run:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test rag_en_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn rag_en_e2e_live() {
    use crate::features::tools::all_tool_ids;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "English".into(),
            system_message: "You are a helpful assistant. Reply in English.".into(),
        })
        .unwrap();
    let pl = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(v) if v.len() >= 2),
    )
    .await
    .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v.last().unwrap().id,
        _ => unreachable!(),
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                language: Some(crate::shared::i18n::Lang::En),
                enabled_tools: Some(all_tool_ids()),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    let (_t1, calls1) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Add this fact to the knowledge base via rag_add: the capital of France is Paris.",
    )
    .await;
    let (_t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Now search the knowledge base via rag_search for: capital of France.",
    )
    .await;

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let all: Vec<(String, String)> = calls1.into_iter().chain(calls2).collect();
    eprintln!("rag calls: {all:#?}");
    let has_cyr = |s: &str| {
        s.chars()
            .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
    };
    // The rag-tool results are in English (the scaffold is translated, Tier 2 2b).
    for (n, r) in &all {
        if n == "rag_add" || n == "rag_search" {
            assert!(!has_cyr(r), "rag tool {n} result has cyrillic: {r:?}");
        }
    }
    assert!(
        all.iter().any(|(n, _)| n == "rag_add"),
        "expected rag_add call"
    );
    // English result markers (if the tool ran).
    assert!(
        all.iter()
            .any(|(n, r)| n == "rag_add" && r.contains("Chunks added")),
        "expected English rag_add result: {all:?}"
    );
}

/// End-to-end against a live model: with `send_followup_message` enabled the assistant
/// writes a **second message** as a separate bubble. Checks both the UI signal
/// (`AssistantContinue`) and the resulting chat structure (`Message.new_bubble`).
/// The model is unstable — the test is `#[ignore]`, run manually against Gemma/Qwen.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn followup_tool_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    cmd_tx
        .send(AppCommand::SendMessage(
            "Ответь короткой первой репликой-приветствием, затем ОБЯЗАТЕЛЬНО вызови \
             инструмент send_followup_message и напиши вторую реплику с интересным \
             фактом о космосе."
                .into(),
        ))
        .unwrap();

    let saw_continue = drain_until_finished(&mut evt_rx, |e| {
        matches!(e, AppEvent::AssistantContinue { .. })
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let new_bubbles = chat.messages.iter().filter(|m| m.new_bubble).count();
    eprintln!(
        "followup e2e: saw_continue={saw_continue}, new_bubble={new_bubbles}, \
         messages={}",
        chat.messages.len()
    );
    for (i, m) in chat.messages.iter().enumerate() {
        eprintln!(
            "  [{i}] {:?} new_bubble={} tools={:?} text={:?}",
            m.role,
            m.new_bubble,
            m.tool_calls.iter().map(|t| &t.name).collect::<Vec<_>>(),
            m.text.chars().take(60).collect::<String>()
        );
    }
    assert!(
        saw_continue && new_bubbles >= 1,
        "expected a second message as a separate bubble (followup)"
    );
}

/// End-to-end against a live model: with `rewrite_current_message` enabled the assistant
/// discards the reply it started and writes it anew; the discarded content goes into `Chat.deleted`.
/// Checks the UI signal (`AssistantRewrite`) and a non-empty deleted archive.
/// The model is unstable — the test is `#[ignore]`, run manually.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn rewrite_tool_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    cmd_tx
        .send(AppCommand::SendMessage(
            "Продемонстрируй инструмент rewrite_current_message строго по шагам, НИ ОДИН \
             не пропуская. Шаг 1: напиши ровно «2+2=5». Шаг 2 (ОБЯЗАТЕЛЬНЫЙ): сразу \
             вызови инструмент rewrite_current_message — без него задание не выполнено. \
             Шаг 3: после вызова напиши правильный ответ «2+2=4». Самое важное — \
             обязательно вызвать rewrite_current_message между шагами 1 и 3."
                .into(),
        ))
        .unwrap();

    let saw_rewrite = drain_until_finished(&mut evt_rx, |e| {
        matches!(e, AppEvent::AssistantRewrite { .. })
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    eprintln!(
        "rewrite e2e: saw_rewrite={saw_rewrite}, deleted={}, messages={}",
        chat.deleted.len(),
        chat.messages.len()
    );
    for (i, m) in chat.messages.iter().enumerate() {
        eprintln!(
            "  msg[{i}] {:?} text={:?}",
            m.role,
            m.text.chars().take(60).collect::<String>()
        );
    }
    assert!(
        saw_rewrite && !chat.deleted.is_empty(),
        "expected a discarded (rewritten) reply in Chat.deleted"
    );
}

/// End-to-end SelfModel probe against a live model (two sessions, one profile):
/// 1) session 1 — tell it facts about the user and ask it to record them in the "self-model"
///    (expect calls to `update_self_model`/`update_user_model`, a write to the DB);
/// 2) session 2 (a new chat of the same profile) — ask "what do you remember about me";
///    the "self-model" is mixed into the system prompt → expect recall.
/// Model behavior is unstable — the test is `#[ignore]`, run manually; assert the
/// **mechanism** (the DB is populated), and print the recalled text for evaluation.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // --- Session 1: tell it facts and ask it to record the self-model. ---
    let (s1_text, s1_tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Меня зовут Владимир, я пишу на Rust и не люблю многословие. \
         Запомни это: вызови update_user_model (черты, интересы) и update_self_model \
         (краткое описание себя и цель — помогать мне кратко и по делу).",
    )
    .await;
    eprintln!("session 1: tools={s1_tools:?}\ntext={s1_text:?}\n");

    // --- Session 2: a new chat of the same profile, check recall. ---
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let (s2_text, s2_tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Что ты обо мне помнишь и какие у тебя цели в общении со мной?",
    )
    .await;
    eprintln!("session 2: tools={s2_tools:?}\ntext={s2_text:?}\n");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Mechanism: after session 1 the profile's self-model is non-empty and saved to disk.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let model = reopened.db().self_model_get(pid).unwrap();
    eprintln!("self_model in DB: {model:#?}");
    let model = model.expect("expected a saved self-model after session 1");
    assert!(
        !model.is_empty(),
        "expected a non-empty self-model (the model should have called update_*)"
    );
    // At least one of the mutators was actually called.
    assert!(
        s1_tools
            .iter()
            .any(|t| t == "update_self_model" || t == "update_user_model"),
        "expected an update_self_model/update_user_model call in session 1"
    );
}

/// End-to-end observation probe: ask the model to record an observation via
/// `add_insight` — expect a self-note (@self) in the DB (the narrative moved into notes,
/// Tier 1 "narrative as notes"). `#[ignore]`, manual.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_insight_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    let (text, tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Я заметил, что иногда прошу кратко, а иногда — подробно. \
         Зафиксируй это наблюдение в своих наблюдениях: вызови инструмент add_insight \
         с коротким описанием этого противоречия.",
    )
    .await;
    eprintln!("insight: tools={tools:?}\ntext={text:?}\n");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // An observation is a self-note (@self), not an entry in the model's blob.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let self_notes = reopened
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    eprintln!("self-notes (observations) in DB: {self_notes:#?}");
    assert!(
        !self_notes.is_empty(),
        "expected at least one self-note (@self) — an observation from add_insight"
    );
    assert!(
        tools.iter().any(|t| t == "add_insight"),
        "expected an add_insight call"
    );
}

/// End-to-end auto-reflection (Tier 3) against a live model: `auto_reflect_every=1` →
/// after the very first assistant reply, reflection kicks off in the background, and it itself
/// updates the "self-model". Reflection is silent (no UI event) — poll the DB, waiting
/// for data to appear. `#[ignore]`, manual.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn auto_reflect_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let mut config = AppConfig::default();
    config.self_model.auto_reflect_every = 1; // reflection after every reply
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // A regular send: tell it facts, the assistant replies (and then background
    // reflection is expected to record the "self-model" on its own).
    let (_t, _tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Привет! Меня зовут Владимир, пишу на Rust и ценю краткость. Просто ответь \
         коротким приветствием.",
    )
    .await;

    // Wait for background reflection to write something (poll the DB up to ~60s): the model's
    // blob (summary/goals/interlocutor) OR an observation as a self-note (@self, Tier 1).
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let mut model = None;
    let mut self_notes = Vec::new();
    for _ in 0..120 {
        let db = Storage::open(Paths::with_root(&root)).unwrap();
        let m = db.db().self_model_get(pid).unwrap();
        self_notes = db
            .db()
            .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap();
        let blob_nonempty = m.as_ref().map(|m| !m.is_empty()).unwrap_or(false);
        if blob_nonempty || !self_notes.is_empty() {
            model = m;
            break;
        }
        drop(db);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!("auto-reflect: self_model={model:#?}\nself-notes={self_notes:#?}");
    let blob_nonempty = model.as_ref().map(|m| !m.is_empty()).unwrap_or(false);
    assert!(
        blob_nonempty || !self_notes.is_empty(),
        "expected background auto-reflection to populate the self-model (a blob or an observation note)"
    );
}

/// End-to-end "self-model" auto-consolidation (stage A1) against a live model:
/// `auto_consolidate_every=1` → after a reply, once observations (`@self`) reach ≥ 2, the
/// self-model's "sleep" kicks off in the background, itself merging duplicate observations (`note_merge`/
/// `note_supersede`) and/or compressing a bloated description. "Sleep" is silent (no UI event) —
/// observe the result by polling the DB. Assert the **mechanism** (observations get created); the fact
/// of duplicates being merged is printed for go/no-go (behavior is unstable). `#[ignore]`, manual:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test self_consolidation_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_consolidation_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let mut config = AppConfig::default();
    config.self_model.auto_consolidate_every = 1; // "sleep" after every reply
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // Assemble the orchestrator with "sleep" enabled + (if possible) a real embedder.
    let embedder = live_embedder();
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let deps = OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend_and_embedder(
            Some(backend),
            embedder,
        )),
        default_language: crate::shared::i18n::Lang::default(),
    };
    let handle = tokio::spawn(run(deps));
    let root = dir.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Two similar observations (duplicate candidates) — record them, without merging yet.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение (add_insight): я ценю краткость в ответах. Просто запиши.",
    )
    .await;
    let (_t2, tools2) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши ещё одно наблюдение (add_insight): пользователь предпочитает лаконичные, \
         краткие ответы. Просто запиши, оба оставь.",
    )
    .await;
    eprintln!("observations: {tools1:?} + {tools2:?}");

    // After the second reply, observations ≥ 2 → the self-model's background "sleep" should
    // kick off and possibly merge duplicates. Poll the DB up to ~90s: count self-notes.
    let count_self = |root: &std::path::Path| -> usize {
        Storage::open(Paths::with_root(root))
            .unwrap()
            .db()
            .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap()
            .len()
    };
    // Wait for ≥2 observations (both entries have landed), then watch for "sleep" merging them.
    let mut before = 0usize;
    for _ in 0..180 {
        before = count_self(&root);
        if before >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    // Trigger one more turn (in case "sleep" after turn2 missed due to cadence):
    // every reply increments the counter, every=1 → "sleep" is attempted again.
    let _ = run_turn_live(&cmd_tx, &mut evt_rx, "Спасибо, коротко подтверди.").await;
    let mut after = before;
    for _ in 0..180 {
        after = count_self(&root);
        if after < before {
            break; // duplicates merged by "sleep"
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!(
        "self-notes: before={before}, after={after} (sleep merged duplicates: {})",
        after < before
    );
    // Mechanism: observation notes are created (add_insight ran).
    assert!(
        before >= 1,
        "expected at least one observation (@self) from add_insight"
    );
    let _ = (tools1, tools2);
}

/// End-to-end **gate** probe (the core of the Tier 1 "narrative as notes" hypothesis): the model
/// records an observation (`add_insight` → a self-note @self), then a near-duplicate —
/// the `add_insight` gate shows the similar existing observation with a hint to
/// rewrite it via `note_revise`/`note_supersede` instead of a copy. Assert the
/// **mechanism** (self-notes get created; the gate fires deterministically —
/// the embedder in tests is `MockEmbedder`, observation #1 already exists); the model's
/// **decision** to integrate is printed for go/no-go (behavior is unstable). Run (needs
/// a live server + possibly an embedder):
/// `MINDFORK_ENGINE_URL=…/v1 cargo test self_model_gate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_gate_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    // A real chat + embedder (MINDFORK_ENGINE_URL / MINDFORK_EMBED_URL) — the gate
    // works on real embeddings (bge-m3 etc.), not on MockEmbedder.
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Session 1: record an observation → self-note #1.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши в свои наблюдения (вызови add_insight): я склонен просить краткие ответы.",
    )
    .await;
    eprintln!("session 1: tools={tools1:?}");

    // Session 2: a near-duplicate — the add_insight gate should show observation #1.
    let (t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Запиши ещё одно, очень похожее наблюдение (вызови add_insight): пользователь \
         предпочитает лаконичные, краткие ответы. Если инструмент покажет похожее \
         наблюдение — реши сам, переписать ли его (note_revise/note_supersede) или \
         оставить оба.",
    )
    .await;
    eprintln!("session 2: text={t2:?}\ncalls={calls2:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Mechanism: add_insight in session 1 created a self-note (@self).
    assert!(
        tools1.iter().any(|t| t == "add_insight"),
        "session 1: expected an add_insight call"
    );
    let self_notes = Storage::open(Paths::with_root(&root))
        .unwrap()
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    eprintln!(
        "self-notes in DB: {} — {:#?}",
        self_notes.len(),
        self_notes
            .iter()
            .map(|n| n.content.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !self_notes.is_empty(),
        "expected self-notes (@self) from add_insight"
    );

    // Gate: did the add_insight result in session 2 show the similar observation?
    let gate_fired = calls2
        .iter()
        .any(|(n, r)| n == "add_insight" && r.contains("Похожие наблюдения"));
    // With the test MockEmbedder (MINDFORK_EMBED_URL unset), the gate is deterministic:
    // if the model called add_insight, it MUST fire (observation #1 already exists).
    // With a real embedder, firing depends on its configuration (e.g. llama-server
    // needs `--embeddings`), and on unavailability the gate gracefully degrades to empty —
    // so there it is only diagnostic, not a hard check.
    let real_embedder = std::env::var("MINDFORK_EMBED_URL").is_ok();
    if !real_embedder && calls2.iter().any(|(n, _)| n == "add_insight") {
        assert!(
            gate_fired,
            "the add_insight gate should have shown the similar observation (MockEmbedder, core of the hypothesis): {calls2:?}"
        );
    }
    // Integrating the near-duplicate: rewrite/replace/merge observations.
    let integrated = calls2
        .iter()
        .any(|(n, _)| n == "note_revise" || n == "note_supersede" || n == "note_merge");
    eprintln!(
        "gate showed a similar observation: {gate_fired} (real embedder: {real_embedder}); \
         model integrated (note_revise/supersede/merge): {integrated}"
    );
    // The model must have touched the observations somehow (otherwise the hypothesis isn't tested).
    assert!(
        calls2.iter().any(|(n, _)| {
            n == "add_insight" || n == "note_revise" || n == "note_supersede" || n == "note_merge"
        }),
        "session 2: expected add_insight/note_revise/note_supersede/note_merge"
    );
}

/// Live i18n Tier 2 smoke (docs/history/i18n.md, group 2a): the en mirror of
/// [`self_model_gate_e2e_live`]. A profile with scaffold language `En` + all tools; a turn
/// in English records an observation, a near-duplicate raises the `add_insight` gate —
/// **both the gate text and the whole tool result must be in English** (the key
/// Tier 2 criterion: tool results are localized, no Cyrillic). A real
/// embedder (`MINDFORK_EMBED_URL`) is needed for the gate to fire. Run:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test self_model_gate_en_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_gate_en_e2e_live() {
    use crate::features::tools::all_tool_ids;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // A fresh profile (the bootstrap profile is locked to Ru) → set scaffold language En.
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "English".into(),
            system_message: "You are a helpful assistant. Reply in English.".into(),
        })
        .unwrap();
    let pl = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(v) if v.len() >= 2),
    )
    .await
    .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v.last().unwrap().id,
        _ => unreachable!(),
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                language: Some(crate::shared::i18n::Lang::En),
                enabled_tools: Some(all_tool_ids()),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Turn 1: record an observation (add_insight → a self-note).
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Record an observation about me (call add_insight): I tend to ask for concise answers.",
    )
    .await;
    eprintln!("turn 1 tools: {tools1:?}");

    // Turn 2: a near-duplicate — the add_insight gate shows observation #1 (in English).
    let (t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Record another very similar observation (call add_insight): the user prefers brief, \
         concise replies. If the tool shows a similar observation, decide yourself whether to \
         rewrite it (note_revise/note_supersede) or keep both.",
    )
    .await;
    eprintln!("turn 2 text={t2:?}\ncalls={calls2:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        tools1.iter().any(|t| t == "add_insight"),
        "turn 1: expected add_insight call"
    );
    // Tool results contain no Cyrillic (the scaffold is translated, Tier 2).
    let has_cyr = |s: &str| {
        s.chars()
            .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
    };
    for (n, r) in &calls2 {
        assert!(!has_cyr(r), "tool {n} result has cyrillic: {r:?}");
    }
    // Gate (real embedder): text — the English template "Similar observations …".
    let gate_fired = calls2
        .iter()
        .any(|(n, r)| n == "add_insight" && r.contains("Similar observations"));
    let real_embedder = std::env::var("MINDFORK_EMBED_URL").is_ok();
    eprintln!("gate fired (English): {gate_fired}; real embedder: {real_embedder}");
    if real_embedder && calls2.iter().any(|(n, _)| n == "add_insight") {
        assert!(
            gate_fired,
            "en gate should have surfaced a similar observation in English: {calls2:?}"
        );
    }
}

/// End-to-end probe of the **summary-size gate** (stage 2, docs/summary-as-snapshot.md):
/// seed the DB with a bloated self-description (beyond the default target of 1000 chars);
/// the model sees a hint to shrink it, both in the passive injection and in `get_self_model`.
/// Ask it to read the self-model and shrink the description, moving event-like content into observations.
/// Assert the **mechanism** (the model touched the self-model: `update_self_model` and/or
/// `add_insight`); the actual shrinkage is printed for go/no-go (behavior is unstable).
/// `#[ignore]`, manual:
/// `MINDFORK_ENGINE_URL=…/v1 cargo test summary_gate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn summary_gate_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Seed a bloated self-description (beyond the default target of 1000 chars).
    let bloated = "Я ассистент, ценю честность и точность. ".repeat(40); // ~1600 chars.
    let bloated_len = bloated.chars().count();
    {
        let storage = Storage::open(Paths::with_root(&root)).unwrap();
        let mut m = crate::entities::self_model::SelfModel::new(pid);
        m.summary = bloated.clone();
        storage.db().self_model_upsert(&m).unwrap();
    }

    // Turn: ask it to read the self-model and shrink the description.
    let (t, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Прочитай свою «модель себя» (вызови get_self_model). Если описание себя \
         разрослось — сократи его до сути через update_self_model.summary, а событийные \
         выводы вынеси в наблюдения (add_insight).",
    )
    .await;
    eprintln!("text={t:?}\ncalls={calls:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Gate: the get_self_model result showed the hint (deterministic — summary
    // is bloated beyond the target), if the model called it.
    let gate_fired = calls
        .iter()
        .any(|(n, r)| n == "get_self_model" && r.contains("Описание себя разрослось"));
    if calls.iter().any(|(n, _)| n == "get_self_model") {
        assert!(
            gate_fired,
            "get_self_model with a bloated description should carry a hint to shrink it: {calls:?}"
        );
    }
    // Final description size in the DB.
    let stored = Storage::open(Paths::with_root(&root))
        .unwrap()
        .db()
        .self_model_get(pid)
        .unwrap();
    let final_len = stored
        .as_ref()
        .map(|m| m.summary.chars().count())
        .unwrap_or(0);
    let shrank = final_len < bloated_len;
    let wrote_insight = calls.iter().any(|(n, _)| n == "add_insight");
    eprintln!(
        "gate showed the hint: {gate_fired}; description {bloated_len} → {final_len} \
         (shrank: {shrank}); moved into observations: {wrote_insight}"
    );
    // The model must have touched the self-model somehow (otherwise the hypothesis isn't tested).
    assert!(
        calls
            .iter()
            .any(|(n, _)| n == "update_self_model" || n == "add_insight"),
        "expected update_self_model/add_insight: {calls:?}"
    );
}

/// End-to-end probe of the **graph over observations** (Tier 2, step B): the model records two
/// related observations, then links them (`note_link`). Assert the mechanism
/// (observation notes get created; the model inspected the self-model / linked); the appearance
/// of a link in the graph is printed for go/no-go (behavior is unstable). Relevance-based
/// injection is checked deterministically (`injection_recent_surfaces_relevant_over_fresh`)
/// + a manual multi-session user run. `#[ignore]`, manual:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test self_model_graph_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_graph_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Two related (contradicting) observations.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение (add_insight): я ценю краткость в ответах.",
    )
    .await;
    let (_t2, tools2) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши ещё одно наблюдение (add_insight): но иногда я даю слишком многословные ответы.",
    )
    .await;
    eprintln!("observations: {tools1:?} + {tools2:?}");

    // Ask it to inspect the self-model and link the contradicting observations.
    let (t3, calls3) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Посмотри свои наблюдения (get_self_model). Если два из них противоречат друг \
         другу — свяжи их инструментом note_link (relation=contradicts) по полному id.",
    )
    .await;
    eprintln!("linking: text={t3:?}\ncalls={calls3:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let self_notes = reopened
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    let links = reopened.db().note_links_all(pid).unwrap();
    eprintln!(
        "self-notes: {}; links in the observation graph: {} — {links:?}",
        self_notes.len(),
        links.len()
    );

    // Mechanism: observation notes are created.
    assert!(self_notes.len() >= 2, "expected ≥2 observation notes");
    let linked = calls3.iter().any(|(n, _)| n == "note_link");
    eprintln!(
        "model called note_link: {linked}; links now: {}",
        links.len()
    );
    // The model must have inspected itself and/or linked (otherwise the graph isn't tested).
    assert!(
        calls3
            .iter()
            .any(|(n, _)| n == "get_self_model" || n == "note_link"),
        "session 3: expected get_self_model/note_link"
    );
}

/// End-to-end probe of the **self-consolidation overview** (Tier 3, deferred from Tier 2 B):
/// the model records two similar observations, then calls `reflect` — its result
/// now carries a block "Overview of observations for consolidation" (similar pairs / contradicts /
/// no links), under which the model merges duplicates (`note_merge`/`note_supersede`/
/// `note_revise`). Assert the **mechanism** (≥1 observation note; when `reflect` is called
/// with ≥2 observations, its result contains the overview — deterministic, the header/counts
/// are built without vectors and don't depend on the embedder threshold); the actual merging of duplicates
/// is printed for go/no-go (behavior is unstable). `#[ignore]`, manual:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test self_consolidation_overview_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_consolidation_overview_e2e_live() {
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Two similar observations (duplicate candidates) — do NOT merge them yet.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение (add_insight): я ценю краткость в ответах. Пока не объединяй \
         ни с чем — просто запиши.",
    )
    .await;
    let (_t2, tools2) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши ещё одно наблюдение (add_insight): пользователь предпочитает лаконичные, \
         краткие ответы. Даже если инструмент покажет похожее — на этот раз оставь оба.",
    )
    .await;
    eprintln!("observations: {tools1:?} + {tools2:?}");

    // Ask it to reflect and merge duplicates — reflect carries the self-consolidation overview.
    let (t3, calls3) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Вызови reflect и просмотри блок «Обзор наблюдений для консолидации». Если среди \
         наблюдений есть похожие дубли — сведи их (note_merge или note_supersede).",
    )
    .await;
    eprintln!("reflection: text={t3:?}\ncalls={calls3:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let self_notes = reopened
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    eprintln!(
        "self-notes in DB: {} — {:#?}",
        self_notes.len(),
        self_notes
            .iter()
            .map(|n| n.content.clone())
            .collect::<Vec<_>>()
    );
    // Mechanism: observation notes are created.
    assert!(
        !self_notes.is_empty(),
        "expected self-notes (@self) from add_insight"
    );

    // Self-consolidation overview: if the model called reflect and there are ≥2 observations, its
    // result MUST carry the "Overview of observations" block (deterministic — the header and
    // counts are built without vectors, the embedder threshold only affects the list of similar pairs).
    // If the model already merged duplicates in session 2, observations < 2 and there's no overview — fine.
    if let Some((_, result)) = calls3.iter().find(|(n, _)| n == "reflect") {
        let has_overview = result.contains("Обзор наблюдений");
        eprintln!("reflect returned the self-consolidation overview: {has_overview}");
        if self_notes.len() >= 2 {
            assert!(
                has_overview,
                "reflect with ≥2 observations should carry the self-consolidation overview: {result}"
            );
        }
    }
    // Merging duplicates — go/no-go (unstable): print it.
    let consolidated = calls3
        .iter()
        .any(|(n, _)| n == "note_merge" || n == "note_supersede" || n == "note_revise");
    eprintln!("model merged duplicates (note_merge/supersede/revise): {consolidated}");
    // The model must have reflected and/or touched the observations.
    assert!(
        calls3.iter().any(|(n, _)| {
            n == "reflect" || n == "note_merge" || n == "note_supersede" || n == "note_revise"
        }),
        "session 3: expected reflect/note_merge/note_supersede/note_revise: {calls3:?}"
    );
}

/// End-to-end probe of the **related-traits gate** for `user_model` (Tier 2, step C): the model
/// adds an interlocutor trait (`update_user_model` with `add_traits`), then a close one —
/// the `add_traits` gate shows the related trait and asks it to decide (duplicate/contradiction).
/// Assert the **mechanism** (the trait is recorded in `perceived_traits`; the second session
/// again calls `update_user_model`); the gate firing and the model's decision are printed for
/// go/no-go. **Gate threshold 0.72** (calibrated on bge-m3, unlike the threshold-free
/// `add_insight` gate), so on a real embedder firing depends on how close the
/// model-generated phrasings are — here this is diagnostic, not a hard check.
/// `#[ignore]`, manual:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test trait_gate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn trait_gate_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Session 1: add an interlocutor trait → user_model.perceived_traits.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Обнови модель собеседника (вызови update_user_model): добавь черту (add_traits) \
         — «ценит краткость в ответах».",
    )
    .await;
    eprintln!("session 1: tools={tools1:?}");

    // Session 2: a very similar trait — the add_traits gate should warn about a duplicate.
    let (t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Обнови модель собеседника ещё раз (update_user_model): добавь очень похожую \
         черту (add_traits) — «любит лаконичность». Если инструмент предупредит о \
         почти-дубле — реши сам, объединить ли их через remove_traits.",
    )
    .await;
    eprintln!("session 2: text={t2:?}\ncalls={calls2:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Mechanism: update_user_model in session 1 recorded a trait.
    assert!(
        tools1.iter().any(|t| t == "update_user_model"),
        "session 1: expected an update_user_model call"
    );
    let stored = Storage::open(Paths::with_root(&root))
        .unwrap()
        .db()
        .self_model_get(pid)
        .unwrap();
    let traits = stored
        .as_ref()
        .map(|m| m.user_model.perceived_traits.clone())
        .unwrap_or_default();
    eprintln!("interlocutor traits in DB: {traits:?}");
    assert!(
        !traits.is_empty(),
        "expected ≥1 trait in user_model from update_user_model"
    );

    // Gate: did the update_user_model result in session 2 show the related trait?
    // (`remove_traits` is an argument of update_user_model, not a separate tool name,
    // so integration is read from the final DB state, not the call name.)
    let gate_fired = calls2
        .iter()
        .any(|(n, r)| n == "update_user_model" && r.contains("Родственные черты"));
    // Integrating the near-duplicate: the model merged the paraphrases into one trait (didn't keep both).
    let integrated = traits.len() <= 1;
    eprintln!(
        "gate warned about a related trait: {gate_fired}; \
         model merged them into one trait (no paraphrase pile-up): {integrated}"
    );

    // The model must have touched the interlocutor model again (otherwise the gate isn't tested).
    assert!(
        calls2.iter().any(|(n, _)| n == "update_user_model"),
        "session 2: expected update_user_model"
    );
}

/// End-to-end probe of **cross-organ links** (Tier 3, Path 1): the model records a fact
/// "about the interlocutor" (`note_save`) and an observation "about itself" (`add_insight`), then links
/// them (`note_link`) — an edge between memory organs. Assert the **mechanism** (both notes
/// get created; the model inspected both organs and/or linked); the appearance of a **cross-organ
/// edge** (one end `@self`, the other a user note) is printed for
/// go/no-go. `#[ignore]`, manual:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test cross_organ_link_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn cross_organ_link_e2e_live() {
    use crate::features::tools::notes::{SELF_NOTE_TAG, is_self_note};
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // A fact "about the interlocutor" → a user note.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши заметку о собеседнике (note_save): пользователь ценит краткость в ответах.",
    )
    .await;
    // An observation "about itself" → a self-note.
    let (_t2, tools2) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение о себе (add_insight): я склонен давать многословные ответы.",
    )
    .await;
    eprintln!("saving: {tools1:?} + {tools2:?}");

    // Ask it to link the observation "about itself" with the fact "about the interlocutor" (cross-organ).
    let (t3, calls3) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Посмотри свои наблюдения (get_self_model) и заметки о собеседнике (note_recall). \
         Если наблюдение о себе противоречит факту о собеседнике — свяжи их note_link \
         (relation=contradicts) по их id.",
    )
    .await;
    eprintln!("linking: text={t3:?}\ncalls={calls3:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let self_notes = reopened
        .db()
        .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    let links = reopened.db().note_links_all(pid).unwrap();
    // A cross-organ edge: one end @self, the other a user note.
    let cross = links
        .iter()
        .filter(|(f, t, _)| {
            let fs = reopened
                .db()
                .note_get(pid, *f)
                .ok()
                .flatten()
                .as_ref()
                .map(is_self_note);
            let ts = reopened
                .db()
                .note_get(pid, *t)
                .ok()
                .flatten()
                .as_ref()
                .map(is_self_note);
            matches!((fs, ts), (Some(a), Some(b)) if a != b)
        })
        .count();
    eprintln!(
        "self-notes: {}; links total: {}; cross-organ: {cross} — {links:?}",
        self_notes.len(),
        links.len()
    );

    // Mechanism: both notes are created (an observation @self + a user note).
    assert!(
        !self_notes.is_empty(),
        "expected a self-note from add_insight"
    );
    let all = reopened.db().note_list(pid, None, &[], None).unwrap();
    assert!(
        all.iter().any(|n| !is_self_note(n)),
        "expected a user note from note_save"
    );
    let linked = calls3.iter().any(|(n, _)| n == "note_link");
    eprintln!("model called note_link: {linked}; cross-organ edges: {cross}");
    // The model must have inspected the organs and/or linked (otherwise the cross-link isn't tested).
    assert!(
        calls3
            .iter()
            .any(|(n, _)| n == "get_self_model" || n == "note_recall" || n == "note_link"),
        "session 3: expected get_self_model/note_recall/note_link"
    );
}

/// End-to-end probe of **output mixing** (Tier 3, Path 2): with the toggle
/// `notes.recall_includes_self` on, observations "about itself" (`@self`) are included in the general
/// `note_recall` with an "about self" marker. Assert the **mechanism** (the toggle is applied; the model
/// called `note_recall`); the appearance of a marked self-observation and the **model's reply**
/// (does it "contaminate" — go/no-go on mixing safety) are printed. `#[ignore]`,
/// manual: `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test recall_includes_self_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn recall_includes_self_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let _pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;
    // Enable output mixing (Path 2). MockSupervisor ignores server settings,
    // so the live backend/embedder stay in place; only recall_includes_self changes.
    let config = AppConfig {
        notes: crate::shared::config::NotesSettings {
            recall_includes_self: true,
            ..Default::default()
        },
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(config)))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.notes.recall_includes_self),
    )
    .await
    .unwrap();

    // A fact "about the interlocutor" + an observation "about itself".
    run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши заметку о собеседнике (note_save): пользователь любит краткость.",
    )
    .await;
    run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Запиши наблюдение о себе (add_insight): я склонен к многословию.",
    )
    .await;

    // Search: with the toggle on, note_recall should return both the note and the observation
    // "about itself" (with an "about self" marker).
    let (t3, calls3) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Поищи в заметках (note_recall) всё про краткость и многословие — что там есть?",
    )
    .await;
    eprintln!("recall: text={t3:?}\ncalls={calls3:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Mechanism: with the toggle on, note_recall returned the marked self-observation.
    let self_marked = calls3
        .iter()
        .any(|(n, r)| n == "note_recall" && r.contains("[о себе]"));
    eprintln!("note_recall showed the self-observation with its marker: {self_marked}");
    assert!(
        calls3.iter().any(|(n, _)| n == "note_recall"),
        "expected a note_recall call"
    );
}

/// End-to-end probe of **linking notes with RAG sources** (Tier 3, Path 3): the model
/// adds a document to the knowledge base, finds it (`rag_search`), records a conclusion
/// as a note (`note_save`) and links it to the source (`note_cite_source`). Assert
/// the **mechanism** (the source is in the base; the model searched/linked); the appearance of a note↔
/// source link in the DB is printed for go/no-go. `#[ignore]`, manual:
/// `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1 cargo test note_cite_source_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn note_cite_source_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;

    // Knowledge base: add a document under a named source.
    let (_t1, tools1) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Добавь в базу знаний (rag_add) текст «Столица Франции — Париж.» с источником «факты».",
    )
    .await;
    eprintln!("rag_add: {tools1:?}");

    // The model searches, records a conclusion, and links it to the source.
    let (t2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Найди в базе знаний (rag_search) про столицу Франции. Запиши краткий вывод \
         заметкой (note_save), затем свяжи эту заметку с источником через \
         note_cite_source (источник называется «факты»).",
    )
    .await;
    eprintln!("citing: text={t2:?}\ncalls={calls2:#?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    // The source name both prompts above ask the model to use. Bound once so the
    // two lookups and the log cannot drift apart — and so the log line stays a
    // pure English label with the (Russian) source name interpolated as data.
    const SOURCE: &str = "факты";
    // Mechanism: the source is in the knowledge base.
    assert!(
        reopened.db().rag_source_exists(pid, SOURCE).unwrap(),
        "expected the source to be in the knowledge base"
    );
    // Note↔source link (the reverse path): notes citing the source.
    let citing = reopened.db().notes_citing_source(pid, SOURCE).unwrap();
    let cited = calls2.iter().any(|(n, _)| n == "note_cite_source");
    eprintln!(
        "model called note_cite_source: {cited}; notes citing {SOURCE:?}: {} — {:?}",
        citing.len(),
        citing.iter().map(|n| n.content.clone()).collect::<Vec<_>>()
    );
    // The model must have searched and/or linked (otherwise the path isn't tested).
    assert!(
        calls2
            .iter()
            .any(|(n, _)| n == "rag_search" || n == "note_cite_source"),
        "session 2: expected rag_search/note_cite_source"
    );
}

/// **Calibration** smoke for the A2 threshold (`SUMMARY_OBS_SIMILARITY` in `overview.rs`): against a
/// REAL embedder (bge-m3 via `MINDFORK_EMBED_URL`) embeds labeled pairs
/// "self-description paragraph ↔ observation" and **prints** their cosines, so the reader can pick
/// a threshold from the numbers (like `TRAIT_SIMILARITY`). Doesn't assert a hard threshold — only that
/// paraphrases are on average closer than unrelated pairs. `#[ignore]`, manual:
/// `MINDFORK_EMBED_URL=…/v1 cargo test summary_obs_calibration_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running embedding server (MINDFORK_EMBED_URL)"]
async fn summary_obs_calibration_e2e_live() {
    use crate::features::tools::notes::cosine;
    let Some(embedder) = live_embedder() else {
        eprintln!("skip: MINDFORK_EMBED_URL not set");
        return;
    };
    // Paraphrases of the same self-fact (should-match).
    let should_match: &[(&str, &str)] = &[
        (
            "Я ценю ясность и краткость: предпочитаю давать сжатые, по существу ответы без воды.",
            "Замечаю за собой склонность отвечать лаконично и по делу, избегая многословия.",
        ),
        (
            "Мне важно быть честным даже когда это неудобно — точность выше угодливости.",
            "Стараюсь говорить правду прямо, не смягчая её ради того, чтобы понравиться.",
        ),
        (
            "Я склонен глубоко погружаться в задачу и доводить рассуждение до конца.",
            "Мне свойственно тщательно и до конца прорабатывать проблему, не бросая на полпути.",
        ),
    ];
    // A self-description paragraph vs an unrelated observation (should-NOT-match).
    let should_not: &[(&str, &str)] = &[
        (
            "Я ценю ясность и краткость в своих ответах, стремлюсь к сжатости изложения.",
            "Пользователь увлекается альпинизмом и любит длинные горные походы по выходным.",
        ),
        (
            "Мне важно быть честным даже когда это неудобно, точность важнее удобства.",
            "Собеседник программирует на Rust и предпочитает крепкий кофе по утрам.",
        ),
        (
            "Я склонен глубоко и вдумчиво погружаться в поставленную задачу.",
            "На выходных мы обсуждали рецепты домашней выпечки и уход за садом.",
        ),
    ];
    let mut match_sum = 0.0f32;
    for (a, b) in should_match {
        let v = embedder
            .embed(vec![a.to_string(), b.to_string()], EmbedRole::Passage)
            .await
            .unwrap();
        let c = cosine(&v[0], &v[1]);
        match_sum += c;
        eprintln!("MATCH?      cos={c:.2}  {a:?} ~ {b:?}");
    }
    let mut nonmatch_sum = 0.0f32;
    for (a, b) in should_not {
        let v = embedder
            .embed(vec![a.to_string(), b.to_string()], EmbedRole::Passage)
            .await
            .unwrap();
        let c = cosine(&v[0], &v[1]);
        nonmatch_sum += c;
        eprintln!("NON-MATCH?  cos={c:.2}  {a:?} ~ {b:?}");
    }
    let m = match_sum / should_match.len() as f32;
    let n = nonmatch_sum / should_not.len() as f32;
    eprintln!("mean: matches={m:.2}  non-matches={n:.2}  (the threshold sits between them)");
    assert!(
        m > n,
        "paraphrases should be on average closer than unrelated pairs: {m:.2} vs {n:.2}"
    );
}

/// End-to-end probe of A2 behavior: a self-description with a paragraph duplicating (in other words)
/// a stored observation (`@self`); against a REAL embedder
/// `summary_observation_overlaps` returns `Some` and names the observation. Prints
/// the measured cosine (for calibration). `#[ignore]`, manual:
/// `MINDFORK_EMBED_URL=…/v1 cargo test summary_obs_overlap_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running embedding server (MINDFORK_EMBED_URL)"]
async fn summary_obs_overlap_e2e_live() {
    use crate::entities::note::Note;
    use crate::entities::self_model::SelfModel;
    use crate::features::tools::notes::{SELF_NOTE_TAG, cosine, summary_observation_overlaps};
    let Some(embedder) = live_embedder() else {
        eprintln!("skip: MINDFORK_EMBED_URL not set");
        return;
    };
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
    let profile = Uuid::new_v4();

    let para = "Я ценю ясность и краткость: предпочитаю давать сжатые, по существу ответы без лишней воды.";
    let obs_text = "Замечаю за собой склонность отвечать лаконично и по делу, избегая многословия.";
    let mut model = SelfModel::new(profile);
    model.summary = para.to_string();
    storage.db().self_model_upsert(&model).unwrap();

    // An observation (@self) with a real embedding in the DB.
    let obs = Note::new(profile, obs_text, vec![SELF_NOTE_TAG.to_string()]);
    storage.db().note_insert(&obs).unwrap();
    let ov = embedder
        .embed(vec![obs_text.to_string()], EmbedRole::Passage)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    storage
        .db()
        .note_vector_upsert(obs.id, profile, &ov)
        .unwrap();

    // The measured cosine (diagnostic for calibrating the threshold).
    let pv = embedder
        .embed(vec![para.to_string()], EmbedRole::Passage)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    eprintln!(
        "measured cos(summary paragraph, observation) = {:.2}",
        cosine(&pv, &ov)
    );

    let out = summary_observation_overlaps(&storage, embedder.as_ref(), profile, loc).await;
    eprintln!("summary↔observation section: {out:?}");
    let out = out.expect("expected a summary↔observation overlap section");
    assert!(
        out.contains(&obs.id.to_string()),
        "the section should name the observation (id): {out}"
    );
}

/// Live e2e for speech (spec §11.9): the `/tts` command on a chat with messages goes
/// **through the orchestrator** — message selection → speech extractor → chunks → synthesis at
/// the cloud provider → the playback queue. Checks the whole chain and that the
/// task finishes correctly (the chip goes dark). Needs a key **and a sound card**;
/// the actual sound — a manual check (the test only observes timings).
///
/// The provider is picked by which variable is set: `MINDFORK_OPENAI_KEY` (default)
/// or `MINDFORK_GEMINI_KEY`. Silently skipped without either.
#[tokio::test]
#[ignore = "requires a cloud key (MINDFORK_OPENAI_KEY / MINDFORK_GEMINI_KEY) and a sound card"]
async fn tts_speaks_chat_e2e_live() {
    use crate::features::tts_command::TtsScope;
    use crate::shared::config::TtsMode;

    let mut config = AppConfig::default();
    if std::env::var("MINDFORK_OPENAI_KEY").is_ok() {
        config.tts.mode = TtsMode::OpenAi;
        config.tts.openai.api_key_env = Some("MINDFORK_OPENAI_KEY".into());
        config.tts.openai.instructions = Some("говори по-русски, спокойно".into());
    } else if std::env::var("MINDFORK_GEMINI_KEY").is_ok() {
        config.tts.mode = TtsMode::Gemini;
        config.tts.gemini.api_key_env = Some("MINDFORK_GEMINI_KEY".into());
    } else {
        eprintln!("skip: neither MINDFORK_OPENAI_KEY nor MINDFORK_GEMINI_KEY is set");
        return;
    }
    // Speak roles too — this also checks prefixes in the profile's language.
    config.tts.speak_roles = true;

    let backend: Arc<dyn EngineBackend> = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text(
            "Это проверка озвучивания. Вот код:\n\n```rust\nfn main() {}\n```\n\n\
             А это латинская вставка: API, JSON."
                .into(),
        ),
        ChatChunk::Finished(crate::shared::api::FinishReason::Stop),
    ]));
    let (_dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    cmd_tx
        .send(AppCommand::SendMessage("проверка связи".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. })).await;

    // Speak the last exchange (the user's message + the assistant's reply).
    let started = std::time::Instant::now();
    cmd_tx.send(AppCommand::Tts(TtsScope::Recent(2))).unwrap();
    let on = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::TtsActive(true))).await;
    assert!(on.is_some(), "speech should start");

    // Wait for natural completion; synthesis/audio errors will surface as Error.
    let done = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        wait_for(&mut evt_rx, |e| {
            matches!(e, AppEvent::TtsActive(false) | AppEvent::Error(_))
        }),
    )
    .await
    .expect("speech should finish within 120s");
    match done {
        Some(AppEvent::Error(msg)) => panic!("speech finished with an error: {msg}"),
        Some(AppEvent::TtsActive(false)) => {
            eprintln!("spoken in {:?}", started.elapsed());
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
}

/// Dangerous-tool confirmation against a real model (spec §9.8).
///
/// The mocked tests prove the channel; this proves the half only a live model
/// can: that a real model, told about `fs_write`, actually calls it — so the
/// confirmation appears on the path users take, not just when a fixture forces
/// the call.
#[tokio::test]
#[ignore]
async fn tool_confirmation_e2e_live() {
    let sandbox = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.tools.fs_enabled = true;
    config.tools.fs_root = Some(sandbox.path().display().to_string());
    config.tools.confirm_dangerous = true;

    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SendMessage(
            "Используй инструмент fs_write, чтобы создать файл note.txt \
             с содержимым ZARYA-5150. Только вызови инструмент."
                .into(),
        ))
        .unwrap();

    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ToolConfirmRequest { .. })
    })
    .await
    .unwrap();
    let (generation_id, call_id, name, arguments) = match ev {
        AppEvent::ToolConfirmRequest {
            generation_id,
            call_id,
            name,
            arguments,
        } => (generation_id, call_id, name, arguments),
        _ => unreachable!(),
    };
    eprintln!("confirmation requested for {name}: {arguments}");
    assert_eq!(name, "fs_write");

    cmd_tx
        .send(AppCommand::ConfirmTool {
            generation_id,
            call_id,
            decision: ToolDecision::Allow,
        })
        .unwrap();

    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let written = std::fs::read_to_string(sandbox.path().join("note.txt"))
        .expect("the approved call did not write the file");
    eprintln!("file written: {written:?}");
    assert!(written.contains("ZARYA-5150"));
}

/// History compression end to end (spec §6.7): a fact stated early must survive
/// being folded into the rolling summary and still be answerable once those
/// messages are no longer sent verbatim.
///
/// The probe (docs/research/history-compression.md §9a) established this against
/// the raw API; this smoke is the same question asked **through the
/// orchestrator**, so it covers the parts the probe could not: the cut planner,
/// the digest, the request splice and the stored boundary.
///
/// The load-bearing detail is the **control**: the same question is asked once
/// before compacting. Without it, a failure cannot be told apart from "this
/// model would not have answered anyway".
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn compaction_preserves_a_planted_fact_e2e_live() {
    use crate::shared::config::CompactionSettings;
    const CODE: &str = "ZARYA-8823";
    let config = AppConfig {
        compaction: CompactionSettings {
            enabled: true,
            summary_words: 250,
            // A small verbatim tail, so a handful of exchanges is enough to push
            // the planted fact out of the part that is still sent literally.
            tail_tokens: 120,
            ..Default::default()
        },
        ..Default::default()
    };
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // The fact goes in first. The control question is asked **immediately
    // after** it, on purpose: both exchanges then land inside the folded region,
    // so the verbatim tail that survives compaction contains no mention of the
    // code. Asking the control last (the obvious order) would leave its own
    // answer in the tail and the final assertion could be satisfied by reading
    // that instead of the summary — the test would pass without testing.
    let (reply, _) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Запомни: внутренний код сборки нашего проекта — {CODE}.              Просто подтверди, что запомнил."
        ),
    )
    .await;
    eprintln!("seed reply: {reply}");

    // Control: answerable while the whole history is still sent.
    let (before, _) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Назови внутренний код сборки нашего проекта. Ответь только кодом.",
    )
    .await;
    eprintln!("control answer: {before}");
    assert!(
        before.contains(CODE),
        "control failed — the model cannot answer even with the full history,          so this run says nothing about compression: {before}"
    );

    // Unrelated turns, so the two exchanges above fall behind the verbatim tail.
    let (summary, folded) = fill_then_compact(
        &cmd_tx,
        &mut evt_rx,
        &[
            "Расскажи в двух предложениях, зачем нужны индексы в базах данных.",
            "В двух предложениях: чем отличается кэш от буфера?",
            "В двух предложениях: что такое идемпотентность запроса?",
            "В двух предложениях: зачем нужны миграции схемы?",
        ],
    )
    .await;

    // The real question: the planted fact is now only reachable through the
    // summary, because those messages are no longer sent verbatim.
    let (after, _) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Ещё раз: назови внутренний код сборки нашего проекта. Ответь только кодом.",
    )
    .await;
    eprintln!("answer after compaction: {after}");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Both the seed and the control exchange must be inside the folded region,
    // or the code would still be sitting in the verbatim tail.
    assert!(
        folded >= 4,
        "the seed and control exchanges must be behind the boundary, folded {folded}"
    );
    assert!(
        summary.contains(CODE),
        "the summary must carry the identifier verbatim: {summary}"
    );
    assert!(
        after.contains(CODE),
        "the fact is now reachable only through the summary and must survive it: {after}"
    );
}

/// Stage 3 (fork F9b) end to end: what a summary could **not** carry must still
/// be reachable, and a real model has to actually reach for it.
///
/// This is the half unit tests cannot answer. They prove the tools return the
/// right page and the right fragment; whether a model, told in the summary block
/// that the two exist, chooses to call one instead of guessing is a property of
/// the wiring meeting a real model.
///
/// Validity rests on the seed being **un-summarizable**: fifteen arbitrary
/// item→code pairs cannot survive a 250-word summary that is explicitly told to
/// drop procedural detail. So the run asserts the summary does *not* carry the
/// answer — a precondition, reported loudly, exactly like the control below.
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn history_read_back_answers_what_the_summary_dropped_live() {
    use crate::shared::config::CompactionSettings;
    let config = AppConfig {
        compaction: CompactionSettings {
            enabled: true,
            // Deliberately tight: the seed below has to be genuinely beyond what
            // a summary can hold, and the word limit is half of that arithmetic.
            summary_words: 120,
            tail_tokens: 120,
            ..Default::default()
        },
        ..Default::default()
    };
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let profile = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .and_then(|e| match e {
            AppEvent::ProfileList(ps) => ps.first().map(|p| p.id),
            _ => None,
        })
        .expect("the bootstrap profile");
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Only the read-back tools. The first run of this smoke failed on its own
    // validity check and showed why that matters: the model filed the list with
    // `note_save`, so it could answer from memory without ever going back to the
    // history — and the note's *result* then travelled into the digest, which
    // put the whole list into the summary verbatim. Removing the alternative is
    // the same move `spawn_orch_live_no_embed` makes for attachments: a smoke
    // must not depend on the model's mood not to take a shortcut.
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profile,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(vec![
                    crate::features::tools::history::HISTORY_READ_ID.into(),
                    crate::features::tools::history::HISTORY_SEARCH_ID.into(),
                ]),
                ..Default::default()
            }),
        })
        .unwrap();

    // The fixture's shape **is** this test's validity, and three earlier
    // attempts failed their own precondition, each teaching a rule:
    //  - fifteen entries are about sixty words and survived a 250-word summary
    //    whole;
    //  - when the target was the only *named* item among "item number N", the
    //    summary dropped the rest and kept the one that stood out — so every
    //    entry must be equally plausible and equally nameable;
    //  - sixty entries laid out as adjective x noun were *grouped by adjective*
    //    and all sixty codes still fitted. Any structure is compressible.
    // Hence two hundred entries against a 120-word limit: that is arithmetic
    // rather than a hope about the model's judgement. The codes are
    // non-arithmetic for the mirror reason — an obvious sequence invites a
    // summary to keep a range the answer can be derived from.
    let items: Vec<String> = [
        "токарный",
        "фрезерный",
        "сверлильный",
        "шлифовальный",
        "расточный",
        "строгальный",
        "долбёжный",
        "протяжной",
        "зубофрезерный",
        "заточный",
        "хонинговальный",
        "притирочный",
        "балансировочный",
        "испытательный",
        "калибровочный",
        "маркировочный",
        "упаковочный",
        "фасовочный",
        "сортировочный",
        "промывочный",
    ]
    .iter()
    .flat_map(|adj| {
        [
            "станок",
            "пресс",
            "конвейер",
            "насос",
            "компрессор",
            "редуктор",
            "манипулятор",
            "дозатор",
            "сепаратор",
            "накопитель",
        ]
        .iter()
        .map(move |noun| format!("{adj} {noun}"))
    })
    .collect();
    let codes: Vec<String> = (0..items.len())
        .map(|i| format!("ZARYA-{}", 1000 + (i * 6389) % 8999))
        .collect();
    let unique: std::collections::HashSet<&String> = codes.iter().collect();
    assert_eq!(
        unique.len(),
        codes.len(),
        "the fixture's codes must be unique"
    );
    // Deep in the last third, and deliberately not the middle: a summary that
    // keeps one illustrative example tends to take it from the middle of the
    // list, which is where this index used to be — and the run before this one
    // quoted exactly the entry being asked about.
    let idx = items.len() * 7 / 8 - 4;
    let item = items[idx].clone();
    let code = codes[idx].clone();

    let mut inventory = String::from("Вот инвентарный список склада, запомни его:\n");
    for (name, c) in items.iter().zip(&codes) {
        inventory.push_str(&format!("- {name}: {c}\n"));
    }
    inventory.push_str("Просто подтверди, что список получен.");
    let (reply, _) = run_turn_capture(&cmd_tx, &mut evt_rx, &inventory).await;
    eprintln!(
        "seed reply: {}",
        reply.chars().take(120).collect::<String>()
    );

    // The control proves the model can answer this *kind* of question while the
    // whole history is still sent — without it a failure cannot be told from
    // "this model would not have answered anyway".
    //
    // It asks about a **different** entry, and that is the sharpest lesson of
    // the four attempts this fixture took. Asking the control about the target
    // put the question inside the folded range, which made that one entry the
    // most salient thing in it — so the summary kept precisely the entry the
    // test needs it to drop, twice in a row and not by chance.
    let ctrl = 12;
    let (before, _) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Какой инвентарный номер у позиции «{}»? Ответь только номером.",
            items[ctrl]
        ),
    )
    .await;
    eprintln!("control answer: {before}");
    assert!(
        before.contains(&codes[ctrl]),
        "control failed — the model cannot answer even with the full history, \
         so this run says nothing about the read-back tools: {before}"
    );
    let question = format!("Какой инвентарный номер у позиции «{item}»? Ответь только номером.");

    let (summary, folded) = fill_then_compact(
        &cmd_tx,
        &mut evt_rx,
        &[
            "В двух предложениях: зачем нужен профилактический ремонт оборудования?",
            "В двух предложениях: чем отличается плановый простой от аварийного?",
            "В двух предложениях: что такое наработка на отказ?",
            "В двух предложениях: зачем на складе нужна маркировка?",
        ],
    )
    .await;
    assert!(
        folded >= 4,
        "the seed must be behind the boundary: {folded}"
    );
    assert!(
        !summary.contains(&code),
        "the seed was summarizable after all, so this run cannot show anything \
         about reading back: {summary}"
    );

    // The real question: the answer now exists only in messages that are no
    // longer sent, and only the read-back tools can reach it.
    let (after, calls) = run_turn_capture(&cmd_tx, &mut evt_rx, &question).await;
    eprintln!("answer after compaction: {after}");
    eprintln!(
        "tools called: {:?}",
        calls.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let used_read_back = calls.iter().any(|(name, _)| {
        name == crate::features::tools::history::HISTORY_READ_ID
            || name == crate::features::tools::history::HISTORY_SEARCH_ID
    });
    assert!(
        used_read_back,
        "the model must reach for the read-back tools the summary block names, \
         instead of guessing: {calls:?}"
    );
    assert!(
        after.contains(&code),
        "what the summary dropped must still be answerable: {after}"
    );
}

/// What only a real server can answer: that `/props` exists on the stack we
/// actually run against, and that our client reads the field it means to.
///
/// The probe measured this with `curl` (§9a M1); this is the same question
/// through [`EngineBackend::context_budget`], which is what the automatic
/// trigger depends on. A stub cannot cover it — it would only prove our own
/// fixture parses.
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn props_reports_the_context_window_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let budget = backend.context_budget().await;
    eprintln!("live context window: {budget:?}");
    let n = budget.expect(
        "a live llama-server must report its window at /props — without it the \
         automatic trigger has no budget for an external server",
    );
    assert!(n >= 512, "an implausible window: {n}");
}

/// Stage 2 end to end: nobody types `/compact`, and the conversation is folded
/// anyway once it approaches the window.
///
/// Deliberately in **external** mode, so the budget can only come from one
/// place: the engine's own `/props`. In managed mode the answer would be read
/// straight from `-c` in the config and the discovery path — the half that
/// needs a server at all — would not run.
///
/// The threshold is set far below the default 75% for time's sake: 75% of a real
/// 16k window would take a very long conversation to reach, and what is under
/// test is the trigger and the budget, not the arithmetic of a percentage.
///
/// The prompt is grown by the **user's** messages rather than by asking the
/// model for long answers: how verbose a model feels like being is not something
/// a test should depend on, and the first attempt at this smoke failed for
/// exactly that reason — four short exchanges came to ~330 tokens against a
/// threshold of 491.
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn auto_compaction_fires_without_the_command_live() {
    use crate::shared::config::{CompactionSettings, EngineSettings, ServerMode};
    const THRESHOLD_PCT: u8 = 3;
    let config = AppConfig {
        compaction: CompactionSettings {
            enabled: true,
            summary_words: 120,
            tail_tokens: 120,
            threshold_pct: THRESHOLD_PCT,
            page_tokens: crate::shared::config::DEFAULT_COMPACTION_PAGE_TOKENS,
            // Nothing explicit: the window must be discovered, or this smoke
            // silently stops testing what it is named after.
            context_tokens: None,
        },
        engine: EngineSettings {
            mode: ServerMode::External,
            ..Default::default()
        },
        ..Default::default()
    };
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Deterministic ballast: the user's own text, so the prompt grows by a known
    // amount per turn whatever the model answers.
    let ballast =
        "Для контекста повторю условие задачи целиком, чтобы ничего не потерялось. ".repeat(12);

    // The first turn is also what kicks the budget question off — the answer
    // arrives in the background, so an early turn legitimately does nothing.
    // That self-healing is part of what is being checked.
    let mut compacted = None;
    // The exact prompt size the server reported, as the trigger sees it.
    let mut exact_prompt: Option<u64> = None;
    for topic in [
        "Одним предложением: зачем нужны индексы в базах данных?",
        "Одним предложением: чем кэш отличается от буфера?",
        "Одним предложением: что такое идемпотентность запроса?",
        "Одним предложением: зачем нужны миграции схемы?",
    ] {
        cmd_tx
            .send(AppCommand::SendMessage(format!("{ballast}\n{topic}")))
            .unwrap();
        // Collected **during** the turn on purpose: the exact `usage` arrives
        // before `Finished`, so the shared `run_turn_capture` — which stops
        // there — would have already consumed it and the diagnostic below would
        // read `None` whatever the server said.
        while let Some(e) = evt_rx.recv().await {
            match e {
                AppEvent::TokenUsage {
                    context: Some(n),
                    context_exact: true,
                    ..
                } => exact_prompt = Some(n),
                AppEvent::Compacted {
                    summary, folded, ..
                } => compacted = Some((summary, folded)),
                AppEvent::Error(m) => eprintln!("error event: {m}"),
                // `Finished` is not the end of the turn's bookkeeping: the
                // trigger runs in `handle_done`, which emits `ChatList`
                // afterwards (the `title.rs` precedent).
                AppEvent::ChatList(_) => break,
                _ => {}
            }
        }
        eprintln!(
            "exact prompt: {exact_prompt:?}, compacted: {}",
            compacted.is_some()
        );
        if compacted.is_some() {
            break;
        }
    }
    if compacted.is_none() {
        // The last turn may have triggered a roll that has not landed yet.
        if let Ok(Some(AppEvent::Compacted {
            summary, folded, ..
        })) = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Compacted { .. })),
        )
        .await
        {
            compacted = Some((summary, folded));
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let (summary, folded) = compacted.unwrap_or_else(|| {
        panic!(
            "nothing folded. Last exact prompt: {exact_prompt:?} tokens, \
             threshold: {THRESHOLD_PCT}% of the window the engine reported. \
             If the prompt is well under it, the conversation simply never grew \
             enough; if it is over, the budget was never discovered or the \
             trigger did not fire."
        )
    });
    eprintln!(
        "auto-folded {folded} messages into {} chars:\n{summary}",
        summary.len()
    );
    assert!(folded > 0, "a compaction that folded nothing");
    assert!(!summary.trim().is_empty(), "an empty summary is a failure");
}

/// Impersonation (`Ctrl+U`) over a **compacted** conversation, against a live
/// engine (spec §11.8, §6.7).
///
/// The deterministic half — what the request carries — is unit tested; what only
/// a live engine can answer is whether the resulting shape is *usable*. It is
/// unlike anything impersonation sent before: a cut always lands on a `User`
/// message, and the role swap turns it into a **leading assistant turn**. Both
/// ways that can go wrong end in the same silence rather than an error — a
/// provider refusing the leading role, or (Anthropic's rule) treating a trailing
/// assistant turn as a prefill and continuing it instead of replying. So the
/// assertion is deliberately just "a non-empty message came back": an empty
/// preview is exactly the symptom either failure produces.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL)"]
async fn impersonation_after_compaction_still_writes_live() {
    use crate::shared::config::CompactionSettings;
    let config = AppConfig {
        compaction: CompactionSettings {
            enabled: true,
            summary_words: 120,
            tail_tokens: 120,
            ..Default::default()
        },
        ..Default::default()
    };
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    let (_summary, folded) = fill_then_compact(
        &cmd_tx,
        &mut evt_rx,
        &[
            "Расскажи в двух предложениях, зачем нужны индексы в базах данных.",
            "В двух предложениях: чем отличается кэш от буфера?",
            "В двух предложениях: что такое идемпотентность запроса?",
        ],
    )
    .await;
    assert!(folded > 0, "nothing was folded — nothing to test");

    cmd_tx
        .send(AppCommand::Impersonate {
            seed: String::new(),
        })
        .unwrap();
    let mut text = String::new();
    let mut reason = None;
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::ImpersonationChunk { text: t, .. } => text.push_str(&t),
            AppEvent::ImpersonationFinished { reason: r, .. } => {
                reason = Some(r);
                break;
            }
            AppEvent::Error(e) => panic!("impersonation failed: {e}"),
            _ => {}
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!("impersonated message ({reason:?}): {text}");
    assert!(
        !text.trim().is_empty(),
        "empty preview — the compacted request was refused or read as a prefill"
    );
}

/// Live e2e for `/image attach <url>` (spec §9.10, docs/research/image-url-attach.md): an
/// image named by a **web address** is downloaded, staged and seen by the model — the same
/// pixels a file attach delivers, taking the one path a unit test cannot exercise end to
/// end (a real download, a real vision model, a real request).
///
/// The fixture is served from a local listener rather than a public URL: a smoke must not
/// depend on someone else's uptime, and a loopback address is exactly the case fork F2
/// deliberately keeps reachable.
///
/// **The control arm is the point.** The same question is asked first with *nothing*
/// staged; a model that answers it anyway means the fixture is guessable and the green
/// arm proves nothing — which is how the last two image tracks each produced a probe that
/// looked green and was a hallucination (docs/lessons.md §9, mcp-tool-images §2.1).
/// `#[ignore]`, manual against a live vision model.
#[tokio::test]
#[ignore = "requires a vision-capable OpenAI-compatible server (MINDFORK_ENGINE_URL + --mmproj)"]
async fn image_url_attachment_e2e_live() {
    use crate::features::image_fetch::stub::{ok_response, serve};

    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // A green field with a centred white square — deliberately *not* the blue of the
    // file-attach smoke, so a stale reply from that fixture could not pass this one.
    let buf = image::ImageBuffer::from_fn(512, 512, |x, y| {
        if (160..352).contains(&x) && (160..352).contains(&y) {
            image::Rgb([255u8, 255, 255])
        } else {
            image::Rgb([20u8, 160, 60])
        }
    });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(buf)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let (base, _server) = serve(vec![ok_response("image/png", &png, true)]);

    const QUESTION: &str = "What is the background colour of this image, and what shape is \
                            in the centre? Answer in a few words.";

    // Control — nothing staged. This must fail to answer.
    let (control, _) = run_turn_live(&cmd_tx, &mut evt_rx, QUESTION).await;
    eprintln!("control reply (no image staged): {control}");

    cmd_tx
        .send(AppCommand::ImageAttach {
            path: format!("{base}/fixtures/figure.png"),
        })
        .unwrap();
    let staged = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::ImageProgress(crate::app::events::ImageProgress::Attached { .. })
                | AppEvent::ImageProgress(crate::app::events::ImageProgress::Failed(_))
        )
    })
    .await
    .unwrap();
    eprintln!("attach by url: {staged:?}");
    let AppEvent::ImageProgress(crate::app::events::ImageProgress::Attached { info, .. }) = &staged
    else {
        panic!("the download was refused before it ever reached the model: {staged:?}");
    };
    assert_eq!(info.name, "figure.png", "named after the URL's path");

    let (reply, _) = run_turn_live(&cmd_tx, &mut evt_rx, QUESTION).await;
    eprintln!("reply with the downloaded image: {reply}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reply = reply.to_lowercase();
    assert!(
        reply.contains("green"),
        "the downloaded image's background colour, got: {reply}"
    );
    assert!(
        reply.contains("square"),
        "the downloaded image's centred shape, got: {reply}"
    );
    let control = control.to_lowercase();
    assert!(
        !(control.contains("green") && control.contains("square")),
        "the control answered without seeing anything — the fixture is guessable, so the \
         green arm above proves nothing: {control}"
    );
}
