//! Запуск локального `llama-server` (llama.cpp, managed-режим): дочерний процесс,
//! ожидание готовности (с обнаружением раннего выхода процесса), остановка при
//! `drop` хэндла (монитор-задача + `kill_on_drop`). См. spec §3.4.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use super::client::OpenAiClient;

/// Конфигурация запуска managed-сервера `llama-server` (llama.cpp).
#[derive(Debug, Clone)]
pub struct ManagedConfig {
    pub binary: PathBuf,
    /// Путь к GGUF-модели (`-m`).
    pub model_path: Option<String>,
    /// Слои на GPU (`-ngl`).
    pub gpu_layers: i32,
    /// Размер контекста (`-c`).
    pub context_size: u32,
    /// Использовать встроенный chat-template модели (`--jinja`).
    pub jinja: bool,
    /// Формат reasoning (`--reasoning-format`); `None` — не задавать.
    pub reasoning_format: Option<String>,
    /// Режим эмбеддингов (`--embeddings`) — для embedding-сервера.
    pub embeddings: bool,
    /// Не использовать mmap при загрузке модели (`--no-mmap`): грузит веса в RAM
    /// целиком. Полезно на сетевых/медленных дисках и при нехватке файлового кэша.
    pub no_mmap: bool,
    /// Интерфейс bind (`--host`).
    pub host: String,
    pub port: u16,
    /// Дополнительные сырые аргументы.
    pub extra_args: Vec<String>,
}

impl ManagedConfig {
    /// URL для подключения к локальному дочернему процессу (всегда `127.0.0.1`,
    /// независимо от `--host`, который управляет лишь интерфейсом bind).
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

/// Аргументы командной строки `llama-server` из конфигурации (чистая функция).
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
    if cfg.jinja {
        args.push("--jinja".into());
    }
    if let Some(rf) = &cfg.reasoning_format {
        args.push("--reasoning-format".into());
        args.push(rf.clone());
    }
    if cfg.embeddings {
        args.push("--embeddings".into());
        // Эмбеддинг-модели non-causal: весь вход обрабатывается в ОДНОМ физическом
        // батче (ubatch). По умолчанию `n_ubatch=512`, и llama-server приравнивает
        // `n_batch` к нему — поэтому чанк длиннее ~512 токенов отвергается целым
        // запросом («input is too large to process. increase the physical batch
        // size»). Поднимаем физический и логический батч до размера контекста, чтобы
        // принимать чанки целиком (для кириллицы/кода 512 токенов — это лишь ~сотни
        // символов, и крупные чанки переставали индексироваться).
        args.push("-ub".into());
        args.push(cfg.context_size.to_string());
        args.push("-b".into());
        args.push(cfg.context_size.to_string());
    }
    if cfg.no_mmap {
        args.push("--no-mmap".into());
    }
    args.extend(cfg.extra_args.iter().cloned());
    args
}

/// Владелец дочернего процесса `llama-server`. При `drop` процесс убивается
/// (сигнал `kill` → монитор-задача делает `start_kill`; плюс `kill_on_drop` как
/// подстраховка, если рантайм роняет монитор-задачу).
pub struct ServerHandle {
    /// Взводится при `drop`: монитор-задача (владелец [`Child`]) убивает процесс.
    kill: CancellationToken,
    /// Взводится монитор-задачей, когда дочерний процесс завершился сам (нормально
    /// или упав на загрузке — битый GGUF, нехватка памяти). Проба следит за ним,
    /// чтобы не ждать таймаут впустую.
    exited: CancellationToken,
    base_url: String,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // Монитор-задача владеет `Child`; сигналим ей убить процесс.
        self.kill.cancel();
    }
}

impl ServerHandle {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Сигнал «дочерний процесс завершился» — для пробы готовности
    /// ([`wait_until_ready`]): ловит ранний выход (битый GGUF/OOM) до таймаута.
    pub fn exited(&self) -> CancellationToken {
        self.exited.clone()
    }

