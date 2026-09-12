//! Python sidecar sandbox on **Wasmer/WASIX**. A separate `wasmer` process
//! (bundled next to the application, in `data/sandbox/`) that runs the code in
//! WASM isolation: the guest has no access to the host FS (it only sees what's
//! mounted), network — via the explicit `--net` flag. Interruption — killing the
//! process (clean and fast, verified in Phase 0). See
//! [docs/research/python-wasmer-sandbox.md](../../docs/research/python-wasmer-sandbox.md)
//! (decision §9.7: a `wasmer` sidecar behind this contract, rather than embedding V8 in a dll).
//!
//! The `shared` layer (FSD): the [`SandboxRunner`] contract behind a trait (mock in
//! tests — the `EngineBackend` pattern); the real implementation [`WasmerSandbox`]
//! builds the command and launches the binary. The `python_exec` tool
//! (`features/tools/python.rs`) uses this contract.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::shared::i18n::Locale;

/// The `wasmer` binary's name in the sandbox directory (per platform).
const WASMER_BIN: &str = if cfg!(windows) {
    "wasmer.exe"
} else {
    "wasmer"
};
/// The default CPython package if there's no local `python.webc` (`wasmer`
/// downloads it from the registry on first launch; Phase 2 puts it into `data/sandbox/`).
const DEFAULT_PYTHON_PKG: &str = "python/python";
/// The guest mount point of the working directory (holding the task script).
const GUEST_WORK: &str = "/w";

/// The script both runners write into the job directory and start.
const JOB_SCRIPT: &str = "job.py";
/// Where `site-packages` sits in the guest: a volume of the packed image, or — for
/// provisioning's warmup only — the mounted directory.
pub(crate) const GUEST_SITE: &str = "/sp";
/// The sandbox image `mindfork sandbox setup` packs into the sandbox directory: CPython
/// and `site-packages` in one self-contained package (ADR 0005 §5, amended). A hyphen
/// in the name, so the i18n key scanner cannot read it as a `sandbox.` key.
pub(crate) const SANDBOX_IMAGE: &str = "packed-sandbox.webc";
/// Environment variable: the path/name of the `wasmer` binary (a lookup override).
const ENV_WASMER: &str = "MINDFORK_SANDBOX_WASMER";
/// Environment variable: the path to `python.webc` or a package reference (an override).
const ENV_PYTHON: &str = "MINDFORK_SANDBOX_PYTHON";

/// A shim mixed in ahead of the user's code: mutes socket options unsupported
/// under WASIX. Without it `http.client`/`urllib`/`requests` fail — WASIX doesn't
/// implement `setsockopt(TCP_NODELAY)` and throws `EINVAL`, and HTTP clients always
/// set it (a Phase 0 finding, §9.3). Wrapped in a function so as not to clutter
/// the user code's global namespace with names.
const SETSOCKOPT_SHIM: &str = "\
def _mf_patch_socket():
    import socket
    _orig = socket.socket.setsockopt
    def _safe(self, *a, **k):
        try:
            return _orig(self, *a, **k)
        except OSError:
            return None
    socket.socket.setsockopt = _safe
_mf_patch_socket()
";

/// A shim mixed in ahead of the user's code so matplotlib works under WASIX. Two
/// failures, both measured before a single line of a plot could run
/// (docs/journal/tools.md, "the starter set grows"):
/// - the guest has no `HOME`, so `import matplotlib` raises "Could not determine home
///   directory" — `MPLCONFIGDIR` gives it a config and cache directory, in `/tmp`,
///   which dies with the call (the font list is rebuilt per call, within the second
///   `import matplotlib.pyplot` takes);
/// - FreeType's autohinter traps in the wasix build ("null function or function
///   signature mismatch"), and matplotlib's own default `text.hinting` forces it for
///   every raster text; `default` hinting renders the chart instead.
///
/// Only an environment variable and a file in `/tmp` — matplotlib itself is not
/// imported here, so code that never plots pays nothing for it.
const MATPLOTLIB_SHIM: &str = "\
def _mf_prepare_matplotlib():
    import os
    d = os.environ.setdefault('MPLCONFIGDIR', '/tmp/matplotlib')
    try:
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, 'matplotlibrc'), 'w') as f:
            f.write('backend: Agg\\ntext.hinting: default\\n')
    except OSError:
        pass
_mf_prepare_matplotlib()
";

/// The raw result of running code in the sandbox (formatting is the tool's
/// job, so it matches the local mode).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SandboxOutput {
    pub stdout: String,
    pub stderr: String,
    /// The process's exit code (`None` — didn't exit normally / was killed).
    pub exit_code: Option<i32>,
    /// Execution was interrupted by a timeout (the process was killed).
    pub timed_out: bool,
    /// The regular files the code left directly in `/w/out`, within
    /// [`OutputLimits::DEFAULT`], in name order — collected after the process exited,
    /// whatever its exit code; none after a timeout (docs/history/sandbox-file-exchange.md F4,
    /// §11 S1–S2).
    pub files: Vec<OutputFile>,
    /// What `/w/out` held that was not collected, each with its reason.
    pub skipped: Vec<SkippedOutput>,
}

/// A file collected from `/w/out`: the name the guest gave it — not sanitized, storing it
/// is the caller's business — and its bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// An entry of `/w/out` that was not collected.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedOutput {
    pub name: String,
    pub reason: SkipReason,
}

/// Why an entry of `/w/out` was not collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// A directory: only files directly in `/w/out` are collected.
    Directory,
    /// A link or a special file — never followed.
    NotAFile,
    /// Larger than [`OutputLimits::max_file_bytes`].
    TooLarge,
    /// Past [`OutputLimits::max_files`].
    TooMany,
    /// Would take the call past [`OutputLimits::max_total_bytes`].
    OverTotal,
    /// Could not be read.
    Unreadable,
    /// Left by a call that timed out — possibly half-written, so not read.
    TimedOut,
}

/// How much one call may leave in `/w/out` (F4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLimits {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl OutputLimits {
    /// 10 files, 25 MB each, 50 MB per call.
    pub const DEFAULT: Self = Self {
        max_files: 10,
        max_file_bytes: 25 * 1024 * 1024,
        max_total_bytes: 50 * 1024 * 1024,
    };
}

/// One file staged into the guest's `/w/in` (docs/history/sandbox-file-exchange.md §12 T12): the
/// name it gets there and where its bytes come from. `shared` knows nothing about chats —
/// which file this is, and what the guest calls it, are the tool's decisions.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxInput {
    /// The name in `/w/in`: one plain path component, sanitized by the caller and
    /// re-checked here ([`is_one_component`]).
    pub name: String,
    pub source: InputSource,
}

