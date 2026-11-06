//! Tests of the evaluation runner (docs/fabula.md §4, PR 13): each system's
//! wiring, the premises held to their hashes, the command's arguments, B0's
//! target, the batch's skipping, and one story written end to end with its
//! markdown and trace. The chapter systems' loops are tested beside the
//! chapter's (`chapter::systems_tests`).

use std::sync::Arc;

use serde_json::json;

use super::{
    Batch, Command, Ending, Job, Options, Route, System, Wire, answer_to, ended, frozen, frozen_of,
    parse, premise, recorded, run_story, sha256_hex, story_dir, target_of, wave,
};
use crate::entities::story::{Decision, QuestionBody};
use crate::features::story::chapter::ChapterConfig;
use crate::features::story::single::Target;
use crate::shared::api::contract::{ChatChunk, FinishReason, Served, TokenUsage};
use crate::shared::api::mock::MockBackend;

const HARBOUR: &str = "src/entities/story/fixtures/harbour.json";

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

#[test]
fn each_system_changes_one_thing_of_fs_chapter() {
    let base = ChapterConfig::default();
    assert_eq!(System::F.chapter(&base), Some(base.clone()));
    assert_eq!(System::B0.chapter(&base), None);
    let a1 = System::A1.chapter(&base).unwrap();
    assert!(!a1.simulate && a1.review && a1.sim == base.sim);
    let a2 = System::A2.chapter(&base).unwrap();
    assert_eq!(a2.sim.branches, 1);
    assert!(a2.simulate && a2.review);
    let a3 = System::A3.chapter(&base).unwrap();
    assert!(!a3.review && a3.simulate && a3.sim == base.sim);
    let a4 = System::A4.chapter(&base).unwrap();
    assert!(a4.sim.omniscient && a4.sim.branches == base.sim.branches);
    for system in System::ALL {
        assert_eq!(System::parse(&system.as_str().to_lowercase()), Some(system));
    }
    assert_eq!(System::parse("A5"), None);
}

#[test]
fn a_frozen_premise_is_held_to_its_recorded_hash() {
    let text = std::fs::read_to_string(HARBOUR).unwrap();
    let hash = sha256_hex(text.as_bytes());
    let sums = format!("{hash}  harbour.json\n");
    let held = frozen_of(&[("harbour", &text)], &sums).unwrap();
    assert_eq!(held[0].sha256, hash);
    assert!(held[0].frozen);
    // `sha256sum`'s binary marker reads the same.
    assert_eq!(
        recorded(&format!("{hash} *harbour.json"), "harbour"),
        Some(hash.clone())
    );
    let changed = text.replacen("No magic.", "No magic at all.", 1);
    let err = frozen_of(&[("harbour", &changed)], &sums).unwrap_err();
    assert!(err.contains("a frozen premise was changed"), "{err}");
    let err = frozen_of(&[("harbour", &text)], "").unwrap_err();
    assert!(err.contains("no hash in SHA256SUMS"), "{err}");
    // The repository's own frozen set holds to its sums.
    frozen().unwrap();
}

/// The planted tokens in `text`, `[SECRET-…]` each.
fn tokens(text: &str) -> Vec<&str> {
    text.match_indices("[SECRET-")
        .filter_map(|(at, _)| text[at..].find(']').map(|end| &text[at..=at + end]))
        .collect()
}

