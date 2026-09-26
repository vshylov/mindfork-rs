//! Orchestrator tests — live #[ignore] e2e smokes (Gemma/bge-m3 via MINDFORK_*_URL). Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;
use crate::features::tools::confirm::ToolDecision;
use crate::shared::api::EmbedRole;

/// Does the reply carry the planted code — whichever dash the model felt like
/// typing?
///
/// Every smoke that plants a fact plants it as `ZARYA-8823`, and the assertion
/// used to be a plain `contains`. On `gpt-oss-120b` that turned three green
/// smokes red while the model was answering *correctly*: it renders the code
/// with a **non-breaking hyphen** (U+2011). Not always, either — 8 of 18
/// occurrences in one run, the same model producing both glyphs, so these were
/// latent flakes on any model rather than a property of this one.
///
/// The fact under test is the code; which dash glyph a model chose to render it
/// with is typography. So both sides are compared with the Unicode dashes folded
/// to ASCII `-`. Nothing else is normalized: case, spacing and the digits stay
/// exactly as strict as they were.
fn mentions_code(haystack: &str, code: &str) -> bool {
    fn fold_dashes(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                // The hyphen/dash block (U+2010 hyphen … U+2015 horizontal bar),
                // the minus sign, and the compatibility forms.
                '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{FE58}' | '\u{FE63}' | '\u{FF0D}' => '-',
                other => other,
            })
            .collect()
    }
    fold_dashes(haystack).contains(&fold_dashes(code))
}

/// Not ceremony: this helper is the only thing standing between eight live
/// assertions and a typographic hyphen, and "why is this not just `contains`?"
/// is a question a future reader will ask. The answer is a test.
#[test]
fn a_planted_code_survives_the_dash_a_model_chose() {
    assert!(mentions_code("the code is ZARYA-8823.", "ZARYA-8823"));
    // U+2011, measured on gpt-oss-120b; U+2013 and U+2212 are the neighbours a
    // different model would reach for.
    assert!(mentions_code("**ZARYA\u{2011}8823**", "ZARYA-8823"));
    assert!(mentions_code("ZARYA\u{2013}8823", "ZARYA-8823"));
    assert!(mentions_code("ZARYA\u{2212}8823", "ZARYA-8823"));
    // Still strict about everything that is not a dash.
    assert!(!mentions_code("ZARYA 8823", "ZARYA-8823"));
    assert!(!mentions_code("zarya-8823", "ZARYA-8823"));
    assert!(!mentions_code("ZARYA-8824", "ZARYA-8823"));
}

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

/// Waits out the bootstrap (profile list + first chat activation) and narrows
/// the default profile to exactly `tools` — the "remove the alternative" rule
/// the tool smokes share: a smoke must not depend on the model's mood not to
/// take a shortcut. Returns `(profile id, the bootstrap chat's id)`.
async fn narrow_profile_to(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    tools: Vec<crate::entities::profile::ToolId>,
) -> (Uuid, Uuid) {
    let profile = wait_for(evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .and_then(|e| match e {
            AppEvent::ProfileList(ps) => ps.first().map(|p| p.id),
            _ => None,
        })
        .expect("the bootstrap profile");
    let chat = wait_for(evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .and_then(|e| match e {
            AppEvent::ChatActivated { id, .. } => Some(id),
            _ => None,
        })
        .expect("the bootstrap chat");
    set_profile_tools(cmd_tx, profile, tools);
    (profile, chat)
}

/// Change a profile's tool set **mid-conversation**, for a smoke whose second
/// turn needs a different set from its first.
///
/// Split out of [`narrow_profile_to`], which can only run once: it waits for the
/// bootstrap's `ProfileList` and `ChatActivated`, and those do not come round
/// again. Ordering is safe without an acknowledgement — this command and the
/// next turn's `SendMessage` travel the same channel, so the orchestrator
/// applies them in that order.
fn set_profile_tools(
    cmd_tx: &UnboundedSender<AppCommand>,
    profile: Uuid,
    tools: Vec<crate::entities::profile::ToolId>,
) {
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profile,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(tools),
                ..Default::default()
            }),
        })
        .unwrap();
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
        mentions_code(&answer, CODE),
        "the code sits deep in the file and must be found: {answer}"
    );
}

/// A page a tool attached, searched in the turn it was born and in the next one
/// (docs/research/attachment-birth-turn.md). The transcript behind it: `fetch_url`
/// offered `attachment_search`, the search answered "no index was built", and the
/// model sampled 20 of 72 pages at random. Now the result offers only the page route
/// and names the dead one, a search the model tries anyway names the file as attached
/// in this turn, and — the promise those texts make — after the turn lands the file is
/// indexed and a question about another part of it is answered by meaning.
///
/// The page is the project's own `spec.md`, fetched for real, and the fact asked for
/// sits in §17.5 — past the old 400 000-character ceiling, so the first turn also
/// shows the whole document arrived. The second turn asks about §9.3.1 instead, a part
/// the first turn does not read: asked the same question again, the model answered
/// from its own previous reply and never searched (seen on the third run). The
/// profile carries the three tools the story needs and nothing else ("remove the
/// alternative", lessons §9).
/// Needs `MINDFORK_ENGINE_URL`, `MINDFORK_EMBED_URL` and network access.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL), embedder (MINDFORK_EMBED_URL) and network access"]
async fn fetched_page_search_across_its_birth_turn_e2e_live() {
    use crate::app::events::FileProgress;
    use crate::features::tools::attachment::{ATTACHMENT_READ_ID, ATTACHMENT_SEARCH_ID};
    use crate::shared::i18n::{Lang, locale};
    const URL: &str = "https://raw.githubusercontent.com/vshylov/mindfork-rs/main/spec.md";
    const QUESTION: &str = "какое значение по умолчанию у параметра prompt_cap модели себя \
         (глава 17 спецификации) и с какого значения его подняли? Назови оба числа.";
    if std::env::var("MINDFORK_EMBED_URL").is_err() {
        eprintln!("skip: MINDFORK_EMBED_URL not set (the second turn needs the index)");
        return;
    }
    let mut config = AppConfig::default();
    config.tools.web_enabled = true;
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        vec![
            crate::features::tools::FETCH_URL_ID.into(),
            ATTACHMENT_READ_ID.into(),
            ATTACHMENT_SEARCH_ID.into(),
        ],
    )
    .await;

    // Both bundles, so the check does not depend on the profile's scaffold language.
    let no_search: Vec<&str> = Lang::ALL
        .iter()
        .map(|&l| locale(l).t("tool.attachment.no_search_this_turn"))
        .collect();
    let born_reason: Vec<String> = Lang::ALL
        .iter()
        .map(|&l| {
            let t = locale(l).t("tool.attachment_search.unindexed.born");
            t.split_once("): ").unwrap().1.to_string()
        })
        .collect();
    let hits_header: Vec<String> = Lang::ALL
        .iter()
        .map(|&l| {
            let t = locale(l).t("tool.attachment_search.result.header");
            t.split_once("{n}").unwrap().0.to_string()
        })
        .collect();

    // Turn 1 — the birth turn.
    let (reply1, calls1) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        &format!("Загрузи спецификацию {URL} и найди в ней, {QUESTION}"),
    )
    .await;
    for (name, result) in &calls1 {
        eprintln!(
            "turn 1 call {name} -> {}",
            result.chars().take(400).collect::<String>()
        );
    }
    eprintln!("turn 1 reply: {reply1}");
    let fetch = calls1
        .iter()
        .find(|(n, _)| n == crate::features::tools::FETCH_URL_ID)
        .expect("the model never fetched the page, so nothing was tested");
    let mut stripped = fetch.1.clone();
    for s in &no_search {
        stripped = stripped.replace(s, "");
    }
    assert!(
        !stripped.contains("attachment_search"),
        "the birth-turn result offers search as a route: {}",
        fetch.1
    );
    assert!(
        no_search.iter().any(|s| fetch.1.contains(s)),
        "the dead route is not named: {}",
        fetch.1
    );
    for (_, result) in calls1.iter().filter(|(n, _)| n == ATTACHMENT_SEARCH_ID) {
        assert!(
            born_reason.iter().any(|b| result.contains(b.as_str())),
            "a birth-turn search must name the file as attached in this turn: {result}"
        );
    }
    let searched1 = calls1
        .iter()
        .filter(|(n, _)| n == ATTACHMENT_SEARCH_ID)
        .count();
    let read1 = calls1
        .iter()
        .filter(|(n, _)| n == ATTACHMENT_READ_ID)
        .count();
    eprintln!("turn 1: {searched1} search call(s), {read1} page read(s)");

    // The promise: after the turn lands, the file is indexed.
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
        "the page was not indexed after its turn: {indexed:?}"
    );

    // Turn 2 — the same question, now answerable by meaning.
    let (reply2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Теперь поищи в приложенной спецификации по смыслу: до скольких символов          обрезается имя вложения, которое fetch_url берёт из названия страницы?",
    )
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;
    for (name, result) in &calls2 {
        eprintln!(
            "turn 2 call {name} -> {}",
            result.chars().take(400).collect::<String>()
        );
    }
    eprintln!("turn 2 reply: {reply2}");
    let searches2: Vec<_> = calls2
        .iter()
        .filter(|(n, _)| n == ATTACHMENT_SEARCH_ID)
        .collect();
    assert!(
        !searches2.is_empty(),
        "turn 2 never searched: {:?}",
        calls2.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert!(
        searches2
            .iter()
            .any(|(_, r)| hits_header.iter().any(|h| r.starts_with(h.as_str()))),
        "the search after the turn landed still found nothing: {searches2:?}"
    );
    assert!(
        reply2.contains("60"),
        "§9.3.1 clips the name to 60 characters: {reply2}"
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
        mentions_code(&answer, CODE),
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
        mentions_code(&answer, CODE),
        "the model must answer from the attached file, got: {answer}"
    );
    assert!(
        !mentions_code(&baseline, CODE),
        "the baseline must not know the invented code (otherwise the test proves nothing): {baseline}"
    );
}

/// The image fixture both vision smokes use: a solid field of `field` with a large white
/// square in the middle. Generated rather than photographed, so the assertion is objective
/// and no pretrained knowledge can answer it — and parameterized by colour, so the two
/// smokes cannot pass on each other's reply.
fn figure_png(field: [u8; 3]) -> Vec<u8> {
    figure_png_with_corner(field, None)
}

/// [`figure_png`] with, optionally, a small square of `corner` in the top-left — a detail a
/// model that never saw the picture cannot know, when the conversation mentions only the
/// field and the white square.
fn figure_png_with_corner(field: [u8; 3], corner: Option<[u8; 3]>) -> Vec<u8> {
    let buf = image::ImageBuffer::from_fn(512, 512, |x, y| match corner {
        Some(c) if x < 96 && y < 96 => image::Rgb(c),
        _ if (160..352).contains(&x) && (160..352).contains(&y) => image::Rgb([255u8, 255, 255]),
        _ => image::Rgb(field),
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(buf)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// Stages `path` (a file path or a URL — the command takes either) and returns what was
/// staged. A refusal fails here rather than three turns later: an image that never reached
/// the model would otherwise read as a model that cannot see.
async fn attach_image_live(
    cmd_tx: &UnboundedSender<AppCommand>,
    rx: &mut UnboundedReceiver<AppEvent>,
    path: String,
) -> crate::entities::message_image::ImageInfo {
    cmd_tx.send(AppCommand::ImageAttach { path }).unwrap();
    let staged = wait_for(rx, |e| {
        matches!(
            e,
            AppEvent::ImageProgress(crate::app::events::ImageProgress::Attached { .. })
                | AppEvent::ImageProgress(crate::app::events::ImageProgress::Failed(_))
        )
    })
    .await
    .unwrap();
    eprintln!("attach: {staged:?}");
    match staged {
        AppEvent::ImageProgress(crate::app::events::ImageProgress::Attached { info, .. }) => info,
        other => panic!("the image was refused before it ever reached the model: {other:?}"),
    }
}

/// `/continue` through a gateway, on the app's own paths (fork H2 of
/// docs/history/gateway-images-and-continue.md): the endpoint's catalogue lands before any
/// turn, a reply cut by the length limit is announced as continuable exactly when
/// the route table says the model continues, and `/continue` then either refuses
/// with the gateway's note or resumes the reply **without** restarting it.
///
/// Declared, not guessed: `MINDFORK_LIVE_CONTINUE_EXPECT` names the stack —
/// `gateway-continues` for a model the table allows (measured:
/// `anthropic/claude-haiku-4.5`), `gateway-refuses` for one it refuses
/// (`google/gemma-4-31b-it`), and `local` for a llama.cpp, which publishes no
/// catalogue and must keep continuing exactly as before (the regression half). The
/// smoke fails rather than skips when the run disagrees. The restart criterion is
/// the one §1.3 measured: a cut this early leaves the question's own words ahead of
/// the answer, so a continuation carries no `capital` and a restart does.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL and MINDFORK_LIVE_CONTINUE_EXPECT (gateway-continues | gateway-refuses | local); a gateway also needs MINDFORK_ENGINE_KEY + MINDFORK_ENGINE_MODEL"]
async fn continue_through_a_gateway_live() {
    use crate::shared::config::ServerMode;
    use std::time::Duration;

    let Ok(declared) = std::env::var("MINDFORK_LIVE_CONTINUE_EXPECT") else {
        eprintln!("skip: MINDFORK_LIVE_CONTINUE_EXPECT not set");
        return;
    };
    let (catalogued, continues) = match declared.trim() {
        "gateway-continues" => (true, true),
        "gateway-refuses" => (true, false),
        "local" => (false, true),
        other => panic!("MINDFORK_LIVE_CONTINUE_EXPECT={other:?} is not a declared stack"),
    };
    let model = std::env::var("MINDFORK_ENGINE_MODEL").ok();
    let mut cfg = no_auto_cfg();
    cfg.engine.mode = ServerMode::External;
    cfg.engine.external.model_name = model.clone();
    let model = model.unwrap_or_else(|| "local".into());
    // "The capital of France" and no further, so the continuation has an answer
    // to carry. Measured at 6 the cut already held "Paris" and the continuation
    // was a lone ".", which a restart could not have produced but a broken one
    // could hardly be told from.
    cfg.default_sampling.max_tokens = Some(4);
    // Thinking off: a local Gemma 4 spent the whole cap reasoning, left no visible
    // text, and a thoughts-only fragment is rightly not continuable — the fixture,
    // not the feature. `reasoning_budget: 0` is llama.cpp's switch and a gateway
    // drops it, so it changes nothing on the routes above.
    cfg.default_sampling.reasoning_budget = Some(0);
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // The discovery lands before any turn either way; what it carries is the claim.
    let landed = tokio::time::timeout(
        Duration::from_secs(60),
        wait_for(&mut evt_rx, |e| {
            matches!(e, AppEvent::EngineSamplingFields(_))
        }),
    )
    .await
    .expect("the engine's facts must land before the first turn")
    .expect("the event stream");
    assert_eq!(
        matches!(landed, AppEvent::EngineSamplingFields(Some(_))),
        catalogued,
        "declared {declared:?}, the endpoint answered {landed:?}"
    );

    cmd_tx
        .send(AppCommand::SendMessage(
            "What is the capital of France? Answer in one short sentence.".into(),
        ))
        .unwrap();
    let mut partial = String::new();
    let (reason, continuable) = loop {
        match evt_rx.recv().await.expect("the turn's events") {
            AppEvent::Chunk { text, .. } => partial.push_str(&text),
            AppEvent::Finished {
                reason,
                continuable,
                ..
            } => break (reason, continuable),
            _ => {}
        }
    };
    eprintln!("[{model}] first turn ({reason:?}, continuable={continuable}): {partial:?}");
    assert_eq!(
        reason,
        FinishReason::Length,
        "the fixture needs a length cut"
    );
    assert_eq!(
        continuable, continues,
        "the note after a cut must promise /continue exactly where it works"
    );

    cmd_tx.send(AppCommand::ContinueLast).unwrap();
    let mut resumed = String::new();
    let mut note = None;
    loop {
        match evt_rx.recv().await.expect("the command's events") {
            AppEvent::Chunk { text, .. } => resumed.push_str(&text),
            AppEvent::Error(text) if !continues => {
                note = Some(text);
                break;
            }
            AppEvent::Finished { .. } if continues => break,
            _ => {}
        }
    }
    eprintln!("[{model}] /continue: note={note:?} resumed={resumed:?}");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    if continues {
        assert!(!resumed.is_empty(), "the continuation brought nothing");
        assert!(
            !resumed.to_lowercase().contains("capital"),
            "the reply restarted instead of continuing: {partial:?} + {resumed:?}"
        );
    } else {
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::default());
        assert_eq!(
            note.as_deref(),
            Some(loc.t("ui.cmd.continue_unsupported_gateway"))
        );
    }
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
    if crate::shared::api::live_text_only() {
        eprintln!("skip: MINDFORK_LIVE_TEXT_ONLY — this stack has no vision projector");
        return;
    }
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // A blue field with a centred white square — two facts to check, both objective.
    let path = dir.path().join("figure.png");
    std::fs::write(&path, figure_png([20, 60, 200])).unwrap();
    attach_image_live(&cmd_tx, &mut evt_rx, path.to_string_lossy().into_owned()).await;

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

/// Whether a gateway's model takes images now comes from its catalogue
/// (docs/research/gateway-vision-catalogue.md, V1(a)) — checked on the app's own paths,
/// through the retry decorator the application wraps the client in.
///
/// Declared, not guessed: `MINDFORK_LIVE_VISION_EXPECT` names the model behind
/// `MINDFORK_ENGINE_MODEL` — `unsupported` for one the catalogue lists as text-only,
/// `supported` for one that lists `image`. Text-only: `/image attach` is refused with the
/// message that names the way out, and with a provisioned `MINDFORK_SANDBOX_DIR` a
/// `python_exec` chart is withheld and said, and the turn completes — where, measured, the
/// chart turned the next round into the gateway's `404` (research §2.2). Vision: the image
/// attaches without the "this engine does not report" note, which would now be untrue.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_MODEL on a gateway with a catalogue, and MINDFORK_LIVE_VISION_EXPECT"]
async fn a_gateways_catalogue_decides_whether_images_are_sent_live() {
    use crate::app::events::ImageProgress;
    let Ok(expect) = std::env::var("MINDFORK_LIVE_VISION_EXPECT") else {
        eprintln!("skip: MINDFORK_LIVE_VISION_EXPECT not set");
        return;
    };
    let supported = match expect.as_str() {
        "supported" => true,
        "unsupported" => false,
        other => {
            panic!("MINDFORK_LIVE_VISION_EXPECT is `supported` or `unsupported`, not {other:?}")
        }
    };
    // As the application runs on a gateway: `external`, with the model named. The mode
    // also picks the refusal's wording — a managed server is told about its projector.
    let gateway = |cfg: &mut AppConfig| {
        cfg.engine.mode = crate::shared::config::ServerMode::External;
        cfg.engine.external.model_name = std::env::var("MINDFORK_ENGINE_MODEL").ok();
    };
    let mut cfg = no_auto_cfg();
    gateway(&mut cfg);
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let path = dir.path().join("figure.png");
    std::fs::write(&path, figure_png([20, 60, 200])).unwrap();
    cmd_tx
        .send(AppCommand::ImageAttach {
            path: path.to_string_lossy().into_owned(),
        })
        .unwrap();
    // The "could not say" note follows `Attached`, and the staged list follows both.
    let mut attached = None;
    let mut refused = None;
    let mut unknown_note = false;
    loop {
        match evt_rx.recv().await.expect("the attach's events") {
            AppEvent::ImageProgress(ImageProgress::Attached { info, .. }) => attached = Some(info),
            AppEvent::ImageProgress(ImageProgress::VisionUnknown) => unknown_note = true,
            AppEvent::ImageProgress(ImageProgress::Failed(msg)) => {
                refused = Some(msg);
                break;
            }
            AppEvent::StagedImages(_) if attached.is_some() => break,
            _ => {}
        }
    }
    eprintln!("[{expect}] attached={attached:?} refused={refused:?} unknown_note={unknown_note}");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    if supported {
        assert!(attached.is_some(), "a vision model's image must attach");
        assert!(
            !unknown_note,
            "the catalogue vouched for the model, so the caveat would be untrue"
        );
        return;
    }
    let told: Vec<String> = crate::shared::i18n::Lang::ALL
        .iter()
        .map(|l| {
            crate::shared::i18n::locale(*l)
                .t("ui.err.image_no_vision")
                .to_string()
        })
        .collect();
    assert!(
        refused
            .as_deref()
            .is_some_and(|m| told.iter().any(|t| m.contains(t.as_str()))),
        "a text-only model's attach must be refused with the way out: {refused:?}"
    );

    // The tool half: a chart the model's own code draws.
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_python_chat_with(gateway).await else {
        return;
    };
    cmd_tx
        .send(AppCommand::SendMessage(
            "Plot these monthly sales as a bar chart and save it as a PNG: Jan 120, Feb 95, \
             Mar 180, Apr 130, May 160. Then tell me in one sentence what the chart shows."
                .into(),
        ))
        .unwrap();
    let mut reply = String::new();
    let mut thoughts = String::new();
    let mut calls: Vec<(String, usize)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::Chunk { text, .. } => reply.push_str(&text),
            AppEvent::Thoughts { text, .. } => thoughts.push_str(&text),
            AppEvent::ToolCall {
                name,
                result,
                images,
                ..
            } if name == crate::features::tools::PYTHON_EXEC_ID => calls.push((result, images)),
            AppEvent::Error(e) => errors.push(e),
            AppEvent::Finished { reason, .. } => {
                eprintln!("finished: {reason:?}, thoughts: {thoughts}");
                break;
            }
            _ => {}
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    for (result, images) in &calls {
        eprintln!("python_exec → {images} image(s) sent:\n{result}");
    }
    eprintln!("reply: {reply}\nerrors: {errors:?}");
    assert!(errors.is_empty(), "the turn must not fail: {errors:?}");
    assert!(!calls.is_empty(), "the model never ran python_exec");
    assert_eq!(
        calls.iter().map(|(_, n)| n).sum::<usize>(),
        0,
        "no image may reach a text-only model"
    );
    // Said on the chart's own line — and never also claimed as shown, which the line did
    // until the loop, the one place that knows, wrote the claim (spec §9.10).
    let said = |key: &'static str| -> Vec<&'static str> {
        crate::shared::i18n::Lang::ALL
            .iter()
            .map(|l| crate::shared::i18n::locale(*l).t(key))
            .collect()
    };
    let (withheld, shown) = (
        said("loop.image_not_shown_no_vision"),
        said("loop.image_shown"),
    );
    assert!(
        calls
            .iter()
            .any(|(r, _)| withheld.iter().any(|t| r.contains(t))),
        "the result must say the chart was not shown"
    );
    assert!(
        !calls
            .iter()
            .any(|(r, _)| shown.iter().any(|t| r.contains(t))),
        "no result may claim a chart was shown to a text-only model"
    );
    assert!(!reply.trim().is_empty(), "the turn must end with a reply");
}

/// A chat whose history already holds an image, after a switch to an engine that takes
/// none (docs/research/history-images-no-vision.md) — the real sequence, on the app's own
/// paths: the app runs on a vision engine (`MINDFORK_ENGINE_URL`, `…_MODEL`), the image is
/// attached and seen; the app is restarted on the **same data** with an engine that takes
/// no images (`MINDFORK_ENGINE_URL_BLIND`, `…_KEY_BLIND`, `…_MODEL_BLIND`) — a llama.cpp
/// without `--mmproj`, or a gateway model the catalogue lists as text-only. There, before
/// this stage, every turn was a `500` or a `404`. Now the turn completes, asked about a
/// detail the conversation never mentioned the model says it cannot see the image rather
/// than denying the detail, the user is told once, and the stored image survives.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL (a vision engine) and MINDFORK_ENGINE_URL_BLIND (one without)"]
async fn a_history_image_on_an_engine_without_vision_live() {
    let Some(blind) =
        crate::shared::api::live_client("MINDFORK_ENGINE_URL_BLIND", "MINDFORK_ENGINE_KEY_BLIND")
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL_BLIND not set");
        return;
    };
    let Some(sighted) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let external = |model: Option<String>| {
        let mut cfg = no_auto_cfg();
        cfg.engine.mode = crate::shared::config::ServerMode::External;
        cfg.engine.external.model_name = model;
        cfg
    };
    let dir = tempfile::tempdir().unwrap();

    // 1. A vision engine: the image is attached and seen.
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(
        dir.path(),
        Some(sighted),
        external(std::env::var("MINDFORK_ENGINE_MODEL").ok()),
    );
    let Some(AppEvent::ChatActivated { id: chat, .. }) =
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await
    else {
        panic!("no chat");
    };
    let path = dir.path().join("figure.png");
    std::fs::write(
        &path,
        figure_png_with_corner([20, 60, 200], Some([220, 30, 30])),
    )
    .unwrap();
    attach_image_live(&cmd_tx, &mut evt_rx, path.to_string_lossy().into_owned()).await;
    let (first, _) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "What is the background colour of this image, and what large shape is in the centre? \
         Mention only those two things, in a few words.",
    )
    .await;
    eprintln!("sighted reply: {first}");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    assert!(
        first.to_lowercase().contains("blue"),
        "the vision engine must see the image before the switch: {first}"
    );

    // 2. The same data, an engine that takes no images.
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(
        dir.path(),
        Some(crate::shared::api::retry::RetryBackend::wrap(Arc::new(
            blind,
        ))),
        external(std::env::var("MINDFORK_ENGINE_MODEL_BLIND").ok()),
    );
    let Some(AppEvent::ChatActivated { id, .. }) =
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await
    else {
        panic!("no chat after the restart");
    };
    assert_eq!(
        id, chat,
        "the restart reopens the chat that holds the image"
    );
    let notes: Vec<String> = crate::shared::i18n::Lang::ALL
        .iter()
        .map(|l| crate::shared::i18n::locale(*l).tf("ui.chat.images_withheld", &[("n", "1")]))
        .collect();
    let turn = |message: &'static str| {
        cmd_tx
            .send(AppCommand::SendMessage(message.into()))
            .unwrap();
    };
    let collect = async |rx: &mut UnboundedReceiver<AppEvent>| {
        let (mut reply, mut errors, mut told) = (String::new(), Vec::new(), 0usize);
        while let Some(ev) = rx.recv().await {
            match ev {
                AppEvent::Chunk { text, .. } => reply.push_str(&text),
                AppEvent::Error(e) => errors.push(e),
                AppEvent::Notice(n) if notes.contains(&n) => told += 1,
                AppEvent::Finished { .. } => break,
                _ => {}
            }
        }
        (reply, errors, told)
    };
    turn(
        "What colour is the small square in the top-left corner of the image I sent? \
         Answer in one short sentence.",
    );
    // Every wait bounded (docs/lessons.md §2): without the fix there is no note to wait
    // for, and an unbounded wait turned that red run into a hang.
    let turn_cap = std::time::Duration::from_secs(600);
    let (second, errors, _) = tokio::time::timeout(turn_cap, collect(&mut evt_rx))
        .await
        .expect("the turn ends");
    // The note follows the turn's result, so it lands after `Finished`.
    let told = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::Notice(n) if notes.contains(n)),
        ),
    )
    .await
    .is_ok_and(|ev| ev.is_some());
    turn("Thanks, that is all.");
    let (third, more_errors, told_again) = tokio::time::timeout(turn_cap, collect(&mut evt_rx))
        .await
        .expect("the turn ends");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    eprintln!(
        "blind reply: {second}\nerrors: {errors:?}\nnext reply: {third}\nerrors: {more_errors:?}"
    );

    assert!(
        errors.is_empty() && more_errors.is_empty(),
        "no turn may be refused: {errors:?} {more_errors:?}"
    );
    assert!(told, "the chat is told its image was not sent");
    assert_eq!(told_again, 0, "…once");
    let low = second.to_lowercase();
    const DECLINES: &[&str] = &[
        "cannot see",
        "can't see",
        "can not see",
        "unable to see",
        "not able to see",
        "cannot view",
        "can't view",
        "don't have access",
        "do not have access",
        "not included",
        "wasn't included",
        "was not included",
        "не вижу",
        "не могу увидеть",
        "не могу видеть",
        "не передан",
    ];
    assert!(
        DECLINES.iter().any(|d| low.contains(d)),
        "asked about a detail it never saw, the model must say it cannot see the image: {second}"
    );
    let stored: usize = Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chat(chat)
        .unwrap()
        .unwrap()
        .messages
        .iter()
        .map(|m| m.images.len())
        .sum();
    assert_eq!(stored, 1, "the stored image survives the switch");
}

