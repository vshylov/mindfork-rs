//! The time row (spec §6.2; docs/research/companion-platform.md C1): a dated line
//! in front of every user message the model reads, when the profile switches it on
//! (`Profile.companion.time_rows`, off by default).
//!
//! **Why.** No timestamp reaches the model otherwise, so it cannot tell three days
//! from twenty-five minutes; the probe in the research's §3.2 found every model
//! wrong in one of the two, and a row on every message right in both.
//!
//! **Derived, not stored.** A row is computed from the messages' own timestamps each
//! time a request is built, so it is the same text on every turn — the prompt's
//! prefix holds as if it were stored (spec §6.6) — while the chat file is unchanged
//! and switching the feature off removes every row at once.
//!
//! **Its shape.** The full form — "[Thursday, 22 October 2026, 18:40 · 2d 21h after
//! the previous message]" — on the first user message the model sees and wherever
//! the day changes; within a day the compact one — "[21:35 · 24m later]". The gap is
//! measured to the message just before, whatever its role, and left out under two
//! minutes. All of it is scaffold text in the profile's language (axis A).

use chrono::{DateTime, Datelike, NaiveDateTime, Timelike, Utc};

use crate::entities::message::{Message, MessageRole};
use crate::shared::i18n::Locale;

/// Below this, the gap since the previous message is not said: messages a minute
/// apart are one moment of the conversation, and "0m later" says nothing.
const GAP_FLOOR_SECS: i64 = 120;

/// The weekday names' keys, Monday first — literals, so the bundle gates see every
/// one of them (`shared::i18n` tests: a key built with `format!` reads as dead).
const WEEKDAYS: [&str; 7] = [
    "time.weekday.1",
    "time.weekday.2",
    "time.weekday.3",
    "time.weekday.4",
    "time.weekday.5",
    "time.weekday.6",
    "time.weekday.7",
];

/// The month names' keys, January first — each in the form a date takes, which in
/// Russian is the genitive case.
const MONTHS: [&str; 12] = [
    "time.month.1",
    "time.month.2",
    "time.month.3",
    "time.month.4",
    "time.month.5",
    "time.month.6",
    "time.month.7",
    "time.month.8",
    "time.month.9",
    "time.month.10",
    "time.month.11",
    "time.month.12",
];

/// What a stored instant reads as on the wall clock the rows are written in.
pub type Clock<'a> = &'a dyn Fn(DateTime<Utc>) -> NaiveDateTime;

/// The machine's local time — the production [`Clock`]. A row of an old message
/// keeps its text as long as the machine's time zone does.
pub fn local(at: DateTime<Utc>) -> NaiveDateTime {
    at.with_timezone(&chrono::Local).naive_local()
}

/// A duration in at most two number-neutral units — "2d 21h", "7h 30m", "24m",
/// "40s" — the locale's own abbreviations (`time.dur.*`), so no plural rules are
/// needed in any language. Negative durations read as zero.
pub fn gap(secs: i64, loc: &Locale) -> String {
    let secs = secs.max(0);
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    let unit = |key: &str, n: i64| loc.tf(key, &[("n", &n.to_string())]);
    let parts: Vec<String> = if days > 0 {
        std::iter::once(unit("time.dur.days", days))
            .chain((hours > 0).then(|| unit("time.dur.hours", hours)))
            .collect()
    } else if hours > 0 {
        std::iter::once(unit("time.dur.hours", hours))
            .chain((minutes > 0).then(|| unit("time.dur.minutes", minutes)))
            .collect()
    } else if minutes > 0 {
        vec![unit("time.dur.minutes", minutes)]
    } else {
        vec![unit("time.dur.seconds", secs)]
    };
    parts.join(" ")
}

