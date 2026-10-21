//! `followup_plan` and `followup_cancel` (spec §9.3.6;
//! docs/research/companion-platform.md C4 (a), §3.5): the companion plans a return
//! to this conversation about something the person said, and cancels one that is
//! no longer needed. Offered only when the profile's *Follow-ups* switch is on
//! ([`super::meta::ToolGate::Followups`]); both write to storage directly, like the
//! note tools — the orchestrator reads the pending ones after the turn and
//! schedules the next.
//!
//! The wording was measured before it shipped (§3.5): five models planned for an
//! interview, a surgery and a reminder they were asked for, and one run in 70
//! planned one after a film, a networking question or a bad day at work. The
//! weekday beside the date is the measured fix for a model that miscounts days: a
//! date and a day that disagree are refused with the next week spelled out.

use anyhow::Result;
use chrono::{Duration, Local, NaiveDateTime, Utc};
use uuid::Uuid;

use super::{Tool, ToolContext, ToolOutcome};
use crate::entities::followup::Followup;
use crate::entities::profile::ToolId;
use crate::features::followups::{
    HORIZON_DAYS, PENDING_PER_CHAT, PENDING_PER_PROFILE, next_days, parse_when, weekday_named,
    weekday_of, whole_minute,
};
use crate::features::time_row::{gap, moment_text, weekday_names};
use crate::shared::i18n::Locale;

pub const FOLLOWUP_PLAN_ID: &str = "followup_plan";
pub const FOLLOWUP_CANCEL_ID: &str = "followup_cancel";

/// `followup_plan` — records a return to this conversation.
pub struct FollowupPlan;

/// `followup_cancel` — cancels one of this conversation's pending returns.
pub struct FollowupCancel;

/// The local wall clock now — what `when` is read against.
fn now_local() -> NaiveDateTime {
    Local::now().naive_local()
}

#[async_trait::async_trait]
impl Tool for FollowupPlan {
    fn id(&self) -> ToolId {
        FOLLOWUP_PLAN_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "plan a follow-up"
    }
    fn gate(&self) -> Option<super::meta::ToolGate> {
        Some(super::meta::ToolGate::Followups)
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.followup_plan.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "when": {"type": "string", "description": loc.t("tool.followup_plan.param.when")},
                "weekday": {"type": "string", "description": loc.t("tool.followup_plan.param.weekday")},
                "about": {"type": "string", "description": loc.t("tool.followup_plan.param.about")}
            },
            "required": ["when", "weekday", "about"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        plan(ctx, &args, now_local()).map(ToolOutcome::text)
    }
}

/// The plan itself, against a given wall clock `now`.
fn plan(ctx: &ToolContext, args: &serde_json::Value, now: NaiveDateTime) -> Result<String> {
    let loc = ctx.loc;
    let text = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("").trim();
    let about = text("about");
    if about.is_empty() {
        anyhow::bail!(loc.t("tool.followup_plan.err.about"));
    }
    let now_text = moment_text(now, loc);
    let days = || next_days(now.date(), 7, loc);
    let Some(when) = parse_when(text("when")) else {
        anyhow::bail!(loc.tf(
            "tool.followup_plan.err.when",
            &[("when", text("when")), ("now", &now_text)]
        ));
    };
    let when = whole_minute(when);
    if when <= now {
        anyhow::bail!(loc.tf(
            "tool.followup_plan.err.past",
            &[
                ("when", &moment_text(when, loc)),
                ("now", &now_text),
                ("days", &days())
            ]
        ));
    }
    if when - now > Duration::days(HORIZON_DAYS) {
        anyhow::bail!(loc.tf(
            "tool.followup_plan.err.far",
            &[("when", &moment_text(when, loc))]
        ));
    }
    let names = weekday_names(loc);
    if let Some(said) = weekday_named(text("weekday"), &names)
        && said != weekday_of(when)
    {
        anyhow::bail!(loc.tf(
            "tool.followup_plan.err.weekday",
            &[
                ("date", &when.format("%Y-%m-%d").to_string()),
                ("actual", names[weekday_of(when) as usize]),
                ("said", names[said as usize]),
                ("now", &now_text),
                ("days", &days()),
            ]
        ));
    }
    let db = ctx.storage.db();
    let others = db.followups_pending_in_chat(ctx.chat_id)?;
    if others.len() >= PENDING_PER_CHAT {
        anyhow::bail!(loc.tf(
            "tool.followup_plan.err.chat_full",
            &[("n", &others.len().to_string())]
        ));
    }
    let in_profile = db.followups_pending_count(ctx.profile_id)?;
    if in_profile >= PENDING_PER_PROFILE {
        anyhow::bail!(loc.tf(
            "tool.followup_plan.err.profile_full",
            &[("n", &in_profile.to_string())]
        ));
    }
    let followup = Followup::new(
        ctx.profile_id,
        ctx.chat_id,
        about,
        crate::features::followups::local_to_utc(when),
    );
    db.followup_insert(&followup)?;
    let mut out = loc.tf(
        "tool.followup_plan.result.planned",
        &[
            ("when", &moment_text(when, loc)),
            ("in", &gap((when - now).num_seconds(), loc)),
            ("now", &now_text),
            ("id", &followup.id.to_string()),
        ],
    );
    if !others.is_empty() {
        let list: Vec<String> = others.iter().map(|f| item(f, loc)).collect();
        out.push_str("\n\n");
        out.push_str(&loc.tf(
            "tool.followup_plan.result.others",
            &[("list", &list.join("\n"))],
        ));
    }
    Ok(out)
}

