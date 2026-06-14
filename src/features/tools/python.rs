//! Инструмент `python_exec` (spec §9.3, §13.2): исполнение Python в отдельном
//! процессе с таймаутом и ограничением вывода. Под глобальным выключателем
//! `tools.python_enabled` (на Windows нет OS-песочницы → по умолчанию выключен).
//!
//! Изоляция в первой версии — только отдельный процесс + таймаут; усиление
//! (лимиты ресурсов/сети на Linux) — позже, без изменения контракта.

use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Таймаут на исполнение.
const PYTHON_TIMEOUT: Duration = Duration::from_secs(10);
/// Максимальный размер захваченного вывода (символов) — защита от лавины.
const MAX_OUTPUT_CHARS: usize = 8000;

/// `python_exec` — исполняет переданный код Python и возвращает stdout/stderr.
pub struct PythonExec {
    /// Путь к интерпретатору (`None` → системный `python3`/`python`).
    python_path: Option<String>,
}

impl PythonExec {
    pub fn new(python_path: Option<String>) -> Self {
        Self { python_path }
    }

    /// Имя/путь интерпретатора с разумным дефолтом по платформе.
    fn interpreter(&self) -> String {
        self.python_path.clone().unwrap_or_else(|| {
            if cfg!(windows) {
                "python".to_string()
            } else {
                "python3".to_string()
            }
        })
    }
}

#[async_trait::async_trait]
impl Tool for PythonExec {
    fn id(&self) -> ToolId {
        "python_exec".into()
    }
    fn description(&self) -> String {
        "Исполнить код Python и вернуть stdout/stderr. Есть таймаут и лимит вывода.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"code": {"type": "string"}},
            "required": ["code"]
        })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let code = args
            .get("code")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("ожидается непустое поле code"))?
            .to_string();

        // Аргумент передаётся напрямую (без шелла) — нет проблем с экранированием.
        let mut cmd = tokio::process::Command::new(self.interpreter());
        cmd.arg("-c")
            .arg(&code)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                return Ok(ToolOutcome::text(format!(
                    "Не удалось запустить Python ({}): {err}",
                    self.interpreter()
                )));
            }
        };

        // На таймауте future дропается → процесс убивается (kill_on_drop).
        let output = match tokio::time::timeout(PYTHON_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(err)) => return Ok(ToolOutcome::text(format!("Ошибка исполнения: {err}"))),
            Err(_) => {
                return Ok(ToolOutcome::text(format!(
                    "Python превысил лимит времени ({} с) и был остановлен.",
                    PYTHON_TIMEOUT.as_secs()
                )));
            }
        };

        Ok(ToolOutcome::text(format_output(&output)))
    }
}

/// Форматирует результат процесса: stdout, stderr и код возврата (с обрезкой).
fn format_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut parts = Vec::new();
    if !stdout.trim().is_empty() {
        parts.push(format!("stdout:\n{}", truncate(&stdout, MAX_OUTPUT_CHARS)));
    }
    if !stderr.trim().is_empty() {
        parts.push(format!("stderr:\n{}", truncate(&stderr, MAX_OUTPUT_CHARS)));
    }
    if !output.status.success() {
        parts.push(format!(
            "код возврата: {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    if parts.is_empty() {
        "(пустой вывод, успех)".to_string()
    } else {
        parts.join("\n\n")
    }
}

/// Обрезает строку до `max` символов с пометкой об усечении.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}\n…(вывод обрезан)")
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn rejects_empty_code() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            PythonExec::new(None)
                .invoke(&ctx, serde_json::json!({"code": "   "}))
                .await
                .is_err()
        );
    }

    #[test]
    fn truncate_marks_cut() {
        let long = "a".repeat(MAX_OUTPUT_CHARS + 10);
        let out = truncate(&long, MAX_OUTPUT_CHARS);
        assert!(out.contains("вывод обрезан"));
    }

    #[tokio::test]
    async fn missing_interpreter_reports_error_not_panic() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let tool = PythonExec::new(Some("definitely-not-a-real-python-xyz".into()));
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(1)"}))
            .await
            .unwrap();
        assert!(out.result.contains("Не удалось запустить Python"));
    }

    /// Реальное исполнение (вручную, если установлен Python): `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "requires a Python interpreter on PATH"]
    async fn runs_real_python() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = PythonExec::new(None)
            .invoke(&ctx, serde_json::json!({"code": "print('hello')"}))
            .await
            .unwrap();
        assert!(out.result.contains("hello"), "got: {}", out.result);
    }
}
