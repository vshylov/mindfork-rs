//! Tests of M3's blind judging (docs/fabula.md §4, PR 14): the pairs and
//! their names, what a judge is shown, the controls' shuffle, both orders,
//! the count of the votes and the binomial test, and one judge's run written
//! end to end and resumed.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use super::{
    Ask, Judge, Judging, Matchup, Order, Ratings, Reading, Record, Rival, instructions, judge,
    pair_of, prompt, records, shown, shuffled, tail, verdict,
};
use crate::entities::sampling::ReasoningEffort;
use crate::features::story::eval::{Command, Route, System, Wire, parse, story_dir};
use crate::features::story::port::Role;
use crate::shared::api::contract::{ChatChunk, FinishReason, Served, TokenUsage};
use crate::shared::api::mock::MockBackend;
use crate::shared::storage::story::StoryDb;

#[test]
fn a_pair_is_named_and_read_back() {
    for name in ["F-B0", "F-A1", "A1-B0", "F-control"] {
        assert_eq!(Matchup::parse(name).unwrap().name(), name);
    }
    assert_eq!(Matchup::parse("control"), Matchup::parse("F-control"));
    assert_eq!(
        Matchup::parse("f-b0"),
        Some(Matchup {
            system: System::F,
            rival: Rival::System(System::B0),
        })
    );
    // A story is never set against its own system's.
    for wrong in ["F-F", "F", "F-A9", "X-B0", ""] {
        assert_eq!(Matchup::parse(wrong), None, "{wrong}");
    }
    assert_eq!(
        Matchup::DECISIONS.map(Matchup::name),
        ["F-B0".to_owned(), "F-A1".to_owned()]
    );
}

#[test]
fn a_judge_is_shown_the_text_with_its_scene_breaks_evened_out() {
    let story = "## Лоток у двери\n\nПервый абзац.\n\n---\n\nВторой.\n\n***\n\n\\*\n\n\
                 ### Аино\n\n– Нина Сергеевна, сядьте.\n\n*\n\nКонец.\n";
    assert_eq!(
        shown(story),
        "## Лоток у двери\n\nПервый абзац.\n\n* * *\n\nВторой.\n\n* * *\n\n### Аино\n\n\
         – Нина Сергеевна, сядьте.\n\n* * *\n\nКонец."
    );
    // A line of dialogue that opens on a dash, and a word between asterisks,
    // are text.
    assert_eq!(shown("– Да.\n\n*Нет.*"), "– Да.\n\n*Нет.*");
}

#[test]
fn a_controls_rival_is_its_own_paragraphs_in_another_order() {
    let text = "## Счёт до семи\n\nодин\n\nдва\n\nтри\n\nчетыре\n\nпять";
    let mixed = shuffled(text, "quiet-2/s1");
    assert!(mixed.starts_with("## Счёт до семи\n\n"), "{mixed}");
    assert_ne!(mixed, text);
    let mut was: Vec<&str> = text.split("\n\n").collect();
    let mut now: Vec<&str> = mixed.split("\n\n").collect();
    was.sort_unstable();
    now.sort_unstable();
    assert_eq!(was, now);
    // The draw is the story's own: the same key, the same order.
    assert_eq!(shuffled(text, "quiet-2/s1"), mixed);
    // Two paragraphs are never left as they were, whatever the draw.
    for key in 0..16 {
        assert_eq!(shuffled("а\n\nб", &key.to_string()), "б\n\nа", "{key}");
    }
}

#[test]
fn every_pair_is_asked_in_both_orders_each_at_its_own_address() {
    let judging = Judging {
        stories: "run".into(),
        out: "out".into(),
        premises: vec!["villain-4".into(), "quiet-2".into()],
        seeds: vec![1, 2],
        matchups: Matchup::DECISIONS.to_vec(),
        parallel: 1,
    };
    let asks = judging.asks();
    assert_eq!(asks.len(), 2 * 2 * 2 * 2);
    let mut addresses: Vec<String> = asks.iter().map(Ask::address).collect();
    addresses.sort();
    addresses.dedup();
    assert_eq!(addresses.len(), asks.len());
    assert_eq!(asks[0].address(), "villain-4/s1/F-B0/ab");
    assert_eq!(asks[1].address(), "villain-4/s1/F-B0/ba");
    assert!(
        asks[1]
            .file(Path::new("out/fable"))
            .ends_with("villain-4/s1/F-B0.ba.json")
    );
}