/// The rows for `messages[from..]`, aligned with that slice: `Some` for every user
/// message, `None` for every other row.
///
/// `from` is where the verbatim history starts (spec §6.7): the first user message
/// at or after it is the first the model sees, so it carries the full date even
/// when its day is the day of the message before it — that one was folded into the
/// summary. A gap is still measured to the real previous message, folded or not.
pub fn rows(
    messages: &[Message],
    from: usize,
    clock: Clock<'_>,
    loc: &Locale,
) -> Vec<Option<String>> {
    let from = from.min(messages.len());
    let mut seen_user = false;
    (from..messages.len())
        .map(|i| {
            let message = &messages[i];
            if message.role != MessageRole::User {
                return None;
            }
            let previous = i.checked_sub(1).map(|p| messages[p].timestamp);
            let first = !seen_user;
            seen_user = true;
            Some(row(message.timestamp, previous, first, clock, loc))
        })
        .collect()
}

/// One row: the full form for the first message seen, for a message with nothing
/// before it, and where the local date changed since the message before; the compact
/// one otherwise. The gap is said from [`GAP_FLOOR_SECS`] on.
fn row(
    at: DateTime<Utc>,
    previous: Option<DateTime<Utc>>,
    first: bool,
    clock: Clock<'_>,
    loc: &Locale,
) -> String {
    let now = clock(at);
    let time = format!("{:02}:{:02}", now.hour(), now.minute());
    let gap_text = previous
        .map(|p| (at - p).num_seconds())
        .filter(|secs| *secs >= GAP_FLOOR_SECS)
        .map(|secs| gap(secs, loc));
    let day_changed = previous.is_none_or(|p| clock(p).date() != now.date());
    if first || day_changed {
        let date = loc.tf(
            "prompt.time_row.date",
            &[
                (
                    "weekday",
                    loc.t(WEEKDAYS[now.weekday().num_days_from_monday() as usize]),
                ),
                ("day", &now.day().to_string()),
                ("month", loc.t(MONTHS[now.month0() as usize])),
                ("year", &now.year().to_string()),
            ],
        );
        match gap_text {
            Some(gap) => loc.tf(
                "prompt.time_row.full_gap",
                &[("date", &date), ("time", &time), ("gap", &gap)],
            ),
            None => loc.tf("prompt.time_row.full", &[("date", &date), ("time", &time)]),
        }
    } else {
        match gap_text {
            Some(gap) => loc.tf(
                "prompt.time_row.compact_gap",
                &[("time", &time), ("gap", &gap)],
            ),
            None => loc.tf("prompt.time_row.compact", &[("time", &time)]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};
    use chrono::TimeZone;

    fn utc(at: DateTime<Utc>) -> NaiveDateTime {
        at.naive_utc()
    }

    fn at(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap()
    }

    fn user(text: &str, when: DateTime<Utc>) -> Message {
        let mut m = Message::user(text);
        m.timestamp = when;
        m
    }

    fn assistant(text: &str, when: DateTime<Utc>) -> Message {
        let mut m = Message::assistant(text);
        m.timestamp = when;
        m
    }

    /// The research's S1: three days later the row says the date and the gap,
    /// measured to the assistant's reply just before it.
    #[test]
    fn three_days_later_reads_full_with_the_gap() {
        let loc = locale(Lang::En);
        let messages = vec![
            user("tomorrow is the interview", at(19, 21, 10)),
            assistant("good luck", at(19, 21, 11)),
            user("hey, I'm back", at(22, 18, 40)),
        ];
        let rows = rows(&messages, 0, &utc, loc);
        assert_eq!(
            rows,
            vec![
                Some("[Monday, 19 October 2026, 21:10]".to_string()),
                None,
                Some(
                    "[Thursday, 22 October 2026, 18:40 · 2d 21h after the previous message]"
                        .to_string()
                ),
            ]
        );
    }

    /// The research's S4: twenty-five minutes later the same day the row is compact.
    #[test]
    fn same_day_reads_compact() {
        let loc = locale(Lang::En);
        let messages = vec![
            user("tomorrow is the interview", at(19, 21, 10)),
            assistant("good luck", at(19, 21, 11)),
            user("hey, I'm back", at(19, 21, 35)),
        ];
        let rows = rows(&messages, 0, &utc, loc);
        assert_eq!(rows[2].as_deref(), Some("[21:35 · 24m later]"));
    }

    /// Under two minutes the gap is not said; a row is still there.
    #[test]
    fn a_short_gap_is_not_said() {
        let loc = locale(Lang::En);
        let messages = vec![
            user("one", at(19, 21, 10)),
            assistant("two", at(19, 21, 10)),
            user("three", at(19, 21, 11)),
        ];
        assert_eq!(rows(&messages, 0, &utc, loc)[2].as_deref(), Some("[21:11]"));
    }

    /// Past midnight the full form returns, even after a short gap.
    #[test]
    fn a_new_day_reads_full() {
        let loc = locale(Lang::En);
        let messages = vec![
            user("late", at(19, 23, 50)),
            assistant("go to sleep", at(19, 23, 51)),
            user("can't", at(20, 0, 10)),
        ];
        assert_eq!(
            rows(&messages, 0, &utc, loc)[2].as_deref(),
            Some("[Tuesday, 20 October 2026, 00:10 · 19m after the previous message]")
        );
    }

    /// After a compaction the first message the model sees carries the date, and
    /// its gap is still measured to the folded message before it.
    #[test]
    fn the_first_visible_message_reads_full() {
        let loc = locale(Lang::En);
        let messages = vec![
            user("folded", at(19, 10, 0)),
            assistant("folded too", at(19, 10, 1)),
            user("seen", at(19, 10, 31)),
            assistant("reply", at(19, 10, 32)),
            user("seen too", at(19, 10, 40)),
        ];
        let rows = rows(&messages, 2, &utc, loc);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].as_deref(),
            Some("[Monday, 19 October 2026, 10:31 · 30m after the previous message]")
        );
        assert_eq!(rows[2].as_deref(), Some("[10:40 · 8m later]"));
    }

    /// A timestamp earlier than the message before it (clock skew, an import that
    /// stamped everything at once) says no gap rather than a negative one.
    #[test]
    fn a_backwards_clock_says_no_gap() {
        let loc = locale(Lang::En);
        let messages = vec![user("a", at(19, 12, 0)), user("b", at(19, 11, 0))];
        assert_eq!(rows(&messages, 0, &utc, loc)[1].as_deref(), Some("[11:00]"));
    }

    /// Russian: lowercase weekday, the month in the genitive, its own wording.
    #[test]
    fn russian_reads_in_russian() {
        let loc = locale(Lang::Ru);
        let messages = vec![
            user("завтра собеседование", at(19, 21, 10)),
            assistant("удачи", at(19, 21, 11)),
            user("я вернулся", at(22, 18, 40)),
            assistant("как прошло?", at(22, 18, 41)),
            user("отлично", at(22, 19, 5)),
        ];
        let rows = rows(&messages, 0, &utc, loc);
        assert_eq!(
            rows[0].as_deref(),
            Some("[понедельник, 19 октября 2026, 21:10]")
        );
        assert_eq!(
            rows[2].as_deref(),
            Some("[четверг, 22 октября 2026, 18:40 · 2 дн 21 ч после предыдущего сообщения]")
        );
        assert_eq!(rows[4].as_deref(), Some("[19:05 · спустя 24 мин]"));
    }

    /// Every weekday and month has a name in both bundles — a missing key would
    /// put a slug in front of the user's words.
    #[test]
    fn every_weekday_and_month_is_named() {
        for lang in [Lang::En, Lang::Ru] {
            let loc = locale(lang);
            for key in WEEKDAYS.iter().chain(MONTHS.iter()) {
                assert_ne!(loc.t(key), *key, "{lang:?} {key}");
            }
        }
    }

    #[test]
    fn gap_uses_two_units_at_most() {
        let loc = locale(Lang::En);
        assert_eq!(gap(40, loc), "40s");
        assert_eq!(gap(24 * 60 + 40, loc), "24m");
        assert_eq!(gap(7 * 3600 + 30 * 60, loc), "7h 30m");
        assert_eq!(gap(3 * 3600, loc), "3h");
        assert_eq!(gap(2 * 86_400 + 21 * 3600 + 29 * 60, loc), "2d 21h");
        assert_eq!(gap(5 * 86_400, loc), "5d");
        assert_eq!(gap(-5, loc), "0s");
    }
}