/// Where a staged file's bytes come from.
#[derive(Debug, Clone, PartialEq)]
pub enum InputSource {
    /// Bytes the caller holds — an attachment's text, a decoded image.
    Bytes(Vec<u8>),
    /// A file on the host: copied, never read into memory and never linked.
    Path(PathBuf),
}

impl SandboxInput {
    /// A staged file whose bytes the caller holds.
    pub fn bytes(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            source: InputSource::Bytes(bytes),
        }
    }

    /// A staged copy of a file on the host.
    pub fn path(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            source: InputSource::Path(path.into()),
        }
    }
}

/// What one call runs: the code, the files staged into `/w/in`, and the two limits a
/// launch takes. One argument instead of four, changed once (§12 T1). The **collection**
/// limits are not here: nothing would ever set them per call, and the tool's description
/// is built from the same [`OutputLimits::DEFAULT`], so a field would be a second source
/// of truth for a value that has one.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxJob<'a> {
    pub code: &'a str,
    pub inputs: &'a [SandboxInput],
    pub net: bool,
    pub timeout: Duration,
}

impl<'a> SandboxJob<'a> {
    /// A job that stages nothing — provisioning's own runs, and every call naming no file.
    pub fn new(code: &'a str, net: bool, timeout: Duration) -> Self {
        Self {
            code,
            inputs: &[],
            net,
            timeout,
        }
    }

    /// The same job with files copied into `/w/in`.
    pub fn with_inputs(mut self, inputs: &'a [SandboxInput]) -> Self {
        self.inputs = inputs;
        self
    }
}

/// Whether a staged file's name is one plain component. The tool sanitizes every name
/// before it gets here; this is the guard `shared` can make without knowing what named it,
/// so a bug upstream cannot write outside the job directory.
fn is_one_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}

/// The sandbox's readiness to launch (cheap, without starting a process).
#[derive(Debug, Clone, PartialEq)]
pub enum SandboxAvailability {
    /// The `wasmer` binary was found — ready to launch.
    Ready,
    /// Not installed/not found — a human-readable reason (goes to the model).
    Missing(String),
}

/// Running code in an isolated sandbox. Behind a trait — for a mock in tests
/// (`features/tools/python.rs`) and swappable implementations.
#[async_trait::async_trait]
pub trait SandboxRunner: Send + Sync {
    /// A readiness check (whether the `wasmer` binary is present). Without
    /// starting a process. `loc` — the language of the unavailability reason
    /// (shown by the caller: `python_exec` — the profile's language, axis A;
    /// provisioning's warmup — the interface language).
    fn availability(&self, loc: &Locale) -> SandboxAvailability;

    /// Runs a [`SandboxJob`]: its Python code, with the files it stages copied into the
    /// guest's `/w/in`, its network access and its timeout. On timeout the process is
    /// killed and `timed_out = true` is returned. An error occurs only at the
    /// process-launch level — a nonzero guest exit code is not one — or when a staged
    /// file cannot be written. `loc` — the language of the error text (embedded by the
    /// caller: `python_exec` — the profile's language, warmup — the interface language).
    async fn run(&self, job: SandboxJob<'_>, loc: &Locale) -> Result<SandboxOutput>;
}

/// Where a launch takes the guest's `site-packages` from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SiteSource {
    /// The packed image ([`SANDBOX_IMAGE`]), where `site-packages` is a volume: what the
    /// guest writes there lands in memory and dies with the call. The runtime's source.
    Image,
    /// The `site-packages` directory mounted from the host — writable, which is the
    /// point for its one user, provisioning's warmup, filling `__pycache__` before the
    /// image is packed. It never runs model code.
    Directory,
}

/// What one launch runs and mounts ([`WasmerSandbox::plan`]).
#[derive(Debug, PartialEq)]
struct LaunchPlan {
    /// The package `wasmer run` starts: the packed image, or CPython itself.
    program: OsString,
    /// A host `site-packages` directory to mount at [`GUEST_SITE`] (the warmup's only).
    site_mount: Option<PathBuf>,
    /// Whether [`GUEST_SITE`] exists in the guest and belongs on `PYTHONPATH`.
    site_on_path: bool,
}

/// The real sandbox: drives the bundled `wasmer` as a child process.
pub struct WasmerSandbox {
    /// The sandbox directory (`data/sandbox/`): `wasmer[.exe]`, `python.webc`,
    /// `site-packages/`, the packed image. `None` — only through an env-override
    /// (tests/default).
    dir: Option<PathBuf>,
    /// Where `site-packages` comes from — the image, except for provisioning.
    site: SiteSource,
    /// Which image file in [`Self::dir`] a launch runs. The installed one
    /// ([`SANDBOX_IMAGE`]) for every caller but provisioning's verification, which has to
    /// start a freshly packed candidate **before** it replaces the sandbox that works.
    image: String,
    /// The "one task at a time" gate (a single permit). Protection against
    /// process/thread leaks and predictable load: a concurrent call is
    /// rejected immediately. In the normal agentic loop, calls are already
    /// sequential anyway — this is defense in depth.
    gate: Arc<Semaphore>,
    /// A hard process memory limit (MB; `None` — no limit). Applied only on
    /// Windows (a Job Object). See [`WasmerSandbox::with_memory_limit`].
    memory_mb: Option<u64>,
}