#[test]
fn the_prompt_names_neither_the_systems_nor_the_engine() {
    let story = |text: &str| super::Story {
        text: text.to_owned(),
        words: 2,
        language: "Russian".to_owned(),
    };
    let p = prompt(
        &story("## Первый\n\nтекст один"),
        &story("## Второй\n\nтекст два"),
    );
    assert_eq!(p.system, instructions("Russian"));
    let first = p.user.find("Первый").unwrap();
    let second = p.user.find("Второй").unwrap();
    assert!(first < second);
    assert!(p.user.starts_with("=== Story 1 ==="));
    let both = format!("{}\n{}", p.system, p.user);
    for told in [
        "B0",
        "A1",
        "Fabula",
        "simulation",
        "system",
        "engine",
        "agent",
    ] {
        assert!(!both.contains(told), "{told}");
    }
    assert!(p.system.contains("written in Russian"));
    assert!(p.system.contains("There is no tie."));
    let schema = Reading::schema("Russian").to_json();
    assert!(schema.contains("from 1 to 10"));
    assert!(schema.contains("sentences of Russian"));
}

#[test]
fn the_tail_is_the_one_sided_binomial_test_of_spec_11() {
    // 24 of 36, p ≈ 0.03, where 23 does not pass at 0.05.
    assert!((tail(36, 24) - 0.0326).abs() < 0.0005, "{}", tail(36, 24));
    assert!(tail(36, 23) > 0.05);
    assert_eq!(tail(1, 1), 0.5);
    assert_eq!(tail(3, 0), 1.0);
    assert_eq!(tail(3, 4), 0.0);
    assert!((tail(3, 1) - 0.875).abs() < 1e-12);
}

/// A reading of a judge: `chose` the side, `words` of the weighed system's
/// story and of its rival's.
fn record(judge: &str, seed: u64, matchup: &str, order: &str, chose: &str) -> Record {
    let m = Matchup::parse(matchup).unwrap();
    let sides = [m.system.as_str().to_owned(), m.rival.label().to_owned()];
    let (shown, words) = if order == "ab" {
        (sides.clone(), [100, 80])
    } else {
        ([sides[1].clone(), sides[0].clone()], [80, 100])
    };
    let ratings = Ratings {
        plot: 7,
        character: 7,
        surprise: 6,
        prose: 8,
    };
    Record {
        judge: judge.to_owned(),
        model: format!("{judge}-model"),
        premise: "villain-4".to_owned(),
        seed,
        matchup: m.name(),
        order: order.to_owned(),
        words,
        chose: chose.to_owned(),
        reading: Reading {
            story_1: ratings.clone(),
            story_2: ratings,
            reason: "Причина.".to_owned(),
            better: if shown[0] == chose { "1" } else { "2" }.to_owned(),
        },
        shown,
        spent: json!({"calls": 1, "input_tokens": 9000, "output_tokens": 1000, "cost_usd": 0.5}),
    }
}

