//! M3's blind judging (docs/fabula.md §4, PR 14; docs/research/fabula-engine.md
//! §11): every pair of stories read by a panel of models, in both orders, and
//! the decision rule's count.
//!
//! - **The judges are models** (docs/fabula.md §6, T24): three flagships of
//!   three families take the place of the three people, each run by its own
//!   `story-eval judge` on its own wire, its calls in its own call log.
//! - **Blind.** A judge reads two stories as "Story 1" and "Story 2", their
//!   scene breaks evened out, and nothing of the systems that wrote them.
//! - **Both orders.** Each pair is read twice, its stories swapped; a judge's
//!   vote counts only when both readings choose the same story.
//! - **Controls.** A story against its own paragraphs shuffled: every judge
//!   must choose the story as it was written.
//! - **Resumable.** A reading already written is not asked again.
//! - **The count** (`story-eval verdict`): a pair goes to the side with more
//!   votes, a tie is no side's win, and the wins are held to the one-sided
//!   binomial test of spec §11.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::engine::{EngineAdapter, SharedLog};
use super::eval::{Route, System, story_dir};
use super::port::{CallAddress, LlmPort, PortError, Prompt, Role, decide};
use super::schema::{Property, Schema};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::entities::story::RunId;
use crate::shared::api::contract::EngineBackend;
use crate::shared::storage::story::StoryDb;

// ---------- the pairs ----------

/// What a pair sets its first system's story against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rival {
    /// Another system's story of the same premise and seed.
    System(System),
    /// The same story, its paragraphs shuffled: a control.
    Shuffled,
}

impl Rival {
    /// Its side's name in a verdict.
    pub fn label(self) -> &'static str {
        match self {
            Rival::System(system) => system.as_str(),
            Rival::Shuffled => "shuffled",
        }
    }
}

/// Two stories to set side by side, by the comparison's name: `F-B0`,
/// `F-A1`, `A1-B0`, or `F-control`, F's story against itself shuffled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Matchup {
    /// The system whose story is weighed.
    pub system: System,
    /// What it is weighed against.
    pub rival: Rival,
}

impl Matchup {
    /// The two decisions of spec §11, F against B0 and against A1.
    pub const DECISIONS: [Matchup; 2] = [
        Matchup {
            system: System::F,
            rival: Rival::System(System::B0),
        },
        Matchup {
            system: System::F,
            rival: Rival::System(System::A1),
        },
    ];

    /// Its name, `F-B0` or `F-control`.
    pub fn name(self) -> String {
        match self.rival {
            Rival::System(rival) => format!("{}-{}", self.system.as_str(), rival.as_str()),
            Rival::Shuffled => format!("{}-control", self.system.as_str()),
        }
    }

    /// The matchup a name spells; `control` alone is F's.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("control") {
            return Some(Matchup {
                system: System::F,
                rival: Rival::Shuffled,
            });
        }
        let (system, rival) = name.split_once('-')?;
        let system = System::parse(system)?;
        let rival = if rival.eq_ignore_ascii_case("control") {
            Rival::Shuffled
        } else {
            Rival::System(System::parse(rival).filter(|r| *r != system)?)
        };
        Some(Matchup { system, rival })
    }
}

/// Which story a reading shows first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Order {
    /// The weighed system's story first.
    Ab,
    /// Its rival's first.
    Ba,
}

impl Order {
    /// Both, as every pair is read.
    pub const BOTH: [Order; 2] = [Order::Ab, Order::Ba];

    /// Its name in a reading's file.
    pub fn as_str(self) -> &'static str {
        match self {
            Order::Ab => "ab",
            Order::Ba => "ba",
        }
    }
}

/// One reading to ask for: a matchup on a premise and seed, in an order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The premise.
    pub premise: String,
    /// The seed.
    pub seed: u64,
    /// The pair.
    pub matchup: Matchup,
    /// The order.
    pub order: Order,
}

impl Ask {
    /// Its address in the judge's call log, and the stem of its file.
    pub fn address(&self) -> String {
        format!(
            "{}/s{}/{}/{}",
            self.premise,
            self.seed,
            self.matchup.name(),
            self.order.as_str()
        )
    }

