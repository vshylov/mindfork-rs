//! Инструмент `current_time` (spec §9.3): текущие дата и время.
//!
//! Модель не знает «сейчас» (её знания статичны), поэтому даёт текущий момент в
//! локальной зоне и UTC. Опционально форматирует по строке `strftime`. Чистый
//! инструмент без I/O — не гейтится выключателями.

use anyhow::Result;
use chrono::{Local, Utc};

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// `current_time` — возвращает текущие дату и время.
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
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Получить текущие дату и время (локальная зона и UTC). Опционально передай \
         format — строку формата strftime (например %Y-%m-%d или %H:%M)."
            .into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "format": {
                    "type": "string",
                    "description": "Необязательная строка формата strftime, напр. %Y-%m-%d %H:%M:%S"
                }
            }
        })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let format = args.get("format").and_then(|v| v.as_str());
        Ok(ToolOutcome::text(render(Local::now(), Utc::now(), format)))
    }
}

/// Формирует ответ: при заданном `format` — локальное время по нему, иначе —
/// человекочитаемые локальное время и UTC (RFC 3339). Вынесено для тестируемости
/// (инъекция момента времени).
fn render(
    local: chrono::DateTime<Local>,
    utc: chrono::DateTime<Utc>,
    format: Option<&str>,
) -> String {
    if let Some(fmt) = format.filter(|f| !f.trim().is_empty()) {
        // `format` с неверной спецификацией паникует при материализации — ловим
        // через отдельную попытку рендера в String.
        return match render_with_format(local, fmt) {
            Some(s) => s,
            None => format!("Неверная строка формата «{fmt}»."),
        };
    }
    format!(
        "Локальное время: {}\nUTC: {}",
        local.format("%Y-%m-%d %H:%M:%S %:z"),
        utc.format("%Y-%m-%d %H:%M:%S UTC")
    )
}

/// Пытается отформатировать момент по строке `strftime`. `None`, если строка
/// содержит неверную спецификацию (иначе `format()` паникует при выводе).
fn render_with_format(local: chrono::DateTime<Local>, fmt: &str) -> Option<String> {
    use std::fmt::Write;
    let mut out = String::new();
    // `write!` с `DelayedFormat` возвращает Err на неверной спецификации, а не паникует.
    write!(out, "{}", local.format(fmt)).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use chrono::TimeZone;
    use uuid::Uuid;

    #[test]
    fn default_render_has_local_and_utc() {
        let utc = Utc.with_ymd_and_hms(2026, 6, 23, 12, 30, 0).unwrap();
        let local = utc.with_timezone(&Local);
        let s = render(local, utc, None);
        assert!(s.contains("Локальное время:"), "got: {s}");
        assert!(s.contains("UTC:"), "got: {s}");
        assert!(s.contains("2026"), "got: {s}");
    }

    #[test]
    fn custom_format_is_applied() {
        let utc = Utc.with_ymd_and_hms(2026, 6, 23, 12, 30, 0).unwrap();
        let local = utc.with_timezone(&Local);
        let s = render(local, utc, Some("%Y-%m-%d"));
        // Локальная дата (зона теста неизвестна) — но год точно присутствует.
        assert!(s.contains("2026"), "got: {s}");
        assert!(!s.contains("UTC"), "формат-режим не печатает UTC: {s}");
    }

    #[test]
    fn invalid_format_reports_error() {
        let utc = Utc.with_ymd_and_hms(2026, 6, 23, 12, 30, 0).unwrap();
        let local = utc.with_timezone(&Local);
        let s = render(local, utc, Some("%Q"));
        assert!(s.contains("Неверная строка формата"), "got: {s}");
    }

    #[test]
    fn empty_format_falls_back_to_default() {
        let utc = Utc.with_ymd_and_hms(2026, 6, 23, 12, 30, 0).unwrap();
        let local = utc.with_timezone(&Local);
        let s = render(local, utc, Some("   "));
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
