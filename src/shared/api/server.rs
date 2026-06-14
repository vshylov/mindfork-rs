//! Супервайзер локального сервера xinfer (managed-режим): запуск дочернего
//! процесса, ожидание готовности, остановка при завершении. См. spec §3.4 и
//! docs/xinfer-contract.md §2.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::client::XinferClient;

/// Переменная окружения, включающая раздельную отдачу reasoning в
/// `delta.reasoning_content`. См. docs/xinfer-contract.md §2.
const STREAM_AS_REASONING_ENV: &str = "XINFER_STREAM_AS_REASONING_CONTENT";

/// Конфигурация запуска managed-сервера xinfer.
#[derive(Debug, Clone)]
pub struct ManagedConfig {
    pub binary: PathBuf,
    pub model_id: Option<String>,
    pub weight_path: Option<String>,
    pub weight_file: Option<String>,
    pub isq: Option<String>,
    pub device_ids: Vec<usize>,
    pub cpu: bool,
    pub port: u16,
    /// Дополнительные сырые аргументы (например `--kvcache_dtype turbo4`).
    pub extra_args: Vec<String>,
}

impl ManagedConfig {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

/// Аргументы командной строки xinfer из конфигурации (чистая функция).
/// Флаги — по docs/xinfer-contract.md §2.
pub fn build_args(cfg: &ManagedConfig) -> Vec<String> {
    let mut args = vec![
        "--server".to_string(),
        "--port".to_string(),
        cfg.port.to_string(),
    ];
    if let Some(m) = &cfg.model_id {
        args.push("--m".into());
        args.push(m.clone());
    }
    if let Some(w) = &cfg.weight_path {
        args.push("--w".into());
        args.push(w.clone());
    }
    if let Some(f) = &cfg.weight_file {
        args.push("--f".into());
        args.push(f.clone());
    }
    if let Some(isq) = &cfg.isq {
        args.push("--isq".into());
        args.push(isq.clone());
    }
    if cfg.cpu {
        args.push("--cpu".into());
    } else if !cfg.device_ids.is_empty() {
        args.push("--d".into());
        args.push(
            cfg.device_ids
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    args.extend(cfg.extra_args.iter().cloned());
    args
}

/// Владелец дочернего процесса xinfer. При `drop` процесс убивается
/// (`kill_on_drop`).
pub struct ServerHandle {
    _child: Child,
    base_url: String,
}

impl ServerHandle {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Запускает дочерний процесс xinfer (без ожидания готовности).
    pub fn launch(cfg: &ManagedConfig) -> Result<Self> {
        let args = build_args(cfg);
        tracing::info!(binary = %cfg.binary.display(), ?args, "launching managed xinfer server");

        let mut child = Command::new(&cfg.binary)
            .args(&args)
            .env(STREAM_AS_REASONING_ENV, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning xinfer at {}", cfg.binary.display()))?;

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
pub async fn wait_until_ready(client: &XinferClient, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if client.probe().await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("xinfer server did not become ready within {timeout:?}");
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
            tracing::warn!(target: "xinfer", "{line}");
        } else {
            tracing::info!(target: "xinfer", "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_cfg() -> ManagedConfig {
        ManagedConfig {
            binary: PathBuf::from("xinfer"),
            model_id: None,
            weight_path: None,
            weight_file: None,
            isq: None,
            device_ids: vec![],
            cpu: false,
            port: 8000,
            extra_args: vec![],
        }
    }

    #[test]
    fn args_include_server_and_port() {
        let args = build_args(&base_cfg());
        assert!(args.contains(&"--server".to_string()));
        let p = args.iter().position(|a| a == "--port").unwrap();
        assert_eq!(args[p + 1], "8000");
    }

    #[test]
    fn args_for_gguf_model_with_gpu() {
        let cfg = ManagedConfig {
            model_id: Some("Qwen/Qwen3-8B".into()),
            isq: Some("q4k".into()),
            device_ids: vec![0, 1],
            ..base_cfg()
        };
        let args = build_args(&cfg);
        let m = args.iter().position(|a| a == "--m").unwrap();
        assert_eq!(args[m + 1], "Qwen/Qwen3-8B");
        let d = args.iter().position(|a| a == "--d").unwrap();
        assert_eq!(args[d + 1], "0,1");
        let isq = args.iter().position(|a| a == "--isq").unwrap();
        assert_eq!(args[isq + 1], "q4k");
    }

    #[test]
    fn cpu_flag_excludes_device_ids() {
        let cfg = ManagedConfig {
            cpu: true,
            device_ids: vec![0],
            ..base_cfg()
        };
        let args = build_args(&cfg);
        assert!(args.contains(&"--cpu".to_string()));
        assert!(!args.contains(&"--d".to_string()));
    }

    #[test]
    fn base_url_uses_port() {
        assert_eq!(base_cfg().base_url(), "http://127.0.0.1:8000/v1");
    }
}
