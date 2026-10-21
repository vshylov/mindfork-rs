//! Follow-ups the companion plans (spec §9.3.6; docs/research/companion-platform.md
//! C4 (a), §3.5): the rules the tool and the orchestrator share, kept pure so they
//! are tested without a clock — when a follow-up may be delivered (quiet hours, the
//! day's cap), how a `when` is read, the notice a due follow-up becomes, and what
//! counts as the woken turn's choice to stay silent.
//!
//! The §7 rules of that research are the design constraints here: a follow-up
//! exists only because the person's words gave the companion a reason (the tool's
//! wording), quiet hours and a daily cap are on by default, every pending one is
//! listed and cancellable (the tasks screen), and silence is a valid answer that
//! leaves no trace.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};

pub use crate::entities::followup::QuietHours;
use crate::shared::i18n::Locale;

/// The most follow-ups one conversation holds pending — a guard against a model
/// that plans one per message, not a number a companion should reach.
pub const PENDING_PER_CHAT: usize = 5;
/// The most a profile holds pending across its conversations.
pub const PENDING_PER_PROFILE: usize = 20;
/// How far ahead a follow-up may be planned.
pub const HORIZON_DAYS: i64 = 366;
/// How late a delivery may be before the notice says when it was due.
pub const LATE_AFTER_MINUTES: i64 = 30;
/// What a woken turn answers to stay silent (the notice asks for exactly this).
pub const SILENT: &str = "<silent>";

/// When a follow-up due at `due` may be delivered, on the local wall clock: not
/// before it is due, outside the quiet hours, and on a day that has had fewer than
/// `cap` deliveries of the profile (`delivered_on`). A due moment held back by
/// either rule is delivered at the first moment both allow — late, and said to be.
pub fn deliverable_at(
    due: NaiveDateTime,
    quiet: QuietHours,
    cap: u8,
    delivered_on: impl Fn(NaiveDate) -> usize,
) -> NaiveDateTime {
    let cap = usize::from(cap.max(1));
    let mut t = due;
    // A cap is reached on a day at most; the next day starts at zero, so two
    // rounds settle it. The bound only keeps a wrong counter from looping.
    for _ in 0..400 {
        t = quiet.after(t);
        if delivered_on(t.date()) < cap {
            return t;
        }
        t = (t.date() + Duration::days(1)).and_time(NaiveTime::MIN);
    }
    t
}

/// Reads the tool's `when`: a local date and time, `YYYY-MM-DD HH:MM`, with `T`
/// for the space and seconds or a zone suffix tolerated and ignored — the model
/// is asked for the person's wall clock, and that is what is kept.
pub fn parse_when(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim().replace('T', " ");
    let head: String = s.chars().take(16).collect();
    NaiveDateTime::parse_from_str(&head, "%Y-%m-%d %H:%M").ok()
}

/// The weekday a name says, Monday = 0 — English, or the profile's language
/// (`names`, Monday first), any case, a three-letter English short form too.
/// `None` for anything else, which the tool then does not check.
pub fn weekday_named(name: &str, names: &[&str; 7]) -> Option<u32> {
    const EN: [&str; 7] = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let n = name.trim().to_lowercase();
    if n.is_empty() {
        return None;
    }
    EN.iter()
        .position(|d| *d == n || (n.len() == 3 && d.starts_with(&n)))
        .or_else(|| names.iter().position(|d| d.to_lowercase() == n))
        .map(|i| i as u32)
}