    /// Запускает дочерний процесс `llama-server` (без ожидания готовности).
    pub fn launch(cfg: &ManagedConfig) -> Result<Self> {
        // Предполётная проверка файла модели. `spawn` ниже успешен даже при
        // отсутствующем GGUF — `llama-server` лишь потом падает на загрузке и
        // выходит, а фоновый probe (`wait_until_ready`) этого не замечает и
        // впустую ждёт до таймаута (минуты), держа UI в «подключение…». Поэтому
        // ловим самую частую причину здесь и сразу возвращаем понятную ошибку
        // (супервайзер превратит её в `ServerStatus::Disconnected`).
        if let Some(model) = &cfg.model_path
            && !std::path::Path::new(model).is_file()
        {
            bail!("файл модели не найден или недоступен: {model}");
        }

        let args = build_args(cfg);
        tracing::info!(binary = %cfg.binary.display(), ?args, "launching managed llama-server");

        let mut child = Command::new(&cfg.binary)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning llama-server at {}", cfg.binary.display()))?;

        // Читаем вывод процесса, чтобы (а) не переполнить пайп, (б) видеть прогресс загрузки.
        if let Some(out) = child.stdout.take() {
            tokio::spawn(forward_lines(out, false));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(forward_lines(err, true));
        }

        // Монитор-задача владеет `Child` и ждёт либо его выхода, либо сигнала на
        // убийство (`drop` хэндла). Ранний выход взводит `exited` — проба
        // готовности это видит и не висит до таймаута на мёртвом процессе.
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

/// Монитор-задача дочернего процесса: ждёт его завершения (взводит `exited`) или
/// сигнала `kill` (убивает процесс). Владеет [`Child`], поэтому `kill_on_drop`
/// сработает и при принудительном сбросе задачи рантаймом.
fn spawn_monitor(mut child: Child, kill: CancellationToken, exited: CancellationToken) {
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(s) => tracing::warn!(status = ?s, "managed llama-server завершился сам"),
                    Err(e) => tracing::warn!(error = %e, "ошибка ожидания дочернего llama-server"),
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

/// Ждёт готовности сервера, поллингом `probe` до таймаута. Свободная функция,
/// чтобы пробу можно было выполнять в фоне, не удерживая [`ServerHandle`].
///
/// `exited` (если задан) — сигнал раннего выхода дочернего процесса (managed):
/// при битом GGUF/нехватке памяти процесс умирает в ходе загрузки, и без этого
/// сигнала проба впустую опрашивала бы порт до таймаута (минуты). Для external
/// процесса нет — передаётся `None`.
pub async fn wait_until_ready(
    client: &OpenAiClient,
    timeout: Duration,
    exited: Option<CancellationToken>,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if client.probe().await.is_ok() {
            return Ok(());
        }
        // Дочерний процесс умер в ходе загрузки — не ждём таймаут.
        if exited.as_ref().is_some_and(|e| e.is_cancelled()) {
            bail!(
                "llama-server завершился до готовности (битый GGUF или нехватка памяти? — см. логи)"
            );
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("llama-server did not become ready within {timeout:?}");
        }
        // Спим до следующей пробы, но просыпаемся сразу, если процесс умер —
        // тогда следующая итерация увидит `exited` и завершится с ошибкой.
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

    fn base_cfg() -> ManagedConfig {
        ManagedConfig {
            binary: PathBuf::from("llama-server"),
            model_path: None,
            gpu_layers: 99,
            context_size: 8192,
            jinja: true,
            reasoning_format: None,
            embeddings: false,
            no_mmap: false,
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
        // Физический/логический батч подняты до размера контекста, иначе чанки
        // длиннее ~512 токенов отвергались бы сервером.
        let ub = args.iter().position(|a| a == "-ub").expect("есть -ub");
        assert_eq!(args[ub + 1], cfg.context_size.to_string());
        let b = args.iter().position(|a| a == "-b").expect("есть -b");
        assert_eq!(args[b + 1], cfg.context_size.to_string());
    }

    #[test]
    fn no_batch_flags_for_non_embedding_server() {
        // Chat-серверу батч-флаги эмбеддера не добавляем.
        let args = build_args(&base_cfg());
        assert!(!args.contains(&"-ub".to_string()));
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
        // Несуществующий GGUF → понятная ошибка ещё до spawn (без рантайма tokio),
        // вместо немого зависания probe в «подключение…» до таймаута.
        let cfg = ManagedConfig {
            model_path: Some("definitely/missing/model-xyz.gguf".into()),
            ..base_cfg()
        };
        let err = match ServerHandle::launch(&cfg) {
            Err(e) => e,
            Ok(_) => panic!("ожидалась ошибка отсутствующего файла модели"),
        };
        assert!(err.to_string().contains("файл модели"), "{err}");
    }

    #[tokio::test]
    async fn wait_until_ready_bails_on_early_exit() {
        // Дочерний процесс умер в ходе загрузки (взведён `exited`), порт мёртв —
        // проба не должна висеть до таймаута, а сразу вернуть понятную ошибку.
        let client = OpenAiClient::new("http://127.0.0.1:1/v1");
        let exited = CancellationToken::new();
        exited.cancel();
        let err = wait_until_ready(&client, Duration::from_secs(600), Some(exited))
            .await
            .expect_err("ожидалась ошибка раннего выхода");
        assert!(
            err.to_string().contains("завершился до готовности"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn monitor_cancels_exited_when_child_dies() {
        // Реальный кратко живущий процесс: монитор должен взвести `exited` по его
        // выходу. Кросс-платформенно: `cmd /C exit` на Windows, `sh -c` на unix.
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
            .expect("spawn короткоживущего процесса");

        let kill = CancellationToken::new();
        let exited = CancellationToken::new();
        spawn_monitor(child, kill, exited.clone());

        tokio::time::timeout(Duration::from_secs(5), exited.cancelled())
            .await
            .expect("монитор должен взвести exited по выходу процесса");
        assert!(exited.is_cancelled());
    }
}
