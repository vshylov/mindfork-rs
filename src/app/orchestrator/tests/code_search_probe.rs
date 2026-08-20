//! **Throwaway probe** for the code-workspace track's stage-5 go/no-go
//! (docs/code-workspace.md §3.7, fork F4). Part of the [`super`] module.
//!
//! The question: does a semantic index over the attached project beat
//! `code_grep` for the local model families this project targets? The plan is
//! explicit that a no is a legitimate answer and gets recorded rather than
//! worked around, so this measures before anything ships.
//!
//! **The design of the measurement is the whole of the work here**, and three
//! choices carry it:
//!
//! - **The corpus is mindfork-rs itself** (user's call, 2026-08-21). A real
//!   project, ground truth I can verify against the source — and the *hardest*
//!   case for an index, because this code is commented in unusually discursive
//!   English prose, which is exactly what makes grep strong. An index that wins
//!   here wins anywhere.
//! - **The questions are in the user's vocabulary, not the code's.** "Why does
//!   the app sometimes shorten the conversation by itself?" — the code says
//!   `compaction`, and nothing in the question does. That gap is the whole
//!   thesis of a semantic index; questions phrased in identifiers would measure
//!   grep against itself.
//! - **The control arm keeps everything except `code_search`.** The question is
//!   whether the index *adds* anything to the tools that shipped, not whether it
//!   could replace them.
//!
//! Correctness is the criterion, rounds the tiebreaker (user's call). Grading is
//! by a marker that only a correct answer can carry — a file name, a setting, a
//! word the source uses — and the raw answers are printed so a grader that lies
//! can be caught by reading them.

use super::*;
use crate::features::tools::code_search_probe::{self as probe, Chunk, Index};

/// One question, and what an answer has to contain to count as correct.
struct Question {
    /// Asked in the user's words. Russian, like the rest of this app's live
    /// smokes — and it doubles as a fair test of a multilingual embedder against
    /// an English corpus, which is the real deployment shape.
    ask: &'static str,
    /// Any one of these in the reply counts. Several spellings, because the
    /// grader must not fail an answer that is right in different words —
    /// weakening it is safe here only because the answers are printed too.
    expect: &'static [&'static str],
}

/// The fixed set. Every one of these is answerable from the source, and none is
/// answerable from general knowledge about Rust or TUIs.
const QUESTIONS: &[Question] = &[
    Question {
        ask: "Почему это приложение иногда само сокращает переписку в чате, и чем это управляется?",
        expect: &["compact", "компакт", "сумм", "summar"],
    },
    Question {
        ask: "Что произойдёт, если модель попросит прочитать файл, лежащий выше прикреплённого каталога?",
        expect: &["canonical", "канониз", "корн", "root", "откаж", "refus"],
    },
    Question {
        ask: "Где хранятся ключи облачных провайдеров и что с ними будет на другом компьютере?",
        expect: &["dpapi", "secrets", "машин", "machine"],
    },
    Question {
        ask: "Как приложение узнаёт, что сервер вообще умеет принимать картинки?",
        expect: &["props", "modalities", "vision", "capab"],
    },
    Question {
        ask: "Что мешает двум копиям приложения испортить друг другу данные?",
        expect: &[
            "single-instance",
            "single_instance",
            "instance",
            "блокиров",
            "lock",
        ],
    },
];

/// Builds the index over `root`, embedding every chunk — or loads it from the
/// scratch cache.
///
/// Cached because the corpus is ~11 000 chunks: paying that per arm, per
/// question, per repeat would make the measurement unaffordable and would
/// change nothing about it.
async fn build_index(root: &std::path::Path, embedder: &Arc<dyn Embedder>) -> Index {
    let corpus = probe::corpus(root);
    // Keyed by what actually went in, so editing the source invalidates it.
    let digest = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for (path, start, _, body) in &corpus {
            h.update(path.as_bytes());
            h.update(start.to_le_bytes());
            h.update(body.as_bytes());
        }
        format!("{:x}", h.finalize())[..16].to_string()
    };
    let cache = probe::cache_path(&digest);
    if let Ok(bytes) = std::fs::read(&cache)
        && let Ok(chunks) = serde_json::from_slice::<Vec<Chunk>>(&bytes)
    {
        eprintln!("index: {} chunks (cached)", chunks.len());
        return Index { chunks };
    }

    eprintln!("index: embedding {} chunks…", corpus.len());
    let started = std::time::Instant::now();
    let mut chunks = Vec::with_capacity(corpus.len());
    for batch in corpus.chunks(32) {
        let texts: Vec<String> = batch.iter().map(|(_, _, _, body)| body.clone()).collect();
        let vectors = embedder
            .embed(texts, crate::shared::api::contract::EmbedRole::Passage)
            .await
            .expect("the embedding server must answer");
        for ((path, start, end, body), vector) in batch.iter().zip(vectors) {
            chunks.push(Chunk {
                path: path.clone(),
                start: *start,
                end: *end,
                text: body.clone(),
                vector,
            });
        }
        if chunks.len() % 2048 < 32 {
            eprintln!("  {} / {}", chunks.len(), corpus.len());
        }
    }
    eprintln!(
        "index: {} chunks in {:.0} s",
        chunks.len(),
        started.elapsed().as_secs_f64()
    );
    let _ = std::fs::write(&cache, serde_json::to_vec(&chunks).unwrap());
    Index { chunks }
}

