//! Live smokes of the managed launch against a real `llama-server`: a port a
//! stranger holds, a restart onto our own port, a load failure's one report
//! (docs/journal/engine.md, "a pod's failures say what they are").
//! `#[ignore]` — not in CI; silently skipped without their variables.
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out, and smokes that never run in CI would otherwise count as
//! production code nothing covers (docs/journal/quality.md).
//!
//! `MINDFORK_LLAMA_BIN=…/llama-server MINDFORK_EMBED_MODEL=…/bge-m3.gguf cargo test
//! supervisor::live_tests -- --ignored --nocapture --test-threads=1`.

use std::time::Duration;

use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;

use super::{LlamaSupervisor, MANAGED_READY_TIMEOUT, ServerSupervisor, managed_embed_config};
use crate::shared::api::managed::PortLedger;
use crate::shared::api::{OpenAiClient, ServerHandle, wait_until_ready};
use crate::shared::config::{
    EmbedSettings, EngineSettings, ManagedEmbedSettings, ManagedSettings, ServerMode,
};
use crate::shared::i18n::Locale;
use crate::shared::server::ServerStatus;

/// The reference (Russian) locale, as the supervisor's unit tests pin it.
fn ru() -> &'static Locale {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
}

/// A local stand-in for RunPod's nginx on 8001: answers every request with a
/// `404`, which the readiness probe reads as "alive and not loading".
async fn spawn_stranger() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (port, task)
}

/// The next status a live server's probe sends, waited for two minutes at most.
async fn next_status(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ServerStatus>,
) -> Option<ServerStatus> {
    tokio::time::timeout(Duration::from_secs(120), rx.recv())
        .await
        .expect("bounded")
}