impl WasmerSandbox {
    /// Creates a sandbox with the assets directory (`data/sandbox/`; `None` —
    /// without it, then the binary is taken only from an env-override).
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self::with_site(dir, SiteSource::Image, SANDBOX_IMAGE)
    }

    /// Runs one named image out of the sandbox directory rather than the installed one.
    /// Provisioning's verification, and nothing else: a candidate that does not start must
    /// fail `setup` while the previous image is still the one on disk.
    pub fn for_candidate(dir: PathBuf, image: &str) -> Self {
        Self::with_site(Some(dir), SiteSource::Image, image)
    }

    /// The sandbox provisioning's warmup runs in: `site-packages` mounted as the
    /// writable directory it is on the host, so the bytecode the warmup compiles lands
    /// where the image is packed from. Only for code the application itself wrote.
    pub fn for_provisioning(dir: PathBuf) -> Self {
        Self::with_site(Some(dir), SiteSource::Directory, SANDBOX_IMAGE)
    }

    fn with_site(dir: Option<PathBuf>, site: SiteSource, image: &str) -> Self {
        Self {
            dir,
            site,
            image: image.to_string(),
            gate: Arc::new(Semaphore::new(1)),
            memory_mb: None,
        }
    }

    /// Sets a hard memory limit (MB; `Some(0)`/`None` — no limit). Windows
    /// only: the `wasmer` process is placed into a Job Object with
    /// `JOB_OBJECT_LIMIT_PROCESS_MEMORY`; exceeding it kills the process
    /// (protects the host from OOM). On Unix the field is ignored (`rlimit`
    /// is unreliable with V8 — it reserves a large virtual address space).
    /// See ADR 0005.
    pub fn with_memory_limit(mut self, mb: Option<u64>) -> Self {
        self.memory_mb = mb.filter(|&m| m > 0);
        self
    }

    /// The `wasmer` binary's path/name: an env-override → the sandbox directory ([`locate_wasmer`]).
    fn resolve_wasmer(&self) -> Option<OsString> {
        if let Some(o) = env_override(ENV_WASMER) {
            return Some(o);
        }
        self.dir
            .as_deref()
            .and_then(locate_wasmer)
            .map(PathBuf::into_os_string)
    }

    /// The CPython source: an env-override → `<dir>/python.webc` → a registry package.
    fn resolve_python(&self) -> OsString {
        if let Some(o) = env_override(ENV_PYTHON) {
            return o;
        }
        if let Some(dir) = &self.dir {
            let webc = dir.join("python.webc");
            if webc.is_file() {
                return webc.into_os_string();
            }
        }
        OsString::from(DEFAULT_PYTHON_PKG)
    }

    /// What a launch runs. The packed image wins wherever it exists — it carries CPython
    /// and `site-packages` both, so an [`ENV_PYTHON`] override has nothing to add to it.
    /// A `site-packages` directory with no image is an install from before the image:
    /// `None`, refused rather than mounted writable, and the caller says to run
    /// `mindfork sandbox setup` again. With neither, plain CPython and no packages.
    fn plan(&self) -> Option<LaunchPlan> {
        let site_dir = self
            .dir
            .as_ref()
            .map(|d| d.join("site-packages"))
            .filter(|p| p.is_dir());
        if self.site == SiteSource::Directory {
            return Some(LaunchPlan {
                program: self.resolve_python(),
                site_on_path: site_dir.is_some(),
                site_mount: site_dir,
            });
        }
        let image = self
            .dir
            .as_ref()
            .map(|d| d.join(&self.image))
            .filter(|p| p.is_file());
        match (image, site_dir) {
            (Some(image), _) => Some(LaunchPlan {
                program: image.into_os_string(),
                site_mount: None,
                site_on_path: true,
            }),
            (None, Some(_)) => None,
            (None, None) => Some(LaunchPlan {
                program: self.resolve_python(),
                site_mount: None,
                site_on_path: false,
            }),
        }
    }
}

#[async_trait::async_trait]
impl SandboxRunner for WasmerSandbox {
    fn availability(&self, loc: &Locale) -> SandboxAvailability {
        match self.resolve_wasmer() {
            None => SandboxAvailability::Missing(loc.t("sandbox.err.not_installed").to_string()),
            Some(_) if self.plan().is_none() => {
                SandboxAvailability::Missing(loc.t("sandbox.err.needs_repack").to_string())
            }
            Some(_) => SandboxAvailability::Ready,
        }
    }

    async fn run(&self, spec: SandboxJob<'_>, loc: &Locale) -> Result<SandboxOutput> {
        // The "one task" gate: a concurrent launch is rejected right away (before spawning).
        let _permit = self
            .gate
            .try_acquire()
            .map_err(|_| anyhow::anyhow!("{}", loc.t("sandbox.err.busy")))?;
        let wasmer = self
            .resolve_wasmer()
            .ok_or_else(|| anyhow::anyhow!("{}", loc.t("sandbox.err.not_found")))?;
        let plan = self
            .plan()
            .ok_or_else(|| anyhow::anyhow!("{}", loc.t("sandbox.err.needs_repack")))?;

        // The script, `in/` and `out/` in a unique temp directory (auto-cleanup via Drop),
        // laid out exactly as Local's — one helper, one layout (§14 V1). The WASIX shims
        // are the guest's own and are added here, not there.
        let job = JobDir::create().with_context(|| loc.t("sandbox.err.job_dir").to_string())?;
        let layout = prepare_job(&job, &build_wrapper(spec.code), spec.inputs, loc).await?;
        let out_dir = layout.out_dir;

        // Mount the working directory — and a `site-packages` directory only for
        // provisioning's warmup, since the image carries its own; PYTHONPATH for the guest.
        let mut mounts: Vec<(PathBuf, &str)> = vec![(job.path.clone(), GUEST_WORK)];
        let mut envs: Vec<(&str, String)> = vec![
            ("PYTHONIOENCODING", "utf-8".into()),
            ("PYTHONUTF8", "1".into()),
        ];
        if let Some(sp) = plan.site_mount {
            mounts.push((sp, GUEST_SITE));
        }
        if plan.site_on_path {
            envs.push(("PYTHONPATH", GUEST_SITE.into()));
        }
        let script_guest = format!("{GUEST_WORK}/{JOB_SCRIPT}");
        let args = build_args(&plan.program, &mounts, &envs, spec.net, &script_guest);

        let mut cmd = tokio::process::Command::new(&wasmer);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Python runs INSIDE the wasmer process (in-process V8, not a
            // child process), so killing wasmer itself also stops the code.
            // On a timeout/cancellation the future is dropped → the process is killed.
            .kill_on_drop(true);
        // The cache of compiled modules lives under the sandbox directory
        // (self-contained, not in ~/.wasmer): the first launch compiles
        // python.wasm (seconds), after that a warm start from the cache.
        // See docs/research/python-wasmer-sandbox.md §2.3.
        if let Some(dir) = &self.dir {
            cmd.env("WASMER_CACHE_DIR", dir.join("cache"));
        }

        let child = cmd
            .spawn()
            .with_context(|| loc.tf("sandbox.err.spawn", &[("path", &wasmer.to_string_lossy())]))?;

        // A hard memory limit (Windows Job Object) — right after spawning,
        // before V8 commits significant memory. "Best effort": a failure is only logged.
        if let Some(mb) = self.memory_mb {
            apply_memory_limit(&child, mb);
        }

        match tokio::time::timeout(spec.timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => {
                // Whatever the exit code: a script that saved its chart and then failed
                // still made the chart (F4). The guest has exited, so nothing races the
                // walk; up to a call's worth of bytes is read on the blocking pool, while
                // `job` keeps the directory alive.
                let collect_from = out_dir.clone();
                let (files, skipped) = tokio::task::spawn_blocking(move || {
                    collect_outputs(&collect_from, OutputLimits::DEFAULT)
                })
                .await
                .unwrap_or_default();
                Ok(SandboxOutput {
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    exit_code: out.status.code(),
                    timed_out: false,
                    files,
                    skipped,
                })
            }
            Ok(Err(e)) => Err(e).with_context(|| loc.t("sandbox.err.wait").to_string()),
            Err(_) => Ok(SandboxOutput {
                timed_out: true,
                // A killed call may have left a file half-written: nothing is read, and
                // what `out/` held is named, so the model is not left guessing (F4).
                skipped: timed_out_outputs(&out_dir),
                ..SandboxOutput::default()
            }),
        }
    }
}

/// The **Local** mode behind the same contract (docs/history/sandbox-file-exchange.md F11 (b),
/// §14 V1): the user's own interpreter, started in a job directory laid out exactly like
/// the guest's — `job.py` beside `in/` and `out/`, the working directory being the job
/// directory, so the relative `in/`/`out/` a call writes mean the same thing in both modes.
///
/// What it is **not** is isolation. The code runs on the machine with the user's
/// permissions and reaches whatever they reach — the network included, which is why this
/// mode has no network flag to honour (§14 V5). The job directory is a place to exchange
/// files, not a boundary; ADR 0005 §5 says the same of the mode as a whole.
pub struct LocalSandbox {
    /// The interpreter: a path or a bare name. `None` → the platform default.
    python: Option<String>,
}

impl LocalSandbox {
    pub fn new(python: Option<String>) -> Self {
        Self { python }
    }