    /// Where its reading is written under the judge's directory.
    pub fn file(&self, judge_dir: &Path) -> PathBuf {
        judge_dir
            .join(&self.premise)
            .join(format!("s{}", self.seed))
            .join(format!(
                "{}.{}.json",
                self.matchup.name(),
                self.order.as_str()
            ))
    }
}

// ---------- what a judge is shown ----------

/// Whether a line is a scene break in any of the ways the systems write one:
/// `---`, `***`, `* * *`, a lone `*`, an escaped `\*`.
fn is_break(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && t.len() <= 12
        && t.chars().any(|c| matches!(c, '*' | '-' | '_'))
        && t.chars().all(|c| matches!(c, '*' | '-' | '_' | '\\' | ' '))
}

/// A story as a judge reads it: the text as written, its scene breaks evened
/// out to `* * *`, one where two met. Headings stay: a section named for the
/// one whose eyes it is told through is the story's own.
pub fn shown(markdown: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in markdown.trim().lines() {
        let line = if is_break(line) { "* * *" } else { line };
        if line == "* * *" {
            let last = out.iter().rev().find(|l| !l.trim().is_empty());
            if last == Some(&"* * *") {
                while out.last().is_some_and(|l| l.trim().is_empty()) {
                    out.pop();
                }
                continue;
            }
        }
        out.push(line);
    }
    out.join("\n").trim().to_owned()
}

/// A story's paragraphs shuffled, its heading kept first: a control's rival.
/// The draw is the story's own, from `key`, and never the order it had.
pub fn shuffled(text: &str, key: &str) -> String {
    let mut paragraphs: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let head = match paragraphs.first() {
        Some(p) if p.starts_with('#') => Some(paragraphs.remove(0)),
        _ => None,
    };
    let original = paragraphs.clone();
    let mut state = u64::from_le_bytes(
        Sha256::digest(key.as_bytes())[..8]
            .try_into()
            .expect("eight bytes"),
    );
    let mut next = || {
        // xorshift64*, seeded by the key's hash.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for i in (1..paragraphs.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        paragraphs.swap(i, j);
    }
    if paragraphs.len() > 1 && paragraphs == original {
        paragraphs.rotate_left(1);
    }
    head.into_iter()
        .chain(paragraphs)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A story of the run, as its trace has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Story {
    /// The text a judge reads.
    pub text: String,
    /// Its length in words, the trace's count.
    pub words: u64,
    /// The language it is written in.
    pub language: String,
}

/// The finished story of `system` on a premise and seed under `stories`, the
/// run's directory; none when it is missing or did not finish.
pub fn story(stories: &Path, premise: &str, system: System, seed: u64) -> Option<Story> {
    let dir = story_dir(stories, premise, system, seed);
    let trace: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("trace.json")).ok()?).ok()?;
    if trace["ending"]["kind"] != "finished" {
        return None;
    }
    let text = std::fs::read_to_string(dir.join("story.md")).ok()?;
    Some(Story {
        text: shown(&text),
        words: trace["words"].as_u64().unwrap_or(0),
        language: trace["language"].as_str().unwrap_or("Russian").to_owned(),
    })
}

/// The two stories of `ask`, the weighed system's first: none when either is
/// missing.
pub fn pair_of(stories: &Path, ask: &Ask) -> Option<(Story, Story)> {
    let a = story(stories, &ask.premise, ask.matchup.system, ask.seed)?;
    let b = match ask.matchup.rival {
        Rival::System(rival) => story(stories, &ask.premise, rival, ask.seed)?,
        Rival::Shuffled => Story {
            text: shuffled(&a.text, &format!("{}/s{}", ask.premise, ask.seed)),
            ..a.clone()
        },
    };
    Some((a, b))
}

// ---------- the reading ----------

/// A judge's ratings of one story, each from 1 to 10.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ratings {
    /// The plot.
    pub plot: i64,
    /// The characters.
    pub character: i64,
    /// Surprise.
    pub surprise: i64,
    /// The prose.
    pub prose: i64,
}

/// A judge's reading of a pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reading {
    /// Story 1's ratings.
    pub story_1: Ratings,
    /// Story 2's ratings.
    pub story_2: Ratings,
    /// Why the one chosen is the better.
    pub reason: String,
    /// The better story, `1` or `2`.
    pub better: String,
}