#[test]
fn a_vote_counts_only_when_both_orders_agree_and_a_tie_is_no_win() {
    let mut all = Vec::new();
    // Seed 1: x and y choose F both ways, z changes its mind with the order.
    for (j, ab, ba) in [("x", "F", "F"), ("y", "F", "F"), ("z", "F", "B0")] {
        all.push(record(j, 1, "F-B0", "ab", ab));
        all.push(record(j, 1, "F-B0", "ba", ba));
    }
    // Seed 2: x for B0, y for F, z read it one way only.
    for (j, ab, ba) in [("x", "B0", "B0"), ("y", "F", "F")] {
        all.push(record(j, 2, "F-B0", "ab", ab));
        all.push(record(j, 2, "F-B0", "ba", ba));
    }
    all.push(record("z", 2, "F-B0", "ab", "F"));
    // Seed 3: all three for B0.
    for j in ["x", "y", "z"] {
        all.push(record(j, 3, "F-B0", "ab", "B0"));
        all.push(record(j, 3, "F-B0", "ba", "B0"));
    }
    // A control x read both ways, keeping the original both times, and one
    // y read one way, choosing the shuffled.
    all.push(record("x", 1, "F-control", "ab", "F"));
    all.push(record("x", 1, "F-control", "ba", "F"));
    all.push(record("y", 1, "F-control", "ab", "shuffled"));

    let v = verdict(&all);
    let f_b0 = &v["matchups"]["F-B0"];
    assert_eq!(f_b0["weighed"], "F");
    assert_eq!(
        (
            &f_b0["pairs"],
            &f_b0["wins"],
            &f_b0["losses"],
            &f_b0["ties"]
        ),
        (&json!(3), &json!(1), &json!(1), &json!(1))
    );
    assert!((f_b0["p"].as_f64().unwrap() - 0.875).abs() < 1e-12);
    let each = f_b0["each"].as_array().unwrap();
    assert_eq!(
        each.iter()
            .map(|e| e["outcome"].clone())
            .collect::<Vec<_>>(),
        [json!("win"), json!("tie"), json!("loss")]
    );
    assert_eq!(each[0]["votes"]["z"], json!(null));
    assert_eq!(each[1]["votes"]["z"], json!(null), "one order is no vote");
    // Each judge's own count, its abstentions among its pairs.
    assert_eq!(f_b0["judges_alone"]["x"]["wins"], 1);
    assert_eq!(f_b0["judges_alone"]["y"]["wins"], 2);
    assert_eq!(f_b0["judges_alone"]["z"]["wins"], 0);
    assert_eq!(f_b0["judges_alone"]["z"]["pairs"], 3);

    let x = &v["judges"]["x"];
    assert_eq!(x["readings"], 8);
    assert_eq!(x["pairs_read_both_ways"], 4);
    assert_eq!(x["same_choice_both_ways"], 4);
    assert_eq!(
        (&x["controls"], &x["controls_kept_the_original"]),
        (&json!(2), &json!(2))
    );
    let y = &v["judges"]["y"];
    assert_eq!(
        (&y["controls"], &y["controls_kept_the_original"]),
        (&json!(1), &json!(0))
    );
    assert_eq!(x["model"], "x-model");
    assert!((x["cost_usd"].as_f64().unwrap() - 4.0).abs() < 1e-9);
    assert_eq!(v["judges"]["z"]["pairs_read_both_ways"], 2);
    assert_eq!(v["judges"]["z"]["same_choice_both_ways"], 1);
    // Story 1 is chosen where the choice is the side shown first: F on
    // seed 1 read ab, B0 on seeds 2 and 3 read ba, and the control read ab.
    assert_eq!(x["chose_story_1"], 4);

    // x and y voted on all three pairs and agreed on two; z on two of them.
    assert_eq!(v["agreement"]["x/y"], json!({"pairs": 3, "same": 2}));
    assert_eq!(v["agreement"]["x/z"], json!({"pairs": 1, "same": 1}));
    // F's story is the longer one in every reading that is not a control,
    // and F is chosen in 2 of x's, 4 of y's and 2 of z's.
    assert_eq!(v["longer_chosen"], json!({"readings": 17, "longer": 8}));
}

/// A finished story of the run, written as the runner writes one.
fn written(root: &Path, system: System, seed: u64, text: &str, words: u64) {
    let dir = story_dir(root, "villain-4", system, seed);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("story.md"), text).unwrap();
    let trace = json!({
        "ending": {"kind": "finished", "how": "accepted"},
        "words": words,
        "language": "Russian",
    });
    std::fs::write(dir.join("trace.json"), trace.to_string()).unwrap();
}

