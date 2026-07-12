//! Инструмент `python_exec` (spec §9.3, §13.2): исполнение Python в одном из двух
//! режимов ([`PythonMode`]):
//!
//! - **Wasmer** (по умолчанию) — изолированная песочница WASIX за сайдкаром `wasmer`
//!   (`shared::sandbox`): нет доступа к хост-ФС, сеть по флагу, предустановленные
//!   пакеты. См. docs/research/python-wasmer-sandbox.md.
//! - **Local** — прежнее поведение: системный интерпретатор отдельным процессом с
//!   таймаутом. Без OS-песочницы (код исполняется на машине пользователя).
//!
//! Мастер-выключатель `tools.python_enabled` гейтит инструмент целиком (по умолчанию
//! выключен). Id инструмента (`python_exec`) от режима не зависит.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::shared::config::PythonMode;
use crate::shared::sandbox::{SandboxAvailability, SandboxRunner};

use super::{Tool, ToolContext, ToolOutcome};

/// Таймаут исполнения в локальном режиме.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Максимальный размер захваченного вывода (символов) — защита от лавины.
const MAX_OUTPUT_CHARS: usize = 8000;

/// `python_exec` — исполняет переданный код Python и возвращает stdout/stderr.
pub struct PythonExec {
    /// Режим исполнения (песочница/локально).
    mode: PythonMode,
    /// Путь к интерпретатору (Local; `None` → системный `python3`/`python`).
    python_path: Option<String>,
    /// Реализация песочницы (Wasmer).
    sandbox: Arc<dyn SandboxRunner>,
    /// Разрешить сеть в песочнице (Wasmer).
    net: bool,
    /// Таймаут исполнения в песочнице (Wasmer).
    wasm_timeout: Duration,
}

impl PythonExec {
    pub fn new(
        mode: PythonMode,
        python_path: Option<String>,
        sandbox: Arc<dyn SandboxRunner>,
        net: bool,
        wasm_timeout: Duration,
    ) -> Self {
        Self {
            mode,
            python_path,
            sandbox,
            net,
            wasm_timeout,
        }
    }

    /// Имя/путь интерпретатора с разумным дефолтом по платформе (Local).
    fn interpreter(&self) -> String {
        self.python_path.clone().unwrap_or_else(|| {
            if cfg!(windows) {
                "python".to_string()
            } else {
                "python3".to_string()
            }
        })
    }