impl Reading {
    /// Its schema: the ratings first, then the reason, then the choice.
    pub fn schema(language: &str) -> Schema {
        let score = |name: &str, what: &str| {
            Property::described(name, &format!("{what}, from 1 to 10."), Schema::Integer)
        };
        let ratings = || {
            Schema::Object(vec![
                score("plot", "The plot"),
                score("character", "The characters"),
                score("surprise", "Its surprise"),
                score("prose", "The prose"),
            ])
        };
        Schema::Object(vec![
            Property::described("story_1", "Story 1's ratings.", ratings()),
            Property::described("story_2", "Story 2's ratings.", ratings()),
            Property::described(
                "reason",
                &format!(
                    "Why the story chosen is the better one, in two or three sentences of \
                     {language}."
                ),
                Schema::String,
            ),
            Property::described(
                "better",
                "The better story.",
                Schema::Enum(vec!["1".to_owned(), "2".to_owned()]),
            ),
        ])
    }
}

/// What a judge is told, the same for every pair.
pub fn instructions(language: &str) -> String {
    format!(
        "You are one judge of a blind comparison of short stories. The two stories below were \
         written in {language} from the same premise, each by a different method. You are not \
         told which, and the order they come in was drawn by chance and says nothing about them. \
         Read both in full, then decide which is the better story for a reader of literary \
         fiction.\n\n\
         Weigh what such a reader weighs:\n\
         - plot: the events follow from one another and from the people's choices, the stakes \
         are clear, and the story reaches an ending that answers what it set up;\n\
         - character: the people are distinct, keep to who they are, and act from what they \
         want and what they know;\n\
         - surprise: its turns are unforeseen and yet earned by what came before;\n\
         - prose: clear, precise and natural {language}, its rhythm and its detail, free of \
         slips of language, of names, of time and of fact.\n\n\
         Length is no merit in itself: a shorter story that does more is the better one. Rate \
         each story from 1 to 10 on each of the four, say why the one you choose is the better, \
         and choose one. There is no tie."
    )
}

/// The prompt of one reading: the two stories in the order asked.
pub fn prompt(first: &Story, second: &Story) -> Prompt {
    Prompt {
        system: instructions(&first.language),
        user: format!(
            "=== Story 1 ===\n\n{}\n\n=== Story 2 ===\n\n{}\n\n=== End ===",
            first.text, second.text
        ),
    }
}

// ---------- a judge's run ----------

/// One judge of the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Judge {
    /// Its name, its directory's under the output.
    pub name: String,
    /// The model and its wire.
    pub route: Route,
    /// How hard it reasons: the most its model takes.
    pub effort: ReasoningEffort,
    /// A reading's cap in tokens, its reasoning included.
    pub max_tokens: usize,
}

impl Judge {
    /// The sampling its calls go out with: thinking at its effort, no
    /// temperature, which the flagships refuse or ignore.
    pub fn sampling(&self) -> SamplingConfig {
        SamplingConfig {
            thinking: Some(true),
            reasoning_effort: Some(self.effort),
            max_tokens: Some(self.max_tokens),
            ..SamplingConfig::default()
        }
    }
}

/// What a judging run reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Judging {
    /// The run's directory, its stories under `premise/system/sN`.
    pub stories: PathBuf,
    /// Where the judges' directories are written.
    pub out: PathBuf,
    /// The premises.
    pub premises: Vec<String>,
    /// The seeds.
    pub seeds: Vec<u64>,
    /// The pairs.
    pub matchups: Vec<Matchup>,
    /// How many readings run at once.
    pub parallel: usize,
}

impl Judging {
    /// Every reading the run asks for, in both orders.
    pub fn asks(&self) -> Vec<Ask> {
        let mut asks = Vec::new();
        for premise in &self.premises {
            for seed in &self.seeds {
                for matchup in &self.matchups {
                    for order in Order::BOTH {
                        asks.push(Ask {
                            premise: premise.clone(),
                            seed: *seed,
                            matchup: *matchup,
                            order,
                        });
                    }
                }
            }
        }
        asks
    }
}

