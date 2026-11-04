//! The Canon keeper's pieces on their own: a reply made a register, the
//! register so far, the schemas and the prompts (docs/fabula.md §4, PR 11).

use super::{
    FactReply, PayoffReply, RegisterReply, SoFar, SummaryFault, ThreadReply, register_check_prompt,
    register_of, register_prompt, summary_check_prompt, summary_prompt,
};
use crate::entities::story::{Fact, Payoff, Register, Thread, fixture_canon};
use crate::features::story::schema::CanonKeys;

fn thread(id: &str, question: &str) -> Thread {
    Thread {
        id: id.to_owned(),
        question: question.to_owned(),
        quote: "a quote".to_owned(),
    }
}

fn fact(id: &str, text: &str) -> Fact {
    Fact {
        id: id.to_owned(),
        fact: text.to_owned(),
        stated_by: "narration".to_owned(),
        quote: "a quote".to_owned(),
    }
}

/// Chapter 1 opened two threads and stated a fact.
fn after_chapter_one() -> SoFar {
    let mut so_far = SoFar::default();
    so_far.add(
        1,
        &Register {
            opened: vec![
                thread("ch1.thread1", "Whose hand wrote the log's last line?"),
                thread("ch1.thread2", "Whose steps were on the path?"),
            ],
            paid: Vec::new(),
            facts: vec![fact("ch1.fact1", "Timur's father died a few years ago.")],
        },
    );
    so_far
}

#[test]
fn a_reply_becomes_a_register_with_the_engines_ids() {
    let so_far = after_chapter_one();
    let reply = RegisterReply {
        paid: vec![
            PayoffReply {
                thread: " `ch1.thread2` ".to_owned(),
                how: " The keeper's sister came down the path. ".to_owned(),
            },
            PayoffReply {
                thread: "ch1.thread2".to_owned(),
                how: "Twice.".to_owned(),
            },
            PayoffReply {
                thread: "ch1.thread9".to_owned(),
                how: "No such thread.".to_owned(),
            },
        ],
        opened: vec![
            ThreadReply {
                quote: "".to_owned(),
                question: "  ".to_owned(),
            },
            ThreadReply {
                quote: " the letter ".to_owned(),
                question: " What does the letter say? ".to_owned(),
            },
        ],
        facts: vec![
            FactReply {
                quote: "twelve".to_owned(),
                stated_by: " ".to_owned(),
                fact: "Timur left the island at twelve.".to_owned(),
            },
            FactReply {
                quote: "forty".to_owned(),
                stated_by: "Galina".to_owned(),
                fact: "".to_owned(),
            },
        ],
    };
    let register = register_of(2, reply, &so_far);
    assert_eq!(
        register.paid,
        [Payoff {
            thread: "ch1.thread2".to_owned(),
            how: "The keeper's sister came down the path.".to_owned(),
        }],
        "a payoff of an open thread, once; one of a thread not open is dropped"
    );
    assert_eq!(
        register.opened,
        [Thread {
            id: "ch2.thread1".to_owned(),
            question: "What does the letter say?".to_owned(),
            quote: "the letter".to_owned(),
        }],
        "a blank is dropped before the ids are counted"
    );
    assert_eq!(
        register.facts,
        [Fact {
            id: "ch2.fact1".to_owned(),
            fact: "Timur left the island at twelve.".to_owned(),
            stated_by: "narration".to_owned(),
            quote: "twelve".to_owned(),
        }]
    );
}

#[test]
fn the_register_so_far_closes_what_is_paid_and_keeps_every_fact() {
    let mut so_far = after_chapter_one();
    so_far.add(
        2,
        &Register {
            opened: vec![thread("ch2.thread1", "What does the letter say?")],
            paid: vec![Payoff {
                thread: "ch1.thread1".to_owned(),
                how: "Her own hand.".to_owned(),
            }],
            facts: vec![fact("ch2.fact1", "Timur is forty.")],
        },
    );
    assert_eq!(so_far.open_ids(), ["ch1.thread2", "ch2.thread1"]);
    assert_eq!(so_far.facts.len(), 2);
    let text = so_far.text();
    assert!(
        text.contains("- `ch1.thread2` (chapter 1): Whose steps were on the path? (\"a quote\")")
    );
    assert!(!text.contains("ch1.thread1"), "a thread paid off is closed");
    assert!(
        text.contains(
            "- `ch2.fact1` (chapter 2, stated by narration): Timur is forty. (\"a quote\")"
        )
    );
    assert!(text.contains("- `ch1.fact1` (chapter 1, stated by narration)"));
    assert_eq!(SoFar::default().text(), "", "nothing noted, nothing said");
}