    /// Локальный режим: системный интерпретатор отдельным процессом. Возвращает уже
    /// отформатированный текст результата (успех/ошибка/таймаут).
    async fn run_local(&self, code: &str) -> String {
        // Аргумент передаётся напрямую (без шелла) — нет проблем с экранированием.
        let mut cmd = tokio::process::Command::new(self.interpreter());
        cmd.arg("-c")
            .arg(code)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Вывод идёт в pipe, а не в консоль, поэтому Python на Windows выбирает
            // кодировку по локали (часто cp1252) и падает на кириллице в `print`
            // (`UnicodeEncodeError`). Мы читаем вывод как UTF-8, поэтому и Python
            // просим писать UTF-8. См. CLAUDE.md (M7).
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                return format!(
                    "Не удалось запустить Python ({}): {err}",
                    self.interpreter()
                );
            }
        };

        // На таймауте future дропается → процесс убивается (kill_on_drop).
        match tokio::time::timeout(LOCAL_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(out)) => format_output_parts(
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
                out.status.success(),
                out.status.code(),
            ),
            Ok(Err(err)) => format!("Ошибка исполнения: {err}"),
            Err(_) => format!(
                "Python превысил лимит времени ({} с) и был остановлен.",
                LOCAL_TIMEOUT.as_secs()
            ),
        }
    }

    /// Режим песочницы Wasmer. Возвращает уже отформатированный текст результата.
    async fn run_wasmer(&self, code: &str) -> String {
        match self.sandbox.availability() {
            SandboxAvailability::Missing(why) => format!(
                "Песочница Python недоступна: {why}.\n\nМожно переключиться на локальный \
                 интерпретатор в настройках (Инструменты → Python → Режим)."
            ),
            SandboxAvailability::Ready => {
                match self.sandbox.run(code, self.net, self.wasm_timeout).await {
                    Ok(out) if out.timed_out => format!(
                        "Python превысил лимит времени ({} с) и был остановлен.",
                        self.wasm_timeout.as_secs()
                    ),
                    Ok(out) => format_output_parts(
                        &out.stdout,
                        &out.stderr,
                        out.exit_code == Some(0),
                        out.exit_code,
                    ),
                    Err(e) => format!("Ошибка песочницы: {e}"),
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Tool for PythonExec {
    fn id(&self) -> ToolId {
        super::PYTHON_EXEC_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::ExternalWorld
    }
    fn ui_label(&self) -> &'static str {
        "исполнить Python"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Python)
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        match self.mode {
            PythonMode::Local => "Исполнить код Python и вернуть stdout/stderr \
                 (локальный интерпретатор). Есть таймаут и лимит вывода."
                .into(),
            PythonMode::Wasmer => {
                let net = if self.net {
                    "есть доступ в сеть"
                } else {
                    "без доступа в сеть"
                };
                format!(
                    "Исполнить код Python в изолированной песочнице (нет доступа к файлам \
                     машины; {net}). При установленной песочнице доступны научные пакеты \
                     (numpy, pandas, requests и т.п.). Есть таймаут и лимит вывода."
                )
            }
        }
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
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

        let result = match self.mode {
            PythonMode::Local => self.run_local(&code).await,
            PythonMode::Wasmer => self.run_wasmer(&code).await,
        };
        Ok(ToolOutcome::text(result))
    }
}

/// Форматирует результат исполнения (stdout/stderr/код возврата) — единый вид для
/// обоих режимов, чтобы презентер ленты (`present::parse_console`) распознавал
/// консоль по меткам `stdout:`/`stderr:`/`код возврата:`.
fn format_output_parts(stdout: &str, stderr: &str, success: bool, code: Option<i32>) -> String {
    let mut parts = Vec::new();
    if !stdout.trim().is_empty() {
        parts.push(format!("stdout:\n{}", truncate(stdout, MAX_OUTPUT_CHARS)));
    }
    if !stderr.trim().is_empty() {
        parts.push(format!("stderr:\n{}", truncate(stderr, MAX_OUTPUT_CHARS)));
    }
    if !success {
        parts.push(format!("код возврата: {}", code.unwrap_or(-1)));
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
    use crate::shared::sandbox::{MockSandbox, SandboxOutput};
    use uuid::Uuid;

    /// Инструмент в локальном режиме с заданным путём интерпретатора.
    fn local(python_path: Option<String>) -> PythonExec {
        PythonExec::new(
            PythonMode::Local,
            python_path,
            Arc::new(MockSandbox::missing("не должно вызываться в Local")),
            false,
            Duration::from_secs(30),
        )
    }

    /// Инструмент в режиме песочницы с заданным mock-раннером.
    fn wasmer(sandbox: Arc<dyn SandboxRunner>, net: bool) -> PythonExec {
        PythonExec::new(
            PythonMode::Wasmer,
            None,
            sandbox,
            net,
            Duration::from_secs(30),
        )
    }

    #[tokio::test]
    async fn rejects_empty_code() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            local(None)
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

    #[test]
    fn format_output_parts_shapes_console() {
        let s = format_output_parts("hi", "oops", false, Some(2));
        assert!(s.contains("stdout:\nhi"));
        assert!(s.contains("stderr:\noops"));
        assert!(s.contains("код возврата: 2"));
        assert_eq!(
            format_output_parts("", "", true, Some(0)),
            "(пустой вывод, успех)"
        );
    }

    #[tokio::test]
    async fn missing_interpreter_reports_error_not_panic() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let tool = local(Some("definitely-not-a-real-python-xyz".into()));
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(1)"}))
            .await
            .unwrap();
        assert!(out.result.contains("Не удалось запустить Python"));
    }

    #[tokio::test]
    async fn wasmer_mode_dispatches_to_sandbox_and_formats() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let sb = Arc::new(MockSandbox::ready(SandboxOutput {
            stdout: "42\n".into(),
            stderr: String::new(),
            exit_code: Some(0),
            timed_out: false,
        }));
        let tool = wasmer(sb.clone(), true);
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(6*7)"}))
            .await
            .unwrap();
        assert!(out.result.contains("stdout:\n42"));
        // Раннер вызван ровно раз, с флагом сети.
        let calls = sb.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1, "net-флаг должен пробрасываться в раннер");
        assert!(calls[0].0.contains("print(6*7)"));
    }

    #[tokio::test]
    async fn wasmer_mode_timeout_message() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let sb = Arc::new(MockSandbox::ready(SandboxOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            timed_out: true,
        }));
        let out = wasmer(sb, false)
            .invoke(&ctx, serde_json::json!({"code": "while True: pass"}))
            .await
            .unwrap();
        assert!(out.result.contains("превысил лимит времени"));
    }

    #[tokio::test]
    async fn wasmer_mode_missing_sandbox_explains() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let tool = wasmer(Arc::new(MockSandbox::missing("нет бинаря")), true);
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(1)"}))
            .await
            .unwrap();
        assert!(out.result.contains("Песочница Python недоступна"));
        assert!(out.result.contains("нет бинаря"));
    }

    #[test]
    fn description_varies_by_mode_and_net() {
        assert!(
            local(None)
                .description(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru))
                .contains("локальный интерпретатор")
        );
        let sb: Arc<dyn SandboxRunner> = Arc::new(MockSandbox::missing("x"));
        assert!(
            wasmer(sb.clone(), true)
                .description(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru))
                .contains("есть доступ в сеть")
        );
        assert!(
            wasmer(sb, false)
                .description(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru))
                .contains("без доступа в сеть")
        );
    }

    /// Реальное исполнение в песочнице (вручную): требует установленного `wasmer`
    /// (env `MINDFORK_SANDBOX_WASMER` или бинарь в `data/sandbox/`) и сети для
    /// первого скачивания `python/python`. `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "requires a bundled wasmer sidecar (MINDFORK_SANDBOX_WASMER)"]
    async fn runs_real_python_in_sandbox() {
        use crate::shared::sandbox::WasmerSandbox;
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let tool = PythonExec::new(
            PythonMode::Wasmer,
            None,
            Arc::new(WasmerSandbox::new(None)),
            false,
            Duration::from_secs(120),
        );
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print('hello sandbox')"}))
            .await
            .unwrap();
        assert!(out.result.contains("hello sandbox"), "got: {}", out.result);
    }

    /// Инструмент над **провизионированной** песочницей (`mindfork sandbox setup`):
    /// каталог задаётся env `MINDFORK_SANDBOX_DIR` (в нём wasmer-dist/python.webc/
    /// site-packages). `None` — env не задан, смоук пропускается.
    fn provisioned(net: bool, timeout_secs: u64) -> Option<PythonExec> {
        use crate::shared::sandbox::WasmerSandbox;
        let dir = std::env::var("MINDFORK_SANDBOX_DIR").ok()?;
        Some(PythonExec::new(
            PythonMode::Wasmer,
            None,
            Arc::new(WasmerSandbox::new(Some(std::path::PathBuf::from(dir)))),
            net,
            Duration::from_secs(timeout_secs),
        ))
    }

    /// numpy (нативные `.so` через динлинковку WASIX) в провизионированной песочнице.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn numpy_in_sandbox() {
        let Some(tool) = provisioned(false, 120) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let code = "import numpy as np; print('numpy', np.__version__); \
                    print('sum', int(np.arange(10).sum()))";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(out.result.contains("numpy 2."), "got: {}", out.result);
        assert!(out.result.contains("sum 45"), "got: {}", out.result);
    }

    /// pandas (нативное wasix-колесо + чистые зависимости) в провизионированной песочнице.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn pandas_in_sandbox() {
        let Some(tool) = provisioned(false, 120) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let code = "import pandas as pd; \
                    df = pd.DataFrame({'a': [1, 2, 3], 'b': [4, 5, 6]}); \
                    print('pandas', pd.__version__); print('total', int(df.values.sum()))";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(out.result.contains("pandas 2."), "got: {}", out.result);
        assert!(out.result.contains("total 21"), "got: {}", out.result);
    }

    /// requests по HTTPS при включённой сети.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox + network (MINDFORK_SANDBOX_DIR)"]
    async fn requests_in_sandbox_with_net() {
        let Some(tool) = provisioned(true, 120) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let code = "import requests; r = requests.get('https://example.com', timeout=20); \
                    print('status', r.status_code)";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(out.result.contains("status 200"), "got: {}", out.result);
    }

    /// Без сети запрос должен провалиться (сокетов в песочнице нет) — не 200.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn requests_blocked_without_net() {
        let Some(tool) = provisioned(false, 60) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let code = "import requests\n\
                    try:\n\
                    \x20   r = requests.get('https://example.com', timeout=10)\n\
                    \x20   print('status', r.status_code)\n\
                    except Exception as e:\n\
                    \x20   print('blocked')";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        assert!(!out.result.contains("status 200"), "got: {}", out.result);
    }

    /// Кириллица в `print` не должна падать (гость WASIX — UTF-8).
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn cyrillic_print_in_sandbox() {
        let Some(tool) = provisioned(false, 60) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print('Привет, мир')"}))
            .await
            .unwrap();
        assert!(out.result.contains("Привет, мир"), "got: {}", out.result);
    }

    /// Бесконечный цикл прерывается по таймауту (kill процесса wasmer).
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn timeout_kills_sandbox() {
        let Some(tool) = provisioned(false, 3) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "while True: pass"}))
            .await
            .unwrap();
        assert!(
            out.result.contains("превысил лимит времени"),
            "got: {}",
            out.result
        );
    }

    /// Инструмент над провизионированной песочницей с лимитом памяти (Windows).
    #[cfg(windows)]
    fn provisioned_capped(memory_mb: u64) -> Option<PythonExec> {
        use crate::shared::sandbox::WasmerSandbox;
        let dir = std::env::var("MINDFORK_SANDBOX_DIR").ok()?;
        Some(PythonExec::new(
            PythonMode::Wasmer,
            None,
            Arc::new(
                WasmerSandbox::new(Some(std::path::PathBuf::from(dir)))
                    .with_memory_limit(Some(memory_mb)),
            ),
            false,
            Duration::from_secs(60),
        ))
    }

    /// Лимит памяти (Windows Job Object) не даёт рантайм-скрипту выесть память хоста:
    /// большой allocation под низким лимитом не проходит (процесс убит).
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn memory_cap_stops_runaway() {
        let Some(tool) = provisioned_capped(1024) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        // Выделение 3 ГБ под лимитом 1 ГБ обязано провалиться — либо graceful
        // MemoryError, либо фатальный крах V8, либо ненулевой код возврата.
        let code = "b = bytearray(3 * 1024 * 1024 * 1024)\nprint(len(b))";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        let r = &out.result;
        assert!(
            r.contains("MemoryError") || r.contains("Fatal") || r.contains("код возврата"),
            "ожидался отказ выделения под лимитом, got: {r}"
        );
        // И 3 ГиБ точно не выделены (число байт в stdout не появилось).
        assert!(!r.contains("3221225472"), "got: {r}");
    }

    /// Разумный лимит (2 ГБ) не мешает лёгкой работе.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn memory_cap_allows_normal_work() {
        let Some(tool) = provisioned_capped(2048) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(sum(range(1000)))"}))
            .await
            .unwrap();
        assert!(out.result.contains("499500"), "got: {}", out.result);
    }

    /// Реальное локальное исполнение (вручную, если установлен Python).
    #[tokio::test]
    #[ignore = "requires a Python interpreter on PATH"]
    async fn runs_real_python_local() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = local(None)
            .invoke(&ctx, serde_json::json!({"code": "print('hello')"}))
            .await
            .unwrap();
        assert!(out.result.contains("hello"), "got: {}", out.result);
    }

    /// Кириллица в `print` не должна падать с `UnicodeEncodeError` (Windows cp1252).
    #[tokio::test]
    #[ignore = "requires a Python interpreter on PATH"]
    async fn prints_cyrillic_without_encoding_error() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = local(None)
            .invoke(&ctx, serde_json::json!({"code": "print('Привет, мир')"}))
            .await
            .unwrap();
        assert!(out.result.contains("Привет, мир"), "got: {}", out.result);
        assert!(
            !out.result.contains("UnicodeEncodeError"),
            "got: {}",
            out.result
        );
    }
}
