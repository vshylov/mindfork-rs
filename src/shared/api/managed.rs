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
    /// The batch (`-b`) the chat server is launched with, `-ub` following it
    /// clamped at [`SERVER_UBATCH`]; `None` — [`CPU_BATCH`] when `gpu_layers`
    /// is 0, nothing passed otherwise (docs/research/cpu-batch.md §4.1). The
    /// embedding server ignores it: its batch is the context size.
    pub batch_size: Option<u32>,
    /// Server slots the app will drive at once (`engine.managed.sessions`).
    /// Above 1 → `-np N --kv-unified`: N slots over the one pool `-c` sizes,
    /// the shape llama.cpp's own auto default has (four slots, unified), so
    /// the pool costs no more memory for it (docs/research/parallel-subagents.md
    /// §4.7). At 1 nothing is passed and the line is what it always was.
    pub parallel: u32,
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
/// The batch the chat server runs at on a CPU-only host when the user typed
/// none (`-ngl 0`): the knee docs/research/cpu-batch.md §3.1 measured — the
/// batch a cancel waits for falls from 23 s to 6.5 s, prompt processing slows
/// by a seventh. A GPU host at its defaults gets no `-b` at all.
pub const CPU_BATCH: u32 = 256;
/// llama.cpp's default micro-batch (`-ub`); the launcher names it beside `-b`
/// so the line reads whole, clamped to the batch as the server would clamp it.
pub const SERVER_UBATCH: u32 = 512;
/// llama.cpp's default batch (`-b`): what a `llama-server` runs when the line
/// names none — a managed GPU host at its defaults, or an external server the
/// user launched without the flag (`/props` does not say; assumed —
/// docs/research/slow-prefill-detection.md fork F3).
pub const LLAMA_DEFAULT_BATCH: u32 = 2048;
/// The slot hold a cancelled stream is worth a note about, in seconds
/// (docs/research/slow-prefill-detection.md §3.2): far from the CPU build's
/// 23 s at the default batch and from a GPU's second.
pub const PREFILL_HOLD_LIMIT_SECS: u32 = 5;
/// The smallest prefill sample the note trusts — a batch's worth of tokens, so
/// the per-request overhead does not pass for throughput.
pub const PREFILL_SAMPLE_MIN: u32 = 256;

/// The batch a managed chat server runs with, from the same two facts
/// [`build_args`] reads: the typed number, else [`CPU_BATCH`] on a host with
/// no GPU layers, else the server's default.
pub fn launched_batch(batch_size: Option<u32>, gpu_layers: i32) -> u32 {
    batch_size
        .or((gpu_layers == 0).then_some(CPU_BATCH))
        .unwrap_or(LLAMA_DEFAULT_BATCH)
}