/// One pending follow-up as a line of a tool's answer.
fn item(f: &Followup, loc: &Locale) -> String {
    loc.tf(
        "tool.followup_plan.result.item",
        &[
            (
                "when",
                &moment_text(f.due_at.with_timezone(&Local).naive_local(), loc),
            ),
            ("id", &f.id.to_string()),
            ("about", &f.about),
        ],
    )
}

#[async_trait::async_trait]
impl Tool for FollowupCancel {
    fn id(&self) -> ToolId {
        FOLLOWUP_CANCEL_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "cancel a follow-up"
    }
    fn gate(&self) -> Option<super::meta::ToolGate> {
        Some(super::meta::ToolGate::Followups)
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.followup_cancel.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": loc.t("tool.followup_cancel.param.id")}
            },
            "required": ["id"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let loc = ctx.loc;
        let raw = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let db = ctx.storage.db();
        let pending = db.followups_pending_in_chat(ctx.chat_id)?;
        let found = Uuid::parse_str(raw)
            .ok()
            .and_then(|id| pending.iter().find(|f| f.id == id));
        let Some(f) = found else {
            let mut msg = loc.tf("tool.followup_cancel.err.unknown", &[("id", raw)]);
            if !pending.is_empty() {
                let list: Vec<String> = pending.iter().map(|f| item(f, loc)).collect();
                msg.push_str("\n\n");
                msg.push_str(&loc.tf(
                    "tool.followup_cancel.err.pending",
                    &[("list", &list.join("\n"))],
                ));
            }
            anyhow::bail!(msg);
        };
        db.followup_close(
            f.id,
            crate::entities::followup::FollowupState::Cancelled,
            Utc::now(),
        )?;
        Ok(ToolOutcome::text(loc.tf(
            "tool.followup_cancel.result.cancelled",
            &[("about", &f.about)],
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use crate::entities::followup::FollowupState;
    use crate::shared::i18n::{Lang, locale};
    use chrono::NaiveDate;

    /// Monday, 19 October 2026, 21:10 — the research's "now".
    fn monday() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 19)
            .unwrap()
            .and_hms_opt(21, 10, 0)
            .unwrap()
    }

    fn en(ctx: &mut ToolContext) {
        ctx.loc = locale(Lang::En);
    }

    #[test]
    fn a_plan_is_stored_for_this_chat_and_answered_with_its_time() {
        let (_d, storage, mut ctx) = ctx_with_storage(Uuid::new_v4());
        en(&mut ctx);
        let args = serde_json::json!({
            "when": "2026-10-20 17:00", "weekday": "Tuesday",
            "about": "Ask how the interview went"
        });
        let out = plan(&ctx, &args, monday()).unwrap();
        assert!(
            out.starts_with("Follow-up planned for Tuesday, 20 October 2026, 17:00 — in 19h 50m (it is now Monday, 19 October 2026, 21:10)"),
            "{out}"
        );
        let pending = storage.db().followups_pending_in_chat(ctx.chat_id).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].about, "Ask how the interview went");
        assert_eq!(pending[0].profile_id, ctx.profile_id);
        assert!(out.contains(&pending[0].id.to_string()));

        // A second plan lists the first, so a duplicate can be noticed.
        let args = serde_json::json!({"when": "2026-10-23 09:00", "weekday": "fri", "about": "Call the bank"});
        let out = plan(&ctx, &args, monday()).unwrap();
        assert!(out.contains("Also pending in this conversation"), "{out}");
        assert!(out.contains("Ask how the interview went"), "{out}");
    }