/// Sandbox file exchange, stage 2 (docs/history/sandbox-file-exchange.md §8): a chart the model's
/// code saves to `/w/out` lands in the chat's folder, is listed in the chat — `/file list`
/// finds its bytes on disk — and is shown to the model; with `tools.python_images` off the
/// file still lands, no image is sent, and the result tells the model it has not seen it.
/// Stage 1's scenario on production code, without the input file stage 3 adds: the numbers
/// are in the request. Needs a vision model (`MINDFORK_ENGINE_URL` + `--mmproj`) and a
/// provisioned `MINDFORK_SANDBOX_DIR`, which the temporary data root is pointed at
/// (§11 S13). `#[ignore]`, manual against a live stack.
#[tokio::test]
#[ignore = "requires a vision model (MINDFORK_ENGINE_URL) and a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
async fn sandbox_outputs_e2e_live() {
    use crate::features::file_command::FileProgress;
    const REQUEST: &str = "Plot these monthly sales as a bar chart and save it as a PNG: \
        Jan 120, Feb 95, Mar 180, Apr 130, May 160. Then tell me in one sentence what the \
        chart shows.";
    if crate::shared::api::live_text_only() {
        eprintln!("skip: MINDFORK_LIVE_TEXT_ONLY — this stack has no vision projector");
        return;
    }
    let Some(sandbox) = std::env::var_os("MINDFORK_SANDBOX_DIR").map(std::path::PathBuf::from)
    else {
        eprintln!("skip: MINDFORK_SANDBOX_DIR not set");
        return;
    };
    for images_on in [true, false] {
        let mut cfg = no_auto_cfg();
        cfg.tools.python_enabled = true;
        cfg.tools.python_mode = crate::shared::config::PythonMode::Wasmer;
        cfg.tools.python_net_enabled = false;
        cfg.tools.python_images = images_on;
        cfg.default_sampling.max_tokens = Some(4096);
        let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_sandbox(cfg, sandbox.clone())
        else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let (_profile, chat) = narrow_profile_to(
            &cmd_tx,
            &mut evt_rx,
            vec![crate::features::tools::PYTHON_EXEC_ID.into()],
        )
        .await;
        cmd_tx
            .send(AppCommand::SendMessage(REQUEST.into()))
            .unwrap();
        let mut reply = String::new();
        let mut calls: Vec<(String, usize)> = Vec::new();
        while let Some(ev) = evt_rx.recv().await {
            match ev {
                AppEvent::Chunk { text, .. } => reply.push_str(&text),
                AppEvent::ToolCall {
                    name,
                    result,
                    images,
                    ..
                } if name == crate::features::tools::PYTHON_EXEC_ID => calls.push((result, images)),
                AppEvent::Finished { .. } => break,
                _ => {}
            }
        }
        for (result, images) in &calls {
            eprintln!("[images {images_on}] result, {images} image(s) sent:\n{result}");
        }
        eprintln!("[images {images_on}] reply: {reply}");
        cmd_tx.send(AppCommand::FileList).unwrap();
        let listed = wait_for(&mut evt_rx, |e| {
            matches!(e, AppEvent::FileProgress(FileProgress::Listed { .. }))
        })
        .await;
        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();

        let Some(AppEvent::FileProgress(FileProgress::Listed { stored, .. })) = listed else {
            panic!("no /file list reply");
        };
        let png = stored
            .iter()
            .find(|f| f.mime == "image/png")
            .unwrap_or_else(|| panic!("no PNG listed: {stored:?}"));
        assert!(!png.missing, "the listed PNG is not in the folder");
        let on_disk = dir
            .path()
            .join("files")
            .join(chat.to_string())
            .join(&png.name);
        assert!(
            std::fs::read(&on_disk).is_ok_and(|b| b.starts_with(b"\x89PNG")),
            "not a PNG: {}",
            on_disk.display()
        );
        assert!(
            calls.iter().any(|(r, _)| r.contains("files:")),
            "no result carried the files section"
        );
        let sent: usize = calls.iter().map(|(_, n)| n).sum();
        if images_on {
            assert!(sent >= 1, "the chart must reach the model");
        } else {
            assert_eq!(sent, 0, "no image may be sent with the switch off");
            let told: Vec<String> = crate::shared::i18n::Lang::ALL
                .iter()
                .map(|l| {
                    crate::shared::i18n::locale(*l)
                        .t("tool.python_exec.files.not_shown_off")
                        .trim_start_matches(" — ")
                        .to_string()
                })
                .collect();
            assert!(
                calls
                    .iter()
                    .any(|(r, _)| told.iter().any(|t| r.contains(t.as_str()))),
                "the result must tell the model it has not seen the image"
            );
        }
    }
}

/// A chat whose only tool is `python_exec` in its sandbox mode, on a temporary data root
/// pointed at a provisioned sandbox (§11 S13). `None` — the live stack or the sandbox is
/// not configured, and the caller skips. One fixture for the stage-3 smokes: three copies
/// of a spawn prologue written minutes apart is the duplication this project keeps paying
/// for (docs/lessons.md §2).
async fn spawn_python_chat() -> Option<(
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
)> {
    spawn_python_chat_with(|_| {}).await
}

/// [`spawn_python_chat`] with the configuration adjusted before the spawn — the engine
/// mode, above all, when a smoke is about what a gateway answers.
async fn spawn_python_chat_with(
    adjust: impl FnOnce(&mut AppConfig),
) -> Option<(
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
)> {
    let sandbox = match std::env::var_os("MINDFORK_SANDBOX_DIR").map(std::path::PathBuf::from) {
        Some(dir) => dir,
        None => {
            eprintln!("skip: MINDFORK_SANDBOX_DIR not set");
            return None;
        }
    };
    let mut cfg = no_auto_cfg();
    cfg.tools.python_enabled = true;
    cfg.tools.python_mode = crate::shared::config::PythonMode::Wasmer;
    cfg.tools.python_net_enabled = false;
    cfg.default_sampling.max_tokens = Some(4096);
    adjust(&mut cfg);
    let (dir, cmd_tx, mut evt_rx, handle) =
        spawn_orch_live_sandbox(cfg, sandbox).or_else(|| {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            None
        })?;
    narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        vec![crate::features::tools::PYTHON_EXEC_ID.into()],
    )
    .await;
    Some((dir, cmd_tx, evt_rx, handle))
}

/// Sends one message and collects the turn: the reply, and every `python_exec` result.
/// Both are printed — a live smoke that fails has to say what the model actually did.
async fn python_turn(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    message: &str,
) -> (String, Vec<PythonCall>) {
    cmd_tx
        .send(AppCommand::SendMessage(message.into()))
        .unwrap();
    let mut reply = String::new();
    let mut results: Vec<PythonCall> = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::Chunk { text, .. } => reply.push_str(&text),
            AppEvent::ToolCall {
                name,
                arguments,
                result,
                ..
            } if name == crate::features::tools::PYTHON_EXEC_ID => {
                results.push(PythonCall { arguments, result });
            }
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    for call in &results {
        eprintln!("python_exec({})\n→ {}", call.arguments, call.result);
    }
    eprintln!("reply: {reply}");
    (reply, results)
}

#[derive(Debug)]
/// One `python_exec` call of a live turn: what the model asked for, and what it got back.
/// The arguments matter to any smoke about **which handle** was named — the result text
/// does not carry it, and a test that cannot see it cannot tell a working numbering from
/// a broken one.
struct PythonCall {
    arguments: String,
    result: String,
}

/// The first run of at least `digits` digits in `s`, with any thousands separators dropped
/// — what the sandbox computed, as the reply would repeat it.
fn first_number(s: &str, digits: usize) -> Option<String> {
    let plain: String = s.chars().filter(|c| *c != ',' && *c != ' ').collect();
    let mut best: Option<String> = None;
    let mut run = String::new();
    for c in plain.chars().chain(std::iter::once('.')) {
        if c.is_ascii_digit() {
            run.push(c);
            continue;
        }
        if run.len() >= digits && best.is_none() {
            best = Some(run.clone());
        }
        run.clear();
    }
    best
}

/// Sandbox file exchange, stage 3 (docs/history/sandbox-file-exchange.md §12 T14): what one call
/// saved, a **later** call reads — D4's persistence, end to end, through the chat's files.
/// Turn 1 writes a workbook into `/w/out` with numbers the model does not choose and is
/// told not to print; turn 2 names that workbook in `files`, so it is copied into `/w/in`,
/// and reads the total back with pandas.
///
/// The sandbox's network reaches the public internet and **not this machine**
/// (docs/research/safe-defaults.md D4). Measured before the change: a bare `--net` gave
/// sandboxed code the host's own loopback and LAN, so a listener here was reachable from
/// inside the guest.
///
/// The count on the host listener is the assertion rather than the code's own error text:
/// a filter that refused *after* connecting would read the same from inside. The control
/// arm — the same code with `tools.web_allow_private` on — must reach it, or the first
/// arm proves only that the sandbox had no network at all.
///
/// `#[ignore]`, manual against a live stack.
#[tokio::test]
#[ignore = "requires a live model (MINDFORK_ENGINE_URL) and a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
async fn sandbox_network_is_public_only_e2e_live() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn host_listener() -> (u16, std::sync::Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&hits);
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                counter.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });
        (port, hits)
    }

    // The code is a raw literal with real newlines: a Rust line continuation would
    // indent every line of it, and Python answers that with an `IndentationError`
    // before it ever reaches the network (measured the first time this ran).
    let ask = |port: u16| {
        let code = format!(
            r#"import socket, urllib.request
def probe(host, port):
    try:
        s = socket.create_connection((host, port), timeout=3)
        s.close()
        return 'CONNECTED'
    except Exception as e:
        return type(e).__name__
print('loopback:', probe('127.0.0.1', {port}))
try:
    print('public:', urllib.request.urlopen('https://example.com', timeout=20).status)
except Exception as e:
    print('public:', type(e).__name__)"#
        );
        format!(
            "Use python_exec exactly once, with this code verbatim and nothing added \
             or reindented:\n\n{code}\n\nThen report what it printed."
        )
    };

    // Arm 1: the default — public addresses only.
    let (port, hits) = host_listener();
    let Some((_dir, cmd_tx, mut evt_rx, handle)) =
        spawn_python_chat_with(|cfg| cfg.tools.python_net_enabled = true).await
    else {
        return;
    };
    let (_reply, calls) = python_turn(&cmd_tx, &mut evt_rx, &ask(port)).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let printed = calls
        .iter()
        .map(|c| c.result.as_str())
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    eprintln!(
        "public-only arm printed:
{printed}"
    );
    assert!(
        printed.contains("public: 200"),
        "the public internet must still work: {printed}"
    );
    assert!(
        !printed.contains("loopback: CONNECTED"),
        "the host's loopback was reachable from the sandbox: {printed}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the sandbox connected to a service on this machine"
    );

    // Arm 2 (control): with private addresses allowed, the same code reaches it.
    let (port, hits) = host_listener();
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_python_chat_with(|cfg| {
        cfg.tools.python_net_enabled = true;
        cfg.tools.web_allow_private = true;
    })
    .await
    else {
        return;
    };
    let (_reply, calls) = python_turn(&cmd_tx, &mut evt_rx, &ask(port)).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let printed = calls
        .iter()
        .map(|c| c.result.as_str())
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    eprintln!(
        "allow-private arm printed:
{printed}"
    );
    assert!(
        printed.contains("loopback: CONNECTED") || hits.load(Ordering::SeqCst) > 0,
        "the control arm reached nothing either — the arms prove nothing apart: {printed}"
    );
}

/// A workbook rather than a CSV on purpose: its bytes have to survive being stored and
/// staged **unchanged**, or openpyxl cannot open it at all. And the number is the
/// sandbox's, not the conversation's — the peak-month probe of §10 showed how easily an
/// answer arrives through a channel the test did not mean to leave open — so the assertion
/// is that the reply repeats what the second call actually printed.
///
/// `#[ignore]`, manual against a live stack.
#[tokio::test]
#[ignore = "requires a live model (MINDFORK_ENGINE_URL) and a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
async fn sandbox_inputs_e2e_live() {
    use crate::features::file_command::FileProgress;
    const MAKE: &str = "Use python_exec once. Build an Excel workbook with openpyxl: one \
        sheet with the header row month,total and twelve data rows, where row i (1..12) has \
        month 2026-i and total = i*i*7 + 13. Save it as /w/out/sales.xlsx. Do NOT print the \
        numbers or the sum — just say the file is saved.";
    const READ: &str = "Use python_exec again. The workbook you saved is one of this chat's \
        files: name it in the files argument of the call so it is copied into /w/in, read it \
        from there with pandas, and print the sum of the total column. Then tell me that sum \
        in your reply.";

    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_python_chat().await else {
        return;
    };
    let (_made, first) = python_turn(&cmd_tx, &mut evt_rx, MAKE).await;
    let (reply, second) = python_turn(&cmd_tx, &mut evt_rx, READ).await;
    cmd_tx.send(AppCommand::FileList).unwrap();
    let listed = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Listed { .. }))
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        first.iter().any(|c| c.result.contains("files:")),
        "the first call saved nothing to /w/out"
    );
    let Some(AppEvent::FileProgress(FileProgress::Listed { stored, .. })) = listed else {
        panic!("no /file list reply");
    };
    let workbook = stored
        .iter()
        .find(|f| f.name.ends_with(".xlsx"))
        .unwrap_or_else(|| panic!("no workbook stored: {stored:?}"));
    assert!(
        !workbook.missing,
        "the listed workbook is not in the folder"
    );
    assert!(
        dir.path().join("files").exists(),
        "the chat's folder was never created"
    );

    // The second call ran (a refusal would have said so and run nothing) and printed a
    // number the sandbox computed from the staged file.
    assert!(
        !second.iter().any(|c| c.result.contains("nothing was run")),
        "the second call was refused: {second:?}"
    );
    let printed = second
        .iter()
        .find_map(|c| first_number(&c.result, 3))
        .unwrap_or_else(|| panic!("the second call printed no number: {second:?}"));
    assert!(
        fold_dashes(&reply).contains(&printed),
        "the reply does not carry what the call read back ({printed}): {reply}"
    );
}

/// Sandbox file exchange, stage 5 (§14 V7): the **Local** mode's round trip through a live
/// model — the half a unit test cannot reach, since what is being checked is that a model
/// told about `in/` and `out/` uses them. The chat's attached table is named in `files`,
/// read from `in/` and summed, and the summary is written to `out/` and kept with the chat.
///
/// Needs `MINDFORK_ENGINE_URL` and a Python on `PATH` — and no sandbox at all, which is the
/// point of the mode. The standard library only: the machine's interpreter is the user's,
/// and pandas may not be on it.
///
/// `#[ignore]`, manual against a live stack.
#[tokio::test]
#[ignore = "requires a live model (MINDFORK_ENGINE_URL) and a Python interpreter on PATH"]
async fn local_mode_files_round_trip_e2e_live() {
    use crate::features::file_command::FileProgress;
    const ASK: &str = "A table is one of this chat's files. Use python_exec once: name that \
        file in the files argument of the call so it is copied into the input folder, read \
        it from there with the standard library only (no pandas), print the sum of the \
        total column, and write that sum into a file called summary.txt in the output \
        folder. Then tell me the sum in your reply.";
    // Twelve rows the model is not told, so the number can only come from the file.
    let rows: Vec<(u32, u32)> = (1..=12).map(|i| (i, i * i * 7 + 13)).collect();
    let total: u32 = rows.iter().map(|(_, t)| t).sum();
    let table = std::iter::once("month,total".to_string())
        .chain(rows.iter().map(|(m, t)| format!("2026-{m:02},{t}")))
        .collect::<Vec<_>>()
        .join("\n");

    let mut cfg = no_auto_cfg();
    cfg.tools.python_enabled = true;
    cfg.tools.python_mode = crate::shared::config::PythonMode::Local;
    cfg.default_sampling.max_tokens = Some(4096);
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg).or_else(|| {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        None
    }) else {
        return;
    };
    narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        vec![crate::features::tools::PYTHON_EXEC_ID.into()],
    )
    .await;

    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("sales.csv");
    std::fs::write(&path, &table).unwrap();
    cmd_tx
        .send(AppCommand::FileAttach {
            path: path.display().to_string(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::FileProgress(FileProgress::Attached { .. })
                | AppEvent::FileProgress(FileProgress::Failed(_))
        )
    })
    .await;

    let (reply, results) = python_turn(&cmd_tx, &mut evt_rx, ASK).await;
    cmd_tx.send(AppCommand::FileList).unwrap();
    let listed = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::FileProgress(FileProgress::Listed { .. }))
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        !results.iter().any(|c| c.result.contains("nothing was run")),
        "the call was refused: {results:?}"
    );
    // The number is the file's, and the interpreter is what computed it.
    assert!(
        results
            .iter()
            .any(|c| c.result.contains(&total.to_string())),
        "the call never printed the total {total}: {results:?}"
    );
    assert!(
        fold_dashes(&reply).contains(&total.to_string()),
        "the reply does not carry the total {total}: {reply}"
    );
    // And the way out works on the host too: what the code wrote was stored with the chat.
    let Some(AppEvent::FileProgress(FileProgress::Listed { stored, .. })) = listed else {
        panic!("no /file list reply");
    };
    let kept = stored
        .iter()
        .find(|f| f.name.starts_with("summary"))
        .unwrap_or_else(|| panic!("nothing was kept from the output folder: {stored:?}"));
    assert!(!kept.missing, "the listed file is not in the folder");
    assert!(
        dir.path().join("files").exists(),
        "the chat's folder was never created"
    );
}

/// Fork F8a live (§12 T9, T14): a **binary** the user attaches is kept with the chat —
/// `/file attach` no longer refuses it — and a call that names it reads the real bytes in
/// `/w/in`. The fixture is a generated 512×512 PNG, so what the code reports is objective
/// and no pretrained knowledge can answer it; pillow is in the sandbox's starter set.
///
/// `#[ignore]`, manual against a live stack.
#[tokio::test]
#[ignore = "requires a live model (MINDFORK_ENGINE_URL) and a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
async fn attached_binary_reaches_the_sandbox_live() {
    use crate::features::file_command::FileProgress;
    const ASK: &str = "A picture is one of this chat's files. Use python_exec: name it in \
        the files argument so it is copied into /w/in, open it there with PIL (pillow) and \
        print its size in pixels. Then tell me the width and the height.";

    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_python_chat().await else {
        return;
    };
    let picture = tempfile::tempdir().unwrap();
    let path = picture.path().join("figure.png");
    std::fs::write(&path, figure_png([200, 30, 30])).unwrap();
    cmd_tx
        .send(AppCommand::FileAttach {
            path: path.display().to_string(),
        })
        .unwrap();
    let kept = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::FileProgress(FileProgress::StoredFile { .. })
                | AppEvent::FileProgress(FileProgress::Failed(_))
        )
    })
    .await;
    // A binary is kept, not refused: that is the half of F8a the model never sees.
    let AppEvent::FileProgress(FileProgress::StoredFile { name, mime, .. }) = kept.unwrap() else {
        panic!("the picture was refused instead of kept");
    };
    assert_eq!(name, "figure.png");
    assert_eq!(mime, "image/png");

    let (reply, results) = python_turn(&cmd_tx, &mut evt_rx, ASK).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    assert!(
        !results.iter().any(|c| c.result.contains("nothing was run")),
        "the call was refused: {results:?}"
    );
    assert!(
        results.iter().any(|c| c.result.contains("512")),
        "the code never read the picture's real size: {results:?}"
    );
    assert!(
        reply.contains("512"),
        "the reply does not carry the size the code read: {reply}"
    );
}

