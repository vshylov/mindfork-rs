//! The live tests of `features::tools::fetch` that need an engine and not the
//! network. Kept under a `tests/` directory so that coverage — which runs
//! without `--ignored` — leaves them out, as it does the orchestrator's live
//! suite.

use std::sync::Arc;

use uuid::Uuid;

use super::*;
use crate::shared::api::Embedder;
use crate::shared::api::mock::MockEmbedder;
use crate::shared::session_budget::{SessionBudget, Shape};

/// The page `summary_usage_e2e_live` summarizes: GitHub's API answer for one
/// repository, the JSON measured to under-count most (1.28,
/// docs/research/page-summary-usage.md §2.1), saved on 2026-10-10.
///
/// A fixture, not a fetch. The smoke measures the summary's usage, and the
/// fetch in front of it was the part that failed: `api.github.com` allows an
/// address sixty unauthenticated requests an hour, a CI runner shares its
/// address, and on 2026-10-10 the gate's runner got a `403` — the summary was
/// never asked for, and the smoke reported "a JSON page under-counts" about a
/// page it never had. Fetching is the network smokes' to prove.
const GITHUB_REPO_JSON: &str = include_str!("../../../../../tests/fixtures/github-repo-rust.json");
const GITHUB_REPO_URL: &str = "https://api.github.com/repos/rust-lang/rust";

/// The summary's usage on a live engine (docs/research/page-summary-usage.md
/// §6, fork F4a): a JSON page — the text measured to under-count most —
/// summarized under a budget of four sessions over the LAN stack's pool, on
/// the path a fetched page of its size takes. The result carries a summary
/// (reasoning muted: not the "summary unavailable" fallback with the JSON
/// behind it), the budget's `Summary` ratio is above 1.0, and the outcome
/// carries the engine's timing of the prompt.
///
/// `MINDFORK_ENGINE_URL=…/v1 cargo test summary_usage_e2e_live -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "requires a running llama-server (MINDFORK_ENGINE_URL)"]
async fn summary_usage_e2e_live() {
    let Some(client) =
        crate::shared::api::live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(16));
    let (_d, _storage, mut ctx) =
        super::super::testkit::ctx_with_backends(Uuid::new_v4(), Arc::new(client), embedder);
    let budget = Arc::new(SessionBudget::new(4, Some(16_384)));
    ctx.sessions = Some(budget.clone());
    // The body reaches the summary as a fetched JSON body does: as is, and
    // small enough for the inline path `invoke` would choose.
    let page = body_to_text("application/json", GITHUB_REPO_JSON).expect("the fixture is JSON");
    assert!(
        crate::shared::tokens::estimate_text(&page.text) as usize
            <= ctx.attachment_cfg.max_file_tokens,
        "the page must take the inline path, as the fetched one did"
    );
    let started = std::time::Instant::now();
    let out = FetchUrl::default()
        .inline_result(&ctx, GITHUB_REPO_URL, None, true, &page)
        .await;
    eprintln!(
        "summary_usage_e2e_live: {:.1} s, ratio {:.2}, sample {:?}\n{}",
        started.elapsed().as_secs_f64(),
        budget.density(Shape::Summary),
        out.prefill,
        out.result
    );
    assert!(
        !out.result.contains("\"node_id\""),
        "the JSON itself came back, not a summary: {}",
        out.result
    );
    assert!(!out.result.trim().is_empty());
    assert!(
        budget.density(Shape::Summary) > 1.0,
        "a JSON page under-counts: {budget:?}"
    );
    assert_eq!(budget.density(Shape::Turn), 1.0, "no other kind touched");
    assert!(
        out.prefill.is_some(),
        "the engine's timing rides the outcome"
    );
}