fn reply(better: &str) -> Vec<ChatChunk> {
    let r = json!({"plot": 6, "character": 7, "surprise": 5, "prose": 8});
    let body = json!({"story_1": r, "story_2": r, "reason": "Цельнее.", "better": better});
    vec![
        ChatChunk::Text(body.to_string()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 9_000,
            completion_tokens: 2_000,
            ..TokenUsage::default()
        }),
        ChatChunk::Served(Served {
            provider: Some("OpenAI".to_owned()),
            cost_nanos: Some(190_000_000),
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ]
}

fn astra() -> Judge {
    Judge {
        name: "astra".into(),
        route: Route {
            wire: Wire::OpenRouter,
            model: "openai/gpt-6-astra".into(),
            url: None,
            key_env: "OPENROUTER_API_KEY".into(),
            pin: vec!["OpenAI".into()],
        },
        effort: ReasoningEffort::Max,
        max_tokens: 64_000,
    }
}

/// Every call the judge's log holds, across its runs.
async fn logged(dir: &Path) -> Vec<crate::shared::storage::story::StoredCall> {
    let db = StoryDb::open(&dir.join("story.db"), &dir.join("calls.db"))
        .await
        .unwrap();
    let mut calls = Vec::new();
    for run in db.runs().await.unwrap() {
        calls.extend(db.calls(&run).await.unwrap());
    }
    db.close().await.unwrap();
    calls
}

#[tokio::test]
async fn a_judge_reads_each_pair_both_ways_writes_each_reading_and_resumes() {
    let root = tempfile::tempdir().unwrap();
    let stories = root.path().join("run");
    let out = root.path().join("judged");
    written(
        &stories,
        System::F,
        1,
        "## Эф\n\nтекст эф\n\n---\n\nдальше",
        4,
    );
    written(&stories, System::B0, 1, "## Бэ\n\nтекст бэ", 3);
    // A1's story did not finish: its pair is left out.
    let a1 = story_dir(&stories, "villain-4", System::A1, 1);
    std::fs::create_dir_all(&a1).unwrap();
    std::fs::write(a1.join("story.md"), "## А\n\nобрыв").unwrap();
    std::fs::write(
        a1.join("trace.json"),
        json!({"ending": {"kind": "stopped"}}).to_string(),
    )
    .unwrap();
    let judging = Judging {
        stories: stories.clone(),
        out: out.clone(),
        premises: vec!["villain-4".into()],
        seeds: vec![1],
        matchups: Matchup::DECISIONS.to_vec(),
        parallel: 1,
    };
    // Both readings choose Story 1: F shown first, then B0 shown first.
    let backend = Arc::new(MockBackend::sequence(vec![reply("1"), reply("1")]));
    judge(&judging, &astra(), backend).await.unwrap();

    let dir = out.join("astra");
    let read = |order: &str| -> Record {
        let file = dir.join(format!("villain-4/s1/F-B0.{order}.json"));
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    };
    let (ab, ba) = (read("ab"), read("ba"));
    assert_eq!(ab.shown, ["F".to_owned(), "B0".to_owned()]);
    assert_eq!(ab.chose, "F");
    assert_eq!(ba.shown, ["B0".to_owned(), "F".to_owned()]);
    assert_eq!(ba.chose, "B0");
    assert_eq!((ab.words, ba.words), ([4, 3], [3, 4]));
    assert_eq!(ab.model, "openai/gpt-6-astra");
    assert_eq!(ab.spent["calls"], 1);
    assert_eq!(ab.spent["output_tokens"], 2_000);
    assert!((ab.spent["cost_usd"].as_f64().unwrap() - 0.19).abs() < 1e-9);
    assert!(!dir.join("villain-4/s1/F-A1.ab.json").exists());

    // The call log holds the readings as the judge's, sent blind, with the
    // judge's sampling: thinking at its effort, no temperature.
    let calls = logged(&dir).await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].call.role, Role::Judge.as_str());
    assert_eq!(calls[0].call.address, "villain-4/s1/F-B0/ab");
    let sent = calls[1].call.request.to_string();
    assert!(
        sent.find("Бэ").unwrap() < sent.find("Эф").unwrap(),
        "B0 first"
    );
    assert!(sent.contains("* * *") && !sent.contains("---"));
    assert!(!sent.contains("B0") && !sent.contains("\"F\""));
    assert_eq!(calls[0].call.params["reasoning_effort"], "max");
    assert_eq!(calls[0].call.params["thinking"], true);
    assert_eq!(calls[0].call.params["temperature"], json!(null));

    // Run again, nothing is asked: an empty engine would give no reading.
    let idle = Arc::new(MockBackend::sequence(Vec::new()));
    judge(&judging, &astra(), idle).await.unwrap();
    assert_eq!(logged(&dir).await.len(), 2);

    let found = records(&out);
    assert_eq!(found.len(), 2);
    let v = verdict(&found);
    // One judge whose two readings disagree: no vote, a tie.
    assert_eq!(v["matchups"]["F-B0"]["ties"], 1);
    assert_eq!(v["judges"]["astra"]["chose_story_1"], 2);
}

