//! Launching a local `llama-server` (llama.cpp, managed mode): a child process,
//! waiting for readiness (with detection of an early process exit), stopping on
//! `drop` of the handle (a monitor task + `kill_on_drop`). See spec §3.4.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::shared::api::OpenAiClient;
use crate::shared::i18n::Locale;

/// Launch configuration for the managed `llama-server` (llama.cpp) server.
#[derive(Debug, Clone)]
pub struct ManagedConfig {
    pub binary: PathBuf,
    /// Path to the GGUF model (`-m`).
    pub model_path: Option<String>,
    /// Path to the multimodal projector (`--mmproj`) — the companion GGUF that
    /// gives a vision model its image encoder. `None` — not passed, and the server
    /// then serves a text-only model.
    pub mmproj: Option<String>,
    /// GPU layers (`-ngl`).
    pub gpu_layers: i32,
    /// Context size (`-c`).
    pub context_size: u32,
    /// Use the model's built-in chat template (`--jinja`).
    pub jinja: bool,
    /// Reasoning format (`--reasoning-format`); `None` — don't set it.
    pub reasoning_format: Option<String>,
    /// Embeddings mode (`--embeddings`) — for the embedding server.
    pub embeddings: bool,
    /// Don't use mmap when loading the model (`--no-mmap`): loads the weights entirely
    /// into RAM. Useful on network/slow disks and when the file cache is scarce.
    pub no_mmap: bool,
    /// FlashAttention (`--flash-attn`): `Some("on"/"off")`; `None` — don't set it (auto).
    pub flash_attn: Option<String>,
    /// Speculative-decoding type (`--spec-type`); `None` — disabled.
    pub spec_type: Option<String>,
    /// The speculative-decoding draft model (`-md`/`--model-draft`).
    pub draft_model: Option<String>,
    /// GPU layers for the draft model (`-ngld`); `None` — auto.
    pub draft_gpu_layers: Option<i32>,
    /// Number of draft tokens per step (`--spec-draft-n-max`); `None` — default.
    pub draft_n_max: Option<u32>,
    /// Minimum draft tokens per step (`--spec-draft-n-min`); `None` — default.
    pub draft_n_min: Option<u32>,
    /// The bind interface (`--host`).
    pub host: String,
    pub port: u16,
    /// Extra raw arguments.
    pub extra_args: Vec<String>,
}

impl ManagedConfig {
    /// The URL for connecting to the local child process (always `127.0.0.1`,
    /// regardless of `--host`, which only controls the bind interface).
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

/// Builds `llama-server`'s command-line arguments from the config (a pure function).
pub fn build_args(cfg: &ManagedConfig) -> Vec<String> {
    let mut args = vec![
        "--host".to_string(),
        cfg.host.clone(),
        "--port".to_string(),
        cfg.port.to_string(),
        "-ngl".to_string(),
        cfg.gpu_layers.to_string(),
        "-c".to_string(),
        cfg.context_size.to_string(),
    ];
    if let Some(m) = &cfg.model_path {
        args.push("-m".into());
        args.push(m.clone());
    }
    // The vision projector: passed only when configured, so a text-only setup keeps
    // the exact command line it had before images existed.
    if let Some(p) = &cfg.mmproj {
        args.push("--mmproj".into());
        args.push(p.clone());
    }
    if cfg.jinja {
        args.push("--jinja".into());
    }
    if let Some(rf) = &cfg.reasoning_format {
        args.push("--reasoning-format".into());
        args.push(rf.clone());
    }
    if cfg.embeddings {
        args.push("--embeddings".into());
        // Embedding models are non-causal: the whole input is processed in ONE physical
        // batch (ubatch). By default `n_ubatch=512`, and llama-server equates
        // `n_batch` to it — so a chunk longer than ~512 tokens gets the whole
        // request rejected ("input is too large to process. increase the physical batch
        // size"). Raise the physical and logical batch to the context size, so
        // chunks are accepted whole (for Cyrillic/code, 512 tokens is only ~a few hundred
        // characters, and large chunks stopped being indexed).
        args.push("-ub".into());
        args.push(cfg.context_size.to_string());
        args.push("-b".into());
        args.push(cfg.context_size.to_string());
    }
    if cfg.no_mmap {
        args.push("--no-mmap".into());
    }
    if let Some(fa) = &cfg.flash_attn {
        args.push("--flash-attn".into());
        args.push(fa.clone());
    }
    // Speculative decoding: the type + the draft model's parameters. Unset
    // (`None`) fields aren't passed — llama.cpp takes its own defaults.
    if let Some(st) = &cfg.spec_type {
        args.push("--spec-type".into());
        args.push(st.clone());
    }
    if let Some(md) = &cfg.draft_model {
        args.push("-md".into());
        args.push(md.clone());
    }
    if let Some(ngld) = cfg.draft_gpu_layers {
        args.push("-ngld".into());
        args.push(ngld.to_string());
    }
    if let Some(n) = cfg.draft_n_max {
        args.push("--spec-draft-n-max".into());
        args.push(n.to_string());
    }
    if let Some(n) = cfg.draft_n_min {
        args.push("--spec-draft-n-min".into());
        args.push(n.to_string());
    }
    args.extend(cfg.extra_args.iter().cloned());
    args
}

/// Owner of the `llama-server` child process. On `drop`, the process gets killed
/// (a `kill` signal → the monitor task does `start_kill`; plus `kill_on_drop` as a
/// fallback in case the runtime drops the monitor task).
pub struct ServerHandle {
    /// Armed on `drop`: the monitor task (the [`Child`] owner) kills the process.
    kill: CancellationToken,
    /// Armed by the monitor task when the child process exits on its own (normally,
    /// or crashing while loading — a corrupt GGUF, out of memory). The probe watches
    /// it to avoid waiting out the timeout pointlessly.
    exited: CancellationToken,
    base_url: String,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // The monitor task owns `Child`; signal it to kill the process.
        self.kill.cancel();
    }
}

