//! `current_time` tool (spec §9.3): the current date and time.
//!
//! The model doesn't know "now" (its knowledge is static), so this gives the
//! current moment in the local zone and UTC. Optionally formats via a
//! `strftime` string. A pure I/O-free tool — not gated by any switches.

use anyhow::Result;
use chrono::{Local, Utc};

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// `current_time` — returns the current date and time.
pub struct CurrentTime;

#[async_trait::async_trait]
impl Tool for CurrentTime {
    fn id(&self) -> ToolId {
        "current_time".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Utils
    }
    fn ui_label(&self) -> &'static str {
        "текущее время"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.current_time.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "format": {
                    "type": "string",
                    "description": loc.t("tool.current_time.param.format")
                }
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let format = args.get("format").and_then(|v| v.as_str());
        Ok(ToolOutcome::text(render(
            Local::now(),
            Utc::now(),
            format,
            ctx.loc,
        )))
    }
}

/// Builds the response: with a given `format` — local time by it, otherwise —
/// human-readable local time and UTC (RFC 3339). Factored out for testability
/// (injecting the moment in time). Texts — in the scaffold language `loc`.
fn render(
    local: chrono::DateTime<Local>,
    utc: chrono::DateTime<Utc>,
    format: Option<&str>,
    loc: &crate::shared::i18n::Locale,
) -> String {
    if let Some(fmt) = format.filter(|f| !f.trim().is_empty()) {
        // `format` with an invalid spec panics when materialized — caught via
        // a separate attempt to render into a String.
        return match render_with_format(local, fmt) {
            Some(s) => s,
            None => loc.tf("tool.current_time.err.bad_format", &[("fmt", fmt)]),
        };
    }
    format!(
        "{} {}\nUTC: {}",
        loc.t("tool.current_time.local_label"),
        local.format("%Y-%m-%d %H:%M:%S %:z"),
        utc.format("%Y-%m-%d %H:%M:%S UTC")
    )
}

/// Tries to format the moment via a `strftime` string. `None` if the string
/// contains an invalid spec (otherwise `format()` panics on output).
fn render_with_format(local: chrono::DateTime<Local>, fmt: &str) -> Option<String> {
    use std::fmt::Write;
    let mut out = String::new();
    // `write!` with `DelayedFormat` returns Err on an invalid spec, rather than panicking.
    write!(out, "{}", local.format(fmt)).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use chrono::TimeZone;
    use uuid::Uuid;

    /// Reference locale (ru) for checking render texts.
    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn default_render_has_local_and_utc() {
        let utc = Utc.with_ymd_and_hms(2026, 6, 23, 12, 30, 0).unwrap();
        let local = utc.with_timezone(&Local);
        let s = render(local, utc, None, ru());
        assert!(s.contains("Локальное время:"), "got: {s}");
        assert!(s.contains("UTC:"), "got: {s}");
        assert!(s.contains("2026"), "got: {s}");
    }

    #[test]
    fn custom_format_is_applied() {
        let utc = Utc.with_ymd_and_hms(2026, 6, 23, 12, 30, 0).unwrap();
        let local = utc.with_timezone(&Local);
        let s = render(local, utc, Some("%Y-%m-%d"), ru());
        // Local date (the test's zone is unknown) — but the year is definitely present.
        assert!(s.contains("2026"), "got: {s}");
        assert!(!s.contains("UTC"), "format mode doesn't print UTC: {s}");
    }

    #[test]
    fn invalid_format_reports_error() {
        let utc = Utc.with_ymd_and_hms(2026, 6, 23, 12, 30, 0).unwrap();
        let local = utc.with_timezone(&Local);
        let s = render(local, utc, Some("%Q"), ru());
        assert!(s.contains("Неверная строка формата"), "got: {s}");
    }

    #[test]
    fn empty_format_falls_back_to_default() {
        let utc = Utc.with_ymd_and_hms(2026, 6, 23, 12, 30, 0).unwrap();
        let local = utc.with_timezone(&Local);
        let s = render(local, utc, Some("   "), ru());
        assert!(s.contains("UTC:"), "got: {s}");
    }

    #[tokio::test]
    async fn tool_returns_time() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = CurrentTime
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("UTC"), "got: {}", out.result);
        assert!(out.effects.is_empty());
    }
}