/// The next `count` days after `today`, each as the profile's language writes a
/// date — what a refused `when` lists, so a model that miscounted weekdays can
/// read the right date off it (the research's §3.5: gpt-oss put "Friday" on a
/// Thursday 4 times in 10).
pub fn next_days(today: NaiveDate, count: i64, loc: &Locale) -> String {
    (1..=count)
        .map(|i| {
            crate::features::time_row::date_text(
                (today + Duration::days(i)).and_time(NaiveTime::MIN),
                loc,
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The stored text of a due follow-up's notice row: what time it is, when the
/// companion planned it and what about, and — delivered late — when it was due.
/// The woken turn's instruction to write or stay silent is not stored: it is
/// added on the wire while the notice stands alone ([`silence_clause`]), so the
/// person's next message never arrives behind it.
pub fn notice(
    now: NaiveDateTime,
    planned: NaiveDateTime,
    due: NaiveDateTime,
    about: &str,
    loc: &Locale,
) -> String {
    use crate::features::time_row::moment_text;
    let late = if now - due >= Duration::minutes(LATE_AFTER_MINUTES) {
        loc.tf("prompt.followup.late", &[("due", &moment_text(due, loc))])
    } else {
        String::new()
    };
    loc.tf(
        "prompt.followup.notice",
        &[
            ("now", &moment_text(now, loc)),
            ("planned", &moment_text(planned, loc)),
            ("about", about.trim()),
            ("late", &late),
        ],
    )
}

/// What follows a follow-up's notice on the wire when nothing the person wrote
/// follows it — the turn the app woke the companion for: write first, or answer
/// exactly [`SILENT`].
pub fn silence_clause(loc: &Locale) -> &str {
    loc.t("prompt.followup.silence")
}

/// Whether a woken turn's reply is its choice to stay silent: [`SILENT`] alone,
/// in any case, with whitespace, quotes or backticks around it, or written as an
/// empty element.
pub fn is_silent(text: &str) -> bool {
    let t = text
        .trim()
        .trim_matches(|c: char| c == '`' || c == '"' || c == '\'' || c.is_whitespace())
        .to_lowercase();
    t == SILENT || t == "<silent/>" || t == "<silent />"
}

/// A local wall-clock moment as the instant it names — the earliest one where a
/// clock change makes it ambiguous, and an hour later where it skips it.
pub fn local_to_utc(at: NaiveDateTime) -> DateTime<Utc> {
    use chrono::TimeZone;
    let local = chrono::Local;
    local
        .from_local_datetime(&at)
        .earliest()
        .or_else(|| {
            local
                .from_local_datetime(&(at + Duration::hours(1)))
                .earliest()
        })
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|| at.and_utc())
}

/// The local wall-clock day an instant falls on, as `[start, end)` instants —
/// what the day's deliveries are counted over.
pub fn local_day_bounds(day: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        local_to_utc(day.and_time(NaiveTime::MIN)),
        local_to_utc((day + Duration::days(1)).and_time(NaiveTime::MIN)),
    )
}

/// `at` with its seconds dropped — a follow-up is due on a whole minute.
pub fn whole_minute(at: NaiveDateTime) -> NaiveDateTime {
    at.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(at)
}

/// The day of the week `at` falls on, Monday = 0.
pub fn weekday_of(at: NaiveDateTime) -> u32 {
    at.weekday().num_days_from_monday()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn at(d: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, d)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    const NIGHT: QuietHours = QuietHours { from: 22, until: 8 };

    #[test]
    fn quiet_hours_across_midnight_hold_a_delivery_until_they_end() {
        assert!(NIGHT.contains(at(20, 23, 0).time()));
        assert!(NIGHT.contains(at(20, 3, 0).time()));
        assert!(!NIGHT.contains(at(20, 8, 0).time()));
        assert!(!NIGHT.contains(at(20, 21, 59).time()));
        assert_eq!(NIGHT.after(at(20, 23, 30)), at(21, 8, 0));
        assert_eq!(NIGHT.after(at(21, 2, 0)), at(21, 8, 0));
        assert_eq!(NIGHT.after(at(21, 17, 0)), at(21, 17, 0));
        // Within one day, and none at all.
        let lunch = QuietHours {
            from: 13,
            until: 14,
        };
        assert_eq!(lunch.after(at(20, 13, 20)), at(20, 14, 0));
        let none = QuietHours { from: 0, until: 0 };
        assert_eq!(none.after(at(20, 3, 0)), at(20, 3, 0));
    }

    #[test]
    fn a_full_day_moves_the_delivery_to_the_next_one_after_its_quiet_hours() {
        let full_on_20 = |d: NaiveDate| if d.day() == 20 { 3 } else { 0 };
        assert_eq!(
            deliverable_at(at(20, 17, 0), NIGHT, 3, full_on_20),
            at(21, 8, 0)
        );
        assert_eq!(
            deliverable_at(at(20, 17, 0), NIGHT, 4, full_on_20),
            at(20, 17, 0)
        );
        assert_eq!(deliverable_at(at(20, 23, 0), NIGHT, 3, |_| 0), at(21, 8, 0));
    }

    #[test]
    fn a_when_is_the_wall_clock_and_its_weekday_is_checked_in_either_language() {
        assert_eq!(parse_when("2026-10-23 09:00"), Some(at(23, 9, 0)));
        assert_eq!(parse_when(" 2026-10-23T09:00:00Z "), Some(at(23, 9, 0)));
        assert_eq!(parse_when("Friday 9am"), None);
        let ru = crate::features::time_row::weekday_names(locale(Lang::Ru));
        assert_eq!(weekday_named("Friday", &ru), Some(4));
        assert_eq!(weekday_named("fri", &ru), Some(4));
        assert_eq!(weekday_named("Пятница", &ru), Some(4));
        assert_eq!(weekday_named("someday", &ru), None);
        assert_eq!(weekday_of(at(23, 9, 0)), 4);
    }

    #[test]
    fn the_notice_says_now_the_plan_and_lateness_only_when_late() {
        let loc = locale(Lang::En);
        let on_time = notice(
            at(20, 17, 0),
            at(19, 21, 11),
            at(20, 17, 0),
            " Ask about the interview ",
            loc,
        );
        assert_eq!(
            on_time,
            "[Follow-up — not a message from the user] Tuesday, 20 October 2026, 17:00. \
             On Monday, 19 October 2026, 21:11 you planned to come back to this: \
             «Ask about the interview»."
        );
        let late = notice(at(22, 19, 40), at(19, 21, 11), at(20, 17, 0), "x", loc);
        assert!(
            late.ends_with("It was due Tuesday, 20 October 2026, 17:00."),
            "{late}"
        );
    }

    #[test]
    fn silence_is_the_marker_alone() {
        for yes in [
            "<silent>",
            " <SILENT>\n",
            "`<silent>`",
            "\"<silent>\"",
            "<silent/>",
        ] {
            assert!(is_silent(yes), "{yes:?}");
        }
        for no in ["", "<silent> hi", "silent", "Hey! <silent>"] {
            assert!(!is_silent(no), "{no:?}");
        }
    }
}