impl ServerHandle {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// A "the child process exited" signal — for the readiness probe
    /// ([`wait_until_ready`]): catches an early exit (a corrupt GGUF/OOM) before the timeout.
    pub fn exited(&self) -> CancellationToken {
        self.exited.clone()
    }

    /// Launches the `llama-server` child process (without waiting for readiness). `loc` —
    /// the interface language for the error text (the supervisor shows it in the status chip as
    /// `ServerStatus::Disconnected`; for the embedding server the error only goes to the log).
    pub fn launch(cfg: &ManagedConfig, loc: &'static Locale) -> Result<Self> {
        // A preflight check of the model file. `spawn` below succeeds even with the
        // GGUF missing — `llama-server` only later crashes while loading and
        // exits, and the background probe (`wait_until_ready`) doesn't notice and
        // waits pointlessly until the timeout (minutes), leaving the UI at "connecting…". So
        // catch the most common cause here and immediately return a clear error
        // (the supervisor turns it into `ServerStatus::Disconnected`).
        if let Some(model) = &cfg.model_path
            && !std::path::Path::new(model).is_file()
        {
            bail!(
                "{}",
                loc.tf("ui.err.managed.model_not_found", &[("path", model)])
            );
        }
        // The same preflight check for the speculative-decoding draft
        // model (`-md`): otherwise `llama-server` would just as silently crash while
        // loading it, and the probe would wait until the timeout.
        if let Some(draft) = &cfg.draft_model
            && !std::path::Path::new(draft).is_file()
        {
            bail!(
                "{}",
                loc.tf("ui.err.managed.draft_not_found", &[("path", draft)])
            );
        }
        // And for the vision projector (`--mmproj`). Worth its own check for a
        // second reason beyond the crash: a typo here is the one failure that could
        // *look* like it worked — the server would come up serving a text-only
        // model, and the only symptom would be images being refused later, far from
        // the setting that caused it (docs/lessons.md §3).
        if let Some(mmproj) = &cfg.mmproj
            && !std::path::Path::new(mmproj).is_file()
        {
            bail!(
                "{}",
                loc.tf("ui.err.managed.mmproj_not_found", &[("path", mmproj)])
            );
        }

        let args = build_args(cfg);
        tracing::info!(binary = %cfg.binary.display(), ?args, "launching managed llama-server");

        let mut child = Command::new(&cfg.binary)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                loc.tf(
                    "ui.err.managed.spawn",
                    &[("path", &cfg.binary.display().to_string())],
                )
            })?;

        // Read the process's output so we (a) don't fill up the pipe, (b) can see loading progress.
        if let Some(out) = child.stdout.take() {
            tokio::spawn(forward_lines(out, false));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(forward_lines(err, true));
        }

        // The monitor task owns `Child` and waits either for it to exit or for a
        // kill signal (`drop` of the handle). An early exit arms `exited` — the readiness
        // probe sees this and doesn't hang until the timeout on a dead process.
        let kill = CancellationToken::new();
        let exited = CancellationToken::new();
        spawn_monitor(child, kill.clone(), exited.clone());

        Ok(Self {
            kill,
            exited,
            base_url: cfg.base_url(),
        })
    }
}