/// Fork F12, with a real model: a number the pinned block gave stays that file for the
/// **whole turn**, across a round that adds one.
///
/// The chat holds exactly one item — an image, `#1`. The turn asks for two `python_exec`
/// calls: the first saves a file, which lands in the chat's files and, in a freshly
/// derived list, sorts **ahead** of every image; the second names `#1` and prints the
/// first bytes of what arrived in `/w/in`.
///
/// The two answers cannot be confused: the image begins with the PNG signature, the file
/// the first call wrote begins with `ROUND-ONE`. Before this fix the second call was
/// handed the file the turn had just created, under the number the model had been given
/// for the picture — silently, with no refusal to notice.
///
/// `#[ignore]`, manual against a live stack.
#[tokio::test]
#[ignore = "requires a live model (MINDFORK_ENGINE_URL) and a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
async fn a_handle_survives_a_round_that_adds_a_file_live() {
    // Two calls in two **rounds**, which is the whole point: the list is reconciled
    // between rounds, so two calls issued together never reach the defect. Call 2 needs a
    // value only call 1 can produce, so it cannot be written until call 1 has answered.
    const ASK: &str = "Do this in two separate python_exec calls, the second written only \
        after you have seen the first one's output. \
        Call 1: write the text ROUND-ONE into /w/out/marker.txt, then print \
        hashlib.sha256(b'ROUND-ONE').hexdigest(). \
        Call 2: put #1 in the files argument, open the file that appears in /w/in in \
        binary mode, and print its first 8 bytes with repr() followed by the hex digest \
        call 1 printed. Then tell me what call 2 printed.";

    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_python_chat().await else {
        return;
    };
    let picture = tempfile::tempdir().unwrap();
    let path = picture.path().join("figure.png");
    std::fs::write(&path, figure_png([30, 120, 60])).unwrap();
    attach_image_live(&cmd_tx, &mut evt_rx, path.to_string_lossy().into_owned()).await;

    let (reply, results) = python_turn(&cmd_tx, &mut evt_rx, ASK).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Said rather than assumed: a turn that made only one call proves nothing either way,
    // and would otherwise pass on the half that never ran.
    assert!(
        results.len() >= 2,
        "the turn had to make two python_exec calls, it made {}: {results:?}",
        results.len()
    );
    // The **second** call, and only it: the first legitimately carries `ROUND-ONE`,
    // because a saved text file is listed with the head of its own content.
    let second = &results[1];
    // The handle has to be the number, not the name. A model that names `figure.png`
    // reaches the picture whatever the numbering does, so this smoke would then pass
    // against a broken list — measured, that is exactly what happened the first time.
    assert!(
        second.arguments.contains("#1"),
        "the second call has to name the handle, not the file: {}",
        second.arguments
    );
    assert!(
        second.result.contains("PNG"),
        "#1 had to stay the picture the block numbered; the call saw: {}",
        second.result
    );
    assert!(
        !second.result.contains("ROUND-ON"),
        "#1 was handed the file this very turn created: {}",
        second.result
    );
    assert!(
        reply.contains("PNG"),
        "the reply has to repeat what the second call printed: {reply}"
    );

    // The file the first call stored carries the mark of a download (§13 U11) — checked on
    // what a real turn wrote through the real store, not on a unit's fixture.
    #[cfg(windows)]
    {
        use crate::shared::os_open::{FROM_ELSEWHERE, zone_of};
        let marker = std::fs::read_dir(dir.path().join("files"))
            .expect("the chat's files folder")
            .filter_map(Result::ok)
            .map(|chat| chat.path().join("marker.txt"))
            .find(|path| path.is_file())
            .expect("the first call's marker.txt was stored");
        assert_eq!(
            zone_of(&marker).as_deref(),
            Some(FROM_ELSEWHERE),
            "{} carries no mark",
            marker.display()
        );
        println!("marked: {}", marker.display());
    }
    #[cfg(not(windows))]
    let _ = dir;
}

/// i18n Tier 1 (docs/history/i18n.md, go/no-go): a profile with agent-scaffold language `En` —
/// the auto-title of an English conversation is English, with NO Cyrillic. A fresh profile
/// (its own texts, independent of the bootstrap profile's state), set it to En, create a
/// chat, run an English turn and auto-titling. `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn i18n_en_profile_title_e2e_live() {
    // Automatic titling off: this smoke exercises the **requested** path
    // (`AutoRenameChat`), and the trigger would race it with a second title
    // task after the turn (the trigger has its own smoke below).
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(no_auto_cfg()) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // A fresh profile with its own English texts (the bootstrap one is Russian).
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

/// Live smoke of the **automatic** titling trigger (spec §11.2), on the default
/// config: the first real reply renames the chat with no command from anyone —
/// the end-to-end path the track added, where the smoke above covers the
/// requested task. See docs/history/auto-chat-title.md.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn auto_title_first_reply_e2e_live() {
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let act = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let (chat_id, default_title) = match act {
        AppEvent::ChatActivated { id, title, .. } => (id, title),
        _ => unreachable!(),
    };
    // One short exchange; no `AutoRenameChat` anywhere in this test.
    let (reply, _) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Почему небо синее? Ответь одним предложением.",
    )
    .await;
    eprintln!("reply: {:?}", reply.chars().take(80).collect::<String>());
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    let (id, title) = match renamed {
        AppEvent::ChatRenamed { id, title } => (id, title),
        _ => unreachable!(),
    };
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    eprintln!("automatic title: {title:?} (was {default_title:?})");
    assert_eq!(id, chat_id, "the rename must be about the active chat");
    assert!(!title.trim().is_empty(), "empty automatic title");
    assert_ne!(
        title, default_title,
        "the default title must actually be replaced"
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
///
/// **The instruction is closed on purpose**, and the thinking is muted; the two
/// together took this smoke from flaky to clean, and each fixed a *different*
/// model's failure.
///
/// It used to ask the model to "demonstrate the tool strictly by steps, skipping
/// none: step 1 write X, step 2 (MANDATORY) call the tool, step 3 write Y" — an
/// invitation to *narrate the sequence*, which Gemma duly accepted: it answered
/// `"2+2=5\n<call:rewrite_current_message/>\n2+2=4"`, writing the call as prose in a
/// syntax no protocol here defines. No call, no archive, red test — and a red
/// dispatch of the live gate (run 31907378154). Measured **1 failure in 8** on the
/// local Gemma stand; reworded as a situation the tool answers, **0 in 30**.
///
/// Qwen 3.6 needed the second half. With thinking on it failed differently and more
/// rarely — 1 in 20, and not by narrating but by producing **nothing at all**: no
/// tool call and empty text, the whole turn spent in `reasoning_content` (the same
/// shape that once broke `simple_generation`). Muting thinking for this turn:
/// **0 in 20**. Nothing is lost by muting, because what this smoke asks is whether
/// a real model reaches for the tool and whether the effect lands; the control-tool
/// path *through* thinking stays covered live by [`followup_tool_e2e_live`], which
/// leaves it on (0 in 12 measured).
///
/// **Narrowing the profile to the one tool was tried and rejected — it made things
/// worse**, 7 failures in 20 on Qwen, all of them the empty turn above. The
/// "remove the alternative" rule (docs/lessons.md §9) is about a model satisfying
/// the request through a *different* tool; that is not this failure, and a
/// one-tool list does not prevent a model from spending the turn thinking.
///
/// The configuration above is **0 in 20 on each family** (2026-08-16,
/// `gemma-4-31B_q4_0-it` and `Qwen3.6-27B-Q4_K_M`).
///
/// The mechanism itself does not depend on this test: `rewrite_tool_discards_partial_
/// and_saves_it` pins it deterministically against a `MockBackend`, and asserts
/// strictly more (the exact history, the discarded text, its tool call). What is
/// live here is the one thing a mock cannot answer — whether a real model reaches
/// for the tool at all. `#[ignore]`, run against a live model.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn rewrite_tool_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    let root = _d.path().to_path_buf();
    let pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;
    // Thinking off for this turn. `resolve` is whole-config, so the profile's
    // sampling replaces the fixture's global one and has to carry its temperature
    // too. Commands share one channel and are applied in order, so this lands
    // before the message below.
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                default_sampling: Some(Some(crate::entities::sampling::SamplingConfig {
                    temperature: Some(0.1),
                    reasoning_budget: Some(0),
                    ..Default::default()
                })),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage(
            "Черновик твоего ответа никуда не годится. ОБЯЗАТЕЛЬНО вызови \
             инструмент rewrite_current_message, чтобы отбросить начатый ответ и \
             написать его заново. Вопрос: сколько будет два плюс два?"
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
    // The tool names are half the diagnosis when this goes red: "the model never
    // called it" and "it called it and the effect did not land" are different
    // failures, and the text alone cannot tell them apart.
    for (i, m) in chat.messages.iter().enumerate() {
        eprintln!(
            "  msg[{i}] {:?} tools={:?} text={:?}",
            m.role,
            m.tool_calls.iter().map(|t| &t.name).collect::<Vec<_>>(),
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
        extra_tools: Vec::new(),
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
    // A fresh profile with its own English texts → set scaffold language En.
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
            ..
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

/// The file tools' reach against a real model (docs/research/safe-defaults.md
/// §4, D1–D2). The mocked tests prove each refusal; this proves they reach a
/// model through a real provider as the answer to its call. Two turns:
///
/// 1. a root that **contains** the data root (the system temp folder), and the
///    model asked to read the data root's `profiles.json` — GO: an `fs_read` call
///    answered with the app-directory refusal, and the reply quotes nothing of
///    the file;
/// 2. no root at all, and the model asked to list a folder — GO: an `fs_list`
///    call answered with the no-root refusal.
///
/// What the model then tells the user is printed, not asserted (lessons §9).
#[tokio::test]
#[ignore]
async fn file_tools_reach_e2e_live() {
    let refusals = |key: &str| -> Vec<String> {
        [crate::shared::i18n::Lang::Ru, crate::shared::i18n::Lang::En]
            .into_iter()
            .map(|l| crate::shared::i18n::locale(l).t(key).to_string())
            .collect()
    };

    let mut config = AppConfig::default();
    config.tools.fs_enabled = true;
    config.tools.fs_root = Some(std::env::temp_dir().display().to_string());
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let profiles = dir.path().join("profiles.json");
    assert!(
        profiles.exists(),
        "the data root has no profiles.json to aim at"
    );
    let (reply, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Call fs_read on the file {} and quote its first three lines. Only call the tool once.",
            profiles.display()
        ),
    )
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    eprintln!("turn 1 calls: {calls:?}\nturn 1 reply: {reply}");
    let app_dir = refusals("tool.fs.err.app_dir");
    assert!(
        calls
            .iter()
            .any(|(name, result)| name == "fs_read" && app_dir.iter().any(|r| result.contains(r))),
        "no fs_read answered with the app-directory refusal: {calls:?}"
    );
    assert!(
        !reply.contains("enabled_tools") && !reply.contains("known_tools"),
        "the reply quotes the profile file: {reply}"
    );

    let listed = tempfile::tempdir().unwrap();
    std::fs::write(listed.path().join("visible.txt"), "x").unwrap();
    let mut config = AppConfig::default();
    config.tools.fs_enabled = true;
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        return;
    };
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let (reply, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Call fs_list on the folder {} and tell me what is in it. Only call the tool once.",
            listed.path().display()
        ),
    )
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    eprintln!("turn 2 calls: {calls:?}\nturn 2 reply: {reply}");
    let no_root = refusals("tool.fs.err.no_root");
    assert!(
        calls
            .iter()
            .any(|(name, result)| name == "fs_list" && no_root.iter().any(|r| result.contains(r))),
        "no fs_list answered with the no-root refusal: {calls:?}"
    );
    assert!(
        !reply.contains("visible.txt"),
        "the reply lists the folder: {reply}"
    );
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
        mentions_code(&before, CODE),
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
        mentions_code(&summary, CODE),
        "the summary must carry the identifier verbatim: {summary}"
    );
    assert!(
        mentions_code(&after, CODE),
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
    // Only the read-back tools. The first run of this smoke failed on its own
    // validity check and showed why that matters: the model filed the list with
    // `note_save`, so it could answer from memory without ever going back to the
    // history — and the note's *result* then travelled into the digest, which
    // put the whole list into the summary verbatim. Removing the alternative is
    // the same move `spawn_orch_live_no_embed` makes for attachments.
    let _ = narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        vec![
            crate::features::tools::history::HISTORY_READ_ID.into(),
            crate::features::tools::history::HISTORY_SEARCH_ID.into(),
        ],
    )
    .await;

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

/// Spec §9.11 go/no-go: a fact that exists only in **another** chat of the
/// profile is reachable through `chat_search`/`chat_read`, and a real model
/// actually reaches for the pair when the user points across conversations.
///
/// Validity: the code is a nonsense token seeded into chat A only, so chat B
/// has no route to it but the pair — the profile is narrowed to exactly these
/// two tools (the read-back smoke's lesson: remove the alternative rather than
/// hope; with `note_save` in reach the model files the fact into profile
/// memory and never crosses a conversation), and the "tool was actually
/// called" assertion keeps a refusal or a guess from reading as a pass (the
/// address-policy smoke's lesson).
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn cross_chat_search_answers_from_another_chat_live() {
    const CODE: &str = "SIREN-7734";
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(AppConfig::default()) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // Only the pair under test, and *before* the seed turn — see the doc
    // comment for why the alternative routes must not exist.
    let (profile, chat_a) = narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        vec![
            crate::features::tools::chats::CHAT_SEARCH_ID.into(),
            crate::features::tools::chats::CHAT_READ_ID.into(),
        ],
    )
    .await;

    // Chat A: the only place the code exists.
    let (reply, _) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Запиши в этом разговоре: код поставки нового компрессора — {CODE}. \
             Просто подтверди, что записал."
        ),
    )
    .await;
    eprintln!(
        "seed reply: {}",
        reply.chars().take(120).collect::<String>()
    );

    // Chat B of the same profile; A becomes an "other" chat. The post-save
    // hook indexes A behind the 800 ms save debounce, and there is no event to
    // wait on for a background index write — so wait the debounce out.
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(profile),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let (answer, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "В другом разговоре этого профиля мы записали код поставки компрессора. \
         Найди его по другим разговорам, назови код и укажи, в каком разговоре \
         он записан.",
    )
    .await;
    eprintln!("answer: {answer}");
    eprintln!(
        "tools called: {:?}",
        calls.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let used_pair = calls.iter().any(|(name, _)| {
        name == crate::features::tools::chats::CHAT_SEARCH_ID
            || name == crate::features::tools::chats::CHAT_READ_ID
    });
    assert!(
        used_pair,
        "the model must reach across conversations through the pair, not \
         guess: {calls:?}"
    );
    assert!(
        mentions_code(&answer, CODE),
        "the fact lives only in the other chat and must come back: {answer}"
    );
    // Spec §11.3: the model is *taught* the address form by the pair's
    // descriptions, so naming the conversation it read must produce a
    // `chat://` reference — resolved here through the same `find_refs` the
    // feed uses, against the one conversation the answer can legitimately
    // name. This is the half that makes the reference navigable; without it
    // the feature rests on one model's habit.
    let cited = crate::features::chat_links::find_refs(&answer, &[chat_a]);
    assert!(
        !cited.is_empty(),
        "the answer must cite the conversation as {}<id>: {answer}",
        crate::features::chat_links::SCHEME
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

/// The fourth self-description question, and the one behind the `sessions`
/// hint (spec §11.6): a live `llama-server` reports its slot count on `/props`
/// (`total_slots`) — `-np N` when given, four for the auto default since
/// December 2025 (docs/research/parallel-subagents.md §2.4) — and our client
/// reads it through the retry decorator every real external setup wraps it in.
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn props_reports_the_slot_count_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let slots = backend.parallel_slots().await;
    eprintln!("live slot count: {slots:?}");
    let n = slots.expect("a live llama-server must report total_slots at /props");
    assert!((1..=256).contains(&n), "an implausible slot count: {n}");
}

/// The other question only a real server can answer: that it will **name the
/// model it is running**, and that our client reads a name a header can show.
///
/// This is the whole of `external` mode's blank caption
/// (docs/research/external-model-name.md): connect by URL with no model named in
/// settings and, until this existed, neither the feed nor a message's metadata
/// could say what answered. A stub only proves our own fixture parses — what
/// this asserts is that the live stack answers `/v1/models` or `/props` at all,
/// and that the answer arrives normalized (an un-aliased `llama-server` reports
/// the whole `-m` path).
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn the_engine_names_the_model_it_is_running_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let name = backend.model_id().await;
    eprintln!("live model name: {name:?}");
    let name = name.expect(
        "a live OpenAI-compatible server must name its model at /v1/models or          /props — without it an external chat has no name to show",
    );
    assert!(!name.trim().is_empty(), "an empty name is not an answer");
    assert!(
        !name.ends_with(crate::shared::gguf::EXT),
        "a file name reached the header unnormalized: {name}"
    );
    assert!(
        !name.contains('\\') && !name.contains('/'),
        "a path reached the header instead of a model name: {name}"
    );
    // …and not a *part* of one. A model too large for a single file is served
    // from `<name>-00001-of-00002.gguf`, and an un-aliased server reports that
    // file — so the name a header shows must have lost the tail as well as the
    // extension (docs/research/e2e-gpt-oss-120b.md §6, T1). Vacuously true on a
    // single-file stack, which is the point: it costs nothing to carry and it is
    // the one assertion a split model would break.
    //
    // The extension is put back on to ask the question, because `parse_shard` is
    // the *only* place this project recognizes the tail shape and a second
    // implementation of it here is exactly what that module exists to prevent.
    assert!(
        crate::shared::gguf::parse_shard(&format!("{name}{}", crate::shared::gguf::EXT)).is_none(),
        "a part number reached the header instead of a model name: {name}"
    );
}

/// …and it lands where the user sees it: the reply the turn stores carries the
/// discovered name in its metadata snapshot (spec §11.3), with nothing named in
/// settings. The end-to-end half of the smoke above — the name is read back off
/// the **stored** message, through a fresh activation, not off the event that
/// announced it.
///
/// The wait for `EngineModel` is not ceremony: discovery is a network round trip
/// started when the engine is applied, and a turn sent in the same tick as the
/// bootstrap genuinely resolves to no name. A human takes seconds to type; this
/// test would otherwise race the first request out of the door (measured — it
/// did, on the first live run).
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn a_reply_records_the_discovered_model_live() {
    let mut cfg = no_auto_cfg();
    // The mode the discovery exists for, with the "Model (opt.)" field blank —
    // which is also what `MINDFORK_ENGINE_URL` alone produces.
    cfg.engine.external.model_name = None;
    let Some((_dir, cmd_tx, mut evt_rx, _task)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };

    // The bootstrap chat and the engine's answer arrive in either order.
    let (chat, discovered) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (mut chat, mut model) = (None, None);
        while chat.is_none() || model.is_none() {
            match evt_rx.recv().await {
                Some(AppEvent::ChatActivated { id, .. }) => chat = Some(id),
                Some(AppEvent::EngineModel(Some(m))) => model = Some(m),
                Some(_) => {}
                None => break,
            }
        }
        (chat, model)
    })
    .await
    .expect("the engine must name its model within 30s of connecting");
    let chat = chat.expect("the bootstrap chat");
    let discovered = discovered.expect("the engine's own name for the model");
    eprintln!("discovered: {discovered}");

    // The live bubble's header: this is the value the stored message must agree
    // with, and disagreement is the failure the single resolve prevents.
    cmd_tx
        .send(AppCommand::SendMessage("Say hi in one word.".into()))
        .unwrap();
    let mut announced = None;
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::GenerationStarted { model, .. } => announced = model.clone(),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    assert_eq!(
        announced.as_deref(),
        Some(discovered.as_str()),
        "the streaming bubble must name what the engine reported"
    );

    // Read the stored reply back through a real activation. A switch **to the
    // open chat is a no-op** and emits nothing, so the way back has to leave
    // first — a fresh chat, then back (the first attempt at this smoke waited
    // forever on an event that was never going to come).
    cmd_tx
        .send(AppCommand::NewChat { profile_id: None })
        .unwrap();
    cmd_tx.send(AppCommand::SwitchChat(chat)).unwrap();
    let messages = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == chat),
    )
    .await
    .and_then(|e| match e {
        AppEvent::ChatActivated { messages, .. } => Some(messages),
        _ => None,
    })
    .expect("the chat re-opens");
    let recorded = messages
        .iter()
        .filter(|m| m.role == crate::entities::message::MessageRole::Assistant)
        .filter_map(|m| m.metadata.as_ref()?.model.clone())
        .next_back();
    eprintln!("recorded in the metadata: {recorded:?}");
    assert_eq!(
        recorded.as_deref(),
        Some(discovered.as_str()),
        "the stored reply must name the model that wrote it"
    );
}

