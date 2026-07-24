//! `python_exec` tool (spec §9.3, §13.2): executes Python in one of two
//! modes ([`PythonMode`]):
//!
//! - **Wasmer** (default) — an isolated WASIX sandbox behind the `wasmer` sidecar
//!   (`shared::sandbox`): no access to the host filesystem, network via a flag, preinstalled
//!   packages. See docs/research/python-wasmer-sandbox.md.
//! - **Local** — the previous behavior: the system interpreter as a separate process with
//!   a timeout. No OS sandbox (the code runs on the user's machine).
//!
//! The master switch `tools.python_enabled` gates the tool as a whole (off
//! by default). The tool's id (`python_exec`) doesn't depend on the mode.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::shared::config::PythonMode;
use crate::shared::sandbox::{SandboxAvailability, SandboxRunner};

use super::{Tool, ToolContext, ToolOutcome};

/// Execution timeout in local mode.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum size of captured output (characters) — protection against a flood.
const MAX_OUTPUT_CHARS: usize = 8000;

/// `python_exec` — executes the given Python code and returns stdout/stderr.
pub struct PythonExec {
    /// The execution mode (sandbox/local).
    mode: PythonMode,
    /// The interpreter path (Local; `None` → the system `python3`/`python`).
    python_path: Option<String>,
    /// The sandbox implementation (Wasmer).
    sandbox: Arc<dyn SandboxRunner>,
    /// Allow network access in the sandbox (Wasmer).
    net: bool,
    /// Execution timeout in the sandbox (Wasmer).
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

    /// The interpreter's name/path with a sensible platform default (Local).
    fn interpreter(&self) -> String {
        self.python_path.clone().unwrap_or_else(|| {
            if cfg!(windows) {
                "python".to_string()
            } else {
                "python3".to_string()
            }
        })
    }

