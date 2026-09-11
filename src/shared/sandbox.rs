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
/// The guest mount point of `site-packages` (preinstalled packages).
const GUEST_SITE: &str = "/sp";
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
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxOutput {
    pub stdout: String,
    pub stderr: String,
    /// The process's exit code (`None` — didn't exit normally / was killed).
    pub exit_code: Option<i32>,
    /// Execution was interrupted by a timeout (the process was killed).
    pub timed_out: bool,
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

    /// Runs `code` (Python) in the sandbox with network access `net` and a
    /// `timeout`. On timeout the process is killed and `timed_out = true` is
    /// returned. An error occurs only at the process-launch level (not on a
    /// nonzero guest exit code). `loc` — the language of the error text
    /// (embedded by the caller: `python_exec` — the profile's language,
    /// warmup — the interface language).
    async fn run(
        &self,
        code: &str,
        net: bool,
        timeout: Duration,
        loc: &Locale,
    ) -> Result<SandboxOutput>;
}

/// The real sandbox: drives the bundled `wasmer` as a child process.
pub struct WasmerSandbox {
    /// The sandbox directory (`data/sandbox/`): `wasmer[.exe]`, `python.webc`,
    /// `site-packages/`. `None` — only through an env-override (tests/default).
    dir: Option<PathBuf>,
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
        Self {
            dir,
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

    /// The `site-packages` directory to mount (if it exists).
    fn site_packages(&self) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let sp = dir.join("site-packages");
        sp.is_dir().then_some(sp)
    }
}

#[async_trait::async_trait]
impl SandboxRunner for WasmerSandbox {
    fn availability(&self, loc: &Locale) -> SandboxAvailability {
        match self.resolve_wasmer() {
            Some(_) => SandboxAvailability::Ready,
            None => SandboxAvailability::Missing(loc.t("sandbox.err.not_installed").to_string()),
        }
    }

    async fn run(
        &self,
        code: &str,
        net: bool,
        timeout: Duration,
        loc: &Locale,
    ) -> Result<SandboxOutput> {
        // The "one task" gate: a concurrent launch is rejected right away (before spawning).
        let _permit = self
            .gate
            .try_acquire()
            .map_err(|_| anyhow::anyhow!("{}", loc.t("sandbox.err.busy")))?;
        let wasmer = self
            .resolve_wasmer()
            .ok_or_else(|| anyhow::anyhow!("{}", loc.t("sandbox.err.not_found")))?;
        let python = self.resolve_python();

        // The task script in a unique temp directory (auto-cleanup via Drop).
        let job = JobDir::create().with_context(|| loc.t("sandbox.err.job_dir").to_string())?;
        let script = job.path.join("job.py");
        tokio::fs::write(&script, build_wrapper(code))
            .await
            .with_context(|| loc.t("sandbox.err.write_script").to_string())?;

        // Mount the working directory and (if present) site-packages; PYTHONPATH for the guest.
        let mut mounts: Vec<(PathBuf, &str)> = vec![(job.path.clone(), GUEST_WORK)];
        let mut envs: Vec<(&str, String)> = vec![
            ("PYTHONIOENCODING", "utf-8".into()),
            ("PYTHONUTF8", "1".into()),
        ];
        if let Some(sp) = self.site_packages() {
            mounts.push((sp, GUEST_SITE));
            envs.push(("PYTHONPATH", GUEST_SITE.into()));
        }
        let script_guest = format!("{GUEST_WORK}/job.py");
        let args = build_args(&python, &mounts, &envs, net, &script_guest);

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

        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => Ok(SandboxOutput {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                exit_code: out.status.code(),
                timed_out: false,
            }),
            Ok(Err(e)) => Err(e).with_context(|| loc.t("sandbox.err.wait").to_string()),
            Err(_) => Ok(SandboxOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                timed_out: true,
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

/// A sandbox mock for `python_exec` tool tests.
#[cfg(test)]
pub struct MockSandbox {
    availability: SandboxAvailability,
    output: SandboxOutput,
    /// Records of `run` calls: (code, the network flag).
    pub calls: std::sync::Mutex<Vec<(String, bool)>>,
}

#[cfg(test)]
impl MockSandbox {
    /// A ready sandbox that returns the given output.
    pub fn ready(output: SandboxOutput) -> Self {
        Self {
            availability: SandboxAvailability::Ready,
            output,
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// An unavailable sandbox with a reason.
    pub fn missing(reason: &str) -> Self {
        Self {
            availability: SandboxAvailability::Missing(reason.into()),
            output: SandboxOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                timed_out: false,
            },
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl SandboxRunner for MockSandbox {
    fn availability(&self, _loc: &Locale) -> SandboxAvailability {
        self.availability.clone()
    }

    async fn run(
        &self,
        code: &str,
        net: bool,
        _timeout: Duration,
        _loc: &Locale,
    ) -> Result<SandboxOutput> {
        self.calls.lock().unwrap().push((code.to_string(), net));
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
            .run("print(1)", false, Duration::from_secs(5), ru())
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
            .run("print(1)", false, Duration::from_secs(5), locale(Lang::En))
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
            .run("print(1)", false, Duration::from_secs(5), ru())
            .await
            .unwrap_err();
        assert!(e1.to_string().contains("wasmer"), "got: {e1}");
        let e2 = sb
            .run("print(1)", false, Duration::from_secs(5), ru())
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
}