#[tokio::test]
async fn a_reading_the_model_spoils_is_left_for_the_next_run() {
    let root = tempfile::tempdir().unwrap();
    let stories = root.path().join("run");
    written(&stories, System::F, 1, "## Эф\n\nтекст", 2);
    written(&stories, System::B0, 1, "## Бэ\n\nтекст", 2);
    let judging = Judging {
        stories,
        out: root.path().join("judged"),
        premises: vec!["villain-4".into()],
        seeds: vec![1],
        matchups: vec![Matchup::DECISIONS[0]],
        parallel: 1,
    };
    let spoilt = || {
        vec![
            ChatChunk::Text("{\"better\": \"3\"}".to_owned()),
            ChatChunk::Finished(FinishReason::Stop),
        ]
    };
    let backend = Arc::new(MockBackend::sequence(vec![spoilt(), spoilt(), reply("2")]));
    judge(&judging, &astra(), backend).await.unwrap();
    let dir = judging.out.join("astra");
    assert!(!dir.join("villain-4/s1/F-B0.ab.json").exists());
    let ba: Record = serde_json::from_str(
        &std::fs::read_to_string(dir.join("villain-4/s1/F-B0.ba.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ba.chose, "F");
}

#[test]
fn a_controls_pair_is_the_story_against_itself_shuffled() {
    let root = tempfile::tempdir().unwrap();
    written(root.path(), System::F, 2, "## Т\n\na\n\nb\n\nc\n\nd", 4);
    let ask = Ask {
        premise: "villain-4".into(),
        seed: 2,
        matchup: Matchup::parse("control").unwrap(),
        order: Order::Ab,
    };
    let (a, b) = pair_of(root.path(), &ask).unwrap();
    assert_eq!(a.text, "## Т\n\na\n\nb\n\nc\n\nd");
    assert_eq!(b.text, shuffled(&a.text, "villain-4/s2"));
    assert_eq!((a.words, b.words), (4, 4));
    let missing = Ask {
        matchup: Matchup::DECISIONS[0],
        ..ask
    };
    assert_eq!(pair_of(root.path(), &missing), None);
}

#[test]
fn the_judge_and_verdict_commands_read_their_arguments() {
    let Command::Judge { judging, judge } = parse(&args(
        "judge --stories run --out judged --name argon --premises villain-4 --seeds 1 \
         --pairs F-B0,control --effort high --engine openrouter --model google/gemini-4-argon \
         --provider google-ai-studio",
    ))
    .unwrap() else {
        panic!("not a judging");
    };
    assert_eq!(judging.premises, ["villain-4"]);
    assert_eq!(judging.seeds, [1]);
    assert_eq!(
        judging
            .matchups
            .iter()
            .map(|m| m.name())
            .collect::<Vec<_>>(),
        ["F-B0", "F-control"]
    );
    assert_eq!(judging.parallel, 4);
    assert_eq!(judge.name, "argon");
    assert_eq!(judge.effort, ReasoningEffort::High);
    assert_eq!(judge.max_tokens, 64_000);
    assert_eq!(judge.route.pin, ["google-ai-studio"]);
    assert_eq!(judge.sampling().temperature, None);

    let Command::Judge { judging, judge } = parse(&args(
        "judge --stories run --out judged --name fable --engine anthropic \
         --model claude-fable-5-1 --key-env CLAUDE_API_KEY",
    ))
    .unwrap() else {
        panic!("not a judging");
    };
    // Every frozen premise, three seeds, the two decisions, the most effort.
    assert_eq!(judging.premises.len(), 12);
    assert_eq!(judging.seeds, [1, 2, 3]);
    assert_eq!(judging.matchups, Matchup::DECISIONS);
    assert_eq!(judge.effort, ReasoningEffort::Max);
    // Fable's dollars are counted at its list price: Anthropic reports none.
    let price = judge.route.list_price().unwrap();
    assert_eq!((price.input, price.output, price.long), (10.0, 50.0, None));

    assert_eq!(
        parse(&args("verdict --out judged")).unwrap(),
        Command::Verdict {
            out: "judged".into()
        }
    );
    for (line, says) in [
        (
            "judge --out o --name n --engine openrouter --model m",
            "--stories is required",
        ),
        (
            "judge --stories s --out o --engine openrouter --model m",
            "--name is required",
        ),
        (
            "judge --stories s --out o --name n --pairs F-F --engine openrouter --model m",
            "no pair is called F-F",
        ),
        (
            "judge --stories s --out o --name n --effort huge --engine openrouter --model m",
            "--effort takes",
        ),
        ("verdict", "--out is required"),
    ] {
        let err = parse(&args(line)).unwrap_err();
        assert!(err.contains(says), "{line}: {err}");
    }
}

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}