/// The `llm_*` pair end to end (spec §9.14): the assistant, asked what model it
/// is, reaches for `get_llm_name` and answers with the name the engine itself
/// reported; and after that first exchange the recorder has written the
/// profile's baseline record, which `get_llm_history` reads back in the next
/// turn. External mode with a blank model field — the discovered-name path,
/// the same setup as the metadata smoke above, and the wait for `EngineModel`
/// matters for the same reason: a turn sent before discovery lands genuinely
/// has no name to answer or record with.
///
/// Both turns assert the tool **was actually called** — a model answering from
/// its own beliefs would otherwise read as a pass (docs/lessons.md §9).
#[tokio::test]
#[ignore = "needs a live engine (MINDFORK_ENGINE_URL)"]
async fn llm_name_and_history_e2e_live() {
    let mut cfg = no_auto_cfg();
    // The harness injects the live backend directly, so the config's default
    // `Managed` would survive into the turn snapshot and the history records
    // unless the mode is set to what the setup actually is — the first live
    // run recorded `(managed)` for a server reached by URL exactly this way.
    cfg.engine.mode = crate::shared::config::ServerMode::External;
    cfg.engine.external.model_name = None;
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };

    // The bootstrap profile/chat and the engine's answer arrive in any order.
    let (profile, discovered) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (mut profile, mut chat, mut model) = (None, None, None);
        while profile.is_none() || chat.is_none() || model.is_none() {
            match evt_rx.recv().await {
                Some(AppEvent::ProfileList(ps)) => {
                    profile = profile.or_else(|| ps.first().map(|p| p.id));
                }
                Some(AppEvent::ChatActivated { id, .. }) => chat = Some(id),
                Some(AppEvent::EngineModel(Some(m))) => model = Some(m),
                Some(_) => {}
                None => break,
            }
        }
        (profile, model)
    })
    .await
    .expect("the engine must name its model within 30s of connecting");
    let profile = profile.expect("the bootstrap profile");
    let discovered = discovered.expect("the engine's own name for the model");
    eprintln!("discovered: {discovered}");

    // Only the pair under test — nothing else to reach for.
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profile,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(vec![
                    crate::features::tools::llm::GET_LLM_NAME_ID.to_string(),
                    crate::features::tools::llm::GET_LLM_HISTORY_ID.to_string(),
                ]),
                ..Default::default()
            }),
        })
        .unwrap();

    let (_text, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Which language model are you running on right now? Check with your \
         get_llm_name tool and tell me.",
    )
    .await;
    let name_calls: Vec<&(String, String)> =
        calls.iter().filter(|(n, _)| n == "get_llm_name").collect();
    eprintln!("get_llm_name calls: {name_calls:#?}");
    assert!(
        !name_calls.is_empty(),
        "the tool was never called: {calls:#?}"
    );
    assert!(
        name_calls.iter().any(|(_, r)| r.contains(&discovered)),
        "the tool's answer must carry the discovered name {discovered:?}: {name_calls:#?}"
    );

    // The first exchange has landed, so the recorder has written the baseline
    // record (fork F6) — the next turn's history tool must read it back.
    let (_text, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "When did your language model last change? Check with your \
         get_llm_history tool and quote the records.",
    )
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let hist_calls: Vec<&(String, String)> = calls
        .iter()
        .filter(|(n, _)| n == "get_llm_history")
        .collect();
    eprintln!("get_llm_history calls: {hist_calls:#?}");
    assert!(
        !hist_calls.is_empty(),
        "the tool was never called: {calls:#?}"
    );
    assert!(
        hist_calls
            .iter()
            .any(|(_, r)| r.contains(&discovered) && r.contains("(external)")),
        "the history must hold the baseline record for {discovered:?}: {hist_calls:#?}"
    );
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

    if crate::shared::api::live_text_only() {
        eprintln!("skip: MINDFORK_LIVE_TEXT_ONLY — this stack has no vision projector");
        return;
    }
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Deliberately *not* the blue of the file-attach smoke, so a stale reply from that
    // fixture could not pass this one.
    let png = figure_png([20, 160, 60]);
    let (base, _server) = serve(vec![ok_response("image/png", &png, true)]);

    const QUESTION: &str = "What is the background colour of this image, and what shape is \
                            in the centre? Answer in a few words.";

    // Control — nothing staged. This must fail to answer.
    let (control, _) = run_turn_live(&cmd_tx, &mut evt_rx, QUESTION).await;
    eprintln!("control reply (no image staged): {control}");

    let info = attach_image_live(&cmd_tx, &mut evt_rx, format!("{base}/fixtures/figure.png")).await;
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

/// The address policy against a real model (spec §9.3,
/// docs/research/fetch-url-address-policy.md).
///
/// The unit tests prove the guard refuses; only a live model can settle the half that
/// matters for a *model-facing* message: that the refusal makes it **stop**. The failure
/// mode this feature could easily create is a model that reads "cannot reach that" and
/// tries the same host by IP, then by name, then through a redirector — three more round
/// trips that must all fail (docs/lessons.md §4).
///
/// **The target is a plain loopback service, not the cloud metadata endpoint.** The first
/// run of this smoke pointed at `169.254.169.254` and the model refused *on its own* —
/// never calling the tool, so the guard was never exercised and the test proved nothing
/// (the "never called the tool" assertion is what caught it). A local address the user
/// plausibly asked about removes that confound. The stub counts connections, so this also
/// proves end to end that nothing reached the service.
#[tokio::test]
#[ignore = "requires a live model (MINDFORK_ENGINE_URL)"]
async fn fetch_url_address_policy_e2e_live() {
    let mut config = AppConfig::default();
    config.tools.web_enabled = true;
    // Off is the default; stated here because it is the thing under test.
    config.tools.web_allow_private = false;

    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    let (url, hits) = crate::shared::net::stub::counting_stub();
    let (reply, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Fetch {url} with fetch_url and tell me what it says. If you cannot, say so and stop."
        ),
    )
    .await;
    eprintln!("reply: {reply}");
    for (name, result) in &calls {
        eprintln!("call {name} -> {result}");
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    let _ = handle.await;

    let fetches: Vec<_> = calls.iter().filter(|(n, _)| n == "fetch_url").collect();
    assert!(
        !fetches.is_empty(),
        "the model never called the tool, so nothing was tested: {calls:?}"
    );
    // Compared through the bundle, not a hardcoded phrase: this message is axis A (the
    // agent scaffold's language), so an English fragment would fail on a `ru` profile for
    // a reason that has nothing to do with the guard.
    let refusal = crate::shared::i18n::locale(crate::shared::i18n::Lang::default())
        .t("tool.fetch_url.err.address_blocked");
    assert!(
        fetches.iter().all(|(_, result)| result.contains(refusal)),
        "every attempt must come back refused, not fetched: {fetches:?}"
    );
    // The door has to close: one attempt is the model trying, four is the model hunting
    // for a way around a message that failed to say there isn't one.
    assert!(
        fetches.len() <= 2,
        "the refusal did not stop the model — {} attempts: {fetches:?}",
        fetches.len()
    );
    // The end-to-end half no unit test can reach: whatever the model tried, the service
    // itself was never connected to.
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the local service was reached despite the policy"
    );
}

/// Code workspace, **stage 1 live check** (docs/history/code-workspace.md §6): with a
/// project attached, does the model actually navigate it — locate a fact it
/// cannot know, read the file that holds it, and answer from what it read?
///
/// The fact is deliberately arbitrary (a five-digit timeout no model has an
/// opinion about) and sits behind one indirection: `main.rs` names a function,
/// the function is in another file, and the number is a constant beside it. A
/// decoy timeout in a third file makes "guessed a plausible number" fail.
///
/// The profile is narrowed to the three workspace tools — the "remove the
/// alternative" rule (docs/lessons.md §9): with `fs_read` in reach the model
/// could answer without the feature under test ever running.
///
/// **Two turns**, because the first live run showed the narrow assertion was
/// wrong: asked for the timeout, the model answered correctly from `code_list` +
/// `code_grep` alone and never opened the file — the hit line carries the whole
/// constant, so a read would have been a wasted round. That is a better outcome,
/// not a failure, and demanding `code_read` there would have pinned the model to
/// the worse route (the same shape as `attachment_search` superseding
/// `attachment_read`, docs/lessons.md §9). So turn 1 asserts the *outcome*, and
/// turn 2 asks for something only a read can produce, keeping that tool covered.
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 cargo test
/// code_workspace_navigate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn code_workspace_navigate_e2e_live() {
    use crate::features::project_command::ProjectProgress;
    use crate::features::tools::code::{CODE_GREP_ID, CODE_LIST_ID, CODE_READ_ID};

    const ANSWER: &str = "7300";
    let files: [(&str, &str); 4] = [
        (
            "src/main.rs",
            "mod config;\nmod util;\n\nfn main() {\n    let ms = config::default_timeout();\n    println!(\"waiting {ms} ms\");\n}\n",
        ),
        (
            "src/config.rs",
            "/// How long a request may take before it is abandoned.\nconst REQUEST_TIMEOUT_MS: u64 = 7300;\n\npub fn default_timeout() -> u64 {\n    REQUEST_TIMEOUT_MS\n}\n",
        ),
        // The decoy: a plausible number in a file the question does not lead to.
        (
            "src/util.rs",
            "/// Delay between reconnect attempts.\npub const RECONNECT_DELAY_MS: u64 = 500;\n",
        ),
        (
            "README.md",
            "# probe\n\nA tiny client. Timeouts are configurable.\n",
        ),
    ];
    let ws = tempfile::tempdir().unwrap();
    for (name, body) in files {
        let path = ws.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (profile, _chat) = narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        vec![
            CODE_LIST_ID.into(),
            CODE_READ_ID.into(),
            CODE_GREP_ID.into(),
        ],
    )
    .await;

    // The command path itself is under test: attaching goes through the
    // orchestrator exactly as a user's `/project attach` does.
    cmd_tx
        .send(AppCommand::ProjectAttach {
            path: ws.path().to_string_lossy().into_owned(),
        })
        .unwrap();
    let attached = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProjectProgress(_)))
        .await
        .unwrap();
    let AppEvent::ProjectProgress(ProjectProgress::Attached { root, .. }) = &attached else {
        panic!("the project did not attach: {attached:?}");
    };
    eprintln!("attached: {root}");

    // Turn 1 — find a fact that is only in the project.
    let (answer, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "В прикреплённом проекте есть таймаут запроса по умолчанию. \
         Найди его и скажи, чему он равен и в каком файле задан.",
    )
    .await;
    let names: Vec<&String> = calls.iter().map(|(n, _)| n).collect();
    eprintln!("turn 1 tool calls: {names:?}");
    eprintln!("turn 1 reply: {answer}");

    // The feature has to have actually run, or the smoke passes on a model that
    // answered from the prompt (docs/lessons.md §9). *Which* tool found it is
    // the model's call.
    assert!(
        calls
            .iter()
            .any(|(n, _)| crate::features::tools::code::is_workspace_tool(n)),
        "the model must go into the project: {names:?}"
    );
    assert!(
        answer.contains(ANSWER),
        "the timeout is only knowable from the project: {answer}"
    );
    assert!(
        answer.contains("config"),
        "the answer must name the file it came from: {answer}"
    );
    // The decoy stayed a decoy.
    assert!(
        !answer.contains("500"),
        "the reconnect delay is not the request timeout: {answer}"
    );

    // Turn 2 — `code_read`, covered on its own. Two things make that
    // deterministic rather than hopeful, and both were learned the hard way: the
    // question is about a file **turn 1 never opened** (asked about
    // `src/config.rs`, `gpt-oss-120b` answered correctly with no tool at all,
    // because its turn-1 `code_read` had already put that file in the
    // conversation), and the profile is narrowed to `code_read` alone, so no
    // other route exists. The second instance of this pattern — the first is
    // `attachment_read` in docs/history/remote-e2e-hf.md §3; an assertion that a
    // *particular* tool must be chosen is only ever true until a model finds a
    // better route, so the smoke has to remove the routes instead of hoping.
    set_profile_tools(&cmd_tx, profile, vec![CODE_READ_ID.into()]);
    let (answer2, calls2) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Что написано в первой строке файла README.md в прикреплённом проекте?",
    )
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let names2: Vec<&String> = calls2.iter().map(|(n, _)| n).collect();
    eprintln!("turn 2 tool calls: {names2:?}");
    eprintln!("turn 2 reply: {answer2}");
    assert!(
        calls2.iter().any(|(n, _)| n == CODE_READ_ID),
        "only a read can answer this: {names2:?}"
    );
    // The heading of README.md, which appears nowhere else in the workspace and
    // is not in the conversation before this turn. The line count is
    // deliberately *not* asserted: a trailing newline makes "how many lines" a
    // question two readers answer differently.
    assert!(
        answer2.to_lowercase().contains("probe"),
        "the first line of README.md is `# probe`: {answer2}"
    );
}

/// Code workspace, stage 1: **no project, no tools**. The gate is what keeps a
/// chat without a workspace byte-identical to what the app sent before the
/// feature, so it is worth one live turn: the model is asked to read a file and
/// must answer that it cannot, without any `code_*` call happening.
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 cargo test
/// code_workspace_gate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn code_workspace_gate_e2e_live() {
    use crate::features::tools::code::{CODE_GREP_ID, CODE_LIST_ID, CODE_READ_ID};

    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // The tools are enabled in the profile and still must not be offered: the
    // project's absence is the gate, not the toggle.
    let _ = narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        vec![
            CODE_LIST_ID.into(),
            CODE_READ_ID.into(),
            CODE_GREP_ID.into(),
        ],
    )
    .await;

    let (answer, calls) = run_turn_capture(
        &cmd_tx,
        &mut evt_rx,
        "Прочитай файл src/main.rs и скажи, что он делает.",
    )
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let names: Vec<&String> = calls.iter().map(|(n, _)| n).collect();
    eprintln!("tool calls: {names:?}");
    eprintln!("reply: {answer}");
    assert!(
        calls
            .iter()
            .all(|(n, _)| !crate::features::tools::code::is_workspace_tool(n)),
        "with no project attached the tools must not even be offered: {names:?}"
    );
}

/// Compiles a probe fixture with `rustc` (no cargo, no manifest, no network) and
/// runs it, returning its stdout. `Err` carries the compiler's diagnostics.
///
/// Output goes to `target/`, which `code_grep` and `code_list` skip — so
/// building does not put artifacts in front of the model, exactly as a real
/// checkout would not.
fn rustc_run(root: &std::path::Path) -> Result<String, String> {
    let build = std::process::Command::new("rustc")
        .current_dir(root)
        .args(["--edition", "2021", "src/main.rs", "--out-dir", "target"])
        .output()
        .map_err(|e| format!("could not start rustc: {e}"))?;
    if !build.status.success() {
        return Err(String::from_utf8_lossy(&build.stderr).into_owned());
    }
    let exe = root
        .join("target")
        .join(if cfg!(windows) { "main.exe" } else { "main" });
    let run = std::process::Command::new(&exe)
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run {}: {e}", exe.display()))?;
    Ok(String::from_utf8_lossy(&run.stdout).into_owned())
}

/// Spins up a live orchestrator with `files` attached as the chat's project and
/// the profile narrowed to the workspace tools, and runs one turn.
///
/// Returns `(the fixture directory, the reply, the calls)`, or `None` with no
/// live server. The narrowing is the "remove the alternative" rule
/// (docs/lessons.md §9): with `fs_write` in reach the model could rewrite a file
/// wholesale and the contract under test would never run.
async fn run_workspace_turn<B: AsRef<[u8]>>(
    files: &[(&str, B)],
    prompt: &str,
) -> Option<(tempfile::TempDir, String, Vec<(String, String, String)>)> {
    run_workspace_turn_with(files, &[], prompt).await
}

/// [`run_workspace_turn`], with command lines put into the project's slots
/// first — the stage-3 shape, where the assistant can also build, run and test.
///
/// The lines go in through the same `AppCommand::ProjectSlot` the user's
/// `/project build-cmd` sends, rather than by writing the field: the smoke is
/// then measuring the route that ships, including the shell-syntax refusal that
/// sits on it.
async fn run_workspace_turn_with<B: AsRef<[u8]>>(
    files: &[(&str, B)],
    commands: &[(crate::entities::workspace::CommandSlot, &str)],
    prompt: &str,
) -> Option<(tempfile::TempDir, String, Vec<(String, String, String)>)> {
    use crate::features::project_command::ProjectProgress;
    use crate::features::tools::code::WORKSPACE_TOOL_IDS;

    let ws = tempfile::tempdir().unwrap();
    for (name, body) in files {
        let path = ws.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_live()?;
    let _ = narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        WORKSPACE_TOOL_IDS.iter().map(|id| (*id).into()).collect(),
    )
    .await;
    cmd_tx
        .send(AppCommand::ProjectAttach {
            path: ws.path().to_string_lossy().into_owned(),
        })
        .unwrap();
    let attached = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProjectProgress(_)))
        .await
        .unwrap();
    assert!(
        matches!(
            attached,
            AppEvent::ProjectProgress(ProjectProgress::Attached { .. })
        ),
        "the project did not attach: {attached:?}"
    );

    for (slot, line) in commands {
        cmd_tx
            .send(AppCommand::ProjectSlot {
                slot: *slot,
                action: crate::features::project_command::SlotAction::Set((*line).to_string()),
            })
            .unwrap();
        let set = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProjectProgress(_)))
            .await
            .unwrap();
        assert!(
            matches!(
                set,
                AppEvent::ProjectProgress(ProjectProgress::CommandSet { .. })
            ),
            "the {slot:?} command did not take: {set:?}"
        );
    }

    let (answer, calls) = run_turn_capture_args(&cmd_tx, &mut evt_rx, prompt).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    drop(dir);
    for (name, args, result) in &calls {
        eprintln!(
            "→ {name}({}) => {}",
            args.chars().take(240).collect::<String>(),
            result.chars().take(240).collect::<String>()
        );
    }
    eprintln!("reply: {answer}");
    Some((ws, answer, calls))
}

/// The stage-0 probe, now on the real mechanism (docs/history/code-workspace.md §7): a
/// project that does not compile, and the assistant fixes it through
/// `code_edit`.
///
/// `rustc` is the ground truth on both sides — the fixture must fail to compile
/// before and succeed after, and the built program must print the right number,
/// so a "fix" that deletes the arithmetic cannot pass.
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 cargo test
/// code_edit_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn code_edit_e2e_live() {
    use crate::features::tools::code::CODE_EDIT_ID;
    let files = [
        (
            "src/main.rs",
            "mod stats;\n\nfn main() {\n    let samples = vec![2.0, 4.0, 6.0, 8.0];\n    println!(\"mean={}\", stats::mean(&samples));\n}\n",
        ),
        (
            "src/stats.rs",
            "/// Arithmetic mean of the samples.\npub fn mean(values: &[f64]) -> f64 {\n    let total: f64 = values.iter().sum();\n    total / values.len()\n}\n",
        ),
    ];
    let prompt = "Проект в рабочей папке не собирается. cargo build выдаёт:\n\n\
         error[E0277]: cannot divide `f64` by `usize`\n\
         \x20--> src/stats.rs:4:5\n\
         \x20 |\n\
         4 |     total / values.len()\n\
         \x20 |     ^^^^^^^^^^^^^^^^^^^^ no implementation for `f64 / usize`\n\n\
         Разберись и почини.";

    let Some((ws, _answer, calls)) = run_workspace_turn(&files, prompt).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    assert!(
        calls.iter().any(|(n, _, _)| n == CODE_EDIT_ID),
        "the model never called {CODE_EDIT_ID}"
    );
    match rustc_run(ws.path()) {
        Ok(stdout) => assert_eq!(
            stdout.trim(),
            "mean=5",
            "it builds, but no longer computes the right answer"
        ),
        Err(err) => panic!("does not compile after the edit:\n{err}"),
    }
}

/// Local files in their own encoding (docs/research/local-file-encoding.md): a
/// windows-1251 source attached as a project, and an edit the assistant makes through
/// `code_edit` — the path research §1 measured writing `EF BF BD` over every Russian
/// letter of such a file's comments while the changes screen showed one line.
///
/// Ground truth is the bytes, and what is asserted is the outcome, not the model's
/// wording: no `EF BF BD`, the untouched first line byte for byte, the file still reading
/// as windows-1251 without loss, and the number changed. Whether the model also rewrote
/// the comment about the number is printed, not asserted — that is a reasonable edit.
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 cargo test
/// code_edit_legacy_encoding_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn code_edit_legacy_encoding_e2e_live() {
    use crate::features::tools::code::CODE_EDIT_ID;
    // cyrillic-ok:start — the fixture is a Russian source file: its comments are the
    // text under test, and a string's continuation line reads to the scanner as a comment.
    const FIRST_LINE: &str = "// Расчёт скидки для постоянного покупателя.\r\n";
    const REST: &str = "pub fn discount(orders: u32) -> u32 {\r\n\
        \x20   // Скидка растёт с каждым десятым заказом.\r\n    orders / 10\r\n}\r\n";
    // cyrillic-ok:end
    let cp1251 = |t: &str| encoding_rs::WINDOWS_1251.encode(t).0.into_owned();
    let files = [("src/discount.rs", cp1251(&format!("{FIRST_LINE}{REST}")))];
    let prompt = "В файле src/discount.rs скидка растёт слишком медленно: пусть она растёт \
        с каждым пятым заказом, а не с каждым десятым. Поменяй это в коде.";

    let Some((ws, answer, calls)) = run_workspace_turn(&files, prompt).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let after = std::fs::read(ws.path().join("src/discount.rs")).unwrap();
    let (text, _) = encoding_rs::WINDOWS_1251.decode_without_bom_handling(&after);
    eprintln!("answer: {answer}\n--- the file after the turn, read as windows-1251 ---\n{text}");
    assert!(
        calls.iter().any(|(n, _, _)| n == CODE_EDIT_ID),
        "the model never called {CODE_EDIT_ID}"
    );
    let replacement = after
        .windows(3)
        .filter(|w| *w == [0xEF, 0xBF, 0xBD])
        .count();
    assert_eq!(replacement, 0, "EF BF BD was written into the file");
    assert!(
        after.starts_with(&cp1251(FIRST_LINE)),
        "the untouched first line changed"
    );
    assert!(
        crate::shared::text_decode::round_trips(&after, encoding_rs::WINDOWS_1251),
        "the file no longer reads as windows-1251 without loss"
    );
    assert!(
        text.contains("orders / 5"),
        "the divisor did not change to 5"
    );
    eprintln!(
        "the comment about the number was {}",
        if text.contains("десятым") {
            "left as it was"
        } else {
            "rewritten too"
        }
    );
}

/// Runs `cargo` in `dir` and returns its combined output, or an error.
fn cargo_in(dir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("cargo")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

/// Code workspace, **stage 3 live check** (docs/history/code-workspace.md §6): a project
/// that does not compile, a build command the *user* configured, and the
/// assistant working the loop — build, read the error, fix, build again.
///
/// `cargo` rather than a bare `rustc`, deliberately (docs/history/code-workspace.md
/// §4.1): only a real build system puts a `cargo → rustc` process tree behind
/// the timeout and the kill this stage exists for, and only a real compiler's
/// diagnostics are what the model has to read to find the fault. The crate has
/// no dependencies and builds `--offline`, so nothing here touches the network.
///
/// Ground truth is on **both** sides and is not a string comparison against the
/// source: the fixture must fail to build before the turn, and afterwards
/// `cargo run` must print the right number — so a "fix" that deletes the
/// arithmetic cannot pass (the stage-0 rule).
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 cargo test
/// code_build_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn code_build_e2e_live() {
    use crate::entities::workspace::CommandSlot;
    use crate::features::tools::code::{CODE_BUILD_ID, CODE_EDIT_ID};

    let files = [
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
        ),
        (
            "src/main.rs",
            "mod stats;\n\nfn main() {\n    let samples = vec![2.0, 4.0, 6.0, 8.0];\n    println!(\"mean={}\", stats::mean(&samples));\n}\n",
        ),
        (
            "src/stats.rs",
            "/// Arithmetic mean of the samples.\npub fn mean(values: &[f64]) -> f64 {\n    let total: f64 = values.iter().sum();\n    total / values.len()\n}\n",
        ),
    ];
    // The prompt says nothing about *what* is wrong and quotes no code: the
    // compiler error can only come from running the build command, which is the
    // whole point of the stage (docs/lessons.md §2 — a test worded so it can be
    // satisfied without doing the thing it checks measures nothing).
    let prompt = "Собери проект в рабочей папке. Если сборка падает — разберись, \
         почини и собери снова.";

    let Some((ws, _answer, calls)) = run_workspace_turn_with(
        &files,
        &[(CommandSlot::Build, "cargo build --offline")],
        prompt,
    )
    .await
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };

    let builds: Vec<&(String, String, String)> = calls
        .iter()
        .filter(|(n, _, _)| n == CODE_BUILD_ID)
        .collect();
    assert!(
        !builds.is_empty(),
        "the model never called {CODE_BUILD_ID} — it cannot have seen the error"
    );
    // The first build must have carried the compiler's own diagnostic: that is
    // what proves the command really ran and its stderr was captured, rather
    // than the model recognizing the bug by eye.
    assert!(
        builds[0].2.contains("E0277") || builds[0].2.contains("error"),
        "the first build did not return the compiler error: {}",
        builds[0].2
    );
    assert!(
        calls.iter().any(|(n, _, _)| n == CODE_EDIT_ID),
        "the model never called {CODE_EDIT_ID}"
    );
    // Reported rather than asserted: whether it rebuilt to confirm is a matter
    // of the model's judgement, and pinning it would pin the route rather than
    // the outcome (the §7.6 pattern).
    eprintln!("builds: {} · calls: {}", builds.len(), calls.len());

    match cargo_in(ws.path(), &["run", "--offline", "-q"]) {
        Ok(stdout) => assert!(
            stdout.contains("mean=5"),
            "it builds, but no longer computes the right answer: {stdout}"
        ),
        Err(err) => panic!("does not build after the turn:\n{err}"),
    }
}