#[test]
fn the_twelve_frozen_premises_keep_to_the_rubric() {
    let all = frozen().unwrap();
    assert_eq!(all.len(), 12);
    let mut seen = std::collections::HashSet::new();
    for genre in ["mystery", "clash", "villain", "quiet"] {
        let n = all.iter().filter(|p| p.id.starts_with(genre)).count();
        assert_eq!(n, 3, "{genre}");
    }
    for p in &all {
        assert!(p.frozen && seen.insert(p.id.clone()), "{}", p.id);
        let canon = &p.canon;
        assert_eq!(canon.characters.len(), 3, "{}", p.id);
        assert_eq!(canon.locations.len(), 5, "{}", p.id);
        assert_eq!(canon.arc.chapters.len(), 1, "{}", p.id);
        // A token on every sheet, none used twice across the twelve: the
        // leak metric's (spec §11).
        for c in &canon.characters {
            let planted: Vec<&str> = c.secrets.iter().flat_map(|s| tokens(&s.text)).collect();
            assert!(!planted.is_empty(), "{} {}", p.id, c.key);
            for t in planted {
                assert!(seen.insert(t.to_owned()), "{t} repeats");
            }
        }
        // A frozen premise is found by its id.
        assert_eq!(premise(&p.id).unwrap(), *p);
    }
}

#[test]
fn a_premise_file_is_read_checked_and_hashed() {
    let p = premise(HARBOUR).unwrap();
    assert_eq!(p.id, "harbour");
    assert!(!p.frozen);
    assert_eq!(p.sha256, sha256_hex(&std::fs::read(HARBOUR).unwrap()));
    assert_eq!(p.canon.characters.len(), 3);
    let dir = tempfile::tempdir().unwrap();
    let roadless = dir.path().join("roadless.json");
    let mut canon: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(HARBOUR).unwrap()).unwrap();
    for place in canon["locations"].as_array_mut().unwrap() {
        place["routes"] = json!([]);
    }
    std::fs::write(&roadless, canon.to_string()).unwrap();
    let err = premise(roadless.to_str().unwrap()).unwrap_err();
    assert!(err.contains("no route leads"), "{err}");
    assert!(premise("no-such-premise").is_err());
}

#[test]
fn the_command_reads_its_arguments() {
    let run = parse(&args(&format!(
        "run --premise {HARBOUR} --system a1 --seed 2 --out out/x \
         --engine openrouter --model qwen/qwen3.8-27b --provider deepinfra --language English \
         --budget-tokens 0 --timeout-hours 0.5"
    )))
    .unwrap();
    let Command::Run {
        job,
        route,
        options,
    } = run
    else {
        panic!("not a run");
    };
    assert_eq!((job.system, job.seed), (System::A1, 2));
    assert_eq!(job.target, None);
    assert_eq!(route.wire, Wire::OpenRouter);
    assert_eq!(route.key_env, "OPENROUTER_API_KEY");
    assert_eq!(route.pin, ["deepinfra"]);
    assert_eq!(options.language, "English");
    assert_eq!(options.budget.tokens, None, "0 is no limit");
    assert_eq!(options.timeout.as_secs(), 1800);

    let batch = parse(&args(&format!(
        "batch --out out --premises {HARBOUR} --engine anthropic --model claude-sonnet-5-5"
    )))
    .unwrap();
    let Command::Batch { batch, options, .. } = batch else {
        panic!("not a batch");
    };
    // The two decisions first (T23), three seeds, in Russian (T20).
    assert_eq!(batch.systems, [System::F, System::B0, System::A1]);
    assert_eq!(batch.seeds, [1, 2, 3]);
    assert_eq!(options.language, "Russian");
    assert_eq!(options, Options::default());

    assert_eq!(parse(&args("premises")).unwrap(), Command::Premises);
    for (line, says) in [
        ("", "usage:"),
        (
            "run --premise x",
            "x is neither a frozen premise nor a readable file",
        ),
        (
            &format!("run --premise {HARBOUR} --system B0 --out o --engine openrouter --model m")
                as &str,
            "B0 needs --target-usd or --target-tokens",
        ),
        (
            &format!(
                "run --premise {HARBOUR} --system F --out o --engine anthropic --model m \
                 --provider deepinfra"
            ),
            "--provider pins OpenRouter's routing",
        ),
        (
            &format!(
                "run --premise {HARBOUR} --system F --out o --engine openrouter --model m --k 3"
            ),
            "--k is not an option",
        ),
        (
            &format!("run --premise {HARBOUR} --system F --out o --model m"),
            "--engine is required",
        ),
        ("walk", "no story-eval command is called walk"),
        (
            &format!(
                "batch --out o --premises {HARBOUR} --seeds one --engine openrouter --model m"
            ),
            "--seeds takes a whole number",
        ),
    ] {
        let err = parse(&args(line)).unwrap_err();
        assert!(err.contains(says), "{line}: {err}");
    }
}

