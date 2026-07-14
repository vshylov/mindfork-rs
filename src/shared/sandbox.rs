//! Сайдкар-песочница Python на **Wasmer/WASIX**. Отдельный процесс `wasmer`
//! (бандленный рядом с приложением, в `data/sandbox/`), исполняющий код в
//! WASM-изоляции: у гостя нет доступа к хост-ФС (видит только смонтированное), сеть —
//! по явному флагу `--net`. Прерывание — kill процесса (чисто и быстро, проверено
//! в Фазе 0). См. [docs/research/python-wasmer-sandbox.md](../../docs/research/python-wasmer-sandbox.md)
//! (решение §9.7: сайдкар `wasmer` за этим контрактом, а не embed V8 в dll).
//!
//! Слой `shared` (FSD): контракт [`SandboxRunner`] за трейтом (mock в тестах —
//! паттерн `EngineBackend`); реальная реализация [`WasmerSandbox`] строит команду и
//! запускает бинарь. Инструмент `python_exec` (`features/tools/python.rs`)
//! использует этот контракт.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::shared::i18n::Locale;

/// Имя бинаря `wasmer` в каталоге песочницы (по платформе).
const WASMER_BIN: &str = if cfg!(windows) {
    "wasmer.exe"
} else {
    "wasmer"
};
/// Пакет CPython по умолчанию, если локального `python.webc` нет (скачивается
/// `wasmer` из реестра при первом запуске; Фаза 2 кладёт его в `data/sandbox/`).
const DEFAULT_PYTHON_PKG: &str = "python/python";
/// Гостевая точка монтирования рабочего каталога (со скриптом задачи).
const GUEST_WORK: &str = "/w";
/// Гостевая точка монтирования `site-packages` (предустановленные пакеты).
const GUEST_SITE: &str = "/sp";
/// Переменная окружения: путь/имя бинаря `wasmer` (override поиска).
const ENV_WASMER: &str = "MINDFORK_SANDBOX_WASMER";
/// Переменная окружения: путь к `python.webc` или ссылка на пакет (override).
const ENV_PYTHON: &str = "MINDFORK_SANDBOX_PYTHON";

/// Шим, подмешиваемый перед пользовательским кодом: глушит неподдержанные в
/// WASIX опции сокета. Без него `http.client`/`urllib`/`requests` падают — WASIX не
/// реализует `setsockopt(TCP_NODELAY)` и бросает `EINVAL`, а http-клиенты его всегда
/// ставят (находка Фазы 0, §9.3). Обёрнуто в функцию, чтобы не сорить именами в
/// глобальном пространстве пользовательского кода.
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

/// Сырой результат исполнения кода в песочнице (форматирование — на стороне
/// инструмента, чтобы совпадать с локальным режимом).
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxOutput {
    pub stdout: String,
    pub stderr: String,
    /// Код возврата процесса (`None` — не завершился нормально/убит).
    pub exit_code: Option<i32>,
    /// Исполнение прервано по таймауту (процесс убит).
    pub timed_out: bool,
}

/// Готовность песочницы к запуску (дёшево, без запуска процесса).
#[derive(Debug, Clone, PartialEq)]
pub enum SandboxAvailability {
    /// Бинарь `wasmer` найден — можно запускать.
    Ready,
    /// Не установлена/не найдена — человекочитаемая причина (уходит модели).
    Missing(String),
}

/// Запуск кода в изолированной песочнице. За трейтом — ради mock в тестах
/// (`features/tools/python.rs`) и заменяемости реализации.
#[async_trait::async_trait]
pub trait SandboxRunner: Send + Sync {
    /// Проверка готовности (наличие бинаря `wasmer`). Без запуска процесса. `loc` —
    /// язык причины недоступности (её показывает вызывающий: `python_exec` — на языке
    /// профиля, ось A; warmup провизии — на языке интерфейса).
    fn availability(&self, loc: &Locale) -> SandboxAvailability;

    /// Исполнить `code` (Python) в песочнице с сетью `net` и таймаутом `timeout`.
    /// По таймауту процесс убивается, возвращается `timed_out = true`. Ошибка —
    /// только на уровне запуска процесса (не на ненулевом коде возврата гостя). `loc` —
    /// язык текста ошибки (её встраивает вызывающий: `python_exec` — язык профиля,
    /// warmup — язык интерфейса).
    async fn run(
        &self,
        code: &str,
        net: bool,
        timeout: Duration,
        loc: &Locale,
    ) -> Result<SandboxOutput>;
}

/// Реальная песочница: драйвит бандленный `wasmer` как дочерний процесс.
pub struct WasmerSandbox {
    /// Каталог песочницы (`data/sandbox/`): `wasmer[.exe]`, `python.webc`,
    /// `site-packages/`. `None` — только через env-override (тесты/дефолт).
    dir: Option<PathBuf>,
    /// Гейт «одна задача за раз» (одно разрешение). Защита от утечки процессов/
    /// потоков и предсказуемая нагрузка: параллельный вызов сразу отклоняется.
    /// В штатном agentic-loop вызовы и так последовательны — это защита в глубину.
    gate: Arc<Semaphore>,
    /// Жёсткий лимит памяти процесса (МБ; `None` — без лимита). Применяется только
    /// на Windows (Job Object). См. [`WasmerSandbox::with_memory_limit`].
    memory_mb: Option<u64>,
}