/// A judge's reading as its file records it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The judge's name.
    pub judge: String,
    /// Its model.
    pub model: String,
    /// The premise.
    pub premise: String,
    /// The seed.
    pub seed: u64,
    /// The matchup's name.
    pub matchup: String,
    /// `ab` or `ba`.
    pub order: String,
    /// The sides shown as Story 1 and Story 2.
    pub shown: [String; 2],
    /// Their lengths in words.
    pub words: [u64; 2],
    /// The side chosen.
    pub chose: String,
    /// The reading.
    pub reading: Reading,
    /// What it cost: calls, tokens and dollars.
    pub spent: Value,
}

/// Reads one pair and writes its record; the record.
async fn read_one(
    port: &EngineAdapter,
    db: &StoryDb,
    run: &RunId,
    judge: &Judge,
    ask: &Ask,
    stories: (Story, Story),
    file: &Path,
) -> Result<Record, String> {
    let (a, b) = stories;
    let labels = [
        ask.matchup.system.as_str().to_owned(),
        ask.matchup.rival.label().to_owned(),
    ];
    let (first, second, shown) = match ask.order {
        Order::Ab => (&a, &b, labels.clone()),
        Order::Ba => (&b, &a, [labels[1].clone(), labels[0].clone()]),
    };
    let address = CallAddress {
        run: run.clone(),
        address: ask.address(),
    };
    let reading: Reading = decide(
        port as &dyn LlmPort,
        &address,
        Role::Judge,
        &Reading::schema(&first.language),
        &prompt(first, second),
    )
    .await
    .map_err(|e: PortError| e.to_string())?;
    let spent = db.spend(&ask.address()).await.map_err(|e| e.to_string())?;
    let chose = if reading.better == "1" {
        shown[0].clone()
    } else {
        shown[1].clone()
    };
    let record = Record {
        judge: judge.name.clone(),
        model: judge.route.model.clone(),
        premise: ask.premise.clone(),
        seed: ask.seed,
        matchup: ask.matchup.name(),
        order: ask.order.as_str().to_owned(),
        words: [first.words, second.words],
        shown,
        chose,
        reading,
        spent: json!({
            "calls": spent.calls,
            "input_tokens": spent.input_tokens,
            "output_tokens": spent.output_tokens,
            "cost_usd": spent.cost_usd,
        }),
    };
    if let Some(parent) = file.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&record).unwrap_or_default();
    tokio::fs::write(file, text)
        .await
        .map_err(|e| e.to_string())?;
    Ok(record)
}

/// Runs `judge` over `judging` on `backend`: every reading not yet written,
/// `parallel` at a time, one line each on stdout. A reading that fails is told
/// and left for the next run.
pub async fn judge(
    judging: &Judging,
    judge: &Judge,
    backend: Arc<dyn EngineBackend>,
) -> Result<(), String> {
    let dir = judging.out.join(&judge.name);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    // The call log is the judge's record; its story store holds no story.
    let db = StoryDb::open(&dir.join("story.db"), &dir.join("calls.db"))
        .await
        .map_err(|e| e.to_string())?;
    let mut log = db.run_log_writer().await.map_err(|e| e.to_string())?;
    let run = log
        .start_run(&json!({
            "judge": judge.name,
            "route": judge.route.model,
            "effort": judge.effort.as_wire(),
            "max_tokens": judge.max_tokens,
        }))
        .await
        .map_err(|e| e.to_string())?;
    let log: SharedLog = Arc::new(Mutex::new(log));
    let mut port = EngineAdapter::sharing(
        backend,
        log,
        judge.route.model.clone(),
        judge.sampling(),
        CancellationToken::new(),
    );
    if let Some(price) = judge.route.list_price() {
        port = port.with_list_price(price);
    }
    let mut todo = Vec::new();
    for ask in judging.asks() {
        let file = ask.file(&dir);
        if file.exists() {
            continue;
        }
        match pair_of(&judging.stories, &ask) {
            Some(stories) => todo.push((ask, stories, file)),
            None => println!(
                "skip {}: a story is missing or did not finish",
                ask.address()
            ),
        }
    }
    println!("{}: {} readings to ask", judge.name, todo.len());
    let (port, store, run) = (&port, &db, &run);
    futures_util::stream::iter(todo)
        .for_each_concurrent(judging.parallel.max(1), |(ask, stories, file)| async move {
            match read_one(port, store, run, judge, &ask, stories, &file).await {
                Ok(record) => println!(
                    "{} {}: chose {} — {}",
                    judge.name,
                    ask.address(),
                    record.chose,
                    record.spent
                ),
                Err(e) => println!("{} {}: failed — {e}", judge.name, ask.address()),
            }
        })
        .await;
    db.close().await.map_err(|e| e.to_string())
}