    /// The interpreter's name or path, with the platform's default when none is set.
    pub fn interpreter(&self) -> String {
        self.python.clone().unwrap_or_else(|| {
            if cfg!(windows) {
                "python".to_string()
            } else {
                "python3".to_string()
            }
        })
    }
}

#[async_trait::async_trait]
impl SandboxRunner for LocalSandbox {
    /// A path check, not a probe (§14 V4): an interpreter named with a separator is
    /// checked as a file, so a wrong setting is reported where a missing `wasmer` is; a
    /// bare name is left to `PATH`, where a failure surfaces as the spawn error it was.
    fn availability(&self, loc: &Locale) -> SandboxAvailability {
        let python = self.interpreter();
        if python.contains(['/', '\\']) && !Path::new(&python).is_file() {
            return SandboxAvailability::Missing(
                loc.tf("sandbox.err.python_not_found", &[("path", &python)]),
            );
        }
        SandboxAvailability::Ready
    }

    async fn run(&self, spec: SandboxJob<'_>, loc: &Locale) -> Result<SandboxOutput> {
        let python = self.interpreter();
        let job = JobDir::create().with_context(|| loc.t("sandbox.err.job_dir").to_string())?;
        // The code as written: the shims `build_wrapper` adds are the guest's libc and its
        // FreeType build, and nothing on the host wants them (§14 V3).
        let layout = prepare_job(&job, spec.code, spec.inputs, loc).await?;

        let mut cmd = tokio::process::Command::new(&python);
        cmd.arg(&layout.script)
            // The working directory is the job directory, which is what makes `in/` and
            // `out/` mean here what `/w/in` and `/w/out` mean in the guest.
            .current_dir(&job.path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Output goes into a pipe, not a console, so Python on Windows picks an
            // encoding by locale (often cp1252) and fails on Cyrillic in `print`
            // (`UnicodeEncodeError`). We read UTF-8, so we ask for UTF-8.
            // See docs/journal/milestones.md (M7).
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            // On a timeout the future is dropped → the process is killed.
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .with_context(|| loc.tf("sandbox.err.spawn_python", &[("path", &python)]))?;

        match tokio::time::timeout(spec.timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => {
                // Whatever the exit code: a script that saved its chart and then failed
                // still made the chart (F4). The same rule, the same collector and the
                // same caps as the guest's.
                let collect_from = layout.out_dir.clone();
                let (files, skipped) = tokio::task::spawn_blocking(move || {
                    collect_outputs(&collect_from, OutputLimits::DEFAULT)
                })
                .await
                .unwrap_or_default();
                Ok(SandboxOutput {
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    exit_code: out.status.code(),
                    timed_out: false,
                    files,
                    skipped,
                })
            }
            Ok(Err(e)) => Err(e).with_context(|| loc.t("sandbox.err.wait_python").to_string()),
            Err(_) => Ok(SandboxOutput {
                timed_out: true,
                // A killed call may have left a file half-written: nothing is read, and
                // what `out/` held is named, so the model is not left guessing (F4).
                skipped: timed_out_outputs(&layout.out_dir),
                ..SandboxOutput::default()
            }),
        }
    }
}

/// Looks for the `wasmer` binary in the sandbox directory (pure, testable). Order:
/// a direct placement `<dir>/wasmer[.exe]` (a manual install) → the setup's unpack
/// `<dir>/wasmer-dist/bin/wasmer[.exe]`. Used both by the runtime ([`WasmerSandbox`])
/// and by provisioning (`features::sandbox_setup`) — a single source of truth about the layout.
pub fn locate_wasmer(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join(WASMER_BIN);
    if direct.is_file() {
        return Some(direct);
    }
    let dist = dir.join("wasmer-dist").join("bin").join(WASMER_BIN);
    dist.is_file().then_some(dist)
}

/// A non-empty env variable value as an `OsString` (a path/name override).
fn env_override(key: &str) -> Option<OsString> {
    std::env::var_os(key).filter(|v| !v.is_empty())
}

/// Applies a hard memory limit to the `wasmer` process (a Windows Job Object).
/// Exceeding it kills the process — protects the host from OOM. "Best
/// effort": a winapi failure is only logged. Verified live (research §9.6):
/// the limit holds even after the job handle is closed (the job lives as
/// long as the process is a member), so the HANDLE isn't held across
/// `await` (important for the future to stay `Send`).
#[cfg(windows)]
fn apply_memory_limit(child: &tokio::process::Child, mb: u64) {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    let Some(raw) = child.raw_handle() else {
        tracing::warn!("sandbox: no process handle — memory limit not applied");
        return;
    };
    // SAFETY: `raw` is a valid handle of the just-spawned process; the job is
    // created and closed within this block, the struct's fields are zero-initialized.
    unsafe {
        let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            tracing::warn!("sandbox: CreateJobObjectW failed");
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.ProcessMemoryLimit = (mb as usize).saturating_mul(1024 * 1024);
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            tracing::warn!("sandbox: SetInformationJobObject failed");
            CloseHandle(job);
            return;
        }
        if AssignProcessToJobObject(job, raw as HANDLE) == 0 {
            tracing::warn!("sandbox: AssignProcessToJobObject failed");
        }
        // The handle can be closed right away: the limit holds as long as
        // the process is a member of the job.
        CloseHandle(job);
    }
}

/// A hard memory limit isn't applied on non-Windows: `rlimit`/`RLIMIT_AS` is
/// unreliable with the V8 backend (it reserves a large virtual address
/// space, so a low limit breaks the very startup). We rely on the timeout
/// and wasm32 (~4 GB). See ADR 0005.
#[cfg(not(windows))]
fn apply_memory_limit(_child: &tokio::process::Child, _mb: u64) {
    tracing::debug!("sandbox: memory limit is supported only on Windows — skipping");
}

/// Wraps the user's code with the WASIX compatibility shims — `setsockopt` and
/// matplotlib (pure, testable).
pub fn build_wrapper(code: &str) -> String {
    format!("{SETSOCKOPT_SHIM}{MATPLOTLIB_SHIM}\n{code}")
}

/// Builds the `wasmer` command-line arguments (pure, testable). Shape:
/// `run --v8 [--net] (--volume HOST:GUEST)* (--env K=V)* <python> -- <script>`.
/// The order and the host:guest mounting were verified live in Phase 0 (incl.
/// with a Windows path, where the drive colon `C:` doesn't break `--volume` parsing).
fn build_args(
    python: &OsStr,
    mounts: &[(PathBuf, &str)],
    envs: &[(&str, String)],
    net: bool,
    script_guest: &str,
) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["run".into(), "--v8".into()];
    if net {
        a.push("--net".into());
    }
    for (host, guest) in mounts {
        a.push("--volume".into());
        let mut v = host.clone().into_os_string();
        v.push(":");
        v.push(guest);
        a.push(v);
    }
    for (k, val) in envs {
        a.push("--env".into());
        a.push(format!("{k}={val}").into());
    }
    a.push(python.to_os_string());
    a.push("--".into());
    a.push(script_guest.into());
    a
}