/// The stage-3 safety property: a command line the user never typed cannot be
/// run. The tools take no arguments at all, so there is nothing for the model to
/// put a second command into — and a slot with no line is not offered.
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 cargo test
/// code_command_gate_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn code_command_gate_e2e_live() {
    use crate::features::tools::code::{CODE_BUILD_ID, CODE_RUN_ID, CODE_TEST_ID};

    let files = [("src/main.rs", "fn main() { println!(\"hi\"); }\n")];
    let prompt = "Запусти тесты этого проекта.";

    // No slot is configured, so none of the three tools exists this turn.
    let Some((_ws, answer, calls)) = run_workspace_turn(&files, prompt).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    for id in [CODE_BUILD_ID, CODE_RUN_ID, CODE_TEST_ID] {
        assert!(
            !calls.iter().any(|(n, _, _)| n == id),
            "{id} must not exist without a configured command: {calls:?}"
        );
    }
    assert!(!answer.trim().is_empty(), "the model said nothing at all");
}

/// Stage 2's own commitment (docs/history/code-workspace.md §7.3): the **refusal paths
/// have to work live**, and stage 0 never exercised them because the model never
/// missed.
///
/// So the miss is arranged: the user quotes the line to change, and their quote
/// is subtly not what the file says — a space that is not there. A model that
/// trusts the quote gets `tool.code.edit.not_found`, whose whole job is to say
/// what to do next. What is asserted is the **outcome** — the file ends up
/// correct — because a model that reads first and never misses is behaving
/// better, not worse; whether the miss happened is *reported*, so the rate is
/// visible across runs rather than assumed (docs/lessons.md §3).
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 cargo test
/// code_edit_recovers_from_a_miss_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn code_edit_recovers_from_a_miss_e2e_live() {
    use crate::features::tools::code::CODE_EDIT_ID;
    let files = [
        (
            "src/main.rs",
            "mod config;\n\nfn main() {\n    println!(\"retries={}\", config::RETRY_LIMIT);\n}\n",
        ),
        // The file says `u32 = 3`; the user's quote below says `u32=3`.
        (
            "src/config.rs",
            "/// How many times a request is retried.\npub const RETRY_LIMIT: u32 = 3;\n",
        ),
    ];
    let prompt = "Подними лимит ретраев с 3 до 5. Строка такая:\n\n\
         pub const RETRY_LIMIT: u32=3;\n\n\
         Поменяй её в проекте.";

    let Some((ws, _answer, calls)) = run_workspace_turn(&files, prompt).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let edits: Vec<&(String, String, String)> =
        calls.iter().filter(|(n, _, _)| n == CODE_EDIT_ID).collect();
    let refusal = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
        .tf("tool.code.edit.not_found", &[("path", "src/config.rs")]);
    let head: String = refusal.chars().take(30).collect();
    let missed = edits.iter().filter(|(_, _, r)| r.contains(&head)).count();
    eprintln!("PROBE: {} edit calls, {missed} of them missed", edits.len());

    assert!(!edits.is_empty(), "the model never called {CODE_EDIT_ID}");
    match rustc_run(ws.path()) {
        Ok(stdout) => assert_eq!(
            stdout.trim(),
            "retries=5",
            "the user's approximate quote must not cost them the change"
        ),
        Err(err) => panic!("does not compile after the edit:\n{err}"),
    }
}

/// A refused edit must leave the file **untouched** — live, not only in unit
/// tests: the ambiguity refusal is what stops a one-line fragment occurring
/// twice from being changed in the wrong place.
///
/// The fixture puts the obvious fragment in the file twice with identical
/// surroundings, and the request names which one to change. Whatever route the
/// model takes, exactly one of them must end up changed and the other left
/// alone.
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 cargo test
/// code_edit_ambiguity_e2e_live -- --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn code_edit_ambiguity_e2e_live() {
    let files = [
        (
            "src/main.rs",
            "mod limits;\n\nfn main() {\n    println!(\"{} {}\", limits::READ_TIMEOUT, limits::WRITE_TIMEOUT);\n}\n",
        ),
        (
            "src/limits.rs",
            "/// Reading.\npub const READ_TIMEOUT: u64 = 30;\n\n/// Writing.\npub const WRITE_TIMEOUT: u64 = 30;\n",
        ),
    ];
    let prompt = "В проекте таймаут записи должен быть 60, а таймаут чтения оставь как есть. \
         Поправь.";

    let Some((ws, _answer, _calls)) = run_workspace_turn(&files, prompt).await else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let after = std::fs::read_to_string(ws.path().join("src/limits.rs")).unwrap();
    eprintln!("PROBE: src/limits.rs now:\n{after}");
    assert!(
        after.contains("WRITE_TIMEOUT: u64 = 60"),
        "the write timeout had to change: {after}"
    );
    assert!(
        after.contains("READ_TIMEOUT: u64 = 30"),
        "the read timeout had to be left alone — a blind replace_all would take both: {after}"
    );
}

/// The sub-agent track's **go/no-go** (docs/research/subagent-chats.md §6):
/// the main agent delegates a question only a tool can answer, the sub-agent
/// actually *uses* the tool, and the answer comes back through the delegation.
/// The planted token is nonsense the model cannot know, and the parent is told
/// not to read the file itself — a parent that shortcuts fails the
/// `call_subagent` assertion, which is the no-go signal this smoke exists for.
/// `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL)"]
async fn subagent_with_tools_e2e_live() {
    let sandbox = tempfile::tempdir().unwrap();
    let file = sandbox.path().join("facts.txt");
    std::fs::write(
        &file,
        "Internal note.\nThe project's secret codename is ZARNOVIK-7741.\nDo not share outside the team.\n",
    )
    .unwrap();
    let mut cfg = AppConfig::default();
    cfg.tools.fs_enabled = true;
    cfg.tools.fs_root = Some(sandbox.path().to_string_lossy().to_string());
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Delegator".into(),
            system_message: "You are a coordinator. Reply in English. You never read files \
                 yourself: whenever a file has to be read, you delegate the whole task to a \
                 sub-agent with the call_subagent tool, giving it the exact file path and \
                 telling it to use fs_read, then you report what the sub-agent found."
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
                // The delegation and the one tool the delegate needs — nothing
                // else can be reached for. `fs_read` has to be in the parent's
                // set because the child's set is derived from it (research §3.3).
                enabled_tools: Some(vec!["call_subagent".to_string(), "fs_read".to_string()]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let chat_id = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .and_then(|e| match e {
            AppEvent::ChatActivated { id, .. } => Some(id),
            _ => None,
        })
        .unwrap();

    let ask = format!(
        "What is the project's secret codename? It is written in the file {}. \
         Do not read it yourself — delegate to a sub-agent (call_subagent) and \
         tell it to read that file with fs_read. Then answer with the codename.",
        file.display()
    );
    let (reply, calls) = run_turn_capture_args(&cmd_tx, &mut evt_rx, &ask).await;
    eprintln!("reply: {reply}");
    for (n, a, r) in &calls {
        eprintln!(
            "call {n}({}) -> {}",
            a.chars().take(160).collect::<String>(),
            r.chars().take(200).collect::<String>()
        );
    }
    // Let the debounced save land before reading the file back.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // GO: the parent delegated (the feed card is the parent's only call)...
    let delegated = calls
        .iter()
        .filter(|(n, _, _)| n == "call_subagent")
        .count();
    assert!(delegated >= 1, "the parent never delegated: {calls:?}");
    assert!(
        !calls.iter().any(|(n, _, _)| n == "fs_read"),
        "the parent read the file itself instead of delegating"
    );
    // ...the sub-agent used the tool (its calls are on the run, not in the feed)...
    let chat = Storage::open(Paths::with_root(_d.path()))
        .unwrap()
        .json()
        .load_chat(chat_id)
        .unwrap()
        .unwrap();
    let run = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find_map(|r| r.subagent.as_deref())
        .expect("a run on the call's record");
    eprintln!(
        "run «{}»: {} messages, outcome {:?}, {} tokens",
        run.title,
        run.messages.len(),
        run.outcome,
        run.tokens
    );
    let child_reads = run
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .filter(|r| r.name == "fs_read")
        .count();
    assert!(
        child_reads >= 1,
        "the sub-agent did not use fs_read: {:?}",
        run.messages
    );
    assert_eq!(
        run.outcome,
        Some(crate::entities::subagent::RunOutcome::Completed)
    );
    // ...and the token came through the delegation into the parent's answer.
    assert!(
        reply.contains("ZARNOVIK-7741") || reply.contains("ZARNOVIK"),
        "the codename did not reach the parent's reply: {reply}"
    );
}

/// End-to-end against a live model (spec §6.4): a reply cancelled mid-stream,
/// then `/continue` — the resumed turn appends into the same message, and the
/// concatenation of everything the two turns streamed equals the stored text
/// byte-for-byte. That single equality carries the whole feature: the echo
/// filter proved itself against the real server (the llama.cpp path echoes
/// the prefill — research §7.1), nothing doubled, nothing was lost at the
/// seam, and the merge landed in place rather than as a second message.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn continue_e2e_live() {
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(no_auto_cfg()) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage(
            "Count from one to thirty in words, one number per line, no other text.".into(),
        ))
        .unwrap();
    // Cancel after the first visible text chunk — a thoughts-only cut would be
    // refused (fork F4), and a thinking model fronts its reasoning.
    let mut streamed = String::new();
    let mut cancelled = false;
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::Chunk { text, .. } => {
                streamed.push_str(&text);
                if !cancelled {
                    cmd_tx.send(AppCommand::Cancel).unwrap();
                    cancelled = true;
                }
            }
            AppEvent::Finished { reason, .. } => {
                assert_eq!(
                    reason,
                    FinishReason::Cancelled,
                    "the reply finished before the cancel landed — the fixture \
                     needs a longer task, not a different feature"
                );
                break;
            }
            _ => {}
        }
    }
    assert!(cancelled, "the model never produced text to cut");
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();

    cmd_tx.send(AppCommand::ContinueLast).unwrap();
    let mut resumed = false;
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::GenerationStarted { continuation, .. } => {
                assert!(continuation, "a text tail must resume via prefill");
                resumed = true;
            }
            AppEvent::Chunk { text, .. } => streamed.push_str(&text),
            AppEvent::Finished { reason, .. } => {
                assert!(
                    matches!(reason, FinishReason::Stop | FinishReason::Length),
                    "the resumed turn must end on its own: {reason:?}"
                );
                break;
            }
            _ => {}
        }
    }
    assert!(resumed, "the continuation turn never started");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(dir.path())).unwrap();
    let chat = reopened
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        chat.messages.len(),
        2,
        "the continuation must land in place, not as a second reply: {:?}",
        chat.messages
            .iter()
            .map(|m| (m.role, m.text.chars().take(30).collect::<String>()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        chat.messages[1].text, streamed,
        "the stored reply must be exactly what streamed across both turns — \
         no echo doubling, no gap at the seam"
    );
    let head: String = streamed.chars().take(160).collect();
    println!(
        "continued reply, {} chars, head: {head:?}",
        streamed.chars().count()
    );
}

/// `run_dialogue` stage-1 go/no-go (spec §9.13,
/// docs/research/two-agent-dialogue.md §6): asked for a short finite scene,
/// the model stages it through the tool — the probe's café fixture graduated
/// into the smoke — and the landed record carries a role-encoded,
/// strictly-alternating transcript that the director ended (or the cap
/// backstopped). The profile is narrowed to the one tool under test
/// ("remove the alternative", lessons §9); the ask is situation-shaped, not
/// a numbered script.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL)"]
async fn dialogue_e2e_live() {
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(AppConfig::default()) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_profile, chat_id) =
        narrow_profile_to(&cmd_tx, &mut evt_rx, vec!["run_dialogue".into()]).await;
    let ask = "I'm drafting a café scene. Stage it live with run_dialogue: \
        barista Mara (warm, frazzled by the morning rush, remakes wrong drinks \
        for free) and customer Jonas (in a hurry for his tram, got an oat \
        latte instead of his double espresso; opens the dialogue politely \
        asking to fix it). Tell each persona to reply with one spoken line \
        only, no narration. Direct it yourself and stop once the mix-up is \
        resolved and they part on good terms; cap it at 10 lines.";
    let (reply, calls) = run_turn_capture(&cmd_tx, &mut evt_rx, ask).await;
    eprintln!(
        "parent reply: {}",
        reply.chars().take(300).collect::<String>()
    );
    // The tool was actually exercised (lessons §2 — a run that never calls it
    // measures nothing).
    assert!(
        calls.iter().any(|(n, _)| n == "run_dialogue"),
        "the model never called run_dialogue; calls: {calls:?}"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = super::subagent::load(dir.path(), chat_id);
    let record = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|r| r.name == "run_dialogue")
        .expect("the call's record");
    let run = record.subagent.as_deref().expect("the run on the record");
    assert_eq!(run.kind, crate::entities::subagent::RunKind::Dialogue);
    assert_eq!(run.participants.len(), 2);
    let outcome = run.outcome.expect("the run reported how it ended");
    assert!(
        matches!(
            outcome,
            crate::entities::subagent::RunOutcome::Completed
                | crate::entities::subagent::RunOutcome::RoundLimit
        ),
        "unexpected outcome: {outcome:?}"
    );
    // The transcript: at least one exchange, spoken lines strictly
    // alternating around the director's System rows.
    let spoken: Vec<MessageRole> = run
        .messages
        .iter()
        .filter(|m| m.role != MessageRole::System)
        .map(|m| m.role)
        .collect();
    assert!(
        spoken.len() >= 3,
        "too short: {} spoken lines",
        spoken.len()
    );
    for pair in spoken.windows(2) {
        assert_ne!(pair[0], pair[1], "two consecutive lines by one side");
    }
    println!(
        "dialogue landed: {:?}, {} spoken lines, {} tokens, title {:?}",
        outcome,
        spoken.len(),
        run.tokens,
        run.title
    );
    for m in &run.messages {
        let side = match m.role {
            MessageRole::Assistant => "A",
            MessageRole::User => "B",
            _ => "D",
        };
        println!(
            "  [{side}] {}",
            m.text.chars().take(120).collect::<String>()
        );
    }
}

