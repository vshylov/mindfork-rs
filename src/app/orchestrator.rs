//! Оркестратор: единственный владелец доменного состояния и автомат генерации.
//! Принимает [`AppCommand`], исполняет генерацию (в отдельной задаче) и
//! рассылает [`AppEvent`]. См. spec §4.4 (однонаправленный поток, `generation_id`,
//! автомат `Idle/Generating/Cancelling`).
//!
//! На M1 история — в памяти (без персистентности; это приходит на M2).

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppCommand, AppEvent, ServerStatus};
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason};

/// Параметры запуска оркестратора.
pub struct OrchestratorDeps {
    pub cmd_rx: UnboundedReceiver<AppCommand>,
    pub evt_tx: UnboundedSender<AppEvent>,
    pub backend: Option<Arc<dyn EngineBackend>>,
    pub sampling: SamplingConfig,
    pub status: ServerStatus,
}

/// Состояние генерации (автомат на чат).
enum State {
    Idle,
    Generating { id: Uuid, cancel: CancellationToken },
    Cancelling { id: Uuid },
}

impl State {
    fn current_id(&self) -> Option<Uuid> {
        match self {
            State::Idle => None,
            State::Generating { id, .. } | State::Cancelling { id } => Some(*id),
        }
    }
}

/// Результат завершившейся задачи генерации (внутренний канал).
struct GenResult {
    id: Uuid,
    text: String,
}