/// The child process's monitor task: waits for it to exit (arms `exited`) or for a
/// `kill` signal (kills the process). Owns [`Child`], so `kill_on_drop`
/// still fires if the runtime force-drops the task.
fn spawn_monitor(mut child: Child, kill: CancellationToken, exited: CancellationToken) {
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(s) => tracing::warn!(status = ?s, "managed llama-server exited on its own"),
                    Err(e) => tracing::warn!(error = %e, "error waiting for the child llama-server"),
                }
                exited.cancel();
            }
            _ = kill.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
    });
}

/// Waits for the server to become ready, polling `probe` until the timeout. A free
/// function, so the probe can run in the background without holding [`ServerHandle`].
///
/// `exited` (if set) — the child process's (managed) early-exit signal:
/// with a corrupt GGUF/out of memory, the process dies while loading, and without this
/// signal the probe would pointlessly poll the port until the timeout (minutes). For an
/// external process there is none — `None` is passed.
pub async fn wait_until_ready(
    client: &OpenAiClient,
    timeout: Duration,
    exited: Option<CancellationToken>,
    loc: &'static Locale,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if client.probe().await.is_ok() {
            return Ok(());
        }
        // The child process died while loading — don't wait for the timeout.
        if exited.as_ref().is_some_and(|e| e.is_cancelled()) {
            bail!("{}", loc.t("ui.err.managed.early_exit"));
        }
        if tokio::time::Instant::now() >= deadline {
            // A generic message: the probe serves not only the managed `llama-server`,
            // but also external/cloud OpenAI-compatible servers (vLLM, Gemini-compat,
            // etc.) — so the text isn't tied to a specific engine.
            bail!(
                "{}",
                loc.tf(
                    "ui.err.managed.timeout",
                    &[("timeout", &format!("{timeout:?}"))]
                )
            );
        }
        // Sleep until the next probe, but wake up right away if the process dies —
        // then the next iteration will see `exited` and finish with an error.
        let sleep = tokio::time::sleep(Duration::from_millis(500));
        match &exited {
            Some(ex) => {
                tokio::select! {
                    _ = sleep => {}
                    _ = ex.cancelled() => {}
                }
            }
            None => sleep.await,
        }
    }
}