// ---------- the count ----------

/// The one-sided binomial tail: the chance of `wins` or more of `n` fair
/// coin tosses.
pub fn tail(n: u64, wins: u64) -> f64 {
    if wins == 0 {
        return 1.0;
    }
    if wins > n {
        return 0.0;
    }
    let mut total = 0.0;
    let mut choose = 1.0_f64;
    for k in 0..=n {
        if k > 0 {
            choose = choose * (n - k + 1) as f64 / k as f64;
        }
        if k >= wins {
            total += choose;
        }
    }
    total / 2f64.powi(n as i32)
}

/// A judge's vote on a pair: the side both orders chose, or none.
fn vote(records: &[&Record]) -> Option<String> {
    let ab = records.iter().find(|r| r.order == "ab")?;
    let ba = records.iter().find(|r| r.order == "ba")?;
    (ab.chose == ba.chose).then(|| ab.chose.clone())
}

/// Every record under `out`, each judge's directory read.
pub fn records(out: &Path) -> Vec<Record> {
    let mut found = Vec::new();
    let mut dirs = vec![out.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "json")
                && let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(record) = serde_json::from_str::<Record>(&text)
            {
                found.push(record);
            }
        }
    }
    found.sort_by(|a, b| {
        (&a.matchup, &a.premise, a.seed, &a.judge, &a.order)
            .cmp(&(&b.matchup, &b.premise, b.seed, &b.judge, &b.order))
    });
    found
}