#[test]
fn an_unset_key_is_named_by_its_variable_and_never_read_out() {
    let route = Route {
        wire: Wire::Anthropic,
        model: "m".into(),
        url: None,
        key_env: "MINDFORK_TEST_NO_SUCH_KEY_PR13".into(),
        pin: Vec::new(),
    };
    let err = route.backend().err().unwrap();
    assert_eq!(err, "set MINDFORK_TEST_NO_SUCH_KEY_PR13 to the key");
    let compatible = Route {
        wire: Wire::Compatible,
        ..route
    };
    assert_eq!(
        compatible.backend().err().unwrap(),
        "--engine compatible needs --url"
    );
}

#[test]
fn the_pauses_are_answered_as_the_plan_has_it() {
    let scene = QuestionBody::Scene {
        chapter: 1,
        scene: 2,
        rerun: 2,
        errors: Vec::new(),
    };
    assert_eq!(
        answer_to(&QuestionBody::Canon, 0),
        Some((Decision::Approve, ""))
    );
    assert_eq!(
        answer_to(&scene, 0),
        Some((Decision::Answer, "Try the scene again."))
    );
    assert_eq!(answer_to(&scene, 1), None, "once");
    let chapter = QuestionBody::Chapter {
        chapter: 1,
        version: 2,
    };
    assert_eq!(answer_to(&chapter, 1), Some((Decision::Approve, "")));
}

fn trace(kind: &str, tokens: u64, usd: Option<f64>) -> serde_json::Value {
    json!({
        "ending": {"kind": kind, "transient": kind == "stopped"},
        "spent": {"input_tokens": tokens, "output_tokens": 0, "cost_usd": usd},
    })
}

#[test]
fn b0s_target_is_the_finished_f_stories_average() {
    assert_eq!(target_of(&[]), None);
    assert_eq!(target_of(&[trace("timed_out", 9, None)]), None);
    let both = target_of(&[
        trace("finished", 1_000, Some(1.0)),
        trace("finished", 3_000, Some(2.0)),
        trace("stopped", 99_000, Some(50.0)),
    ])
    .unwrap();
    assert_eq!(both.tokens, 2_000);
    assert_eq!(both.usd, Some(1.5));
    let mixed = target_of(&[
        trace("finished", 1_000, Some(1.0)),
        trace("finished", 3_000, None),
    ])
    .unwrap();
    assert_eq!(mixed.usd, None, "dollars only where every story has them");
    assert!(ended(&trace("finished", 0, None)));
    assert!(
        !ended(&trace("stopped", 0, None)),
        "a failed call is tried again"
    );
    assert!(!ended(&json!({"ending": {"kind": "timed_out"}})));
    assert!(ended(
        &json!({"ending": {"kind": "stopped", "transient": false}})
    ));
}