#[test]
fn the_register_schema_names_only_the_threads_open() {
    let keys = CanonKeys::of(&fixture_canon());
    let none = RegisterReply::schema_for(&keys, &[]).to_json();
    assert!(
        !none.contains("\"paid\""),
        "nothing open, nothing to pay off"
    );
    let open = RegisterReply::schema_for(&keys, &after_chapter_one().open_ids()).to_json();
    assert!(open.contains("\"paid\""));
    assert!(open.contains("\"enum\":[\"ch1.thread1\",\"ch1.thread2\"]"));
    let reply: RegisterReply =
        serde_json::from_value(serde_json::json!({"opened": [], "facts": []})).unwrap();
    assert!(
        reply.paid.is_empty(),
        "a reply with nothing open has no payoffs"
    );
}

#[test]
fn the_keepers_prompts_hold_the_text_and_nothing_of_the_canon() {
    let text = "## Light\n\nThe line for that night is in another hand.";
    let first = register_prompt(&SoFar::default(), 1, text, "Russian");
    assert!(
        first
            .user
            .contains("The published chapters left no thread open.")
    );
    assert!(first.user.contains("Chapter 1:\n\n## Light"));
    assert!(first.system.contains("Write what you note in Russian."));
    let second = register_prompt(&after_chapter_one(), 2, text, "Russian");
    assert!(
        second
            .user
            .starts_with("Threads the published chapters left open")
    );
    assert!(second.user.contains("`ch1.fact1`"));
    let canon = fixture_canon();
    let secret = &canon.characters[0].secrets[0].text;
    let held = register_check_prompt(&after_chapter_one(), 2, text, true);
    assert!(
        held.user
            .starts_with("Threads the published chapters left open")
    );
    assert!(
        held.user.contains("`ch1.fact1`")
            && held.user.contains(
                "Chapter 2:

## Light"
            )
    );
    assert!(held.user.contains("This is the story's last chapter"));
    assert!(held.system.contains("by arithmetic"));
    let middle = register_check_prompt(&after_chapter_one(), 2, text, false);
    assert!(!middle.user.contains("last chapter"), "not the arc's last");
    let nothing_open = register_check_prompt(&SoFar::default(), 1, text, true);
    assert!(
        !nothing_open.user.contains("last chapter"),
        "no thread to report"
    );
    for prompt in [
        &first,
        &second,
        &held,
        &summary_prompt("Russian", text, None),
        &summary_check_prompt(text, "A summary."),
    ] {
        for part in [&prompt.system, &prompt.user] {
            assert!(!part.contains(secret.as_str()) && !part.contains(&canon.arc.premise));
            assert!(!part.contains("Author of a story"));
        }
    }
    let summary = summary_prompt("Russian", text, None);
    assert!(summary.user.contains("Summarize it in Russian"));
    assert!(summary.user.contains("another hand"));
    let faults = [SummaryFault {
        sentence: " She hides it in the chapel. ".to_owned(),
        text_says: "She carries it out of the chapel.".to_owned(),
    }];
    let again = summary_prompt(
        "Russian",
        text,
        Some(("She hides it in the chapel.", &faults)),
    );
    assert!(
        again
            .user
            .contains("A first summary of it:\n\nShe hides it in the chapel.")
    );
    assert!(
        again
            .user
            .contains("- \"She hides it in the chapel.\": She carries it out of the chapel.")
    );
    assert!(again.user.contains("true to the text"));
    let check = summary_check_prompt(text, "She hides it in the chapel.");
    assert!(
        check
            .user
            .contains("Its summary:\n\nShe hides it in the chapel.")
    );
}