/// The job directory a call runs in, laid out the same way for both runners
/// (docs/history/sandbox-file-exchange.md §14 V1): `job.py` beside `in/` and `out/`.
///
/// `in/` and `out/` are created whatever the call stages, so code that looks into either
/// finds a folder rather than an error, and the staged files are the call's own copies:
/// `wasmer` 7.2.0 has no read-only volume, and on the host there is nothing to make one —
/// the guest may overwrite a copy and nothing follows, since the chat's files are
/// elsewhere and only `out/` is collected.
async fn prepare_job(
    job: &JobDir,
    code: &str,
    inputs: &[SandboxInput],
    loc: &Locale,
) -> Result<JobLayout> {
    let script = job.path.join(JOB_SCRIPT);
    tokio::fs::write(&script, code)
        .await
        .with_context(|| loc.t("sandbox.err.write_script").to_string())?;
    let out_dir = job.path.join("out");
    let in_dir = job.path.join("in");
    for dir in [&out_dir, &in_dir] {
        tokio::fs::create_dir(dir)
            .await
            .with_context(|| loc.t("sandbox.err.job_dir").to_string())?;
    }
    for input in inputs {
        if !is_one_component(&input.name) {
            anyhow::bail!(
                "{}",
                loc.tf("sandbox.err.input_name", &[("name", &input.name)])
            );
        }
        let to = in_dir.join(&input.name);
        match &input.source {
            InputSource::Bytes(bytes) => tokio::fs::write(&to, bytes).await.map(|()| 0),
            InputSource::Path(from) => tokio::fs::copy(from, &to).await,
        }
        .with_context(|| loc.tf("sandbox.err.stage_input", &[("name", &input.name)]))?;
    }
    Ok(JobLayout { script, out_dir })
}

/// What [`prepare_job`] laid out and the run then needs: the script to start, and the
/// directory collected once the process has exited.
#[derive(Debug)]
struct JobLayout {
    script: PathBuf,
    out_dir: PathBuf,
}

/// A temp directory for the task script (auto-cleanup on `Drop`). Lives in the
/// system tmp; unique by UUID — no `tempfile` dependency at runtime.
struct JobDir {
    path: PathBuf,
}

impl JobDir {
    fn create() -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("mindfork-sbx-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for JobDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Collects the regular files directly in `out` within `limits` (F4,
/// docs/history/sandbox-file-exchange.md §11 S2), after the guest has exited. Entries are taken in
/// name order, so which ones a cap keeps does not depend on the file system. An entry that
/// is not a regular file by `symlink_metadata` is skipped, never followed. A file is read
/// at most one byte past its cap, so a size the metadata understated cannot slip through;
/// one that would take the call past the total is skipped, and later ones are still tried.
fn collect_outputs(out: &Path, limits: OutputLimits) -> (Vec<OutputFile>, Vec<SkippedOutput>) {
    let mut files: Vec<OutputFile> = Vec::new();
    let mut skipped = Vec::new();
    let mut total = 0u64;
    for (name, path) in sorted_entries(out) {
        let reason = match std::fs::symlink_metadata(&path) {
            Err(_) => SkipReason::Unreadable,
            Ok(meta) if meta.is_dir() => SkipReason::Directory,
            Ok(meta) if !meta.is_file() => SkipReason::NotAFile,
            Ok(_) if files.len() >= limits.max_files => SkipReason::TooMany,
            Ok(meta) if meta.len() > limits.max_file_bytes => SkipReason::TooLarge,
            Ok(_) => match read_capped(&path, limits.max_file_bytes) {
                Err(_) => SkipReason::Unreadable,
                Ok(None) => SkipReason::TooLarge,
                Ok(Some(bytes)) if total + bytes.len() as u64 > limits.max_total_bytes => {
                    SkipReason::OverTotal
                }
                Ok(Some(bytes)) => {
                    total += bytes.len() as u64;
                    files.push(OutputFile { name, bytes });
                    continue;
                }
            },
        };
        skipped.push(SkippedOutput { name, reason });
    }
    (files, skipped)
}

/// The entries of `dir` as (name, path) in name order — the name lossy when it is not
/// UTF-8. None when the directory cannot be read.
fn sorted_entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    entries.sort();
    entries
}

/// Reads at most `cap` bytes of `path`; `None` when the file holds more.
fn read_capped(path: &Path, cap: u64) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= cap).then_some(bytes))
}