    /// Local mode: the system interpreter as a separate process. Returns an already-
    /// formatted result text (success/error/timeout) in the language `loc`.
    async fn run_local(&self, code: &str, loc: &crate::shared::i18n::Locale) -> String {
        // The argument is passed directly (no shell) — no escaping issues.
        let mut cmd = tokio::process::Command::new(self.interpreter());
        cmd.arg("-c")
            .arg(code)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Output goes into a pipe, not the console, so Python on Windows picks
            // an encoding by locale (often cp1252) and fails on Cyrillic in `print`
            // (`UnicodeEncodeError`). We read the output as UTF-8, so we
            // also ask Python to write UTF-8. See CLAUDE.md (M7).
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                return loc.tf(
                    "tool.python_exec.err.spawn",
                    &[("py", &self.interpreter()), ("err", &err.to_string())],
                );
            }
        };

        // On a timeout the future is dropped → the process is killed (kill_on_drop).
        match tokio::time::timeout(LOCAL_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(out)) => format_output_parts(
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
                out.status.success(),
                out.status.code(),
                loc,
            ),
            Ok(Err(err)) => loc.tf("tool.python_exec.err.exec", &[("err", &err.to_string())]),
            Err(_) => loc.tf(
                "tool.python_exec.err.timeout",
                &[("secs", &LOCAL_TIMEOUT.as_secs().to_string())],
            ),
        }
    }

    /// Wasmer sandbox mode. Returns an already-formatted result text in
    /// the language `loc`.
    async fn run_wasmer(&self, code: &str, loc: &crate::shared::i18n::Locale) -> String {
        match self.sandbox.availability(loc) {
            SandboxAvailability::Missing(why) => {
                loc.tf("tool.python_exec.err.sandbox_missing", &[("why", &why)])
            }
            SandboxAvailability::Ready => {
                match self
                    .sandbox
                    .run(code, self.net, self.wasm_timeout, loc)
                    .await
                {
                    Ok(out) if out.timed_out => loc.tf(
                        "tool.python_exec.err.timeout",
                        &[("secs", &self.wasm_timeout.as_secs().to_string())],
                    ),
                    Ok(out) => format_output_parts(
                        &out.stdout,
                        &out.stderr,
                        out.exit_code == Some(0),
                        out.exit_code,
                        loc,
                    ),
                    Err(e) => loc.tf("tool.python_exec.err.sandbox", &[("e", &e.to_string())]),
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
        "run Python"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Python)
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        match self.mode {
            PythonMode::Local => loc.t("tool.python_exec.desc.local").into(),
            PythonMode::Wasmer => {
                let net = if self.net {
                    loc.t("tool.python_exec.net.on")
                } else {
                    loc.t("tool.python_exec.net.off")
                };
                loc.tf("tool.python_exec.desc.wasmer", &[("net", net)])
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
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let code = args
            .get("code")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.python_exec.err.code_empty")))?
            .to_string();

        let result = match self.mode {
            PythonMode::Local => self.run_local(&code, ctx.loc).await,
            PythonMode::Wasmer => self.run_wasmer(&code, ctx.loc).await,
        };
        Ok(ToolOutcome::text(result))
    }
}

/// Formats the execution result (stdout/stderr/exit code) — a shared shape for
/// both modes, so the feed's presenter (`present::parse_console`) recognizes the
/// console by its labels. The `stdout:`/`stderr:` labels are universal (not translated);
/// the exit-code label (`python.console.exit`) and service strings are localized via `loc`,
/// and `parse_console` recognizes the code label across all locales.
fn format_output_parts(
    stdout: &str,
    stderr: &str,
    success: bool,
    code: Option<i32>,
    loc: &crate::shared::i18n::Locale,
) -> String {
    let mut parts = Vec::new();
    if !stdout.trim().is_empty() {
        parts.push(format!(
            "stdout:\n{}",
            truncate(stdout, MAX_OUTPUT_CHARS, loc)
        ));
    }
    if !stderr.trim().is_empty() {
        parts.push(format!(
            "stderr:\n{}",
            truncate(stderr, MAX_OUTPUT_CHARS, loc)
        ));
    }
    if !success {
        parts.push(format!(
            "{} {}",
            loc.t("python.console.exit"),
            code.unwrap_or(-1)
        ));
    }
    if parts.is_empty() {
        loc.t("python.console.empty").to_string()
    } else {
        parts.join("\n\n")
    }
}

/// Truncates a string to `max` characters with a truncation note (in the language `loc`).
fn truncate(s: &str, max: usize, loc: &crate::shared::i18n::Locale) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}\n{}", loc.t("python.truncated"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{ctx_with_storage, ctx_with_storage_lang};
    use super::*;
    use crate::shared::i18n::Lang;
    use crate::shared::sandbox::{MockSandbox, SandboxOutput};
    use uuid::Uuid;

    /// Whether the string has no Cyrillic (Russian leaking on an en profile).
    fn no_cyrillic(s: &str) -> bool {
        !s.chars()
            .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё')
    }

    /// Reference locale (ru) for direct calls to output formatting.
    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// The tool in local mode with a given interpreter path.
    fn local(python_path: Option<String>) -> PythonExec {
        PythonExec::new(
            PythonMode::Local,
            python_path,
            Arc::new(MockSandbox::missing("не должно вызываться в Local")),
            false,
            Duration::from_secs(30),
        )
    }

    /// The tool in sandbox mode with a given mock runner.
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
        let out = truncate(&long, MAX_OUTPUT_CHARS, ru());
        assert!(out.contains("вывод обрезан"));
    }

    #[test]
    fn format_output_parts_shapes_console() {
        let s = format_output_parts("hi", "oops", false, Some(2), ru());
        assert!(s.contains("stdout:\nhi"));
        assert!(s.contains("stderr:\noops"));
        assert!(s.contains("код возврата: 2"));
        assert_eq!(
            format_output_parts("", "", true, Some(0), ru()),
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
        // The runner is called exactly once, with the net flag.
        let calls = sb.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1, "the net flag must be forwarded to the runner");
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

    /// Real execution in the sandbox (manual): requires an installed `wasmer`
    /// (env `MINDFORK_SANDBOX_WASMER` or a binary in `data/sandbox/`) and network for the
    /// first download of `python/python`. `cargo test -- --ignored`.
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

    /// The tool over a **provisioned** sandbox (`mindfork sandbox setup`):
    /// the directory is given by env `MINDFORK_SANDBOX_DIR` (holding wasmer-dist/python.webc/
    /// site-packages). `None` — the env isn't set, the smoke is skipped.
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

    /// numpy (native `.so` via WASIX dynamic linking) in a provisioned sandbox.
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

    /// pandas (a native wasix wheel + pure dependencies) in a provisioned sandbox.
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

    /// requests over HTTPS with network access enabled.
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

    /// With no network access the request must fail (no sockets in the sandbox) — not 200.
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

    /// Cyrillic in `print` must not fail (the WASIX guest is UTF-8).
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

    /// An infinite loop is interrupted by the timeout (killing the wasmer process).
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

    /// The tool over a provisioned sandbox with a memory limit (Windows).
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

    /// A memory limit (Windows Job Object) keeps a runaway script from eating the host's
    /// memory: a large allocation under a low limit fails (the process is killed).
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn memory_cap_stops_runaway() {
        let Some(tool) = provisioned_capped(1024) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        // A 3 GB allocation under a 1 GB limit must fail — either a graceful
        // MemoryError, a fatal V8 crash, or a non-zero exit code.
        let code = "b = bytearray(3 * 1024 * 1024 * 1024)\nprint(len(b))";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        let r = &out.result;
        assert!(
            r.contains("MemoryError") || r.contains("Fatal") || r.contains("код возврата"),
            "expected the allocation to fail under the limit, got: {r}"
        );
        // And 3 GiB definitely weren't allocated (the byte count didn't show up in stdout).
        assert!(!r.contains("3221225472"), "got: {r}");
    }

    /// A reasonable limit (2 GB) doesn't get in the way of light work.
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

    /// On an en profile, sandbox unavailability is explained **in English** (axis A):
    /// the `python_exec` wrapper + the nested reason from `sandbox.rs` — both English, with no
    /// Russian leaking. Not `#[ignore]` (doesn't need a real `wasmer` — the Missing path).
    #[tokio::test]
    async fn en_sandbox_missing_is_localized() {
        use crate::shared::sandbox::WasmerSandbox;
        if std::env::var_os("MINDFORK_SANDBOX_WASMER").is_some() {
            return; // the environment supplies a binary — the Missing path won't reproduce
        }
        let empty = tempfile::tempdir().unwrap();
        let tool = PythonExec::new(
            PythonMode::Wasmer,
            None,
            Arc::new(WasmerSandbox::new(Some(empty.path().to_path_buf()))),
            false,
            Duration::from_secs(30),
        );
        let (_d, _s, ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "print(1)"}))
            .await
            .unwrap();
        let r = &out.result;
        assert!(r.contains("The Python sandbox is unavailable"), "{r}");
        assert!(r.contains("`wasmer` binary not found"), "{r}");
        assert!(no_cyrillic(r), "cyrillic leaked on en-profile: {r}");
    }

    /// On an en profile, the output and the **exit-code label** are English (axis A). A real
    /// (provisioned) sandbox: `print` + a non-zero `sys.exit`.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn en_sandbox_output_and_exit_label_localized() {
        let Some(tool) = provisioned(false, 60) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
        let code = "print('hello'); import sys; sys.exit(3)";
        let out = tool
            .invoke(&ctx, serde_json::json!({ "code": code }))
            .await
            .unwrap();
        let r = &out.result;
        assert!(r.contains("hello"), "{r}");
        assert!(r.contains("exit code:"), "the en exit-code label: {r}");
        assert!(no_cyrillic(r), "cyrillic leaked on en-profile: {r}");
    }

    /// On an en profile the timeout message is English (axis A). A real sandbox.
    #[tokio::test]
    #[ignore = "requires a provisioned sandbox (MINDFORK_SANDBOX_DIR)"]
    async fn en_sandbox_timeout_localized() {
        let Some(tool) = provisioned(false, 3) else {
            return;
        };
        let (_d, _s, ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
        let out = tool
            .invoke(&ctx, serde_json::json!({"code": "while True: pass"}))
            .await
            .unwrap();
        let r = &out.result;
        assert!(r.contains("exceeded the time limit"), "{r}");
        assert!(no_cyrillic(r), "cyrillic leaked on en-profile: {r}");
    }

    /// Real local execution (manual, if Python is installed).
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

    /// Cyrillic in `print` must not fail with `UnicodeEncodeError` (Windows cp1252).
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