/// The embedder settings the pod ran with, on `port`.
fn managed_embed_on(bin: &str, model: &str, port: u16) -> EmbedSettings {
    EmbedSettings {
        mode: ServerMode::Managed,
        managed: ManagedEmbedSettings {
            binary: Some(bin.to_string()),
            model_path: Some(model.to_string()),
            port,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The pod's busy port, against a real `llama-server` and a stranger that
/// answers like nginx. The control arm is the launch as it was: the real
/// child started anyway, the probe reads the stranger's `404` as a ready
/// server while the child dies on its bind — the start of the 17-relaunch
/// loop. Through the supervisor the launch is refused, nothing is spawned,
/// and the same settings on a free port come up.
///
///     MINDFORK_LLAMA_BIN=.../llama-server.exe MINDFORK_EMBED_MODEL=.../bge-m3.gguf \
///       cargo test a_stranger_on_the_port_is_refused_live -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a local llama-server binary + model (MINDFORK_LLAMA_BIN, MINDFORK_EMBED_MODEL)"]
async fn a_stranger_on_the_port_is_refused_live() {
    let (Ok(bin), Ok(model)) = (
        std::env::var("MINDFORK_LLAMA_BIN"),
        std::env::var("MINDFORK_EMBED_MODEL"),
    ) else {
        eprintln!("skip: MINDFORK_LLAMA_BIN / MINDFORK_EMBED_MODEL not set");
        return;
    };
    let (port, _stranger) = spawn_stranger().await;
    let settings = managed_embed_on(&bin, &model, port);

    // Control: the launch without the ledger.
    let cfg = managed_embed_config(&settings.managed, bin.clone().into());
    let handle = ServerHandle::launch(&cfg, ru()).expect("the old launch spawns");
    let client = OpenAiClient::new(handle.base_url());
    let probe = tokio::time::timeout(
        Duration::from_secs(60),
        wait_until_ready(&client, MANAGED_READY_TIMEOUT, Some(handle.exited()), ru()),
    )
    .await
    .expect("bounded");
    println!("control: the probe said {probe:?} with a stranger on the port");
    assert!(
        probe.is_ok(),
        "control: the stranger passes for a ready server"
    );
    tokio::time::timeout(Duration::from_secs(60), handle.exited().wait())
        .await
        .expect("control: our child dies on the bind");
    let died = handle
        .exited()
        .message(crate::shared::i18n::locale(crate::shared::i18n::Lang::En));
    println!("control: the child said {died}");
    drop(handle);

    // Through the supervisor: refused, nothing spawned.
    let supervisor = LlamaSupervisor::default();
    let (tx, mut rx) = unbounded_channel();
    let setup = supervisor.apply_embed(&settings, None, CancellationToken::new(), tx, ru());
    println!("refused: {:?}", setup.status);
    assert!(setup.handle.is_none(), "nothing is spawned");
    match &setup.status {
        ServerStatus::Disconnected(msg) => {
            assert!(
                msg.contains(&format!("порт {port} занят другой программой")),
                "{msg}"
            )
        }
        other => panic!("expected the busy-port refusal, got {other:?}"),
    }
    assert!(rx.recv().await.is_none(), "no probe behind a refusal");

    // The same settings on a free port come up.
    let free = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (tx, mut rx) = unbounded_channel();
    let setup = supervisor.apply_embed(
        &managed_embed_on(&bin, &model, free),
        None,
        CancellationToken::new(),
        tx,
        ru(),
    );
    let _handle = setup.handle.expect("a free port is launched onto");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(120), rx.recv())
            .await
            .expect("bounded"),
        Some(ServerStatus::Ready)
    );
}

/// A restart, as the orchestrator makes it: the old handle is dropped and the
/// new server launched at once, onto the port the old process still holds —
/// on a current-thread runtime the monitor task cannot even have run the
/// kill yet. The ledger lets its own server through; the control — a ledger
/// that never launched it — calls the port a stranger's.
///
///     MINDFORK_LLAMA_BIN=.../llama-server.exe MINDFORK_EMBED_MODEL=.../bge-m3.gguf \
///       cargo test a_restart_onto_our_own_port_goes_through_live -- --ignored --nocapture
#[tokio::test]
#[ignore = "requires a local llama-server binary + model (MINDFORK_LLAMA_BIN, MINDFORK_EMBED_MODEL)"]
async fn a_restart_onto_our_own_port_goes_through_live() {
    let (Ok(bin), Ok(model)) = (
        std::env::var("MINDFORK_LLAMA_BIN"),
        std::env::var("MINDFORK_EMBED_MODEL"),
    ) else {
        eprintln!("skip: MINDFORK_LLAMA_BIN / MINDFORK_EMBED_MODEL not set");
        return;
    };
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let settings = managed_embed_on(&bin, &model, port);
    let supervisor = LlamaSupervisor::default();

    let (tx, mut rx) = unbounded_channel();
    let first = supervisor.apply_embed(&settings, None, CancellationToken::new(), tx, ru());
    assert_eq!(
        next_status(&mut rx).await,
        Some(ServerStatus::Ready),
        "first launch"
    );

    drop(first.handle); // the kill is asked for, not yet done
    let cfg = managed_embed_config(&settings.managed, bin.clone().into());
    let control = PortLedger::default().launch(&cfg, ru());
    println!(
        "control: {:?}",
        control.as_ref().err().map(ToString::to_string)
    );
    assert!(
        control.is_err(),
        "control: the old process still holds the port"
    );

    let (tx, mut rx) = unbounded_channel();
    let second = supervisor.apply_embed(&settings, None, CancellationToken::new(), tx, ru());
    assert_eq!(
        second.status,
        ServerStatus::Connecting,
        "{:?}",
        second.status
    );
    assert_eq!(
        next_status(&mut rx).await,
        Some(ServerStatus::Ready),
        "the restart"
    );
}

/// A server that dies while loading — a file that is not a GGUF (M10 of
/// docs/research/managed-extra-args.md) — is reported once, in the words it
/// logged as it died, not the generic guess.
///
///     MINDFORK_LLAMA_BIN=.../llama-server.exe \
///       cargo test a_load_failure_is_reported_once_with_its_cause_live -- --ignored --nocapture
#[tokio::test]
#[ignore = "requires a local llama-server binary (MINDFORK_LLAMA_BIN)"]
async fn a_load_failure_is_reported_once_with_its_cause_live() {
    let Ok(bin) = std::env::var("MINDFORK_LLAMA_BIN") else {
        eprintln!("skip: MINDFORK_LLAMA_BIN not set");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let fake = dir.path().join("fake.gguf");
    std::fs::write(&fake, b"this is not a gguf file").expect("write");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let settings = EngineSettings {
        mode: ServerMode::Managed,
        managed: ManagedSettings {
            binary: Some(bin),
            model_path: Some(fake.display().to_string()),
            port,
            ..Default::default()
        },
        ..Default::default()
    };
    let (tx, mut rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_chat(
        &settings,
        None,
        CancellationToken::new(),
        tx,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::En),
    );
    let _handle = setup.handle.expect("spawned");
    let mut seen = Vec::new();
    while let Some(status) = tokio::time::timeout(Duration::from_secs(60), rx.recv())
        .await
        .expect("the probe must end")
    {
        seen.push(status);
    }
    println!("reported: {seen:?}");
    match seen.as_slice() {
        [ServerStatus::Disconnected(msg)] => assert!(
            msg.starts_with("llama-server stopped with an error: "),
            "the logged cause, not the generic guess: {msg}"
        ),
        other => panic!("expected exactly one Disconnected, got {other:?}"),
    }
}