/// What a timed-out call left in `out`: named, never read.
fn timed_out_outputs(out: &Path) -> Vec<SkippedOutput> {
    sorted_entries(out)
        .into_iter()
        .map(|(name, _)| SkippedOutput {
            name,
            reason: SkipReason::TimedOut,
        })
        .collect()
}

#[cfg(test)]
mod collect_tests {
    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    fn names(files: &[OutputFile]) -> Vec<&str> {
        files.iter().map(|f| f.name.as_str()).collect()
    }

    #[test]
    fn collects_regular_files_in_name_order() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "b.txt", b"bb");
        write(dir.path(), "a.png", b"aa");
        let (files, skipped) = collect_outputs(dir.path(), OutputLimits::DEFAULT);
        assert_eq!(names(&files), ["a.png", "b.txt"]);
        assert_eq!(files[0].bytes, b"aa");
        assert!(skipped.is_empty());
    }

    #[test]
    fn a_directory_is_skipped_and_named_as_one() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("charts");
        std::fs::create_dir(&sub).unwrap();
        write(&sub, "inner.png", b"x");
        let (files, skipped) = collect_outputs(dir.path(), OutputLimits::DEFAULT);
        assert!(files.is_empty());
        assert_eq!(
            skipped,
            [SkippedOutput {
                name: "charts".into(),
                reason: SkipReason::Directory
            }]
        );
    }

    /// The violation attempted, across the boundary that matters: a link in `/w/out` to a
    /// file outside it must not bring that file's bytes back (docs/lessons.md §3).
    #[cfg(unix)]
    #[test]
    fn a_link_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.txt", b"host");
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), dir.path().join("l.txt"))
            .unwrap();
        let (files, skipped) = collect_outputs(dir.path(), OutputLimits::DEFAULT);
        assert!(files.is_empty(), "a link's target was read");
        assert_eq!(skipped[0].reason, SkipReason::NotAFile);
    }

    /// The same on Windows, where making a symlink takes a privilege or developer mode —
    /// without one there is nothing to test, and the skip says so.
    #[cfg(windows)]
    #[test]
    fn a_link_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.txt", b"host");
        let link = dir.path().join("l.txt");
        if std::os::windows::fs::symlink_file(outside.path().join("secret.txt"), &link).is_err() {
            eprintln!("skip: creating a symlink needs a privilege here");
            return;
        }
        let (files, skipped) = collect_outputs(dir.path(), OutputLimits::DEFAULT);
        assert!(files.is_empty(), "a link's target was read");
        assert_eq!(skipped[0].reason, SkipReason::NotAFile);
    }

    #[test]
    fn the_caps_skip_what_they_drop_and_name_it() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "1.bin", b"12345678"); // 8: at the file cap, kept (8)
        write(dir.path(), "2.bin", b"123456789"); // 9: over the file cap
        write(dir.path(), "3.bin", b"1234567"); // 8 + 7 > 14: over the total
        write(dir.path(), "4.bin", b"1234"); // still fits: kept (12)
        write(dir.path(), "5.bin", b"1"); // kept (13), the third file
        write(dir.path(), "6.bin", b"1"); // past three files
        let limits = OutputLimits {
            max_files: 3,
            max_file_bytes: 8,
            max_total_bytes: 14,
        };
        let (files, skipped) = collect_outputs(dir.path(), limits);
        assert_eq!(names(&files), ["1.bin", "4.bin", "5.bin"]);
        let reasons: Vec<(&str, SkipReason)> = skipped
            .iter()
            .map(|s| (s.name.as_str(), s.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                ("2.bin", SkipReason::TooLarge),
                ("3.bin", SkipReason::OverTotal),
                ("6.bin", SkipReason::TooMany),
            ]
        );
    }

    #[test]
    fn a_timed_out_call_names_what_it_left_and_reads_none_of_it() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "half.png", b"\x89PNG");
        assert_eq!(
            timed_out_outputs(dir.path()),
            [SkippedOutput {
                name: "half.png".into(),
                reason: SkipReason::TimedOut
            }]
        );
    }

    #[test]
    fn a_missing_directory_collects_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (files, skipped) = collect_outputs(&dir.path().join("absent"), OutputLimits::DEFAULT);
        assert!(files.is_empty() && skipped.is_empty());
    }
}

/// What one call staged into `/w/in`: each file's guest name and the bytes that reached
/// it, in the order the call named them.
#[cfg(test)]
pub type StagedFiles = Vec<(String, Vec<u8>)>;

/// A sandbox mock for `python_exec` tool tests.
#[cfg(test)]
pub struct MockSandbox {
    availability: SandboxAvailability,
    output: SandboxOutput,
    /// Records of `run` calls: (code, the network flag).
    pub calls: std::sync::Mutex<Vec<(String, bool)>>,
    /// What each call staged into `/w/in`: the guest's name and the bytes that reached it
    /// — a [`InputSource::Path`] read back, as the guest would read it.
    pub staged: std::sync::Mutex<Vec<StagedFiles>>,
}