/// `start_dialogue` go/no-go (spec §9.13, docs/research/background-dialogues.md
/// §7): asked for a scene it will read later **and** for something it can
/// answer now, the model stages the scene in the background and answers the
/// rest in the same reply; the scene outlives that turn, lands on the record
/// by id, and its closing result arrives as a task notification the app's own
/// woken turn reports. The profile carries both dialogue tools, so choosing
/// the background one is the model's decision and not a lack of alternatives
/// — the one place this smoke deliberately departs from "remove the
/// alternative" (lessons §9), because the choice *is* what is measured.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL)"]
async fn background_dialogue_e2e_live() {
    let mut cfg = AppConfig::default();
    cfg.tools.subagent_background = true;
    // Two sessions, so the scene and the parent's turns need not queue behind
    // each other on a stack that has the room; the budget is what decides.
    cfg.engine.external.sessions = 2;
    cfg.engine.managed.sessions = 2;
    // The measured ceiling for an open-ended ask on a thinking model
    // (docs/journal/ci.md; lessons §9): at the default 2048 this smoke's
    // two-part question spent the whole cap in `reasoning_content` and the
    // parent said nothing at all, twice in a row.
    cfg.default_sampling.max_tokens = Some(4096);
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let Some((dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (_profile, chat_id) = narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        vec!["start_dialogue".into(), "run_dialogue".into()],
    )
    .await;

    let ask = "Two things. (1) Stage me a café scene I will read later: barista \
        Mara (warm, frazzled by the morning rush) and customer Jonas (in a hurry \
        for his tram, got an oat latte instead of his double espresso; he opens \
        by asking politely to fix it). Tell each persona to reply with one spoken \
        line only, no narration. Direct it yourself, stop once the mix-up is \
        resolved, cap it at 6 lines — and do not wait for it, I want the \
        transcript later, not now. (2) Right now: which is larger, 17 × 23 or 400?";
    let (reply, calls) = run_turn_capture(&cmd_tx, &mut evt_rx, ask).await;
    eprintln!(
        "parent reply: {}",
        reply.chars().take(300).collect::<String>()
    );
    for (n, a) in &calls {
        eprintln!("call {n}({})", a.chars().take(160).collect::<String>());
    }
    assert!(
        !reply.trim().is_empty() || !calls.is_empty(),
        "the parent turn produced nothing at all — the all-thinking empty turn \
         (lessons §9), not a verdict on the tool: re-run before reading it as one"
    );
    assert!(
        calls.iter().any(|(n, _)| n == "start_dialogue"),
        "the model did not stage the scene in the background; calls: {calls:?}"
    );
    assert!(
        !calls.iter().any(|(n, _)| n == "run_dialogue"),
        "the model waited for the scene instead of backgrounding it: {calls:?}"
    );

    // The scene ends and the app wakes the assistant on the notification: a
    // turn nobody sent a message for.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(900);
    let mut wake = String::new();
    loop {
        let ev = tokio::time::timeout_at(deadline, evt_rx.recv())
            .await
            .expect("the scene lands and the wake turn ends within fifteen minutes")
            .expect("the event stream stays open");
        match ev {
            AppEvent::Chunk { text, .. } => wake.push_str(&text),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    eprintln!("wake reply: {wake}");
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = super::subagent::load(dir.path(), chat_id);
    let run = chat
        .children()
        .find(|r| r.background)
        .expect("a background run on the record");
    assert_eq!(run.kind, crate::entities::subagent::RunKind::Dialogue);
    assert_eq!(run.participants.len(), 2);
    let outcome = run.outcome.expect("the scene reported how it ended");
    assert!(
        matches!(
            outcome,
            crate::entities::subagent::RunOutcome::Completed
                | crate::entities::subagent::RunOutcome::RoundLimit
        ),
        "unexpected outcome: {outcome:?}"
    );
    let spoken: Vec<MessageRole> = run
        .messages
        .iter()
        .filter(|m| m.role != MessageRole::System)
        .map(|m| m.role)
        .collect();
    assert!(
        spoken.len() >= 3,
        "too short: {} spoken lines",
        spoken.len()
    );
    for pair in spoken.windows(2) {
        assert_ne!(pair[0], pair[1], "two consecutive lines by one side");
    }
    println!(
        "scene landed: {:?}, {} spoken lines, {} tokens, title {:?}",
        outcome,
        spoken.len(),
        run.tokens,
        run.title
    );

    // The notification is the dialogue's, names the scene, and carries the
    // closing result; the woken reply is about the scene rather than empty.
    let note = chat
        .messages
        .iter()
        .find(|m| m.is_notification())
        .expect("a task notification row");
    println!("notification: {}", note.text);
    assert_eq!(note.notification, Some(run.id));
    assert!(
        note.text.contains(&run.title),
        "the notification does not name the scene: {}",
        note.text
    );
    assert!(
        !wake.trim().is_empty(),
        "the woken turn said nothing at all — the empty-turn mode (research §3)"
    );
}

/// The parallel group's live proof (docs/research/parallel-subagents.md §7):
/// the parent is asked for two codenames in two files and told to delegate
/// **both** reads in one reply; with `subagent_parallel = 2` and two sessions
/// the two runs must overlap in time — the second started before the first
/// finished — and both must land on the one assistant message, each having
/// used `fs_read`, with both tokens in the parent's reply. A parent that
/// delegates one at a time (two rounds) fails the overlap assertion, which is
/// the no-go signal this smoke exists for. The dashes are folded on both
/// sides: a model may render a code with a non-breaking hyphen
/// (docs/lessons.md §9). `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL)"]
async fn parallel_subagents_e2e_live() {
    let sandbox = tempfile::tempdir().unwrap();
    let file_a = sandbox.path().join("alpha.txt");
    let file_b = sandbox.path().join("beta.txt");
    std::fs::write(
        &file_a,
        "Internal note.\nThe alpha codename is ZARNOVIK-7741.\n",
    )
    .unwrap();
    std::fs::write(
        &file_b,
        "Internal note.\nThe beta codename is KELVAR-3390.\n",
    )
    .unwrap();
    let mut cfg = AppConfig::default();
    cfg.tools.fs_enabled = true;
    cfg.tools.fs_root = Some(sandbox.path().to_string_lossy().to_string());
    cfg.tools.subagent_parallel = 2;
    cfg.engine.external.sessions = 2;
    cfg.engine.managed.sessions = 2;
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Delegator".into(),
            system_message: "You are a coordinator. Reply in English. You never read files \
                 yourself: whenever files have to be read, you delegate each file to its own \
                 sub-agent with the call_subagent tool — several call_subagent calls in ONE \
                 reply when there are several files — giving each the exact file path and \
                 telling it to use fs_read, then you report what the sub-agents found."
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
                enabled_tools: Some(vec!["call_subagent".to_string(), "fs_read".to_string()]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let chat_id = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .and_then(|e| match e {
            AppEvent::ChatActivated { id, .. } => Some(id),
            _ => None,
        })
        .unwrap();

    let ask = format!(
        "What are the alpha and beta codenames? The alpha one is in the file {} and the \
         beta one in {}. Do not read them yourself — delegate each file to its own \
         sub-agent (call_subagent), both in this same reply, and tell each to read its \
         file with fs_read. Then answer with both codenames.",
        file_a.display(),
        file_b.display()
    );
    let (reply, calls) = run_turn_capture_args(&cmd_tx, &mut evt_rx, &ask).await;
    eprintln!("reply: {reply}");
    for (n, a, r) in &calls {
        eprintln!(
            "call {n}({}) -> {}",
            a.chars().take(120).collect::<String>(),
            r.chars().take(160).collect::<String>()
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let delegated = calls
        .iter()
        .filter(|(n, _, _)| n == "call_subagent")
        .count();
    assert!(
        delegated >= 2,
        "the parent did not delegate twice: {calls:?}"
    );
    assert!(
        !calls.iter().any(|(n, _, _)| n == "fs_read"),
        "the parent read a file itself instead of delegating"
    );
    let chat = Storage::open(Paths::with_root(_d.path()))
        .unwrap()
        .json()
        .load_chat(chat_id)
        .unwrap()
        .unwrap();
    // Both runs on one assistant message — one reply, one group.
    let grouped: Vec<&crate::entities::subagent::SubagentRun> = chat
        .messages
        .iter()
        .filter_map(|m| {
            let runs: Vec<_> = m
                .tool_calls
                .iter()
                .filter_map(|r| r.subagent.as_deref())
                .collect();
            (runs.len() >= 2).then_some(runs)
        })
        .next()
        .expect("two runs on one assistant message");
    for run in &grouped {
        eprintln!(
            "run «{}»: {} messages, outcome {:?}, {:?} → {:?}",
            run.title,
            run.messages.len(),
            run.outcome,
            run.created_at,
            run.finished_at
        );
        let reads = run
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .filter(|r| r.name == "fs_read")
            .count();
        assert!(
            reads >= 1,
            "the sub-agent «{}» never used fs_read",
            run.title
        );
    }
    // GO: the second run started before the first finished — they ran at once.
    let first_end = grouped[0].finished_at.expect("the first run ended");
    assert!(
        grouped[1].created_at < first_end,
        "the runs did not overlap: {:?} vs {:?}",
        grouped[1].created_at,
        first_end
    );
    let fold = |s: &str| s.replace(['\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}'], "-");
    let reply = fold(&reply);
    assert!(
        reply.contains("ZARNOVIK-7741"),
        "alpha token missing: {reply}"
    );
    assert!(reply.contains("KELVAR-3390"), "beta token missing: {reply}");
}

/// The concurrent segment on a live model (docs/research/concurrent-tools.md
/// §7): two planted-token files, a profile with `fs_read` alone, the
/// assistant told to read both **in one reply**, `concurrent_calls = 2`.
/// GO: two `fs_read` records on one assistant message, each holding the
/// result of its own call; both cards opened before either closed — the
/// overlap proof, since the segment opens every card first and the
/// sequential path never does (it closes the cards at the round's end,
/// after every call ran, but opens the second only after the first ran);
/// and both codenames in the reply.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL)"]
async fn concurrent_tools_e2e_live() {
    let sandbox = tempfile::tempdir().unwrap();
    let file_a = sandbox.path().join("alpha.txt");
    let file_b = sandbox.path().join("beta.txt");
    std::fs::write(
        &file_a,
        "Internal note.\nThe alpha codename is ZARNOVIK-7741.\n",
    )
    .unwrap();
    std::fs::write(
        &file_b,
        "Internal note.\nThe beta codename is KELVAR-3390.\n",
    )
    .unwrap();
    let mut cfg = AppConfig::default();
    cfg.tools.fs_enabled = true;
    cfg.tools.fs_root = Some(sandbox.path().to_string_lossy().to_string());
    cfg.engine.external.concurrent_calls = 2;
    cfg.engine.managed.concurrent_calls = 2;
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Clerk".into(),
            system_message: "You are a file clerk. Reply in English. When asked about several \
                 files, read them all with fs_read in ONE reply — one fs_read call per file, \
                 all in the same message — and only then answer."
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
                enabled_tools: Some(vec!["fs_read".to_string()]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let chat_id = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .and_then(|e| match e {
            AppEvent::ChatActivated { id, .. } => Some(id),
            _ => None,
        })
        .unwrap();

    let ask = format!(
        "What are the alpha and beta codenames? The alpha one is in the file {} and the \
         beta one in {}. Read both files with fs_read in this same reply, then answer \
         with both codenames.",
        file_a.display(),
        file_b.display()
    );
    cmd_tx.send(AppCommand::SendMessage(ask)).unwrap();
    let mut reply = String::new();
    let mut opens: Vec<usize> = Vec::new();
    let mut closes: Vec<usize> = Vec::new();
    let mut n = 0usize;
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::Chunk { text, .. } => reply.push_str(text),
            AppEvent::ToolCallStarted { name, .. } if name == "fs_read" => opens.push(n),
            AppEvent::ToolCall {
                name,
                arguments,
                result,
                ..
            } if name == "fs_read" => {
                closes.push(n);
                eprintln!(
                    "call fs_read({}) -> {}",
                    arguments.chars().take(100).collect::<String>(),
                    result.chars().take(100).collect::<String>()
                );
            }
            AppEvent::Finished { .. } => break,
            _ => {}
        }
        n += 1;
    }
    eprintln!("reply: {reply}");
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        opens.len() >= 2,
        "the model did not read both files in one reply (opens {opens:?})"
    );
    assert_eq!(opens.len(), closes.len(), "every card closed");
    // GO: the second card opened before the first closed — one segment.
    assert!(
        opens[1] < closes[0],
        "the reads did not run as one segment: opens {opens:?}, closes {closes:?}"
    );
    let chat = Storage::open(Paths::with_root(_d.path()))
        .unwrap()
        .json()
        .load_chat(chat_id)
        .unwrap()
        .unwrap();
    let round = chat
        .messages
        .iter()
        .find(|m| m.tool_calls.iter().filter(|r| r.name == "fs_read").count() >= 2)
        .expect("two fs_read records on one assistant message");
    // Each record holds the result of its own call, whatever order they landed.
    for r in round.tool_calls.iter().filter(|r| r.name == "fs_read") {
        let path = r.arguments["path"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        let result = r.result.as_deref().unwrap_or_default();
        if path.contains("alpha") {
            assert!(result.contains("ZARNOVIK-7741"), "{path}: {result}");
        } else if path.contains("beta") {
            assert!(result.contains("KELVAR-3390"), "{path}: {result}");
        } else {
            panic!("a read of an unexpected path: {path}");
        }
    }
    let fold = |s: &str| s.replace(['\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}'], "-");
    let reply = fold(&reply);
    assert!(
        reply.contains("ZARNOVIK-7741"),
        "alpha token missing: {reply}"
    );
    assert!(reply.contains("KELVAR-3390"), "beta token missing: {reply}");
}

use futures_util::StreamExt as _;

// ---------------------------------------------------------------------------
// Admission by budget (docs/research/admission-by-budget.md §7): the guard
// against the unified pool's collective failure, driven through the app
// against a real `llama-server` small enough to overflow on purpose.
// ---------------------------------------------------------------------------

/// The parent's rounds scripted, the children's live: requests whose system
/// message carries the children's persona go to the real server (and are
/// counted — how many streams the server saw open at once), every other
/// request plays the next script. The small model's willingness to delegate
/// twice is not what the smoke measures, so it is taken out of the picture.
struct ScriptedParent {
    live: Arc<dyn EngineBackend>,
    scripts: std::sync::Mutex<std::collections::VecDeque<Vec<ChatChunk>>>,
    /// A request whose system message carries any of these goes to the live
    /// server (a child's persona, a compaction roll's prompt); every other
    /// request plays the next script.
    live_markers: &'static [&'static str],
    /// When the scripts run out, a request goes to the live server instead
    /// of playing an empty reply — a smoke whose *measured* turn is live
    /// after a scripted seed.
    live_after_scripts: bool,
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
    max_in_flight: Arc<std::sync::atomic::AtomicUsize>,
}

struct LiveOpen(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for LiveOpen {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl EngineBackend for ScriptedParent {
    async fn chat_stream(
        &self,
        req: crate::shared::api::ChatRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::shared::api::contract::ChatStream> {
        use std::sync::atomic::Ordering::SeqCst;
        let by_marker = req
            .system
            .as_deref()
            .is_some_and(|s| self.live_markers.iter().any(|m| s.contains(m)));
        let script = if by_marker {
            None
        } else {
            self.scripts.lock().unwrap().pop_front()
        };
        if let Some(chunks) = script {
            return Ok(Box::pin(futures_util::stream::iter(chunks)));
        }
        if by_marker || self.live_after_scripts {
            let open = self.in_flight.fetch_add(1, SeqCst) + 1;
            self.max_in_flight.fetch_max(open, SeqCst);
            let guard = LiveOpen(self.in_flight.clone());
            let mut inner = self.live.chat_stream(req, cancel).await?;
            let s = async_stream::stream! {
                let _open = guard;
                while let Some(chunk) = inner.next().await {
                    yield chunk;
                }
            };
            return Ok(Box::pin(s));
        }
        Ok(Box::pin(futures_util::stream::iter(vec![
            ChatChunk::Finished(FinishReason::Stop),
        ])))
    }

    async fn context_budget(&self) -> Option<u32> {
        self.live.context_budget().await
    }

    async fn parallel_slots(&self) -> Option<u32> {
        self.live.parallel_slots().await
    }
}

const READER: &str = "You are a reader.";

/// `paragraphs` paragraphs of ~30 tokens each on both gate tokenizers
/// (measured: 36 of them are 1150 tokens with the persona on Gemma 3),
/// distinct per `tag` so no prefix is shared between two archives and the
/// cache cannot help either.
fn archive(tag: &str, paragraphs: usize) -> String {
    (0..paragraphs)
        .map(|i| format!("Paragraph {i} of the {tag} archive describes a lighthouse keeper's ordinary evening: the lamp is lit, the log is written, the tide is noted."))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The delegator's chat, as the pool smokes open it: the orchestrator on
/// `cfg` over `backend`, the server's slot count waited for (external mode's
/// pool rule needs it in hand), an English "Delegator" profile with `tool`
/// enabled, and a fresh chat on that profile. Returns the orchestrator's
/// handles and the chat's id.
async fn delegator_chat(
    backend: Arc<dyn EngineBackend>,
    cfg: AppConfig,
    tool: &str,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
    Uuid,
) {
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), cfg);
    let reported = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::EngineSlots(Some(_)))),
    )
    .await
    .ok()
    .flatten();
    eprintln!("slots reported to the orchestrator: {reported:?}");

    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Delegator".into(),
            system_message: "You coordinate readers.".into(),
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
                enabled_tools: Some(vec![tool.to_string()]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let chat_id = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .and_then(|e| match e {
            AppEvent::ChatActivated { id, .. } => Some(id),
            _ => None,
        })
        .unwrap();
    (dir, cmd_tx, evt_rx, handle, chat_id)
}

/// The live server with several slots over one pool, as `/props` reports
/// them — `(backend, slots, pool)` — or `None` with the reason printed: no
/// server, no `/props`, or one slot, where `what` (a pool-sharing smoke's
/// subject) has nothing to measure.
async fn pooled_live(what: &str) -> Option<(Arc<dyn EngineBackend>, u32, u64)> {
    let live = live_backend()?;
    let slots = live.parallel_slots().await;
    let pool = live.context_budget().await;
    let (Some(slots), Some(pool)) = (slots, pool) else {
        eprintln!("skip: the server reports slots {slots:?}, pool {pool:?}");
        return None;
    };
    if slots < 2 {
        eprintln!("skip: the server reports {slots} slot(s); {what} needs two or more");
        return None;
    }
    Some((live, slots, pool as u64))
}

/// One arm of the admission smoke, sized to the pool the server reports.
struct AdmissionArm {
    /// How many readers the parent delegates in its one reply.
    children: usize,
    /// The engine section's `sessions`, and `tools.subagent_parallel` with it.
    sessions: u32,
    /// Each child's prompt as a fraction of the pool (the planted archive).
    share: f64,
    /// `None`: the app believes the pool the server reports; `Some(k)`: it is
    /// told the pool is `k` times that — the control arm's lie.
    lie: Option<u64>,
}

const CODES: [&str; 4] = ["ZARNOVIK-7741", "KELVAR-3390", "MORVAX-5518", "TELUNE-8827"];
const TAGS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];

struct AdmissionResult {
    /// Per child, in the model's order: the run's outcome and its final reply.
    runs: Vec<(Option<crate::entities::subagent::RunOutcome>, String)>,
    /// The most live streams the server saw open at once.
    most: usize,
}

/// One run of the admission smoke: `arm.children` readers, each handed an
/// archive of `arm.share` of the pool with a codename in it and asked for the
/// codename, under a scripted parent that believes the pool is what the
/// server reports (or `arm.lie` times that). Sized from `/props`, so the same
/// arms run on the CPU build's 2048-token pool and the LAN stack's 16384.
/// `None` when there is no live server, or when it reports at most one slot
/// — the guard is off by design below two (`pool.rs`), and one slot queues.
async fn admission_smoke(arm: AdmissionArm) -> Option<AdmissionResult> {
    let (live, slots, pool) = pooled_live("the guard").await?;
    // A paragraph is ~30 tokens on both gate tokenizers (measured: 36 of them
    // are 1150 tokens with the persona on Gemma 3). The planted text rides the
    // message rather than a file, since the CPU build's Gemma 3 template drops
    // tools (`supports_tools: false` on `/props`), and the pool does not care
    // where the tokens came from. The reply cap is an eighth of the pool.
    let paragraphs = ((arm.share * pool as f64) / 30.0) as usize;
    let max_tokens = (pool / 8) as usize;
    let belief = pool * arm.lie.unwrap_or(1);
    eprintln!(
        "server: {slots} slots over {pool}; {} children × {paragraphs} paragraphs, cap {max_tokens}, belief {belief}",
        arm.children
    );
    let filler = |tag: &str| archive(tag, paragraphs);
    let delegate = |index: usize| {
        let (tag, code) = (TAGS[index], CODES[index]);
        ChatChunk::ToolCall(crate::shared::api::contract::ToolCallDelta {
            thought_signature: None,
            index,
            id: Some(format!("c{index}")),
            name: Some("call_subagent".into()),
            arguments: serde_json::json!({
                "name": format!("{tag} reader"),
                "system_message": format!("{READER} You are given an archive; report the codename it contains in one short sentence, quoting it exactly."),
                "message": format!("{}\nThe {tag} codename is {code}.\n\nWhat is the codename?", filler(tag)),
            })
            .to_string(),
        })
    };
    let mut first: Vec<ChatChunk> = (0..arm.children).map(delegate).collect();
    first.push(ChatChunk::Finished(FinishReason::ToolCalls));
    let backend = Arc::new(ScriptedParent {
        live,
        scripts: std::sync::Mutex::new(
            vec![
                first,
                vec![
                    ChatChunk::Text("All readers reported.".into()),
                    ChatChunk::Finished(FinishReason::Stop),
                ],
            ]
            .into(),
        ),
        live_markers: &[READER],
        live_after_scripts: false,
        in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });

    let mut cfg = AppConfig::default();
    cfg.tools.subagent_parallel = arm.sessions;
    cfg.tools.subagent_max_tokens = max_tokens;
    cfg.default_sampling.max_tokens = Some(max_tokens);
    cfg.engine.external.sessions = arm.sessions;
    cfg.engine.managed.sessions = arm.sessions;
    // The pool the app believes in — the explicit window outranks what the
    // server reports (pool.rs). Compaction is off so the window is the
    // pool's and nothing else's.
    cfg.compaction.context_tokens = Some(belief as usize);
    cfg.compaction.enabled = false;
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let (dir, cmd_tx, mut evt_rx, handle, chat_id) = delegator_chat(
        backend.clone() as Arc<dyn EngineBackend>,
        cfg,
        "call_subagent",
    )
    .await;

    let started = std::time::Instant::now();
    let (reply, calls) =
        run_turn_capture_args(&cmd_tx, &mut evt_rx, "Ask every reader for its codename.").await;
    eprintln!("reply: {reply} ({:.1} s)", started.elapsed().as_secs_f64());
    for (n, a, r) in &calls {
        eprintln!(
            "call {n}({}) -> {}",
            a.chars().take(60).collect::<String>(),
            r.chars().take(160).collect::<String>()
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chat(chat_id)
        .unwrap()
        .unwrap();
    let runs: Vec<_> = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .filter_map(|r| r.subagent.as_deref())
        .map(|run| {
            eprintln!(
                "run «{}»: {} messages, outcome {:?}, reply {:?}",
                run.title,
                run.messages.len(),
                run.outcome,
                run.final_reply()
                    .map(|r| r.chars().take(120).collect::<String>())
            );
            (
                run.outcome,
                run.final_reply().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let most = backend
        .max_in_flight
        .load(std::sync::atomic::Ordering::SeqCst);
    eprintln!("most live streams open at once: {most}");
    Some(AdmissionResult { runs, most })
}

/// A model may render a code with a non-breaking or typographic hyphen
/// (docs/lessons.md §9); fold before comparing.
fn fold_dashes(s: &str) -> String {
    s.replace(['\u{2011}', '\u{2010}', '\u{2012}', '\u{2013}'], "-")
}

fn assert_all_completed_with_their_codes(
    runs: &[(Option<crate::entities::subagent::RunOutcome>, String)],
) {
    assert!(
        runs.iter()
            .all(|(o, _)| *o == Some(crate::entities::subagent::RunOutcome::Completed)),
        "every child completes under the guard: {runs:?}"
    );
    for (i, (_, reply)) in runs.iter().enumerate() {
        assert!(
            fold_dashes(reply).contains(CODES[i]),
            "{} code missing: {reply}",
            TAGS[i]
        );
    }
}

/// **The guard, live** (research §7): against a `llama-server` whose slots
/// share one pool (`-np N --kv-unified` — the local CPU build at `-c 2048`
/// with Gemma 3 4B, or the LAN stack at 16384), two children whose prompts
/// (55% of the pool each) do not fit together both complete and both report
/// their codename — the budget made them take turns where the server would
/// have ended both (the control arm below). `#[ignore]`, manual.
#[tokio::test]
#[ignore = "requires a live chat server launched with -np N --kv-unified (MINDFORK_ENGINE_URL)"]
async fn admission_e2e_live() {
    let Some(res) = admission_smoke(AdmissionArm {
        children: 2,
        sessions: 2,
        share: 0.55,
        lie: None,
    })
    .await
    else {
        eprintln!("skipped (see above)");
        return;
    };
    assert_eq!(res.runs.len(), 2, "two runs: {:?}", res.runs);
    assert_all_completed_with_their_codes(&res.runs);
    assert_eq!(
        res.most, 1,
        "two reservations above half the pool do not fit: the children took turns"
    );
}

/// **The control arm**: the same run with the app told the pool is four
/// times what it is — the guard admits both prompts, the real pool cannot
/// hold them, and the server ends the running conversations together
/// (research §3.2): at least one child does not complete. This is what
/// proves the guarded run above did something rather than the server being
/// polite. `#[ignore]`, manual, the same server as above.
#[tokio::test]
#[ignore = "requires a live chat server launched with -np N --kv-unified (MINDFORK_ENGINE_URL)"]
async fn admission_control_e2e_live() {
    let Some(res) = admission_smoke(AdmissionArm {
        children: 2,
        sessions: 2,
        share: 0.55,
        lie: Some(4),
    })
    .await
    else {
        eprintln!("skipped (see above)");
        return;
    };
    assert_eq!(res.runs.len(), 2, "two runs: {:?}", res.runs);
    assert_eq!(res.most, 2, "unguarded, both children streamed at once");
    assert!(
        res.runs
            .iter()
            .any(|(o, _)| *o != Some(crate::entities::subagent::RunOutcome::Completed)),
        "the lie about the pool went unpunished — the server did not overflow: {:?}",
        res.runs
    );
}

/// **Four at once, two admitted** (research §7's LAN-stack variant): four
/// children at `sessions = 4`, each a quarter of the pool plus the cap — two
/// fit together, three do not — so the guard keeps exactly two streaming at
/// any time and all four complete with their codenames. `#[ignore]`, manual,
/// a server with four or more slots over one pool.
#[tokio::test]
#[ignore = "requires a live chat server launched with -np 4 --kv-unified (MINDFORK_ENGINE_URL)"]
async fn admission_four_e2e_live() {
    let Some(res) = admission_smoke(AdmissionArm {
        children: 4,
        sessions: 4,
        share: 0.25,
        lie: None,
    })
    .await
    else {
        eprintln!("skipped (see above)");
        return;
    };
    assert_eq!(res.runs.len(), 4, "four runs: {:?}", res.runs);
    assert_all_completed_with_their_codes(&res.runs);
    assert_eq!(
        res.most, 2,
        "a quarter of the pool plus the cap each: two fit together, three do not"
    );
}

/// A stable line of the summarizer's system prompt (`prompt.compact.system`,
/// en) — what routes a compaction roll to the live server in the hybrid.
const COMPACT_MARKER: &str = "compressing the earlier part of a conversation";

/// One arm of the silent-lane probe (docs/research/silent-tasks-budget.md §3).
struct SilentArm {
    /// `None`: the app believes the pool the server reports; `Some(k)`: it is
    /// told the pool is `k` times that — the control arm's lie, which disarms
    /// the guard the same way `admission_control_e2e_live`'s does.
    lie: Option<u64>,
}

struct SilentResult {
    /// The background run's outcome and its final reply.
    run: (Option<crate::entities::subagent::RunOutcome>, String),
    /// The roll: the summary it produced, or the error it reported.
    roll: Result<String, String>,
    /// The most live streams the server saw open at once.
    most: usize,
}

/// **The silent-lane probe** (stage 0 of docs/research/silent-tasks-budget.md,
/// its §3 arm 1): a compaction roll opened beside a background run, at
/// `sessions = 1`, on a server whose slots share one pool — the shape the
/// managed launcher produces at the default (no `-np`: the server's own
/// four-slot unified default). The parent is scripted; the run's persona and
/// the roll's request go to the real server. Sized from `/props`: the chat's
/// earlier history and the run's archive are each 55% of the pool, so the
/// roll's digest and the run's prompt do not fit together. The roll is asked
/// for by `/compact` rather than the automatic trigger, which needs the exact
/// usage a scripted parent cannot report. `None` when there is no live
/// server or it reports at most one slot.
async fn silent_roll_smoke(arm: SilentArm) -> Option<SilentResult> {
    let (live, slots, pool) = pooled_live("the collision").await?;
    let paragraphs = ((0.55 * pool as f64) / 30.0) as usize;
    let max_tokens = (pool / 8) as usize;
    let belief = pool * arm.lie.unwrap_or(1);
    eprintln!(
        "server: {slots} slots over {pool}; history and archive {paragraphs} paragraphs each, cap {max_tokens}, belief {belief}"
    );
    let delegate = ChatChunk::ToolCall(crate::shared::api::contract::ToolCallDelta {
        thought_signature: None,
        index: 0,
        id: Some("c0".into()),
        name: Some("start_subagent".into()),
        arguments: serde_json::json!({
            "name": "alpha reader",
            "system_message": format!("{READER} You are given an archive; report the codename it contains in one short sentence, quoting it exactly."),
            "message": format!("{}\nThe alpha codename is {}.\n\nWhat is the codename?", archive("alpha", paragraphs), CODES[0]),
        })
        .to_string(),
    });
    let backend = Arc::new(ScriptedParent {
        live,
        scripts: std::sync::Mutex::new(
            vec![
                // Turn 1 seeds the history: the user's archive, a one-word reply.
                vec![
                    ChatChunk::Text("Noted.".into()),
                    ChatChunk::Finished(FinishReason::Stop),
                ],
                // Turn 2 starts the run and ends at once.
                vec![delegate, ChatChunk::Finished(FinishReason::ToolCalls)],
                vec![
                    ChatChunk::Text("Started.".into()),
                    ChatChunk::Finished(FinishReason::Stop),
                ],
            ]
            .into(),
        ),
        live_markers: &[READER, COMPACT_MARKER],
        live_after_scripts: false,
        in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });

    let mut cfg = AppConfig::default();
    cfg.tools.subagent_background = true;
    // No wake: the probe is about the roll and the run, not the turn after.
    cfg.tools.subagent_background_wake = false;
    cfg.tools.subagent_max_tokens = max_tokens;
    cfg.default_sampling.max_tokens = Some(max_tokens);
    cfg.engine.external.sessions = 1;
    cfg.engine.managed.sessions = 1;
    cfg.compaction.enabled = true;
    // The automatic trigger stays off (the roll is typed); the window the app
    // believes in is the pool, or the lie.
    cfg.compaction.threshold_pct = 0;
    cfg.compaction.context_tokens = Some(belief as usize);
    // A short tail, so the roll folds the seeded history and nothing less.
    cfg.compaction.tail_tokens = 64;
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let (dir, cmd_tx, mut evt_rx, handle, chat_id) = delegator_chat(
        backend.clone() as Arc<dyn EngineBackend>,
        cfg,
        "start_subagent",
    )
    .await;

    // The history the roll will fold: the user's own archive, scripted "Noted.".
    run_turn_capture_args(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "{}\nKeep this archive in mind.",
            archive("history", paragraphs)
        ),
    )
    .await;
    let started = std::time::Instant::now();
    let (reply, calls) = run_turn_capture_args(
        &cmd_tx,
        &mut evt_rx,
        "Have the alpha reader report its codename in the background.",
    )
    .await;
    eprintln!("parent: {reply} ({:.1} s)", started.elapsed().as_secs_f64());
    for (n, _, r) in &calls {
        eprintln!("call {n} -> {}", r.chars().take(120).collect::<String>());
    }
    // The run's stream is opening on the server; now the roll beside it.
    cmd_tx.send(AppCommand::Compact).unwrap();
    let mut roll: Option<Result<String, String>> = None;
    let mut landed = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while (roll.is_none() || !landed) && std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(left, evt_rx.recv()).await {
            Ok(Some(AppEvent::Compacted { summary, .. })) => roll = Some(Ok(summary)),
            Ok(Some(AppEvent::Error(msg))) if roll.is_none() => roll = Some(Err(msg)),
            Ok(Some(AppEvent::Notice(msg))) if roll.is_none() => roll = Some(Err(msg)),
            Ok(Some(AppEvent::BackgroundRuns { out: 0 })) => landed = true,
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    eprintln!(
        "roll: {roll:?}, run landed: {landed} ({:.1} s)",
        started.elapsed().as_secs_f64()
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chat(chat_id)
        .unwrap()
        .unwrap();
    let run = chat
        .children()
        .next()
        .map(|run| {
            eprintln!(
                "run «{}»: {} messages, outcome {:?}, reply {:?}",
                run.title,
                run.messages.len(),
                run.outcome,
                run.final_reply()
                    .map(|r| r.chars().take(120).collect::<String>())
            );
            (
                run.outcome,
                run.final_reply().unwrap_or_default().to_string(),
            )
        })
        .expect("the run landed on its record");
    let most = backend
        .max_in_flight
        .load(std::sync::atomic::Ordering::SeqCst);
    eprintln!("most live streams open at once: {most}");
    Some(SilentResult {
        run,
        roll: roll.unwrap_or_else(|| Err("the roll never reported".into())),
        most,
    })
}

/// **The silent lane, live** (docs/research/silent-tasks-budget.md §7): the
/// roll waits for the run's stream to end — one live stream at a time — and
/// both complete: the run with its codename, the roll with a summary. On the
/// code before the lane this arm **fails**, which is the probe's GO (§3).
/// `#[ignore]`, manual: the CPU build launched as the managed launcher would
/// at one session (`-c 2048`, no `-np`), `MINDFORK_ENGINE_URL` at it.
#[tokio::test]
#[ignore = "requires a live llama-server with several slots over one pool (MINDFORK_ENGINE_URL)"]
async fn silent_roll_e2e_live() {
    let Some(res) = silent_roll_smoke(SilentArm { lie: None }).await else {
        eprintln!("skipped (see above)");
        return;
    };
    assert_eq!(
        res.run.0,
        Some(crate::entities::subagent::RunOutcome::Completed),
        "the run completes beside the roll: {:?}",
        res.run
    );
    assert!(
        fold_dashes(&res.run.1).contains(CODES[0]),
        "the run's code: {}",
        res.run.1
    );
    let summary = res.roll.expect("the roll produced a summary");
    assert!(!summary.trim().is_empty(), "an empty summary");
    assert_eq!(
        res.most, 1,
        "the roll's digest and the run's archive do not fit the pool together: one at a time"
    );
}

/// **The control arm**: the same run with the app told the pool is four
/// times what it is — the roll opens beside the run, the real pool cannot
/// hold both prompts, and the server ends the running conversations together
/// (admission-by-budget §3.2): the run does not complete or the roll fails.
/// This is what proves the guarded arm above did something. `#[ignore]`,
/// manual, the same server.
#[tokio::test]
#[ignore = "requires a live llama-server with several slots over one pool (MINDFORK_ENGINE_URL)"]
async fn silent_roll_control_e2e_live() {
    let Some(res) = silent_roll_smoke(SilentArm { lie: Some(4) }).await else {
        eprintln!("skipped (see above)");
        return;
    };
    assert_eq!(
        res.most, 2,
        "unguarded, the roll and the run streamed at once"
    );
    assert!(
        res.run.0 != Some(crate::entities::subagent::RunOutcome::Completed) || res.roll.is_err(),
        "the lie about the pool went unpunished — the server did not overflow: run {:?}, roll {:?}",
        res.run,
        res.roll
    );
}

/// The two arms of the preemption probe (docs/research/silent-preemption.md
/// §3): what a turn pays today behind a silent stream, and the floor a
/// preemption could reach.
#[derive(Clone, Copy, Debug)]
enum PreemptArm {
    /// The roll first, then a one-word turn that does not fit beside it.
    RollThenTurn,
    /// A long turn cancelled at its first token, then a one-word turn.
    CancelThenTurn,
    /// The one-word turn alone on an idle server: its cold prefill, the
    /// reference the other arms' first tokens are read against.
    TurnAlone,
}

/// What the probe read, all in seconds from the moment the measured turn was
/// sent unless said otherwise.
#[derive(Debug, Default)]
struct PreemptProbe {
    /// Arm 1: from `/compact` to the roll's result, and the result itself.
    roll: Option<(f64, Result<String, String>)>,
    /// Arm 1: from the turn's send to the roll's end — the wait the lane's
    /// rule makes the turn pay today.
    wait: Option<f64>,
    /// Arm 2: from the cancel to the cancelled turn's `Finished`.
    cancel_latency: Option<f64>,
    /// The measured turn's time to first token and to `Finished`.
    first_token: Option<f64>,
    finished: Option<f64>,
    /// The measured turn's reply.
    reply: String,
}

/// Arm 1: the roll first, the one-word turn half a second behind it; reads
/// the roll's end and result, the turn's first token and its end.
async fn probe_roll_then_turn(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    word: &str,
) -> PreemptProbe {
    let mut probe = PreemptProbe::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let compacted = std::time::Instant::now();
    cmd_tx.send(AppCommand::Compact).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let sent = std::time::Instant::now();
    cmd_tx.send(AppCommand::SendMessage(word.into())).unwrap();
    let mut turn_done = false;
    while (probe.roll.is_none() || !turn_done) && std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(left, evt_rx.recv()).await {
            Ok(Some(AppEvent::Compacted { summary, .. })) => {
                probe.roll = Some((compacted.elapsed().as_secs_f64(), Ok(summary)));
                probe.wait = Some(sent.elapsed().as_secs_f64());
            }
            Ok(Some(AppEvent::Error(msg))) | Ok(Some(AppEvent::Notice(msg)))
                if probe.roll.is_none() =>
            {
                probe.roll = Some((compacted.elapsed().as_secs_f64(), Err(msg)));
                probe.wait = Some(sent.elapsed().as_secs_f64());
            }
            Ok(Some(AppEvent::Chunk { text, .. })) => {
                probe
                    .first_token
                    .get_or_insert(sent.elapsed().as_secs_f64());
                probe.reply.push_str(&text);
            }
            Ok(Some(AppEvent::Finished { .. })) => {
                probe.finished = Some(sent.elapsed().as_secs_f64());
                turn_done = true;
            }
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    probe
}

/// Arm 2: a long turn cancelled at its first token (or thought), the
/// one-word turn sent 300 ms after the cancelled one reports `Finished`.
async fn probe_cancel_then_turn(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    word: &str,
) -> PreemptProbe {
    let mut probe = PreemptProbe::default();
    cmd_tx
        .send(AppCommand::SendMessage(
            "Write a long story about the lighthouse keeper's year, at least four hundred words."
                .into(),
        ))
        .unwrap();
    let mut cancelled_at: Option<std::time::Instant> = None;
    loop {
        match evt_rx.recv().await {
            Some(AppEvent::Chunk { .. }) | Some(AppEvent::Thoughts { .. })
                if cancelled_at.is_none() =>
            {
                cancelled_at = Some(std::time::Instant::now());
                cmd_tx.send(AppCommand::Cancel).unwrap();
            }
            Some(AppEvent::Finished { .. }) => {
                probe.cancel_latency = cancelled_at.map(|t| t.elapsed().as_secs_f64());
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    // The client's `Finished` comes before the server has noticed the
    // closed connection (measured: its `cancel task` is ~1 ms behind, the
    // slot's release ~110 ms) — a request sent in that gap lands on another
    // slot and pays a cold prefill for the same prompt. A preemption's
    // waiter is admitted after the displaced stream's reservation drops, on
    // the same side of that gap.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    read_word_turn(cmd_tx, evt_rx, word, &mut probe).await;
    probe
}

/// Sends the one-word turn and reads its first token and its end into
/// `probe`, from the moment it was sent.
async fn read_word_turn(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    word: &str,
    probe: &mut PreemptProbe,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let sent = std::time::Instant::now();
    cmd_tx.send(AppCommand::SendMessage(word.into())).unwrap();
    while std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(left, evt_rx.recv()).await {
            Ok(Some(AppEvent::Chunk { text, .. })) => {
                probe
                    .first_token
                    .get_or_insert(sent.elapsed().as_secs_f64());
                probe.reply.push_str(&text);
            }
            Ok(Some(AppEvent::Finished { .. })) => {
                probe.finished = Some(sent.elapsed().as_secs_f64());
                break;
            }
            Ok(Some(_)) => {}
            _ => break,
        }
    }
}

/// **The preemption probe** (stage 0 of docs/research/silent-preemption.md):
/// a chat seeded with 55 % of the pool by a scripted turn; then either the
/// compaction roll opened on the live server with a one-word turn sent half
/// a second behind it (the turn's prompt is the same 55 %, so the two do
/// not fit together and the turn waits — the lane's rule, whose length is
/// what is measured), or a long live turn cancelled at its first token with
/// the one-word turn sent the instant it reports `Finished` (the whole path
/// a preemption would take). `None` without a live server or with at most
/// one reported slot.
async fn preemption_smoke(arm: PreemptArm) -> Option<PreemptProbe> {
    let (live, slots, pool) = pooled_live("the shape").await?;
    let paragraphs = ((0.55 * pool as f64) / 30.0) as usize;
    eprintln!("server: {slots} slots over {pool}; history {paragraphs} paragraphs, arm {arm:?}");
    let backend = Arc::new(ScriptedParent {
        live,
        scripts: std::sync::Mutex::new(
            vec![
                vec![
                    ChatChunk::Text("Noted.".into()),
                    ChatChunk::Finished(FinishReason::Stop),
                ],
                vec![
                    ChatChunk::Text("Nothing to add.".into()),
                    ChatChunk::Finished(FinishReason::Stop),
                ],
            ]
            .into(),
        ),
        live_markers: &[COMPACT_MARKER],
        live_after_scripts: true,
        in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let mut cfg = AppConfig::default();
    cfg.default_sampling.max_tokens = Some(256);
    cfg.engine.external.sessions = 1;
    cfg.engine.managed.sessions = 1;
    cfg.compaction.enabled = true;
    cfg.compaction.threshold_pct = 0;
    cfg.compaction.context_tokens = Some(pool as usize);
    cfg.compaction.tail_tokens = 64;
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let (_dir, cmd_tx, mut evt_rx, handle, _chat_id) = delegator_chat(
        backend.clone() as Arc<dyn EngineBackend>,
        cfg,
        "call_subagent",
    )
    .await;
    run_turn_capture_args(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "{}\nKeep this archive in mind.",
            archive("history", paragraphs)
        ),
    )
    .await;
    // A second exchange longer than the tail (`tail_tokens` 64): `plan_cut`
    // keeps that many trailing tokens whole and folds the exchanges before
    // them, so this one is the tail and the archive's is what the roll folds.
    run_turn_capture_args(
        &cmd_tx,
        &mut evt_rx,
        &format!("{}\nAnything to add?", archive("tail", 4)),
    )
    .await;

    let word = "Reply with the single word READY and nothing else.";
    let probe = match arm {
        PreemptArm::RollThenTurn => probe_roll_then_turn(&cmd_tx, &mut evt_rx, word).await,
        PreemptArm::CancelThenTurn => probe_cancel_then_turn(&cmd_tx, &mut evt_rx, word).await,
        PreemptArm::TurnAlone => {
            let mut probe = PreemptProbe::default();
            read_word_turn(&cmd_tx, &mut evt_rx, word, &mut probe).await;
            probe
        }
    };
    let most = backend
        .max_in_flight
        .load(std::sync::atomic::Ordering::SeqCst);
    eprintln!("{probe:#?}\nmost live streams open at once: {most}");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    Some(probe)
}

/// **Arm 1 — the turn behind the roll** (docs/research/silent-preemption.md
/// §3, §7): before the track the turn started only when the roll ended (the
/// wait it measured, §3.1); now the turn **displaces** the roll — its first
/// token arrives before the roll's end — and the roll is made again after
/// it and lands a summary. `#[ignore]`, manual: the CPU build launched as
/// the managed launcher would at one session (`-c 2048`, no `-np`),
/// `MINDFORK_ENGINE_URL` at it; the LAN stack for the GPU numbers.
#[tokio::test]
#[ignore = "requires a live llama-server with several slots over one pool (MINDFORK_ENGINE_URL)"]
async fn preemption_wait_e2e_live() {
    let Some(probe) = preemption_smoke(PreemptArm::RollThenTurn).await else {
        eprintln!("skipped (see above)");
        return;
    };
    let (_, roll) = probe.roll.expect("the roll reported");
    assert!(
        roll.is_ok_and(|s| !s.trim().is_empty()),
        "the roll produced a summary"
    );
    assert!(probe.finished.is_some(), "the turn finished");
    assert!(!probe.reply.trim().is_empty(), "the turn replied");
    let (first_token, roll_end) = (probe.first_token.unwrap(), probe.wait.unwrap());
    assert!(
        first_token < roll_end,
        "the turn streamed before the roll ended: first token at {first_token:.1} s, the roll's end at {roll_end:.1} s"
    );
}

/// **The reference — the one-word turn alone** (docs/research/cpu-batch.md
/// §3): on an idle server its first token is the cold prefill of the seeded
/// prompt and nothing else — what the other arms' first tokens are read
/// against. `#[ignore]`, manual, the same servers.
#[tokio::test]
#[ignore = "requires a live llama-server with several slots over one pool (MINDFORK_ENGINE_URL)"]
async fn preemption_cold_e2e_live() {
    let Some(probe) = preemption_smoke(PreemptArm::TurnAlone).await else {
        eprintln!("skipped (see above)");
        return;
    };
    assert!(probe.finished.is_some(), "the turn finished");
    assert!(!probe.reply.trim().is_empty(), "the turn replied");
}

/// **Arm 2 — the floor**: a turn cancelled at its first token, the next one
/// sent at once; its time to first token is the whole path a preemption
/// would take. `#[ignore]`, manual, the same server.
#[tokio::test]
#[ignore = "requires a live llama-server with several slots over one pool (MINDFORK_ENGINE_URL)"]
async fn preemption_floor_e2e_live() {
    let Some(probe) = preemption_smoke(PreemptArm::CancelThenTurn).await else {
        eprintln!("skipped (see above)");
        return;
    };
    assert!(
        probe.cancel_latency.is_some(),
        "the cancelled turn finished"
    );
    assert!(
        probe.finished.is_some(),
        "the turn after the cancel finished"
    );
    assert!(!probe.reply.trim().is_empty(), "the turn replied");
}

/// **A roll stopped from the tasks screen, live**
/// (docs/research/stop-silent-task.md §6): `/compact` on a seeded chat, the
/// stop command a second later — the notice arrives, nothing is folded, the
/// live stream ended early — then `/compact` again completes. `#[ignore]`,
/// manual: `MINDFORK_ENGINE_URL` at a server with several slots over one
/// pool (the LAN stack, or the CPU build as the launcher launches it).
#[tokio::test]
#[ignore = "requires a live llama-server with several slots over one pool (MINDFORK_ENGINE_URL)"]
async fn stop_silent_task_e2e_live() {
    let Some((live, slots, pool)) = pooled_live("the roll").await else {
        eprintln!("skipped (see above)");
        return;
    };
    let paragraphs = ((0.55 * pool as f64) / 30.0) as usize;
    eprintln!("server: {slots} slots over {pool}; history {paragraphs} paragraphs");
    let backend = Arc::new(ScriptedParent {
        live,
        scripts: std::sync::Mutex::new(
            vec![
                vec![
                    ChatChunk::Text("Noted.".into()),
                    ChatChunk::Finished(FinishReason::Stop),
                ],
                vec![
                    ChatChunk::Text("Nothing to add.".into()),
                    ChatChunk::Finished(FinishReason::Stop),
                ],
            ]
            .into(),
        ),
        live_markers: &[COMPACT_MARKER],
        live_after_scripts: false,
        in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let mut cfg = AppConfig::default();
    cfg.engine.external.sessions = 1;
    cfg.engine.managed.sessions = 1;
    cfg.compaction.enabled = true;
    cfg.compaction.threshold_pct = 0;
    cfg.compaction.context_tokens = Some(pool as usize);
    cfg.compaction.tail_tokens = 64;
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let (dir, cmd_tx, mut evt_rx, handle, chat_id) = delegator_chat(
        backend.clone() as Arc<dyn EngineBackend>,
        cfg,
        "call_subagent",
    )
    .await;
    run_turn_capture_args(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "{}\nKeep this archive in mind.",
            archive("history", paragraphs)
        ),
    )
    .await;
    run_turn_capture_args(
        &cmd_tx,
        &mut evt_rx,
        &format!("{}\nAnything to add?", archive("tail", 4)),
    )
    .await;

    let started = std::time::Instant::now();
    cmd_tx.send(AppCommand::Compact).unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    cmd_tx
        .send(AppCommand::StopBackgroundTask {
            kind: BackgroundKind::Compaction,
        })
        .unwrap();
    let stopped_at = std::time::Instant::now();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let (mut notice, mut compacted) = (None, false);
    while notice.is_none() && std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(left, evt_rx.recv()).await {
            Ok(Some(AppEvent::Notice(m))) => notice = Some(m),
            Ok(Some(AppEvent::Compacted { .. })) => compacted = true,
            Ok(Some(AppEvent::Error(m))) => panic!("the stop reported an error: {m}"),
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    let notice = notice.expect("the stopped roll answered with a notice");
    eprintln!(
        "stopped {:.1} s after /compact; the notice {:.2} s after the stop: {notice}",
        (stopped_at - started).as_secs_f64(),
        stopped_at.elapsed().as_secs_f64()
    );
    assert!(!compacted, "nothing was folded by the stopped roll");
    assert_eq!(
        backend
            .max_in_flight
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    let again = std::time::Instant::now();
    cmd_tx.send(AppCommand::Compact).unwrap();
    let done = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Compacted { .. })),
    )
    .await
    .ok()
    .flatten();
    eprintln!(
        "the second roll: {:?} ({:.1} s)",
        done.as_ref().map(|_| "compacted"),
        again.elapsed().as_secs_f64()
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    assert!(done.is_some(), "the next /compact completed");
    let chat = Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chat(chat_id)
        .unwrap()
        .unwrap();
    assert!(
        chat.compaction.is_some(),
        "the second roll's summary landed"
    );
}

/// The background run's live proof (docs/research/background-subagents.md
/// §7): the parent is told to have a planted file read in the background and
/// to answer an unrelated question at once; the turn lands with a
/// `start_subagent` call and no `fs_read` of its own; the run reads the file
/// and ends; the assistant is woken on the task notification and its reply
/// carries the planted codename. The dashes are folded on both sides
/// (docs/lessons.md §9). `#[ignore]`, manual against a live model.
#[tokio::test]
#[ignore = "requires a live chat server (MINDFORK_ENGINE_URL)"]
async fn background_subagent_e2e_live() {
    let sandbox = tempfile::tempdir().unwrap();
    let file = sandbox.path().join("alpha.txt");
    std::fs::write(
        &file,
        "Internal note.\nThe alpha codename is ZARNOVIK-7741.\n",
    )
    .unwrap();
    let mut cfg = AppConfig::default();
    cfg.tools.fs_enabled = true;
    cfg.tools.fs_root = Some(sandbox.path().to_string_lossy().to_string());
    cfg.tools.subagent_background = true;
    cfg.engine.external.sessions = 2;
    cfg.engine.managed.sessions = 2;
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let Some((_d, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Delegator".into(),
            system_message: "You are a coordinator. Reply in English. You never read files \
                 yourself. When asked to have a file read in the background, start ONE \
                 sub-agent with the start_subagent tool — give it the exact file path and \
                 tell it to use fs_read — and answer the rest of the request at once, \
                 without waiting for it. When a task notification with the sub-agent's \
                 result arrives, report what it found."
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
                enabled_tools: Some(vec![
                    "start_subagent".to_string(),
                    "call_subagent".to_string(),
                    "fs_read".to_string(),
                ]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(pid),
        })
        .unwrap();
    let chat_id = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .and_then(|e| match e {
            AppEvent::ChatActivated { id, .. } => Some(id),
            _ => None,
        })
        .unwrap();

    let ask = format!(
        "Two things. (1) Have a sub-agent read the file {} in the background and find the \
         alpha codename — start it now with start_subagent and do not wait for it. \
         (2) Right now: which is larger, 17 × 23 or 400?",
        file.display()
    );
    cmd_tx.send(AppCommand::SendMessage(ask)).unwrap();
    let mut reply = String::new();
    let mut thoughts = 0usize;
    let mut calls: Vec<(String, String, String)> = Vec::new();
    let mut finish = None;
    while let Some(ev) = evt_rx.recv().await {
        match ev {
            AppEvent::Chunk { text, .. } => reply.push_str(&text),
            AppEvent::Thoughts { text, .. } => thoughts += text.len(),
            AppEvent::ToolCall {
                name,
                arguments,
                result,
                ..
            } => calls.push((name, arguments, result)),
            AppEvent::Error(e) => eprintln!("error event: {e}"),
            AppEvent::Finished { reason, .. } => {
                finish = Some(reason);
                break;
            }
            _ => {}
        }
    }
    eprintln!("reply ({finish:?}, {thoughts} bytes of thoughts): {reply}");
    for (n, a, r) in &calls {
        eprintln!(
            "call {n}({}) -> {}",
            a.chars().take(120).collect::<String>(),
            r.chars().take(160).collect::<String>()
        );
    }
    assert!(
        calls.iter().any(|(n, _, _)| n == "start_subagent"),
        "the parent did not start a background run: {calls:?} ({finish:?}, reply {reply:?})"
    );
    assert!(
        !calls.iter().any(|(n, _, _)| n == "fs_read"),
        "the parent read the file itself instead of delegating"
    );

    // The run ends and the assistant is woken on the task notification: a
    // turn nobody sent a message for, whose reply carries the codename.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    let mut wake = String::new();
    loop {
        let ev = tokio::time::timeout_at(deadline, evt_rx.recv())
            .await
            .expect("the run lands and the wake turn ends within five minutes")
            .expect("the event stream stays open");
        match ev {
            AppEvent::Chunk { text, .. } => wake.push_str(&text),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    eprintln!("wake reply: {wake}");
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = Storage::open(Paths::with_root(_d.path()))
        .unwrap()
        .json()
        .load_chat(chat_id)
        .unwrap()
        .unwrap();
    let run = chat
        .children()
        .find(|r| r.background)
        .expect("a background run on the record");
    eprintln!(
        "run «{}»: {} messages, outcome {:?}",
        run.title,
        run.messages.len(),
        run.outcome
    );
    assert_eq!(
        run.outcome,
        Some(crate::entities::subagent::RunOutcome::Completed),
        "the run did not complete"
    );
    assert!(
        run.messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .any(|r| r.name == "fs_read"),
        "the run never used fs_read"
    );
    let fold = |s: &str| {
        s.replace(['-', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}'], "")
            .to_uppercase()
    };
    let note = chat
        .messages
        .iter()
        .find(|m| m.is_notification())
        .expect("a task notification row");
    assert!(
        fold(&note.text).contains("ZARNOVIK7741"),
        "the notification lacks the codename: {}",
        note.text
    );
    // GO: the woken assistant reports what the run found.
    assert!(
        fold(&wake).contains("ZARNOVIK7741"),
        "the wake reply lacks the codename: {wake}"
    );
}

/// The slow-prefill note against a live `llama-server` in external mode
/// (docs/research/slow-prefill-detection.md §6): a seeded turn processes a
/// batch's worth of prompt tokens, the engine's `timings` travel to the
/// landing, and the note comes — or not — by the rule. Which is expected
/// depends on the host: set `MINDFORK_EXPECT_SLOW_PREFILL=1` on a host whose
/// prompt processing is slow (the CPU build at the default batch), leave it
/// unset on a GPU stack. Either way the figures are printed.
/// `MINDFORK_ENGINE_URL=…/v1 [MINDFORK_EXPECT_SLOW_PREFILL=1] cargo test slow_prefill_e2e_live -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires a running llama-server (MINDFORK_ENGINE_URL)"]
async fn slow_prefill_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let expect_note = std::env::var("MINDFORK_EXPECT_SLOW_PREFILL").is_ok();
    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // A prompt of a batch's worth and more: forty lines of a lighthouse
    // keeper's evening, then one question.
    let seed: String = (0..40)
        .map(|i| {
            format!(
                "Paragraph {i}: the keeper climbs the stairs, lights the lamp, writes the log, \
                 notes the tide, and looks out over the dark water for a while.\n"
            )
        })
        .collect();
    let started = std::time::Instant::now();
    let (reply, _tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        &format!("{seed}\nIn one word: what does the keeper light?"),
    )
    .await;
    let turn = started.elapsed();
    // The landing's note, if any, follows `Finished` at once.
    let note = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Notice(_))),
    )
    .await
    .ok()
    .flatten()
    .and_then(|e| match e {
        AppEvent::Notice(t) => Some(t),
        _ => None,
    });
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!(
        "slow prefill: the turn took {:.1} s, reply {:?}; note = {}",
        turn.as_secs_f64(),
        reply.trim(),
        note.as_deref().unwrap_or("none")
    );
    match (expect_note, note) {
        (true, Some(text)) => assert!(text.contains("-b 256 -ub 256"), "{text}"),
        (true, None) => panic!("a slow host was expected to be told"),
        (false, None) => {}
        (false, Some(text)) => panic!("a fast host was told: {text}"),
    }
}

/// The roll's timings (docs/research/roll-timings.md §6): a chat reopened
/// from disk and `/compact` the session's first request — the roll is the
/// session's first cold prompt, and the slow-prefill note, if any, comes
/// from it. `MINDFORK_EXPECT_SLOW_PREFILL=1` on a slow host (the CPU build),
/// unset on a GPU host.
///
/// `MINDFORK_ENGINE_URL=…/v1 [MINDFORK_EXPECT_SLOW_PREFILL=1] cargo test roll_prefill_e2e_live -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires a running llama-server (MINDFORK_ENGINE_URL)"]
async fn roll_prefill_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let expect_note = std::env::var("MINDFORK_EXPECT_SLOW_PREFILL").is_ok();

    // The conversation is on disk before the app starts: forty lines of the
    // keeper's evening over four exchanges, none of it sent this session.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(&root));
    let profile = Profile::new("Keeper", "You are a concise assistant.");
    json.upsert_profile(&profile).unwrap();
    let mut chat = Chat::from_profile(&profile, "the keeper's evening");
    for part in 0..4u32 {
        let seed: String = (part * 10..part * 10 + 10)
            .map(|i| {
                format!(
                    "Paragraph {i}: the keeper climbs the stairs, lights the lamp, writes the log, \
                     notes the tide, and looks out over the dark water for a while.\n"
                )
            })
            .collect();
        chat.push_message(Message::user(format!("{seed}\nAcknowledge in one word.")));
        chat.push_message(Message::assistant(String::from("Noted.")));
    }
    let chat_id = chat.id;
    json.save_chat(&chat).unwrap();
    // The database beside the chats, so the bootstrap has no note of its own
    // to put in the feed before the roll's.
    drop(Storage::open(Paths::with_root(&root)).unwrap());

    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
    config.compaction = crate::shared::config::CompactionSettings {
        enabled: true,
        summary_words: 150,
        // A small verbatim tail, so the four exchanges leave something to fold.
        tail_tokens: 120,
        ..Default::default()
    };
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, Some(backend), config);
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    if !matches!(activated, AppEvent::ChatActivated { id, .. } if id == chat_id) {
        cmd_tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == chat_id),
        )
        .await
        .unwrap();
    }

    // The session's first request is the roll.
    let started = std::time::Instant::now();
    cmd_tx.send(AppCommand::Compact).unwrap();
    let landed = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::Compacted { .. } | AppEvent::Error(_) | AppEvent::Notice(_)
        )
    })
    .await
    .unwrap();
    let AppEvent::Compacted {
        summary, folded, ..
    } = landed
    else {
        panic!("the roll did not land as a summary: {landed:?}");
    };
    let roll = started.elapsed();
    // The landing's note, if any, follows `Compacted` at once.
    let note = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Notice(_))),
    )
    .await
    .ok()
    .flatten()
    .and_then(|e| match e {
        AppEvent::Notice(t) => Some(t),
        _ => None,
    });
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!(
        "roll prefill: the roll took {:.1} s, folded {folded} messages into {} chars; note = {}",
        roll.as_secs_f64(),
        summary.chars().count(),
        note.as_deref().unwrap_or("none")
    );
    match (expect_note, note) {
        (true, Some(text)) => assert!(text.contains("-b 256 -ub 256"), "{text}"),
        (true, None) => panic!("a slow host was expected to be told by its roll"),
        (false, None) => {}
        (false, Some(text)) => panic!("a fast host was told: {text}"),
    }
}

/// The loops' timings (docs/research/loop-timings.md §6): two phases on one
/// data root against one server. Phase 1 warms the server's cache with the
/// chat's prefix — a long turn, no reflection — and quits. Phase 2 restarts
/// the app on the same root: a short turn, warm and under the floor, then
/// the reflection — its first round cold — and the note, if any, from its
/// landing. `MINDFORK_EXPECT_SLOW_PREFILL=1` on a slow host (the CPU build),
/// unset on a GPU host.
///
/// `MINDFORK_ENGINE_URL=…/v1 [MINDFORK_EXPECT_SLOW_PREFILL=1] cargo test loop_prefill_e2e_live -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires a running llama-server (MINDFORK_ENGINE_URL)"]
async fn loop_prefill_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let expect_note = std::env::var("MINDFORK_EXPECT_SLOW_PREFILL").is_ok();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let external = || {
        let mut config = AppConfig::default();
        config.engine.mode = crate::shared::config::ServerMode::External;
        config
    };

    // Phase 1: the chat's prefix into the server's cache. The memory tools
    // are enabled here, before the turn — the reflection is gated on the
    // profile's tool set, and the schemas are part of the prefix phase 2
    // must find warm; the edit is persisted, so phase 2 inherits it.
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, Some(backend.clone()), external());
    let _pid = enable_all_tools(&cmd_tx, &mut evt_rx).await;
    let seed: String = (0..40)
        .map(|i| {
            format!(
                "Paragraph {i}: the keeper climbs the stairs, lights the lamp, writes the log, \
                 notes the tide, and looks out over the dark water for a while.\n"
            )
        })
        .collect();
    let (reply, _) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        &format!("{seed}\nIn one word: what does the keeper light?"),
    )
    .await;
    // The exchange is stored by `handle_done`, after `Finished`: the `ChatList`
    // it emits is what says the quit will flush it.
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    eprintln!("phase 1: reply {:?}", reply.trim());

    // Phase 2: the app again on the same root, the same server; the
    // reflection after the reply.
    let mut config = external();
    config.self_model.auto_reflect_every = 1;
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, Some(backend), config);
    // The app reopens on the chat it left; the reply shows it did.
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let (reply, _) =
        run_turn_live(&cmd_tx, &mut evt_rx, "And in one word: what does he note?").await;
    let turn = started.elapsed();
    // Spawned at all? A loop the profile's tool set gates out never lands,
    // and "did not land" would hide that.
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        wait_for(&mut evt_rx, |e| {
            matches!(
                e,
                AppEvent::BackgroundTask {
                    kind: BackgroundKind::Reflection,
                    active: true
                }
            )
        }),
    )
    .await
    .expect("the reflection was spawned at the landing");
    // The reflection's own landing — bounded by its time limit over its
    // streaming, plus its wait for the lane.
    let landed = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        wait_for(&mut evt_rx, |e| {
            matches!(
                e,
                AppEvent::BackgroundTask {
                    kind: BackgroundKind::Reflection,
                    active: false
                }
            )
        }),
    )
    .await;
    let loop_took = started.elapsed() - turn;
    // The landing's note, if any, follows it at once.
    let note = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Notice(_))),
    )
    .await
    .ok()
    .flatten()
    .and_then(|e| match e {
        AppEvent::Notice(t) => Some(t),
        _ => None,
    });
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!(
        "loop prefill: the turn took {:.1} s (reply {:?}), the reflection landed = {}, {:.1} s after it; note = {}",
        turn.as_secs_f64(),
        reply.trim(),
        landed.is_ok(),
        loop_took.as_secs_f64(),
        note.as_deref().unwrap_or("none")
    );
    assert!(landed.is_ok(), "the reflection did not land");
    match (expect_note, note) {
        (true, Some(text)) => assert!(text.contains("-b 256 -ub 256"), "{text}"),
        (true, None) => panic!("a slow host was expected to be told by its loop"),
        (false, None) => {}
        (false, Some(text)) => panic!("a fast host was told: {text}"),
    }
}