async fn forward_lines<R>(reader: R, is_err: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if is_err {
            tracing::warn!(target: "llama-server", "{line}");
        } else {
            tracing::info!(target: "llama-server", "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    /// Reference locale for tests (ru byte-for-byte — the previous substring asserts stay intact).
    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    fn base_cfg() -> ManagedConfig {
        ManagedConfig {
            binary: PathBuf::from("llama-server"),
            model_path: None,
            mmproj: None,
            gpu_layers: 99,
            context_size: 8192,
            jinja: true,
            reasoning_format: None,
            embeddings: false,
            no_mmap: false,
            flash_attn: None,
            spec_type: None,
            draft_model: None,
            draft_gpu_layers: None,
            draft_n_max: None,
            draft_n_min: None,
            host: "127.0.0.1".into(),
            port: 8000,
            extra_args: vec![],
        }
    }

    #[test]
    fn args_include_host_port_ngl_ctx_jinja() {
        let args = build_args(&base_cfg());
        let host = args.iter().position(|a| a == "--host").unwrap();
        assert_eq!(args[host + 1], "127.0.0.1");
        let p = args.iter().position(|a| a == "--port").unwrap();
        assert_eq!(args[p + 1], "8000");
        let ngl = args.iter().position(|a| a == "-ngl").unwrap();
        assert_eq!(args[ngl + 1], "99");
        let c = args.iter().position(|a| a == "-c").unwrap();
        assert_eq!(args[c + 1], "8192");
        assert!(args.contains(&"--jinja".to_string()));
    }

    #[test]
    fn args_for_model_with_reasoning() {
        let cfg = ManagedConfig {
            model_path: Some("gemma.gguf".into()),
            reasoning_format: Some("auto".into()),
            ..base_cfg()
        };
        let args = build_args(&cfg);
        let m = args.iter().position(|a| a == "-m").unwrap();
        assert_eq!(args[m + 1], "gemma.gguf");
        let rf = args.iter().position(|a| a == "--reasoning-format").unwrap();
        assert_eq!(args[rf + 1], "auto");
        assert!(!args.contains(&"--embeddings".to_string()));
    }

    #[test]
    fn embeddings_flag_and_no_jinja() {
        let cfg = ManagedConfig {
            embeddings: true,
            jinja: false,
            ..base_cfg()
        };
        let args = build_args(&cfg);
        assert!(args.contains(&"--embeddings".to_string()));
        assert!(!args.contains(&"--jinja".to_string()));
        // The physical/logical batch is raised to the context size, otherwise chunks
        // longer than ~512 tokens would be rejected by the server.
        let ub = args.iter().position(|a| a == "-ub").expect("-ub present");
        assert_eq!(args[ub + 1], cfg.context_size.to_string());
        let b = args.iter().position(|a| a == "-b").expect("-b present");
        assert_eq!(args[b + 1], cfg.context_size.to_string());
    }

    #[test]
    fn no_batch_flags_for_non_embedding_server() {
        // The chat server doesn't get the embedder's batch flags.
        let args = build_args(&base_cfg());
        assert!(!args.contains(&"-ub".to_string()));
    }

    #[test]
    fn flash_attn_flag_present_only_when_set() {
        // Auto (None) — the flag isn't passed.
        assert!(!build_args(&base_cfg()).contains(&"--flash-attn".to_string()));
        let cfg = ManagedConfig {
            flash_attn: Some("on".into()),
            ..base_cfg()
        };
        let args = build_args(&cfg);
        let fa = args.iter().position(|a| a == "--flash-attn").unwrap();
        assert_eq!(args[fa + 1], "on");
    }

    #[test]
    fn spec_decoding_args_for_mtp_draft() {
        // A configuration for an MTP model: type draft-mtp + the draft model and its parameters.
        let cfg = ManagedConfig {
            spec_type: Some("draft-mtp".into()),
            draft_model: Some("mtp.gguf".into()),
            draft_gpu_layers: Some(99),
            draft_n_max: Some(5),
            draft_n_min: Some(1),
            ..base_cfg()
        };
        let args = build_args(&cfg);
        let st = args.iter().position(|a| a == "--spec-type").unwrap();
        assert_eq!(args[st + 1], "draft-mtp");
        let md = args.iter().position(|a| a == "-md").unwrap();
        assert_eq!(args[md + 1], "mtp.gguf");
        let ngld = args.iter().position(|a| a == "-ngld").unwrap();
        assert_eq!(args[ngld + 1], "99");
        let nmax = args.iter().position(|a| a == "--spec-draft-n-max").unwrap();
        assert_eq!(args[nmax + 1], "5");
        let nmin = args.iter().position(|a| a == "--spec-draft-n-min").unwrap();
        assert_eq!(args[nmin + 1], "1");
    }

    #[test]
    fn no_spec_args_by_default() {
        let args = build_args(&base_cfg());
        assert!(!args.contains(&"--spec-type".to_string()));
        assert!(!args.contains(&"-md".to_string()));
        assert!(!args.contains(&"-ngld".to_string()));
    }

    #[test]
    fn launch_missing_draft_model_file_errors_before_spawn() {
        // A nonexistent draft GGUF → a clear error even before spawn.
        let cfg = ManagedConfig {
            model_path: None,
            draft_model: Some("definitely/missing/draft-xyz.gguf".into()),
            ..base_cfg()
        };
        let err = match ServerHandle::launch(&cfg, ru()) {
            Err(e) => e,
            Ok(_) => panic!("expected an error about a missing draft model file"),
        };
        assert!(err.to_string().contains("черновой модели"), "{err}");
    }

    /// The projector flag is emitted only when configured: a text-only setup must
    /// keep the exact command line it had before images existed.
    #[test]
    fn mmproj_flag_present_only_when_set() {
        assert!(!build_args(&base_cfg()).contains(&"--mmproj".to_string()));
        let cfg = ManagedConfig {
            model_path: Some("gemma.gguf".into()),
            mmproj: Some("mmproj-gemma.gguf".into()),
            ..base_cfg()
        };
        let args = build_args(&cfg);
        let p = args
            .iter()
            .position(|a| a == "--mmproj")
            .expect("--mmproj present");
        assert_eq!(args[p + 1], "mmproj-gemma.gguf");
    }

    /// A typo'd projector path must be caught before `spawn`. Left unchecked it is
    /// the one managed misconfiguration that *looks* healthy: the server comes up
    /// serving a text-only model and only image sends fail, far from the cause.
    #[test]
    fn launch_missing_mmproj_file_errors_before_spawn() {
        const MISSING: &str = "definitely/missing/mmproj-xyz.gguf";
        let cfg = ManagedConfig {
            model_path: None,
            mmproj: Some(MISSING.into()),
            ..base_cfg()
        };
        let err = match ServerHandle::launch(&cfg, ru()) {
            Err(e) => e,
            Ok(_) => panic!("expected an error about a missing projector file"),
        };
        // Compared against the locale's own text rather than a hardcoded substring:
        // this pins that the *projector* preflight fired (and not `spawn`, or one of
        // the neighbouring preflights) without duplicating the wording here.
        let expected = ru().tf("ui.err.managed.mmproj_not_found", &[("path", MISSING)]);
        assert!(err.to_string().contains(&expected), "{err}");
    }

    #[test]
    fn no_mmap_flag_present_only_when_enabled() {
        assert!(!build_args(&base_cfg()).contains(&"--no-mmap".to_string()));
        let cfg = ManagedConfig {
            no_mmap: true,
            ..base_cfg()
        };
        assert!(build_args(&cfg).contains(&"--no-mmap".to_string()));
    }

    #[test]
    fn base_url_uses_port() {
        assert_eq!(base_cfg().base_url(), "http://127.0.0.1:8000/v1");
    }

    #[test]
    fn launch_missing_model_file_errors_before_spawn() {
        // A nonexistent GGUF → a clear error even before spawn (no tokio runtime),
        // instead of the probe silently hanging at "connecting…" until the timeout.
        let cfg = ManagedConfig {
            model_path: Some("definitely/missing/model-xyz.gguf".into()),
            ..base_cfg()
        };
        let err = match ServerHandle::launch(&cfg, ru()) {
            Err(e) => e,
            Ok(_) => panic!("expected an error about a missing model file"),
        };
        assert!(err.to_string().contains("файл модели"), "{err}");
    }

    #[test]
    fn launch_error_is_localized() {
        // Regression against a forgotten `loc`: en message with no Cyrillic, ru — Russian.
        let cfg = ManagedConfig {
            model_path: Some("definitely/missing/model-xyz.gguf".into()),
            ..base_cfg()
        };
        let en = match ServerHandle::launch(&cfg, locale(Lang::En)) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected an error about a missing model file"),
        };
        assert!(en.contains("model file not found"), "{en}");
        assert!(!en.chars().any(|c| ('а'..='я').contains(&c)), "{en}");
    }

    #[tokio::test]
    async fn wait_until_ready_bails_on_early_exit() {
        // The child process died while loading (`exited` armed), the port is dead —
        // the probe shouldn't hang until the timeout, but return a clear error right away.
        let client = OpenAiClient::new("http://127.0.0.1:1/v1");
        let exited = CancellationToken::new();
        exited.cancel();
        let err = wait_until_ready(&client, Duration::from_secs(600), Some(exited), ru())
            .await
            .expect_err("expected an early-exit error");
        assert!(
            err.to_string().contains("завершился до готовности"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn monitor_cancels_exited_when_child_dies() {
        // A real short-lived process: the monitor must arm `exited` on its
        // exit. Cross-platform: `cmd /C exit` on Windows, `sh -c` on unix.
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "exit"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "exit 0"]);
            c
        };
        let child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn of a short-lived process");

        let kill = CancellationToken::new();
        let exited = CancellationToken::new();
        spawn_monitor(child, kill, exited.clone());

        tokio::time::timeout(Duration::from_secs(5), exited.cancelled())
            .await
            .expect("the monitor must arm exited on process exit");
        assert!(exited.is_cancelled());
    }
}