impl WasmerSandbox {
    /// Создаёт песочницу с каталогом ассетов (`data/sandbox/`; `None` — без него,
    /// тогда бинарь берётся только из env-override).
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            gate: Arc::new(Semaphore::new(1)),
            memory_mb: None,
        }
    }

    /// Задаёт жёсткий лимит памяти (МБ; `Some(0)`/`None` — без лимита). Только
    /// Windows: процесс `wasmer` помещается в Job Object с
    /// `JOB_OBJECT_LIMIT_PROCESS_MEMORY`; превышение убивает процесс (защита хоста
    /// от OOM). На Unix поле игнорируется (rlimit ненадёжен с V8 — резервирует
    /// большое виртуальное пространство). См. ADR 0005.
    pub fn with_memory_limit(mut self, mb: Option<u64>) -> Self {
        self.memory_mb = mb.filter(|&m| m > 0);
        self
    }

    /// Путь/имя бинаря `wasmer`: env-override → каталог песочницы ([`locate_wasmer`]).
    fn resolve_wasmer(&self) -> Option<OsString> {
        if let Some(o) = env_override(ENV_WASMER) {
            return Some(o);
        }
        self.dir
            .as_deref()
            .and_then(locate_wasmer)
            .map(PathBuf::into_os_string)
    }

    /// Источник CPython: env-override → `<dir>/python.webc` → пакет реестра.
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

    /// Каталог `site-packages` для монтирования (если существует).
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
        // Гейт «одна задача»: параллельный запуск сразу отклоняется (до спавна).
        let _permit = self
            .gate
            .try_acquire()
            .map_err(|_| anyhow::anyhow!("{}", loc.t("sandbox.err.busy")))?;
        let wasmer = self
            .resolve_wasmer()
            .ok_or_else(|| anyhow::anyhow!("{}", loc.t("sandbox.err.not_found")))?;
        let python = self.resolve_python();

        // Скрипт задачи в уникальном временном каталоге (авто-очистка через Drop).
        let job = JobDir::create().with_context(|| loc.t("sandbox.err.job_dir").to_string())?;
        let script = job.path.join("job.py");
        tokio::fs::write(&script, build_wrapper(code))
            .await
            .with_context(|| loc.t("sandbox.err.write_script").to_string())?;

        // Монтируем рабочий каталог и (если есть) site-packages; PYTHONPATH на гостя.
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
            // Python исполняется ВНУТРИ процесса wasmer (V8 in-process, не дочерний
            // процесс), поэтому kill самого wasmer останавливает и код. На таймауте/
            // отмене future дропается → процесс убивается.
            .kill_on_drop(true);
        // Кэш скомпилированных модулей — под каталогом песочницы (самодостаточно,
        // не в ~/.wasmer): первый запуск компилирует python.wasm (секунды), дальше
        // тёплый старт из кэша. См. docs/research/python-wasmer-sandbox.md §2.3.
        if let Some(dir) = &self.dir {
            cmd.env("WASMER_CACHE_DIR", dir.join("cache"));
        }

        let child = cmd
            .spawn()
            .with_context(|| loc.tf("sandbox.err.spawn", &[("path", &wasmer.to_string_lossy())]))?;

        // Жёсткий лимит памяти (Windows Job Object) — сразу после спавна, до того как
        // V8 закоммитит существенную память. «Лучшее усилие»: сбой лишь логируется.
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

/// Ищет бинарь `wasmer` в каталоге песочницы (чистая, тестируемая). Порядок:
/// прямое размещение `<dir>/wasmer[.exe]` (ручная установка) → распаковка setup'ом
/// `<dir>/wasmer-dist/bin/wasmer[.exe]`. Используется и рантаймом ([`WasmerSandbox`]),
/// и провизией (`features::sandbox_setup`) — единый источник истины о раскладке.
pub fn locate_wasmer(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join(WASMER_BIN);
    if direct.is_file() {
        return Some(direct);
    }
    let dist = dir.join("wasmer-dist").join("bin").join(WASMER_BIN);
    dist.is_file().then_some(dist)
}

/// Непустое значение env-переменной как `OsString` (override пути/имени).
fn env_override(key: &str) -> Option<OsString> {
    std::env::var_os(key).filter(|v| !v.is_empty())
}