/// The slow-prefill rule (docs/research/slow-prefill-detection.md §3.2): the
/// seconds a stream cancelled during its prompt would hold its slot — one
/// `batch` of tokens at the measured throughput — when that is worth saying:
/// a sample of at least [`PREFILL_SAMPLE_MIN`] processed tokens, a hold above
/// [`PREFILL_HOLD_LIMIT_SECS`], and a batch above [`CPU_BATCH`] (at the knee
/// the change the note would advise is already made). `None` otherwise.
pub fn prefill_hold(batch: u32, prefill: crate::shared::api::contract::Prefill) -> Option<u32> {
    if prefill.tokens < PREFILL_SAMPLE_MIN || batch <= CPU_BATCH {
        return None;
    }
    let tps = prefill.tokens_per_second()?;
    let hold = f64::from(batch) / tps;
    (hold > f64::from(PREFILL_HOLD_LIMIT_SECS)).then(|| hold.round() as u32)
}

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
    // Explicit `-np` switches llama.cpp to the *split* shape (each slot gets
    // `-c / N`), so the unified pool has to be asked for by name — and the two
    // flags together are exactly what the server does on its own for `-np -1`.
    if cfg.parallel > 1 {
        args.push("-np".into());
        args.push(cfg.parallel.to_string());
        args.push("--kv-unified".into());
    }
    // The batch a cancel waits for (docs/research/cpu-batch.md §4.1): the
    // server looks at its queue between batches of `-b` prompt tokens, so a
    // stopped or displaced stream holds its slot for one — 23 s at the
    // default on a CPU-only host, 6.5 s at 256. Auto passes `CPU_BATCH` where
    // the user runs the engine on the CPU (`-ngl 0`); a typed number is passed
    // as is; a GPU host at its defaults keeps the line byte for byte. `-ub` is
    // named too, clamped as the server clamps it. The embedding server has a
    // batch of its own below.
    if !cfg.embeddings
        && let Some(batch) = cfg
            .batch_size
            .or((cfg.gpu_layers == 0).then_some(CPU_BATCH))
    {
        args.push("-b".into());
        args.push(batch.to_string());
        args.push("-ub".into());
        args.push(batch.min(SERVER_UBATCH).to_string());
    }
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
        if let Some(model) = &cfg.model_path {
            if !std::path::Path::new(model).is_file() {
                bail!(
                    "{}",
                    loc.tf("ui.err.managed.model_not_found", &[("path", model)])
                );
            }
            check_split_model(model, loc)?;
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

/// The preflight for a **multi-file** GGUF (`shared/gguf.rs`, spec §3.4): the
/// weights of a large model are split across `-00001-of-0000N.gguf` parts, and
/// llama.cpp is given the first one and finds the rest itself, by name, in the
/// same directory. Two ways that goes wrong are worth catching before `spawn`,
/// because both look identical from outside — the server exits during loading and
/// the status bar shows the same "the process exited before it was ready":
///
/// - the path points at a part that is not the first (llama.cpp refuses it
///   outright: "model must be loaded with the first split");
/// - a part is missing — downloaded halfway, or one file copied out of the
///   directory. `is_file()` on the configured path passes, since *that* file is
///   there; the model still cannot load.
///
/// A name that isn't in the split shape means a single-file model and no check.
fn check_split_model(model: &str, loc: &'static Locale) -> Result<()> {
    let Some(shard) = crate::shared::gguf::parse_shard(model) else {
        return Ok(());
    };
    if shard.index != 1 {
        bail!(
            "{}",
            loc.tf(
                "ui.err.managed.shard_not_first",
                &[("path", &shard.first())]
            )
        );
    }
    // Part 1 is the configured path, already checked by the caller.
    for part in shard.all().iter().skip(1) {
        if !std::path::Path::new(part).is_file() {
            bail!(
                "{}",
                loc.tf("ui.err.managed.shard_missing", &[("path", part)])
            );
        }
    }
    Ok(())
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
            batch_size: None,
            parallel: 1,
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

    /// A CPU-only host (`-ngl 0`, no batch typed) gets the measured batch,
    /// `-ub` named beside it (docs/research/cpu-batch.md §4.1).
    #[test]
    fn a_cpu_only_line_gets_the_measured_batch() {
        let cfg = ManagedConfig {
            gpu_layers: 0,
            ..base_cfg()
        };
        let args = build_args(&cfg);
        let at = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };
        assert_eq!(at("-b").as_deref(), Some("256"));
        assert_eq!(at("-ub").as_deref(), Some("256"));
    }

    /// A GPU host at its defaults keeps its line byte for byte (R2): no
    /// `-b`, no `-ub`.
    #[test]
    fn a_gpu_line_is_byte_for_byte_what_it_was() {
        let args = build_args(&base_cfg());
        assert!(!args.iter().any(|a| a == "-b" || a == "-ub"), "{args:?}");
        let partial = ManagedConfig {
            gpu_layers: 20,
            ..base_cfg()
        };
        assert!(!build_args(&partial).iter().any(|a| a == "-b"));
    }

    /// A typed batch is passed as is, whatever `-ngl` says, and the
    /// micro-batch follows it clamped at the server's default.
    #[test]
    fn a_typed_batch_is_passed_as_is_with_the_micro_batch_clamped() {
        let at = |cfg: &ManagedConfig, flag: &str| {
            let args = build_args(cfg);
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };
        let big = ManagedConfig {
            batch_size: Some(1024),
            ..base_cfg()
        };
        assert_eq!(at(&big, "-b").as_deref(), Some("1024"));
        assert_eq!(at(&big, "-ub").as_deref(), Some("512"));
        let small = ManagedConfig {
            batch_size: Some(128),
            gpu_layers: 0,
            ..base_cfg()
        };
        assert_eq!(at(&small, "-b").as_deref(), Some("128"));
        assert_eq!(at(&small, "-ub").as_deref(), Some("128"));
        let cpu_default = ManagedConfig {
            batch_size: Some(2048),
            gpu_layers: 0,
            ..base_cfg()
        };
        assert_eq!(at(&cpu_default, "-b").as_deref(), Some("2048"));
        assert_eq!(at(&cpu_default, "-ub").as_deref(), Some("512"));
    }

    /// The embedding server keeps its own batch — the context size, for a
    /// non-causal model's sake — whatever the field or `-ngl` say (R4).
    #[test]
    fn the_embedding_server_keeps_its_own_batch() {
        let cfg = ManagedConfig {
            embeddings: true,
            gpu_layers: 0,
            batch_size: Some(128),
            context_size: 8192,
            ..base_cfg()
        };
        let args = build_args(&cfg);
        let values: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| (*a == "-b" || *a == "-ub") && *i + 1 < args.len())
            .map(|(i, _)| &args[i + 1])
            .collect();
        assert_eq!(values, vec!["8192", "8192"], "{args:?}");
    }

    /// One session leaves the launch line exactly as it was before the setting
    /// existed — no `-np` at all, so llama.cpp's own auto default (four slots,
    /// unified) still applies. Above one, both flags: an explicit `-np` alone
    /// would split the context between the slots.
    #[test]
    fn sessions_above_one_add_np_with_a_unified_pool() {
        let one = build_args(&base_cfg());
        assert!(
            !one.iter().any(|a| a == "-np" || a == "--kv-unified"),
            "{one:?}"
        );

        let cfg = ManagedConfig {
            parallel: 3,
            ..base_cfg()
        };
        let args = build_args(&cfg);
        let np = args.iter().position(|a| a == "-np").expect("-np present");
        assert_eq!(args[np + 1], "3");
        assert!(args.contains(&"--kv-unified".to_string()), "{args:?}");
        // The pool itself is unchanged: `-c` is still the configured size.
        let c = args.iter().position(|a| a == "-c").unwrap();
        assert_eq!(args[c + 1], "8192");
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

    /// The three split-model tests below share one opening — write parts, point
    /// the config at one of them, read back what `launch` refused with — so it is
    /// a fixture, not a test (lessons §2: the third test that starts like the
    /// first two is where sliding self-duplication comes from).
    ///
    /// The binary is a path inside the same directory that deliberately does not
    /// exist: past the preflight the launch must fail at `spawn` and say so, and
    /// a bare `llama-server` would make that depend on the developer's `PATH`.
    fn preflight_error(dir: &std::path::Path, parts: &[&str], model: &str) -> String {
        for part in parts {
            std::fs::write(dir.join(part), b"gguf").expect("write a part");
        }
        let cfg = ManagedConfig {
            binary: dir.join("no-such-llama-server"),
            model_path: Some(dir.join(model).display().to_string()),
            ..base_cfg()
        };
        match ServerHandle::launch(&cfg, ru()) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("launch cannot succeed: the binary does not exist"),
        }
    }

    /// The localized preflight error for `key`, naming `file` inside `dir`.
    fn expect_about(dir: &std::path::Path, key: &str, file: &str) -> String {
        ru().tf(key, &[("path", &dir.join(file).display().to_string())])
    }

    /// What a launch that got *past* every preflight fails with: `spawn` of the
    /// binary that isn't there. The negative control of the tests below.
    fn expect_spawn(dir: &std::path::Path) -> String {
        expect_about(dir, "ui.err.managed.spawn", "no-such-llama-server")
    }

    /// A multi-file GGUF needs the parts beside the one it is pointed at. Without
    /// this check a half-downloaded model reaches `spawn` and dies while loading,
    /// which reads as the generic early exit.
    #[test]
    fn launch_missing_split_part_errors_before_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (d, first) = (dir.path(), "model-00001-of-00003.gguf");

        let err = preflight_error(d, &[first], first);
        let missing = expect_about(
            d,
            "ui.err.managed.shard_missing",
            "model-00002-of-00003.gguf",
        );
        assert!(err.contains(&missing), "{err}");

        // The negative control: with every part present the preflight passes and the
        // launch gets as far as `spawn`. Without it the test would pass with the
        // whole split check deleted (lessons §2).
        let all = ["model-00002-of-00003.gguf", "model-00003-of-00003.gguf"];
        let err = preflight_error(d, &all, first);
        assert!(err.contains(&expect_spawn(d)), "{err}");
    }

    /// llama.cpp refuses any part but the first ("model must be loaded with the
    /// first split"), so the preflight says which file to point at.
    #[test]
    fn launch_from_a_later_split_part_points_at_the_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path();
        let err = preflight_error(
            d,
            &["model-00002-of-00003.gguf"],
            "model-00002-of-00003.gguf",
        );
        let first = expect_about(
            d,
            "ui.err.managed.shard_not_first",
            "model-00001-of-00003.gguf",
        );
        assert!(err.contains(&first), "{err}");
    }

    /// A single-file model keeps the preflight it always had: the split check must
    /// not invent parts for a name that has no tail.
    #[test]
    fn a_single_file_model_is_not_split_checked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = preflight_error(dir.path(), &["gemma-4-it.gguf"], "gemma-4-it.gguf");
        assert!(err.contains(&expect_spawn(dir.path())), "{err}");
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

    /// A **real** `llama-server` launched by the app's own line for a CPU-only
    /// host (`-ngl 0`, no batch typed) must accept `-b 256 -ub 256` and come
    /// up with its four unified slots over the whole `-c`
    /// (docs/research/cpu-batch.md §4.1, §7). The numbers that batch buys
    /// are §3.1's, measured on this same line launched by hand.
    ///
    ///     MINDFORK_LLAMA_BIN=.../llama-server.exe MINDFORK_MODEL=.../small.gguf \
    ///       cargo test managed_cpu_line -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires a local llama-server binary + model (MINDFORK_LLAMA_BIN, MINDFORK_MODEL)"]
    async fn managed_cpu_line_launches_with_the_measured_batch_live() {
        use crate::shared::api::EngineBackend;
        let (Ok(bin), Ok(model)) = (
            std::env::var("MINDFORK_LLAMA_BIN"),
            std::env::var("MINDFORK_MODEL"),
        ) else {
            eprintln!("skip: MINDFORK_LLAMA_BIN / MINDFORK_MODEL not set");
            return;
        };
        let cfg = ManagedConfig {
            binary: bin.into(),
            model_path: Some(model),
            gpu_layers: 0,
            context_size: 2048,
            port: 18124,
            ..base_cfg()
        };
        let args = build_args(&cfg);
        eprintln!("the line: {}", args.join(" "));
        let at = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };
        assert_eq!(at("-b").as_deref(), Some("256"));
        assert_eq!(at("-ub").as_deref(), Some("256"));
        let handle = ServerHandle::launch(&cfg, locale(Lang::En)).expect("launch");
        let client = OpenAiClient::new(handle.base_url());
        wait_until_ready(
            &client,
            Duration::from_secs(600),
            Some(handle.exited()),
            locale(Lang::En),
        )
        .await
        .expect("the server should come up on the CPU line");
        let slots = client.parallel_slots().await;
        let window = client.context_budget().await;
        eprintln!("live CPU launch: slots {slots:?}, window {window:?}");
        assert_eq!(
            slots,
            Some(4),
            "the server's own default: four unified slots"
        );
        assert_eq!(window, Some(2048));
    }

    /// A **real** `llama-server` launched with the pair of flags `sessions > 1`
    /// adds must report exactly that many slots over an **undivided** context:
    /// `-np N` alone would split `-c` between them
    /// (docs/research/parallel-subagents.md §2.4, fork F4), and a launcher that
    /// dropped `--kv-unified` would quarter the main chat's window with no unit
    /// test noticing — the flag pair's meaning is the server's, not ours.
    ///
    ///     MINDFORK_LLAMA_BIN=.../llama-server.exe MINDFORK_MODEL=.../small.gguf \
    ///       cargo test managed_sessions_launch -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires a local llama-server binary + model (MINDFORK_LLAMA_BIN, MINDFORK_MODEL)"]
    async fn managed_sessions_launch_three_slots_over_one_pool_live() {
        use crate::shared::api::EngineBackend;
        let (Ok(bin), Ok(model)) = (
            std::env::var("MINDFORK_LLAMA_BIN"),
            std::env::var("MINDFORK_MODEL"),
        ) else {
            eprintln!("skip: MINDFORK_LLAMA_BIN / MINDFORK_MODEL not set");
            return;
        };
        let cfg = ManagedConfig {
            binary: bin.into(),
            model_path: Some(model),
            parallel: 3,
            context_size: 4096,
            port: 18123,
            ..base_cfg()
        };
        let handle = ServerHandle::launch(&cfg, locale(Lang::En)).expect("launch");
        let client = OpenAiClient::new(handle.base_url());
        wait_until_ready(
            &client,
            Duration::from_secs(600),
            Some(handle.exited()),
            locale(Lang::En),
        )
        .await
        .expect("the server should come up");
        let slots = client.parallel_slots().await;
        let window = client.context_budget().await;
        eprintln!("live managed launch: slots {slots:?}, per-slot window {window:?}");
        assert_eq!(slots, Some(3));
        assert_eq!(
            window,
            Some(4096),
            "unified pool: every slot may use the whole -c"
        );
    }
    /// The batch the launch line implies, from the same two facts `build_args`
    /// reads (docs/research/slow-prefill-detection.md §3.2).
    #[test]
    fn the_launched_batch_follows_the_launch_line() {
        assert_eq!(launched_batch(Some(1024), 99), 1024, "typed wins");
        assert_eq!(launched_batch(Some(1024), 0), 1024);
        assert_eq!(launched_batch(None, 0), CPU_BATCH, "the CPU auto");
        assert_eq!(
            launched_batch(None, 99),
            LLAMA_DEFAULT_BATCH,
            "a GPU host at its defaults runs the server's"
        );
    }

    /// The slow-prefill rule: the hold `batch / tps` in seconds when it is worth
    /// saying — a sample of a batch's worth, a hold above the limit, a batch
    /// above the knee (docs/research/slow-prefill-detection.md §3.2).
    #[test]
    fn the_prefill_hold_is_said_only_when_worth_it() {
        use crate::shared::api::contract::Prefill;
        // The CPU build at the default batch: ~90 tok/s → 23 s.
        let slow = Prefill {
            tokens: 1800,
            ms: 20_000,
        };
        assert_eq!(prefill_hold(LLAMA_DEFAULT_BATCH, slow), Some(23));
        // The same host at the knee: the change the note would advise is made.
        assert_eq!(prefill_hold(CPU_BATCH, slow), None);
        // A GPU host: thousands of tokens a second, a second's hold.
        let fast = Prefill {
            tokens: 1800,
            ms: 900,
        };
        assert_eq!(prefill_hold(LLAMA_DEFAULT_BATCH, fast), None);
        // Too short a sample to trust — the per-request overhead would pass
        // for throughput.
        let short = Prefill {
            tokens: 100,
            ms: 5_000,
        };
        assert_eq!(prefill_hold(LLAMA_DEFAULT_BATCH, short), None);
        // A typed 1024 on a slowish host: 150 tok/s → 7 s, still worth it.
        let mid = Prefill {
            tokens: 1500,
            ms: 10_000,
        };
        assert_eq!(prefill_hold(1024, mid), Some(7));
        // Nothing processed, or no clock: nothing to say.
        assert_eq!(
            prefill_hold(LLAMA_DEFAULT_BATCH, Prefill { tokens: 0, ms: 0 }),
            None
        );
    }
}