/// Главный цикл оркестратора. Завершается, когда закрыт канал команд или
/// получена [`AppCommand::Quit`].
pub async fn run(deps: OrchestratorDeps) {
    let OrchestratorDeps {
        mut cmd_rx,
        evt_tx,
        backend,
        sampling,
        status,
    } = deps;

    let _ = evt_tx.send(AppEvent::ServerStatus(status));

    let mut history: Vec<ApiMessage> = Vec::new();
    let mut state = State::Idle;
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<GenResult>();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    AppCommand::Quit => {
                        if let State::Generating { cancel, .. } = &state {
                            cancel.cancel();
                        }
                        break;
                    }
                    AppCommand::Cancel => {
                        if let State::Generating { id, cancel } = &state {
                            cancel.cancel();
                            state = State::Cancelling { id: *id };
                        }
                    }
                    AppCommand::SendMessage(text) => {
                        handle_send(
                            text,
                            &mut state,
                            &mut history,
                            &backend,
                            &sampling,
                            &evt_tx,
                            &done_tx,
                        );
                    }
                }
            }
            done = done_rx.recv() => {
                let Some(res) = done else { continue };
                // Применяем только результат текущей генерации (защита от устаревших).
                if state.current_id() == Some(res.id) {
                    if !res.text.is_empty() {
                        history.push(ApiMessage::assistant(res.text));
                    }
                    state = State::Idle;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_send(
    text: String,
    state: &mut State,
    history: &mut Vec<ApiMessage>,
    backend: &Option<Arc<dyn EngineBackend>>,
    sampling: &SamplingConfig,
    evt_tx: &UnboundedSender<AppEvent>,
    done_tx: &UnboundedSender<GenResult>,
) {
    // Валидация по состоянию (defense in depth) — генерируем только из Idle.
    if !matches!(state, State::Idle) {
        return;
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    let Some(backend) = backend.clone() else {
        let _ = evt_tx.send(AppEvent::Error("LLM-сервер не настроен".into()));
        return;
    };

    history.push(ApiMessage::user(&text));
    let _ = evt_tx.send(AppEvent::UserMessage(text));

    let id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    let _ = evt_tx.send(AppEvent::GenerationStarted { generation_id: id });

    let req = ChatRequest {
        system: None,
        messages: history.clone(),
        sampling: sampling.clone(),
    };
    *state = State::Generating {
        id,
        cancel: cancel.clone(),
    };

    spawn_generation(backend, req, cancel, id, evt_tx.clone(), done_tx.clone());
}

fn spawn_generation(
    backend: Arc<dyn EngineBackend>,
    req: ChatRequest,
    cancel: CancellationToken,
    id: Uuid,
    evt_tx: UnboundedSender<AppEvent>,
    done_tx: UnboundedSender<GenResult>,
) {
    tokio::spawn(async move {
        let mut text = String::new();
        match backend.chat_stream(req, cancel).await {
            Ok(mut stream) => {
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        ChatChunk::Text(t) => {
                            text.push_str(&t);
                            let _ = evt_tx.send(AppEvent::Chunk {
                                generation_id: id,
                                text: t,
                            });
                        }
                        ChatChunk::Thoughts(t) => {
                            let _ = evt_tx.send(AppEvent::Thoughts {
                                generation_id: id,
                                text: t,
                            });
                        }
                        ChatChunk::Finished(reason) => {
                            let _ = evt_tx.send(AppEvent::Finished {
                                generation_id: id,
                                reason,
                            });
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                let _ = evt_tx.send(AppEvent::Error(format!("Ошибка генерации: {err}")));
                let _ = evt_tx.send(AppEvent::Finished {
                    generation_id: id,
                    reason: FinishReason::Error,
                });
            }
        }
        let _ = done_tx.send(GenResult { id, text });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::mock::MockBackend;
    use tokio::sync::mpsc::unbounded_channel;

    fn spawn_orch(
        backend: Option<Arc<dyn EngineBackend>>,
    ) -> (
        UnboundedSender<AppCommand>,
        UnboundedReceiver<AppEvent>,
        tokio::task::JoinHandle<()>,
    ) {
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, evt_rx) = unbounded_channel();
        let deps = OrchestratorDeps {
            cmd_rx,
            evt_tx,
            backend,
            sampling: SamplingConfig::default(),
            status: ServerStatus::Ready,
        };
        let handle = tokio::spawn(run(deps));
        (cmd_tx, evt_rx, handle)
    }

    #[tokio::test]
    async fn send_streams_text_and_returns_to_idle() {
        let backend = Arc::new(MockBackend::scripted(vec![
            ChatChunk::Thoughts("thinking".into()),
            ChatChunk::Text("Hello".into()),
            ChatChunk::Text(", world".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ])) as Arc<dyn EngineBackend>;
        let (cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));

        cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();

        let mut text = String::new();
        let mut thoughts = String::new();
        let (mut user, mut started, mut finished) = (false, false, false);
        while let Some(ev) = evt_rx.recv().await {
            match ev {
                AppEvent::UserMessage(m) => {
                    assert_eq!(m, "hi");
                    user = true;
                }
                AppEvent::GenerationStarted { .. } => started = true,
                AppEvent::Chunk { text: t, .. } => text.push_str(&t),
                AppEvent::Thoughts { text: t, .. } => thoughts.push_str(&t),
                AppEvent::Finished { reason, .. } => {
                    assert_eq!(reason, FinishReason::Stop);
                    finished = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(user && started && finished);
        assert_eq!(text, "Hello, world");
        assert_eq!(thoughts, "thinking");

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cancel_stops_generation() {
        let backend = Arc::new(MockBackend::cancellable(vec![ChatChunk::Text(
            "part".into(),
        )])) as Arc<dyn EngineBackend>;
        let (cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));

        cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();

        // Дождаться первого чанка, затем отменить.
        let mut cancelled = false;
        let mut sent_cancel = false;
        while let Some(ev) = evt_rx.recv().await {
            match ev {
                AppEvent::Chunk { .. } if !sent_cancel => {
                    cmd_tx.send(AppCommand::Cancel).unwrap();
                    sent_cancel = true;
                }
                AppEvent::Finished { reason, .. } => {
                    assert_eq!(reason, FinishReason::Cancelled);
                    cancelled = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(cancelled);

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn send_without_backend_emits_error() {
        let (cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();

        let mut got_error = false;
        // Первое событие — ServerStatus; затем ожидаем Error.
        while let Some(ev) = evt_rx.recv().await {
            if let AppEvent::Error(_) = ev {
                got_error = true;
                break;
            }
        }
        assert!(got_error);

        drop(cmd_tx);
        handle.await.unwrap();
    }
}