#[cfg(test)]
impl MockSandbox {
    /// A ready sandbox that returns the given output.
    pub fn ready(output: SandboxOutput) -> Self {
        Self {
            availability: SandboxAvailability::Ready,
            output,
            calls: std::sync::Mutex::new(Vec::new()),
            staged: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// An unavailable sandbox with a reason.
    pub fn missing(reason: &str) -> Self {
        Self {
            availability: SandboxAvailability::Missing(reason.into()),
            output: SandboxOutput::default(),
            calls: std::sync::Mutex::new(Vec::new()),
            staged: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl SandboxRunner for MockSandbox {
    fn availability(&self, _loc: &Locale) -> SandboxAvailability {
        self.availability.clone()
    }

    async fn run(&self, job: SandboxJob<'_>, _loc: &Locale) -> Result<SandboxOutput> {
        self.calls
            .lock()
            .unwrap()
            .push((job.code.to_string(), job.net));
        self.staged.lock().unwrap().push(
            job.inputs
                .iter()
                .map(|input| {
                    let bytes = match &input.source {
                        InputSource::Bytes(bytes) => bytes.clone(),
                        InputSource::Path(from) => std::fs::read(from).unwrap_or_default(),
                    };
                    (input.name.clone(), bytes)
                })
                .collect(),
        );
        Ok(self.output.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    /// The reference locale for tests (ru byte-for-byte — the previous substring asserts stay intact).
    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    /// Both runners lay the job directory out through one helper (§14 V1): the script, the
    /// staged copies under their own names in `in/`, and an `out/` that exists even when
    /// the call staged nothing — code that looks into either finds a folder, not an error.
    #[tokio::test]
    async fn a_job_directory_holds_the_script_the_copies_and_an_out_folder() {
        let host = tempfile::tempdir().expect("a source dir");
        let from = host.path().join("sales.xlsx");
        std::fs::write(&from, b"PK\x03\x04").unwrap();

        let job = JobDir::create().expect("a job dir");
        let inputs = [
            SandboxInput::bytes("memo.txt", b"the note".to_vec()),
            SandboxInput::path("sales.xlsx", &from),
        ];
        let layout = prepare_job(&job, "print(1)", &inputs, ru()).await.unwrap();

        assert_eq!(std::fs::read_to_string(&layout.script).unwrap(), "print(1)");
        assert!(layout.out_dir.is_dir(), "out/ must exist before the run");
        let staged = job.path.join("in");
        assert_eq!(
            std::fs::read_to_string(staged.join("memo.txt")).unwrap(),
            "the note"
        );
        assert_eq!(
            std::fs::read(staged.join("sales.xlsx")).unwrap(),
            b"PK\x03\x04"
        );
        // The copy is the call's own: the source is untouched by anything the run does.
        assert!(from.is_file());
    }

    /// The guard `shared` can make without knowing what named a file: a name that is not
    /// one plain component is refused, so a bug upstream cannot write outside the job.
    #[tokio::test]
    async fn a_staged_name_that_is_not_one_component_is_refused() {
        for name in ["../escape.txt", "sub/dir.txt", "..", ""] {
            let job = JobDir::create().expect("a job dir");
            let inputs = [SandboxInput::bytes(name, b"x".to_vec())];
            let err = prepare_job(&job, "print(1)", &inputs, ru())
                .await
                .expect_err("the name must be refused");
            assert!(format!("{err:#}").contains("имя"), "{name:?}: {err:#}");
        }
    }

    /// §14 V4: a path is checked as a file, a bare name is left to `PATH` — no probe.
    #[test]
    fn a_local_interpreter_path_is_checked_and_a_bare_name_is_not() {
        let missing = LocalSandbox::new(Some("D:\\nowhere\\python.exe".into()));
        assert!(matches!(
            missing.availability(ru()),
            SandboxAvailability::Missing(_)
        ));
        assert_eq!(
            LocalSandbox::new(None).availability(ru()),
            SandboxAvailability::Ready
        );
        assert_eq!(
            LocalSandbox::new(Some("python3".into())).availability(ru()),
            SandboxAvailability::Ready
        );
        // The platform's default is what the mode runs when nothing is configured.
        let default = LocalSandbox::new(None).interpreter();
        assert_eq!(default, if cfg!(windows) { "python" } else { "python3" });
    }

    #[test]
    fn wrapper_prepends_setsockopt_shim() {
        let w = build_wrapper("print(1)");
        assert!(w.contains("_mf_patch_socket"));
        assert!(w.contains("setsockopt"));
        // The user's code comes after the shim.
        assert!(w.trim_end().ends_with("print(1)"));
    }

    /// Both halves of the matplotlib shim are in the wrapper, ahead of the user's code.
    #[test]
    fn wrapper_prepares_matplotlib_for_wasix() {
        let w = build_wrapper("print(1)");
        assert!(w.contains("MPLCONFIGDIR"), "{w}");
        // A `\n` inside the Python string literal, not a real line break.
        assert!(w.contains(r"text.hinting: default\n"), "{w}");
        let shim = w
            .find("_mf_prepare_matplotlib()")
            .expect("the shim is called");
        assert!(shim < w.find("print(1)").unwrap(), "{w}");
    }

    #[test]
    fn build_args_without_net_omits_flag() {
        let mounts = [(PathBuf::from("/tmp/job"), GUEST_WORK)];
        let envs = [("PYTHONUTF8", "1".to_string())];
        let a = build_args(
            OsStr::new("python/python"),
            &mounts,
            &envs,
            false,
            "/w/job.py",
        );
        let s: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert_eq!(s[0], "run");
        assert_eq!(s[1], "--v8");
        assert!(!s.iter().any(|x| x == "--net"));
        assert!(s.iter().any(|x| x == "--volume"));
        assert!(s.iter().any(|x| x.ends_with(":/w")));
        assert!(s.iter().any(|x| x == "--env"));
        assert!(s.iter().any(|x| x == "PYTHONUTF8=1"));
        // The python source, then the separator, then the script — right at the end.
        assert_eq!(s[s.len() - 3], "python/python");
        assert_eq!(s[s.len() - 2], "--");
        assert_eq!(s[s.len() - 1], "/w/job.py");
    }

    #[test]
    fn build_args_with_net_adds_flag_before_mounts() {
        let mounts = [(PathBuf::from("/tmp/job"), GUEST_WORK)];
        let a = build_args(OsStr::new("python/python"), &mounts, &[], true, "/w/job.py");
        let s: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert_eq!(s[2], "--net");
    }

    #[test]
    fn build_args_mount_is_host_colon_guest() {
        let mounts = [(PathBuf::from("/home/u/job"), GUEST_WORK)];
        let a = build_args(
            OsStr::new("python/python"),
            &mounts,
            &[],
            false,
            "/w/job.py",
        );
        let vol = a
            .iter()
            .position(|x| x == OsStr::new("--volume"))
            .map(|i| a[i + 1].to_string_lossy().into_owned())
            .unwrap();
        assert!(vol.ends_with(":/w"), "vol = {vol}");
        assert!(vol.starts_with("/home/u/job"), "vol = {vol}");
    }

    #[test]
    fn locate_wasmer_finds_direct_and_dist() {
        let dir = tempfile::tempdir().unwrap();
        assert!(locate_wasmer(dir.path()).is_none());
        // The setup's unpack: <dir>/wasmer-dist/bin/wasmer[.exe].
        let bin_dir = dir.path().join("wasmer-dist").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join(WASMER_BIN), b"stub").unwrap();
        assert!(locate_wasmer(dir.path()).unwrap().ends_with(WASMER_BIN));
        // A direct placement takes priority.
        std::fs::write(dir.path().join(WASMER_BIN), b"stub").unwrap();
        let found = locate_wasmer(dir.path()).unwrap();
        assert_eq!(found, dir.path().join(WASMER_BIN));
    }

    #[test]
    fn availability_missing_without_binary() {
        // A directory with no binary → Missing (given no env-override in the CI environment).
        if env_override(ENV_WASMER).is_some() {
            return; // the environment sets an override — the test is uninformative
        }
        let dir = tempfile::tempdir().unwrap();
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        assert!(matches!(
            sb.availability(ru()),
            SandboxAvailability::Missing(_)
        ));
    }

    #[test]
    fn availability_ready_with_binary() {
        if env_override(ENV_WASMER).is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(WASMER_BIN), b"stub").unwrap();
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        assert_eq!(sb.availability(ru()), SandboxAvailability::Ready);
    }

    #[tokio::test]
    async fn gate_rejects_second_concurrent_task() {
        let dir = tempfile::tempdir().unwrap();
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        // Hold the sole permit — emulate "a task is already running".
        let _held = sb.gate.try_acquire().unwrap();
        // The second launch is rejected instantly (before resolve_wasmer/spawning a process).
        let err = sb
            .run(
                SandboxJob::new("print(1)", false, Duration::from_secs(5)),
                ru(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("занята"), "got: {err}");
    }

    #[tokio::test]
    async fn busy_error_is_localized() {
        // A regression against a forgotten `loc`: the "busy" reason in the locale's language.
        let dir = tempfile::tempdir().unwrap();
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        let _held = sb.gate.try_acquire().unwrap();
        let en = sb
            .run(
                SandboxJob::new("print(1)", false, Duration::from_secs(5)),
                locale(Lang::En),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(en.contains("busy"), "{en}");
        assert!(!en.chars().any(|c| ('а'..='я').contains(&c)), "{en}");
    }

    #[tokio::test]
    async fn gate_permit_released_after_run() {
        // After `run` finishes (here — with a "no binary" error), the permit
        // is returned, and the next call again reaches the logic (rather than hitting the gate).
        let dir = tempfile::tempdir().unwrap();
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        if env_override(ENV_WASMER).is_some() {
            return; // the environment sets a binary — this test is about a missing binary
        }
        let e1 = sb
            .run(
                SandboxJob::new("print(1)", false, Duration::from_secs(5)),
                ru(),
            )
            .await
            .unwrap_err();
        assert!(e1.to_string().contains("wasmer"), "got: {e1}");
        let e2 = sb
            .run(
                SandboxJob::new("print(1)", false, Duration::from_secs(5)),
                ru(),
            )
            .await
            .unwrap_err();
        assert!(e2.to_string().contains("wasmer"), "got: {e2}");
    }

    #[test]
    fn resolve_python_falls_back_to_registry_package() {
        if env_override(ENV_PYTHON).is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        assert_eq!(sb.resolve_python(), OsString::from(DEFAULT_PYTHON_PKG));
    }

    /// A sandbox directory with a stub `wasmer`, and optionally a `site-packages`
    /// directory and a packed image.
    fn sandbox_dir(site_packages: bool, image: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(WASMER_BIN), b"stub").unwrap();
        if site_packages {
            std::fs::create_dir(dir.path().join("site-packages")).unwrap();
        }
        if image {
            std::fs::write(dir.path().join(SANDBOX_IMAGE), b"stub").unwrap();
        }
        dir
    }

    /// Provisioning verifies a **candidate** image, so it has to be able to start one that
    /// is not the installed file — that is what lets `setup` reject a bad build while the
    /// sandbox that works is still on disk.
    #[test]
    fn a_candidate_image_is_what_runs_when_one_is_named() {
        let dir = sandbox_dir(true, true);
        let candidate = format!("{SANDBOX_IMAGE}.partial");
        std::fs::write(dir.path().join(&candidate), b"fresh").unwrap();

        let installed = WasmerSandbox::new(Some(dir.path().to_path_buf()))
            .plan()
            .unwrap();
        assert_eq!(installed.program, dir.path().join(SANDBOX_IMAGE));

        let fresh = WasmerSandbox::for_candidate(dir.path().to_path_buf(), &candidate)
            .plan()
            .unwrap();
        assert_eq!(fresh.program, dir.path().join(&candidate));

        // And a candidate that was never built is not silently the installed one.
        std::fs::remove_file(dir.path().join(&candidate)).unwrap();
        let plan = WasmerSandbox::for_candidate(dir.path().to_path_buf(), &candidate).plan();
        assert!(
            plan.is_none(),
            "a missing candidate must not fall through to the installed image: {plan:?}"
        );
    }

    /// The image carries its own `site-packages`: it is what runs, nothing is mounted
    /// beside the job, and `/sp` goes on `PYTHONPATH` — with the directory present too.
    #[test]
    fn the_packed_image_runs_and_nothing_is_mounted() {
        let dir = sandbox_dir(true, true);
        let plan = WasmerSandbox::new(Some(dir.path().to_path_buf()))
            .plan()
            .unwrap();
        assert_eq!(
            plan.program,
            dir.path().join(SANDBOX_IMAGE).into_os_string()
        );
        assert_eq!(plan.site_mount, None);
        assert!(plan.site_on_path);
    }

    /// A `site-packages` directory with no image — an install from before the image — is
    /// refused rather than mounted writable, and the reason names the command that packs it.
    #[tokio::test]
    async fn unpacked_site_packages_is_refused_with_the_way_out() {
        let dir = sandbox_dir(true, false);
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        assert_eq!(sb.plan(), None);
        let SandboxAvailability::Missing(why) = sb.availability(locale(Lang::En)) else {
            panic!("an unpacked site-packages must not be Ready");
        };
        assert!(why.contains("mindfork sandbox setup"), "{why}");
        let err = sb
            .run(
                SandboxJob::new("print(1)", false, Duration::from_secs(5)),
                locale(Lang::En),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("mindfork sandbox setup"), "{err}");
    }

    /// Provisioning's warmup is the one launch that mounts the directory — writably, to
    /// fill `__pycache__` before packing — and it does so even once an image exists.
    #[test]
    fn provisioning_mounts_the_directory() {
        let dir = sandbox_dir(true, true);
        let plan = WasmerSandbox::for_provisioning(dir.path().to_path_buf())
            .plan()
            .unwrap();
        assert_eq!(plan.site_mount, Some(dir.path().join("site-packages")));
        assert!(plan.site_on_path);
        assert_ne!(
            plan.program,
            dir.path().join(SANDBOX_IMAGE).into_os_string()
        );
    }

    /// Neither a directory nor an image: plain CPython, nothing on `PYTHONPATH`.
    #[test]
    fn without_packages_plain_python_runs() {
        let dir = sandbox_dir(false, false);
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        let plan = sb.plan().unwrap();
        assert_eq!(plan.program, sb.resolve_python());
        assert_eq!(plan.site_mount, None);
        assert!(!plan.site_on_path);
        assert_eq!(sb.availability(ru()), SandboxAvailability::Ready);
    }
}