#[tokio::test]
async fn a_batch_leaves_out_what_ended_and_waits_for_f_before_b0() {
    let out = tempfile::tempdir().unwrap();
    let harbour = premise(HARBOUR).unwrap();
    let batch = Batch {
        premises: vec![harbour],
        systems: vec![System::F, System::B0, System::A1],
        seeds: vec![1, 2],
        out: out.path().to_owned(),
        parallel: 2,
    };
    let names = |jobs: &[Job]| -> Vec<String> {
        jobs.iter()
            .map(|j| format!("{}/s{}", j.system.as_str(), j.seed))
            .collect()
    };
    assert_eq!(
        names(&wave(&batch, false).await),
        ["F/s1", "F/s2", "A1/s1", "A1/s2"]
    );
    assert!(
        wave(&batch, true).await.is_empty(),
        "no F story has finished"
    );
    let write = |system: System, seed: u64, trace: serde_json::Value| {
        let dir = story_dir(out.path(), "harbour", system, seed);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("trace.json"), trace.to_string()).unwrap();
    };
    write(System::F, 1, trace("finished", 1_000, None));
    write(System::F, 2, trace("stopped", 5, None));
    write(System::A1, 1, trace("finished", 10, None));
    assert_eq!(names(&wave(&batch, false).await), ["F/s2", "A1/s2"]);
    let b0 = wave(&batch, true).await;
    assert_eq!(names(&b0), ["B0/s1", "B0/s2"]);
    assert_eq!(
        b0[0].target,
        Some(Target {
            usd: None,
            tokens: 1_000
        })
    );
    assert!(b0[0].dir.ends_with("harbour/B0/s1"));
}

fn turn(text: &str) -> Vec<ChatChunk> {
    vec![
        ChatChunk::Text(text.to_owned()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 120,
            completion_tokens: 30,
            ..TokenUsage::default()
        }),
        ChatChunk::Served(Served {
            provider: Some("DeepInfra".to_owned()),
            cost_nanos: Some(1_000_000),
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ]
}

#[tokio::test]
async fn a_story_is_written_into_its_directory_with_its_markdown_and_trace() {
    let out = tempfile::tempdir().unwrap();
    let job = Job {
        premise: premise(HARBOUR).unwrap(),
        system: System::B0,
        seed: 2,
        dir: story_dir(out.path(), "harbour", System::B0, 2),
        target: Some(Target {
            usd: Some(0.0025),
            tokens: u64::MAX,
        }),
    };
    let route = Route {
        wire: Wire::OpenRouter,
        model: "qwen/qwen3.8-27b".into(),
        url: None,
        key_env: "OPENROUTER_API_KEY".into(),
        pin: vec!["deepinfra".into()],
    };
    let backend = Arc::new(MockBackend::sequence(vec![
        turn("## Draft\n\nOne two three."),
        turn("Too short."),
        turn("## Again\n\nOne two three four."),
        turn("Still short."),
        turn("## Third\n\nNever asked for."),
    ]));
    let trace = run_story(&job, &route, backend, &Options::default())
        .await
        .unwrap();
    // A thousandth of a dollar a turn: three turns pass the quarter cent.
    assert_eq!(trace["ending"], json!({"kind": "finished", "how": "spent"}));
    assert_eq!(trace["system"], "B0");
    assert_eq!(trace["seed"], 2);
    assert_eq!(trace["premise"], "harbour");
    assert_eq!(trace["sha256"], job.premise.sha256);
    assert_eq!(trace["language"], "Russian");
    assert_eq!(trace["versions"], 2);
    assert_eq!(trace["spent"]["calls"], 3);
    assert_eq!(trace["roles"]["author"]["calls"], 3);
    assert_eq!(trace["route"]["pin"], json!(["deepinfra"]));
    assert_eq!(trace["leaks"], json!([]));
    let markdown = std::fs::read_to_string(job.dir.join("story.md")).unwrap();
    assert_eq!(markdown, "## Again\n\nOne two three four.\n");
    // The heading's `##` is no word (F2's count).
    assert_eq!(trace["words"], 5);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(job.dir.join("trace.json")).unwrap())
            .unwrap();
    assert_eq!(written, trace);
    assert!(job.dir.join("story.db").exists() && job.dir.join("calls.db").exists());
    assert!(ended(&trace));
    assert!(Ending::Finished("spent".into()).is_final());
}