/// Runs one question with the profile narrowed to `tools`, against the
/// repository as the attached project. Returns the reply and the calls made.
async fn ask(tools: &[&str], question: &str) -> Option<(String, Vec<(String, String, String)>)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_live()?;
    let _ = super::live::narrow_profile_to(
        &cmd_tx,
        &mut evt_rx,
        tools.iter().map(|id| (*id).into()).collect(),
    )
    .await;
    cmd_tx
        .send(AppCommand::ProjectAttach {
            path: root.to_string_lossy().into_owned(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProjectProgress(_)))
        .await
        .unwrap();
    let (answer, calls) = run_turn_capture_args(&cmd_tx, &mut evt_rx, question).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    drop(dir);
    Some((answer, calls))
}

/// Whether a reply carries any of the markers a correct answer must.
fn graded(answer: &str, expect: &[&str]) -> bool {
    let lower = answer.to_lowercase();
    expect.iter().any(|m| lower.contains(m))
}

/// The go/no-go itself (docs/code-workspace.md §3.7, fork F4).
///
/// Asserts **nothing** about which arm wins — that is the decision this exists
/// to inform, and an assertion would be the answer written before the
/// measurement. It prints a table and the raw replies; the verdict is recorded
/// in the plan.
///
/// `#[ignore]`, manual: `MINDFORK_ENGINE_URL=…/v1 MINDFORK_EMBED_URL=…/v1
/// MINDFORK_CODE_SEARCH_PROBE=1 cargo test code_search_probe_e2e_live --
/// --ignored --nocapture --test-threads=1`.
#[tokio::test]
#[ignore = "requires a live chat server, an embedder, and MINDFORK_CODE_SEARCH_PROBE=1"]
async fn code_search_probe_e2e_live() {
    use crate::features::tools::code::{CODE_GREP_ID, CODE_LIST_ID, CODE_READ_ID};
    use crate::features::tools::code_search_probe::CODE_SEARCH_ID;

    if std::env::var("MINDFORK_CODE_SEARCH_PROBE").is_err() {
        eprintln!("skip: MINDFORK_CODE_SEARCH_PROBE is not set");
        return;
    }
    let Some(embedder) = live_embedder() else {
        eprintln!("skip: MINDFORK_EMBED_URL not set");
        return;
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    probe::install(build_index(root, &embedder).await);

    let control: &[&str] = &[CODE_LIST_ID, CODE_READ_ID, CODE_GREP_ID];
    let treatment: &[&str] = &[CODE_LIST_ID, CODE_READ_ID, CODE_GREP_ID, CODE_SEARCH_ID];

    let mut rows: Vec<(usize, bool, usize, bool, usize)> = Vec::new();
    for (i, q) in QUESTIONS.iter().enumerate() {
        for (arm, tools) in [("grep", control), ("grep+search", treatment)] {
            let started = std::time::Instant::now();
            let Some((answer, calls)) = ask(tools, q.ask).await else {
                eprintln!("skip: MINDFORK_ENGINE_URL not set");
                return;
            };
            let ok = graded(&answer, q.expect);
            eprintln!(
                "\n=== Q{i} [{arm}] {:.0}s · {} calls · {}\nQ: {}\nA: {}\ncalls: {:?}",
                started.elapsed().as_secs_f64(),
                calls.len(),
                if ok { "CORRECT" } else { "WRONG" },
                q.ask,
                answer.chars().take(700).collect::<String>(),
                calls
                    .iter()
                    .map(|(n, a, _)| format!("{n}({a})"))
                    .collect::<Vec<_>>()
            );
            match arm {
                "grep" => rows.push((i, ok, calls.len(), false, 0)),
                _ => {
                    if let Some(row) = rows.last_mut() {
                        row.3 = ok;
                        row.4 = calls.len();
                    }
                }
            }
        }
    }

    eprintln!("\n===== go/no-go (fork F4) =====");
    eprintln!("  q | grep       | grep+search");
    for (i, a_ok, a_calls, b_ok, b_calls) in &rows {
        eprintln!(
            " Q{i} | {:<9} | {:<9}",
            format!("{} {}c", if *a_ok { "ok " } else { "err" }, a_calls),
            format!("{} {}c", if *b_ok { "ok " } else { "err" }, b_calls),
        );
    }
    let a_correct = rows.iter().filter(|r| r.1).count();
    let b_correct = rows.iter().filter(|r| r.3).count();
    let a_calls: usize = rows.iter().map(|r| r.2).sum();
    let b_calls: usize = rows.iter().map(|r| r.4).sum();
    eprintln!(
        "\ncorrect: grep {a_correct}/{n} · grep+search {b_correct}/{n}\ncalls:   grep {a_calls} · grep+search {b_calls}",
        n = rows.len()
    );
}