/// Применяет жёсткий лимит памяти к процессу `wasmer` (Windows Job Object). При
/// превышении процесс убивается — защита хоста от OOM. «Лучшее усилие»: сбой winapi
/// лишь логируется. Проверено вживую (§9.6 исследования): лимит держится и после
/// закрытия хэндла job'а (job живёт, пока процесс — его член), поэтому HANDLE не
/// удерживается через `await` (важно для `Send`-фьючи).
#[cfg(windows)]
fn apply_memory_limit(child: &tokio::process::Child, mb: u64) {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    let Some(raw) = child.raw_handle() else {
        tracing::warn!("песочница: нет хэндла процесса — лимит памяти не применён");
        return;
    };
    // SAFETY: `raw` — валидный хэндл только что запущенного процесса; job создаётся и
    // закрывается в пределах этого блока, поля структуры инициализированы нулями.
    unsafe {
        let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            tracing::warn!("песочница: CreateJobObjectW не удался");
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
            tracing::warn!("песочница: SetInformationJobObject не удался");
            CloseHandle(job);
            return;
        }
        if AssignProcessToJobObject(job, raw as HANDLE) == 0 {
            tracing::warn!("песочница: AssignProcessToJobObject не удался");
        }
        // Хэндл можно закрыть сразу: лимит остаётся, пока процесс — член job'а.
        CloseHandle(job);
    }
}

/// На не-Windows жёсткий лимит памяти не применяется: `rlimit`/`RLIMIT_AS`
/// ненадёжен с бэкендом V8 (он резервирует большое виртуальное адресное
/// пространство, из-за чего низкий лимит ломает сам старт). Полагаемся на таймаут
/// и wasm32 (~4 ГБ). См. ADR 0005.
#[cfg(not(windows))]
fn apply_memory_limit(_child: &tokio::process::Child, _mb: u64) {
    tracing::debug!("песочница: лимит памяти поддержан только на Windows — пропуск");
}

/// Оборачивает пользовательский код шимом `setsockopt` (чистая, тестируемая).
pub fn build_wrapper(code: &str) -> String {
    format!("{SETSOCKOPT_SHIM}\n{code}")
}

/// Собирает аргументы командной строки `wasmer` (чистая, тестируемая). Форма:
/// `run --v8 [--net] (--volume HOST:GUEST)* (--env K=V)* <python> -- <script>`.
/// Порядок и хост:гость-монтирование проверены живьём в Фазе 0 (в т.ч. с
/// Windows-путём, где двоеточие драйва `C:` не ломает разбор `--volume`).
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

/// Временный каталог для скрипта задачи (авто-очистка при `Drop`). Живёт в системном
/// tmp; уникален по UUID — без зависимости `tempfile` в рантайме.
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

/// Мок песочницы для тестов инструмента `python_exec`.
#[cfg(test)]
pub struct MockSandbox {
    availability: SandboxAvailability,
    output: SandboxOutput,
    /// Записи вызовов `run`: (код, признак сети).
    pub calls: std::sync::Mutex<Vec<(String, bool)>>,
}

#[cfg(test)]
impl MockSandbox {
    /// Готовая песочница, возвращающая заданный вывод.
    pub fn ready(output: SandboxOutput) -> Self {
        Self {
            availability: SandboxAvailability::Ready,
            output,
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Недоступная песочница с причиной.
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

    /// Референсная локаль для тестов (ru байт-в-байт — прежние ассерты подстрок целы).
    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    #[test]
    fn wrapper_prepends_setsockopt_shim() {
        let w = build_wrapper("print(1)");
        assert!(w.contains("_mf_patch_socket"));
        assert!(w.contains("setsockopt"));
        // Пользовательский код идёт после шима.
        assert!(w.trim_end().ends_with("print(1)"));
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
        // python-источник, затем разделитель, затем скрипт — в самом конце.
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
        // Распаковка setup'ом: <dir>/wasmer-dist/bin/wasmer[.exe].
        let bin_dir = dir.path().join("wasmer-dist").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join(WASMER_BIN), b"stub").unwrap();
        assert!(locate_wasmer(dir.path()).unwrap().ends_with(WASMER_BIN));
        // Прямое размещение имеет приоритет.
        std::fs::write(dir.path().join(WASMER_BIN), b"stub").unwrap();
        let found = locate_wasmer(dir.path()).unwrap();
        assert_eq!(found, dir.path().join(WASMER_BIN));
    }

    #[test]
    fn availability_missing_without_binary() {
        // Каталог без бинаря → Missing (при отсутствии env-override в окружении CI).
        if env_override(ENV_WASMER).is_some() {
            return; // окружение задаёт override — тест неинформативен
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
        // Держим единственное разрешение — эмулируем «уже идёт задача».
        let _held = sb.gate.try_acquire().unwrap();
        // Второй запуск отклоняется мгновенно (до resolve_wasmer/спавна процесса).
        let err = sb
            .run("print(1)", false, Duration::from_secs(5), ru())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("занята"), "got: {err}");
    }

    #[tokio::test]
    async fn busy_error_is_localized() {
        // Регрессия против забытого `loc`: причина «занята» на языке локали.
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
        // После завершения run (тут — ошибкой «нет бинаря») разрешение возвращается,
        // и следующий вызов снова доходит до логики (а не упирается в гейт).
        let dir = tempfile::tempdir().unwrap();
        let sb = WasmerSandbox::new(Some(dir.path().to_path_buf()));
        if env_override(ENV_WASMER).is_some() {
            return; // окружение задаёт бинарь — этот тест про отсутствие бинаря
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