    #[test]
    fn a_date_its_weekday_contradicts_is_refused_with_the_week_spelled_out() {
        let (_d, storage, mut ctx) = ctx_with_storage(Uuid::new_v4());
        en(&mut ctx);
        // gpt-oss's measured slip: "Friday" on the 22nd, a Thursday.
        let args = serde_json::json!({"when": "2026-10-22 09:00", "weekday": "Friday", "about": "Call the bank"});
        let err = plan(&ctx, &args, monday()).unwrap_err().to_string();
        assert!(
            err.contains("2026-10-22 is a Thursday, not a Friday"),
            "{err}"
        );
        assert!(err.contains("Friday, 23 October 2026"), "{err}");
        assert!(storage.db().followups_pending().unwrap().is_empty());
        // A day it cannot read is not checked.
        let args = serde_json::json!({"when": "2026-10-22 09:00", "weekday": "?", "about": "x"});
        assert!(plan(&ctx, &args, monday()).is_ok());
    }

    #[test]
    fn the_past_a_bad_form_an_empty_about_and_a_full_chat_are_refused() {
        let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
        en(&mut ctx);
        let refuse = |args: serde_json::Value| plan(&ctx, &args, monday()).unwrap_err().to_string();
        let past = refuse(
            serde_json::json!({"when": "2026-10-19 09:00", "weekday": "Monday", "about": "x"}),
        );
        assert!(
            past.contains("not in the future") && past.contains("It is now Monday"),
            "{past}"
        );
        let form = refuse(serde_json::json!({"when": "tomorrow", "weekday": "", "about": "x"}));
        assert!(form.contains("YYYY-MM-DD HH:MM"), "{form}");
        let far =
            refuse(serde_json::json!({"when": "2028-01-01 09:00", "weekday": "", "about": "x"}));
        assert!(far.contains("more than a year"), "{far}");
        let empty =
            refuse(serde_json::json!({"when": "2026-10-20 09:00", "weekday": "", "about": " "}));
        assert!(!empty.is_empty());
        for _ in 0..PENDING_PER_CHAT {
            plan(
                &ctx,
                &serde_json::json!({"when": "2026-10-20 12:00", "weekday": "", "about": "y"}),
                monday(),
            )
            .unwrap();
        }
        let full =
            refuse(serde_json::json!({"when": "2026-10-20 12:00", "weekday": "", "about": "y"}));
        assert!(full.contains("followup_cancel"), "{full}");
    }

    #[tokio::test]
    async fn cancel_closes_one_of_this_chats_and_names_the_rest_when_it_misses() {
        let (_d, storage, mut ctx) = ctx_with_storage(Uuid::new_v4());
        en(&mut ctx);
        let f = Followup::new(
            ctx.profile_id,
            ctx.chat_id,
            "Ask about the run",
            Utc::now() + Duration::days(1),
        );
        let elsewhere = Followup::new(
            ctx.profile_id,
            Uuid::new_v4(),
            "Other chat",
            Utc::now() + Duration::days(1),
        );
        storage.db().followup_insert(&f).unwrap();
        storage.db().followup_insert(&elsewhere).unwrap();

        // Another conversation's follow-up is not this one's to cancel.
        let err = FollowupCancel
            .invoke(&ctx, serde_json::json!({"id": elsewhere.id.to_string()}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Ask about the run"), "{err}");

        let out = FollowupCancel
            .invoke(&ctx, serde_json::json!({"id": f.id.to_string()}))
            .await
            .unwrap();
        assert!(out.result.contains("Ask about the run"), "{}", out.result);
        assert_eq!(
            storage.db().followup_get(f.id).unwrap().unwrap().state,
            FollowupState::Cancelled
        );
        assert_eq!(
            storage
                .db()
                .followup_get(elsewhere.id)
                .unwrap()
                .unwrap()
                .state,
            FollowupState::Pending
        );
    }
}
