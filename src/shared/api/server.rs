//! Запуск локального `llama-server` (llama.cpp, managed-режим): дочерний процесс,
//! ожидание готовности, остановка при завершении (`kill_on_drop`). См. spec §3.4.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

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
    }
    if cfg.no_mmap {
        args.push("--no-mmap".into());
    }
    args.extend(cfg.extra_args.iter().cloned());
    args
}

/// Владелец дочернего процесса `llama-server`. При `drop` процесс убивается
/// (`kill_on_drop`).
pub struct ServerHandle {
    _child: Child,
    base_url: String,
}

impl ServerHandle {
    pub fn base_url(&self) -> &str {
        &self.base_url
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

        Ok(Self {
            _child: child,
            base_url: cfg.base_url(),
        })
    }
}

/// Ждёт готовности сервера, поллингом `probe` до таймаута. Свободная функция,
/// чтобы пробу можно было выполнять в фоне, не удерживая [`ServerHandle`].
pub async fn wait_until_ready(client: &OpenAiClient, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if client.probe().await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("llama-server did not become ready within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
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
}