/// The verdict of the records: each matchup's pairs, wins and test; each
/// judge's consistency, its first-place choices, its own count and cost; the
/// judges' agreement; and whether the longer story won.
pub fn verdict(records: &[Record]) -> Value {
    // matchup → (premise, seed) → judge → its records.
    type Pairs<'a> = BTreeMap<(String, u64), BTreeMap<String, Vec<&'a Record>>>;
    let mut by: BTreeMap<String, Pairs> = BTreeMap::new();
    for r in records {
        by.entry(r.matchup.clone())
            .or_default()
            .entry((r.premise.clone(), r.seed))
            .or_default()
            .entry(r.judge.clone())
            .or_default()
            .push(r);
    }
    let judges: Vec<String> = {
        let mut names: Vec<String> = records.iter().map(|r| r.judge.clone()).collect();
        names.sort();
        names.dedup();
        names
    };
    let mut matchups = serde_json::Map::new();
    for (name, pairs) in &by {
        let side = |r: &Record| r.matchup.split_once('-').map(|(a, _)| a.to_owned());
        let weighed = pairs
            .values()
            .flat_map(|j| j.values().flatten())
            .find_map(|r| side(r))
            .unwrap_or_default();
        let (mut wins, mut losses, mut ties) = (0u64, 0u64, 0u64);
        let mut each = Vec::new();
        let mut alone: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for ((premise, seed), by_judge) in pairs {
            let votes: BTreeMap<&str, Option<String>> = by_judge
                .iter()
                .map(|(j, rs)| (j.as_str(), vote(rs)))
                .collect();
            let for_it = votes
                .values()
                .filter(|v| v.as_deref() == Some(weighed.as_str()))
                .count();
            let against = votes.values().filter(|v| v.is_some()).count() - for_it;
            for (j, v) in &votes {
                let counted = alone.entry(j).or_default();
                counted.1 += 1;
                if v.as_deref() == Some(weighed.as_str()) {
                    counted.0 += 1;
                }
            }
            let outcome = match for_it.cmp(&against) {
                std::cmp::Ordering::Greater => {
                    wins += 1;
                    "win"
                }
                std::cmp::Ordering::Less => {
                    losses += 1;
                    "loss"
                }
                std::cmp::Ordering::Equal => {
                    ties += 1;
                    "tie"
                }
            };
            each.push(json!({
                "premise": premise,
                "seed": seed,
                "votes": votes,
                "outcome": outcome,
            }));
        }
        let n = wins + losses + ties;
        matchups.insert(
            name.clone(),
            json!({
                "weighed": weighed,
                "pairs": n,
                "wins": wins,
                "losses": losses,
                "ties": ties,
                // A tie is no win: the test counts it against the side weighed.
                "p": tail(n, wins),
                "judges_alone": alone.iter().map(|(j, (w, n))| (j.to_string(), json!({"wins": w, "pairs": n, "p": tail(*n, *w)}))).collect::<serde_json::Map<_, _>>(),
                "each": each,
            }),
        );
    }
    let mut panel = serde_json::Map::new();
    for judge in &judges {
        let mine: Vec<&Record> = records.iter().filter(|r| &r.judge == judge).collect();
        let mut both = 0;
        let mut agreed = 0;
        for pairs in by.values() {
            for by_judge in pairs.values() {
                if let Some(rs) = by_judge.get(judge)
                    && rs.iter().any(|r| r.order == "ab")
                    && rs.iter().any(|r| r.order == "ba")
                {
                    both += 1;
                    agreed += usize::from(vote(rs).is_some());
                }
            }
        }
        let first = mine.iter().filter(|r| r.reading.better == "1").count();
        let controls: Vec<&&Record> = mine
            .iter()
            .filter(|r| r.matchup.ends_with("-control"))
            .collect();
        let kept = controls.iter().filter(|r| r.chose != "shuffled").count();
        let usd: f64 = mine
            .iter()
            .filter_map(|r| r.spent["cost_usd"].as_f64())
            .sum();
        let output: u64 = mine
            .iter()
            .filter_map(|r| r.spent["output_tokens"].as_u64())
            .sum();
        let input: u64 = mine
            .iter()
            .filter_map(|r| r.spent["input_tokens"].as_u64())
            .sum();
        panel.insert(
            judge.clone(),
            json!({
                "model": mine.first().map(|r| r.model.clone()),
                "readings": mine.len(),
                "pairs_read_both_ways": both,
                "same_choice_both_ways": agreed,
                "chose_story_1": first,
                "controls": controls.len(),
                "controls_kept_the_original": kept,
                "input_tokens": input,
                "output_tokens": output,
                "cost_usd": usd,
            }),
        );
    }
    // The judges two by two, over the pairs both gave a vote on.
    let mut agreement = serde_json::Map::new();
    for (i, a) in judges.iter().enumerate() {
        for b in &judges[i + 1..] {
            let (mut shared, mut same) = (0, 0);
            for pairs in by.values() {
                for by_judge in pairs.values() {
                    let va = by_judge.get(a).and_then(|rs| vote(rs));
                    let vb = by_judge.get(b).and_then(|rs| vote(rs));
                    if let (Some(va), Some(vb)) = (va, vb) {
                        shared += 1;
                        same += usize::from(va == vb);
                    }
                }
            }
            agreement.insert(format!("{a}/{b}"), json!({"pairs": shared, "same": same}));
        }
    }
    // Whether the longer story is chosen more often, over the readings of
    // stories of different lengths that are not controls.
    let unequal: Vec<&Record> = records
        .iter()
        .filter(|r| !r.matchup.ends_with("-control") && r.words[0] != r.words[1])
        .collect();
    let longer = unequal
        .iter()
        .filter(|r| {
            let longer_is_1 = r.words[0] > r.words[1];
            (r.reading.better == "1") == longer_is_1
        })
        .count();
    json!({
        "matchups": matchups,
        "judges": panel,
        "agreement": agreement,
        "longer_chosen": {"readings": unequal.len(), "longer": longer},
        "cost_usd": records.iter().filter_map(|r| r.spent["cost_usd"].as_f64()).sum::<f64>(),
    })
}

#[cfg(test)]
#[path = "judge_tests.rs"]
mod tests;