/// A story its time limit cuts stops at the step in flight, its trace says
/// so, and a batch tries it again (docs/fabula.md §6, T2).
#[tokio::test]
async fn a_story_cut_by_its_time_limit_is_told_so_and_is_not_ended() {
    let out = tempfile::tempdir().unwrap();
    let job = Job {
        premise: premise(HARBOUR).unwrap(),
        system: System::B0,
        seed: 1,
        dir: story_dir(out.path(), "harbour", System::B0, 1),
        target: Some(Target {
            usd: None,
            tokens: 1,
        }),
    };
    let route = Route {
        wire: Wire::Compatible,
        model: "m".into(),
        url: Some("http://127.0.0.1:9".into()),
        key_env: "MINDFORK_ENGINE_KEY".into(),
        pin: Vec::new(),
    };
    let options = Options {
        timeout: std::time::Duration::from_millis(50),
        ..Options::default()
    };
    let backend = Arc::new(crate::features::story::run::tests::Hanging::default());
    let trace = run_story(&job, &route, backend, &options).await.unwrap();
    assert_eq!(
        trace["ending"],
        json!({"kind": "timed_out", "step": "b0/t1"})
    );
    assert!(!ended(&trace));
}

#[test]
fn the_claude_models_m3_may_run_on_have_a_list_price() {
    let route = |wire, model: &str| Route {
        wire,
        model: model.into(),
        url: None,
        key_env: "K".into(),
        pin: Vec::new(),
    };
    let sonnet = route(Wire::Anthropic, "claude-sonnet-5-5")
        .list_price()
        .unwrap();
    assert_eq!(
        (sonnet.input, sonnet.output, sonnet.long),
        (2.0, 10.0, None)
    );
    let haiku = route(Wire::Anthropic, "claude-haiku-5-5")
        .list_price()
        .unwrap();
    let long = haiku.long.unwrap();
    assert_eq!((haiku.input, haiku.output), (0.10, 0.50));
    assert_eq!((long.above, long.input, long.output), (100_000, 0.50, 2.50));
    // A model with no price here, and any other wire, are matched in tokens.
    assert_eq!(route(Wire::Anthropic, "claude-opus-5-5").list_price(), None);
    assert_eq!(
        route(Wire::OpenRouter, "claude-haiku-5-5").list_price(),
        None
    );
    let json = route(Wire::Anthropic, "claude-haiku-5-5").to_json();
    assert_eq!(json["list_price"]["long"]["above"], 100_000);
}

/// A turn Anthropic answers with no price, its prompt past Haiku's 100,000.
fn unpriced(text: &str) -> Vec<ChatChunk> {
    vec![
        ChatChunk::Text(text.to_owned()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 120_000,
            completion_tokens: 2_000,
            ..TokenUsage::default()
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ]
}

#[tokio::test]
async fn b0_on_claude_spends_to_fs_dollars_at_the_list_price() {
    let out = tempfile::tempdir().unwrap();
    let job = Job {
        premise: premise(HARBOUR).unwrap(),
        system: System::B0,
        seed: 1,
        dir: story_dir(out.path(), "harbour", System::B0, 1),
        target: Some(Target {
            usd: Some(0.1),
            tokens: u64::MAX,
        }),
    };
    let route = Route {
        wire: Wire::Anthropic,
        model: "claude-haiku-5-5".into(),
        url: None,
        key_env: "CLAUDE_API_KEY".into(),
        pin: Vec::new(),
    };
    let backend = Arc::new(MockBackend::sequence(vec![
        unpriced("## Draft\n\nOne two three."),
        unpriced("Too short."),
        unpriced("## Again\n\nOne two three four."),
        unpriced("Never asked for."),
    ]));
    let trace = run_story(&job, &route, backend, &Options::default())
        .await
        .unwrap();
    // $0.065 a turn: the draft and one critique and revision pass $0.10,
    // which tokens alone (a target of u64::MAX) would never reach.
    assert_eq!(trace["ending"], json!({"kind": "finished", "how": "spent"}));
    assert_eq!(trace["spent"]["calls"], 3);
    let cost = trace["spent"]["cost_usd"].as_f64().unwrap();
    assert!((cost - 0.195).abs() < 1e-9, "{cost}");
    assert_eq!(trace["route"]["list_price"]["input"], 0.10);
}