/// An engine that forwards every request to the live one and keeps it beside
/// the exact prompt size the server reported for it — the stage-0 probe of
/// docs/research/roll-usage-calibration.md §2.1 as a test.
/// What the probe keeps: each request beside its exact prompt size.
type Seen = Arc<std::sync::Mutex<Vec<(crate::shared::api::ChatRequest, u32)>>>;

struct EstimateProbe {
    inner: Arc<dyn EngineBackend>,
    seen: Seen,
}

#[async_trait::async_trait]
impl EngineBackend for EstimateProbe {
    async fn chat_stream(
        &self,
        req: crate::shared::api::ChatRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::shared::api::contract::ChatStream> {
        use futures_util::StreamExt as _;
        let mut inner = self.inner.chat_stream(req.clone(), cancel).await?;
        let seen = self.seen.clone();
        // Kept the moment the usage arrives: the consumer drops the stream at
        // `Finished` without reading it to its end, so nothing after the loop
        // would run.
        let s = async_stream::stream! {
            while let Some(chunk) = inner.next().await {
                if let ChatChunk::Usage(u) = &chunk {
                    seen.lock().unwrap().push((req.clone(), u.prompt_tokens));
                }
                yield chunk;
            }
        };
        Ok(Box::pin(s))
    }

    async fn context_budget(&self) -> Option<u32> {
        self.inner.context_budget().await
    }
}

/// The prompt estimate against the server's exact count, on the requests the
/// app itself builds (docs/research/roll-usage-calibration.md §6,
/// title-impersonation-usage.md §6): two prose turns, a third carrying 25 KB
/// of JSON — the shape of a tool result — then `/compact` and an
/// impersonation. A prose turn's exact over its estimate within 0.75–1.25 —
/// the schemas counted, the compact JSON a little denser than four bytes a
/// token; a request carrying the JSON above 1.0 — the under-count the
/// per-kind ratio exists to keep out of the other kinds' reach; the roll's
/// and the title's at most 1.0 — prose over-counted, never under.
///
/// `MINDFORK_ENGINE_URL=…/v1 cargo test prompt_estimate_e2e_live -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires a running llama-server (MINDFORK_ENGINE_URL)"]
async fn prompt_estimate_e2e_live() {
    let Some(live) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let probe = Arc::new(EstimateProbe {
        inner: live,
        seen: Arc::new(std::sync::Mutex::new(Vec::new())),
    });
    let config = AppConfig {
        compaction: crate::shared::config::CompactionSettings {
            enabled: true,
            summary_words: 150,
            tail_tokens: 120,
            ..Default::default()
        },
        ..Default::default()
    };
    let (_d, cmd_tx, mut evt_rx, handle) =
        spawn_orch_cfg(Some(probe.clone() as Arc<dyn EngineBackend>), config);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    // A catalogue as a tool result would carry it: JSON runs at about 2.4
    // bytes a token on this tokenizer, against the estimate's four.
    let blob = serde_json::to_string(
        &(0..220)
            .map(|i| {
                serde_json::json!({
                    "id": i,
                    "sku": format!("A{i:04}-{:03}", i * 7 % 1000),
                    "price": (i as f64) * 1.25 + 0.99,
                    "tags": ["alpha", "beta", "gamma"],
                    "stock": {"warehouse": i % 5, "count": i * 3},
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let json_turn = format!(
        "Here is a catalogue as JSON; answer in one sentence how many items it has:\n{blob}"
    );
    for text in [
        "Расскажи в двух предложениях, зачем нужны индексы в базах данных.",
        "В двух предложениях: чем отличается кэш от буфера?",
        json_turn.as_str(),
    ] {
        let (reply, _) = run_turn_capture(&cmd_tx, &mut evt_rx, text).await;
        eprintln!("reply: {}", reply.chars().take(60).collect::<String>());
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
            .await
            .unwrap();
    }
    cmd_tx.send(AppCommand::Compact).unwrap();
    let landed = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Compacted { .. } | AppEvent::Error(_))
    })
    .await
    .unwrap();
    assert!(
        matches!(landed, AppEvent::Compacted { .. }),
        "the roll did not land: {landed:?}"
    );
    cmd_tx
        .send(AppCommand::Impersonate {
            seed: String::new(),
        })
        .unwrap();
    let ended = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::ImpersonationFinished { .. } | AppEvent::Error(_)
        )
    })
    .await
    .unwrap();
    assert!(
        matches!(ended, AppEvent::ImpersonationFinished { .. }),
        "impersonation did not finish: {ended:?}"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let roll_system = crate::features::compaction::summary_system_message(
        crate::shared::i18n::locale(crate::shared::i18n::Lang::default()),
        150,
    );
    let seen = probe.seen.lock().unwrap().clone();
    let (mut turns, mut rolls, mut dense, mut impersonations) = (0, 0, 0, 0);
    for (req, exact) in &seen {
        let estimate = super::super::generation::estimate_prompt_tokens(req);
        let ratio = *exact as f64 / estimate as f64;
        let is_roll = req.system.as_deref() == Some(roll_system.as_str());
        // The JSON message in the history: the turn that carried it, and
        // impersonation, which sends the whole conversation — after the
        // roll the cut lands on that very message, and the swap's leading
        // assistant turn is folded into the persona, so impersonation's JSON
        // rides its system prompt (docs/research/gemma-impersonation.md §3.1).
        let carries_json = req
            .messages
            .iter()
            .any(|m| m.content.contains("catalogue as JSON"))
            || req
                .system
                .as_deref()
                .is_some_and(|s| s.contains("catalogue as JSON"));
        let kind = match (req.tools.is_empty(), is_roll, carries_json) {
            (false, _, _) => "turn",
            (true, true, _) => "roll",
            (true, false, true) => "impersonation",
            (true, false, false) => "title",
        };
        eprintln!(
            "prompt estimate: {kind:<13} tools={:>2} json={} estimate={estimate:>5} exact={exact:>5} exact/estimate={ratio:.2}",
            req.tools.len(),
            u8::from(carries_json)
        );
        match kind {
            "turn" if carries_json => {
                turns += 1;
                dense += 1;
                assert!(ratio > 1.0, "the JSON turn did not under-count: {ratio:.2}");
            }
            "turn" => {
                turns += 1;
                assert!(
                    (0.75..=1.25).contains(&ratio),
                    "a prose turn's estimate is off by more than a quarter: {ratio:.2}"
                );
            }
            "impersonation" => {
                impersonations += 1;
                assert!(
                    ratio > 1.0,
                    "impersonation over the JSON did not under-count: {ratio:.2}"
                );
            }
            "roll" => {
                rolls += 1;
                assert!(ratio <= 1.0, "the roll under-counted: {ratio:.2}");
            }
            _ => assert!(ratio <= 1.0, "the title under-counted: {ratio:.2}"),
        }
    }
    assert!(
        turns >= 3 && dense >= 1 && rolls >= 1 && impersonations >= 1,
        "{turns} turns ({dense} dense), {rolls} rolls, {impersonations} impersonations seen"
    );
}

/// A chat seeded on disk for the one-shot smokes: four long exchanges the
/// user opens — the shape Gemma 3's template refused until the opening was
/// folded into the persona (docs/research/gemma-impersonation.md) — about a
/// thousand tokens swapped or digested — above the rule's floor, and
/// inside a minute of prefill on the CPU build. Seeded rather than made by
/// turns: a turn's first round on a fresh root processes its schemas cold
/// and would claim the session's one note before the request under test
/// (docs/research/oneshot-samples.md §4). Returns the chat's id; the
/// database is opened once so the bootstrap says nothing about it.
fn seed_long_chat(root: &std::path::Path) -> Uuid {
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(root));
    let profile = Profile::new("Keeper", "You are a concise assistant.");
    json.upsert_profile(&profile).unwrap();
    let mut chat = Chat::from_profile(&profile, "the keeper's evening");
    for part in 0..4u32 {
        let seed: String = (part * 8..part * 8 + 8)
            .map(|i| {
                format!(
                    "Paragraph {i}: the keeper climbs the stairs, lights the lamp, writes the log, \
                     notes the tide, and looks out over the dark water for a while.\n"
                )
            })
            .collect();
        chat.push_message(Message::user(format!("{seed}\nAcknowledge in one word.")));
        chat.push_message(Message::assistant(String::from("Noted.")));
    }
    let chat_id = chat.id;
    json.save_chat(&chat).unwrap();
    drop(Storage::open(Paths::with_root(root)).unwrap());
    chat_id
}

/// The orchestrator on a seeded root with the chat active and every startup
/// event drained: what the one-shot smokes start from.
async fn spawn_on_seeded_chat(
    root: &std::path::Path,
    backend: Arc<dyn EngineBackend>,
    chat_id: Uuid,
) -> (
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(root, Some(backend), config);
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    if !matches!(activated, AppEvent::ChatActivated { id, .. } if id == chat_id) {
        cmd_tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == chat_id),
        )
        .await
        .unwrap();
    }
    (cmd_tx, evt_rx, handle)
}

/// The note within a few seconds of the landing, or none.
async fn note_after_landing(evt_rx: &mut UnboundedReceiver<AppEvent>) -> Option<String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        wait_for(evt_rx, |e| matches!(e, AppEvent::Notice(_))),
    )
    .await
    .ok()
    .flatten()
    .and_then(|e| match e {
        AppEvent::Notice(t) => Some(t),
        _ => None,
    })
}

/// Whether the host was told, against what the host is
/// (`MINDFORK_EXPECT_SLOW_PREFILL=1` on a slow one).
fn assert_note(expect_note: bool, note: Option<String>, what: &str) {
    match (expect_note, note) {
        (true, Some(text)) => assert!(text.contains("-b 256 -ub 256"), "{text}"),
        (true, None) => panic!("a slow host was expected to be told by its {what}"),
        (false, None) => {}
        (false, Some(text)) => panic!("a fast host was told: {text}"),
    }
}

/// Impersonation's sample end to end (docs/research/oneshot-samples.md §6,
/// fork F4a): a seeded chat, `Ctrl+U` with no turn before it, and the note
/// within a few seconds of `ImpersonationFinished` on a slow host — none on
/// a fast one. The prompt is the whole conversation swapped under its own
/// system, processed cold: about a thousand tokens here.
///
/// `MINDFORK_ENGINE_URL=…/v1 [MINDFORK_EXPECT_SLOW_PREFILL=1] cargo test impersonation_prefill_e2e_live -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires a running llama-server (MINDFORK_ENGINE_URL)"]
async fn impersonation_prefill_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let expect_note = std::env::var("MINDFORK_EXPECT_SLOW_PREFILL").is_ok();
    let dir = tempfile::tempdir().unwrap();
    let chat_id = seed_long_chat(dir.path());
    let (cmd_tx, mut evt_rx, handle) = spawn_on_seeded_chat(dir.path(), backend, chat_id).await;

    let started = std::time::Instant::now();
    cmd_tx
        .send(AppCommand::Impersonate {
            seed: String::new(),
        })
        .unwrap();
    let ended = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::ImpersonationFinished { .. } | AppEvent::Error(_)
        )
    })
    .await
    .unwrap();
    assert!(
        matches!(ended, AppEvent::ImpersonationFinished { .. }),
        "impersonation did not finish: {ended:?}"
    );
    let took = started.elapsed();
    let note = note_after_landing(&mut evt_rx).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!(
        "impersonation prefill: the request took {:.1} s; note = {}",
        took.as_secs_f64(),
        note.as_deref().unwrap_or("none")
    );
    assert_note(expect_note, note, "impersonation");
}

/// The title's sample end to end (docs/research/oneshot-samples.md §6, fork
/// F4a): a seeded chat, a requested title with no turn before it — the
/// digest capped at 4000 characters, about a thousand tokens under the
/// title's own system, processed cold — and the note within a few seconds
/// of the rename on a slow host, none on a fast one.
///
/// `MINDFORK_ENGINE_URL=…/v1 [MINDFORK_EXPECT_SLOW_PREFILL=1] cargo test title_prefill_e2e_live -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires a running llama-server (MINDFORK_ENGINE_URL)"]
async fn title_prefill_e2e_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let expect_note = std::env::var("MINDFORK_EXPECT_SLOW_PREFILL").is_ok();
    let dir = tempfile::tempdir().unwrap();
    let chat_id = seed_long_chat(dir.path());
    let (cmd_tx, mut evt_rx, handle) = spawn_on_seeded_chat(dir.path(), backend, chat_id).await;

    let started = std::time::Instant::now();
    cmd_tx.send(AppCommand::AutoRenameChat(chat_id)).unwrap();
    let landed = wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::ChatRenamed { .. } | AppEvent::ChatListError(_) | AppEvent::Error(_)
        )
    })
    .await
    .unwrap();
    let AppEvent::ChatRenamed { title, .. } = landed else {
        panic!("the title did not land: {landed:?}");
    };
    let took = started.elapsed();
    let note = note_after_landing(&mut evt_rx).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    eprintln!(
        "title prefill: the request took {:.1} s, the title {title:?}; note = {}",
        took.as_secs_f64(),
        note.as_deref().unwrap_or("none")
    );
    assert_note(expect_note, note, "title");
}

/// The settings screen's one question, end to end: `ListModels` for the
/// assistant's slot, the orchestrator resolving the address and key from the
/// config it already holds, and the answer landing on the UI channel as
/// `ModelCatalogue` (docs/research/model-picker.md, stage 4b).
///
/// Against `MINDFORK_ENGINE_URL` — a `llama-server` lists the model it loaded,
/// and that id is what a multi-model endpoint would route on, so it is what the
/// picker writes into the field.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL (a live OpenAI-compatible server)"]
async fn the_model_catalogue_reaches_the_ui_e2e_live() {
    let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let mut cfg = no_auto_cfg();
    cfg.engine.mode = crate::shared::config::ServerMode::External;
    cfg.engine.external.url = Some(url.clone());
    let Some((_dir, cmd_tx, mut evt_rx, handle)) = spawn_orch_live_cfg(cfg) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    cmd_tx
        .send(AppCommand::ListModels(
            crate::shared::api::catalogue::ModelSlot::Assistant,
        ))
        .unwrap();
    let landed = tokio::time::timeout(
        Duration::from_secs(30),
        wait_for(&mut evt_rx, |e| {
            matches!(e, AppEvent::ModelCatalogue { .. })
        }),
    )
    .await
    .expect("the catalogue must answer the screen that asked")
    .expect("the event stream");
    match landed {
        AppEvent::ModelCatalogue { slot, models } => {
            assert_eq!(slot, crate::shared::api::catalogue::ModelSlot::Assistant);
            let list = models.expect("the server answered its catalogue");
            eprintln!("{url} listed {} model(s):", list.len());
            for m in list.iter().take(5) {
                eprintln!("    {} {:?}", m.id, m.role);
            }
            assert!(!list.is_empty(), "a server with a model loaded lists it");
            assert!(
                list.iter().all(|m| !m.id.is_empty()),
                "an empty id could not be written into the field"
            );
        }
        other => panic!("expected a catalogue, got {other:?}"),
    }
    drop(cmd_tx);
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}
