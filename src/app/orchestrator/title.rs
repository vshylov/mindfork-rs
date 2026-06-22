//! Авто-название чата (spec §11.2): фоновая задача просит модель придумать
//! короткий заголовок по переписке; результат применяется к чату в петле.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest, EngineBackend};

use super::Orchestrator;

/// Потолок токенов ответа при генерации авто-названия. Пытаемся выключить «мысли»
/// (`reasoning_budget=0` + `chat_template_kwargs.enable_thinking=false`), но
/// некоторые модели (вшитый в GGUF thinking, напр. Gemma `peg-gemma4`) их
/// игнорируют и всё равно «рассуждают» сотни токенов перед ответом — поэтому
/// бюджет щедрый, чтобы модель успела завершить «мысли» и выдать заголовок.
const TITLE_MAX_TOKENS: usize = 2048;

/// Лимит времени на генерацию авто-названия чата (с запасом на «думающие» модели).
const TITLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Результат фоновой задачи авто-названия чата (внутренний канал).
pub(super) struct TitleResult {
    pub(super) chat_id: Uuid,
    /// Сырой текст ответа модели (или сообщение об ошибке для показа в UI).
    pub(super) text: Result<String, String>,
}

impl Orchestrator {
    /// Авто-название чата (spec §11.2): модель читает переписку (или её начало и
    /// конец, если она длинная) и придумывает короткий заголовок. Запрос идёт
    /// фоновой задачей; результат прилетает в [`Orchestrator::handle_title_result`].
    /// Чат-сервер должен быть готов (`Ready`) — иначе понятная ошибка.
    pub(super) fn handle_auto_rename(&mut self, id: Uuid) {
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        let Some(digest) = crate::features::rename_chat::build_conversation_digest(&chat.messages)
        else {
            let _ = self.evt_tx.send(AppEvent::ChatListError(
                "Недостаточно сообщений для авто-названия".into(),
            ));
            return;
        };
        let backend = match self.engines.backend_if_ready() {
            Ok(backend) => backend,
            Err(msg) => {
                // Авто-название — операция списка чатов: ошибку готовности сервера
                // показываем в оверлее списка, а не в ленте чата (где её скрыл бы
                // полноэкранный оверлей).
                let _ = self.evt_tx.send(AppEvent::ChatListError(msg));
                return;
            }
        };
        // Свежий компактный семплинг (не наследуем override чата): короткий ответ,
        // умеренная температура, reasoning выключен (заголовку «мысли» не нужны и
        // только съедают бюджет токенов), без инструментов. Ключевое — `reasoning_
        // budget=0`: для моделей со «вшитым» в шаблон thinking (Gemma `peg-gemma4`,
        // Qwen) только он реально гасит «мысли»; поля `thinking`/`reasoning_effort`
        // сервер для таких шаблонов игнорирует (иначе модель тратила весь бюджет на
        // «мысли» и ответный текст приходил пустым).
        let sampling = SamplingConfig {
            max_tokens: Some(TITLE_MAX_TOKENS),
            temperature: Some(0.3),
            thinking: Some(false),
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(crate::features::rename_chat::TITLE_SYSTEM_MESSAGE.to_string()),
            messages: vec![ApiMessage::user(digest)],
            sampling,
            tools: Vec::new(),
        };
        spawn_title(backend, request, id, self.title_tx.clone());
    }

    /// Применяет результат фоновой генерации авто-названия: чистит/нормализует
    /// заголовок и переименовывает чат (или показывает ошибку).
    pub(super) fn handle_title_result(&mut self, res: TitleResult) {
        match res.text {
            Ok(raw) => {
                let Some(title) = crate::features::rename_chat::clean_generated_title(&raw) else {
                    let _ = self.evt_tx.send(AppEvent::ChatListError(
                        "Модель не вернула название чата".into(),
                    ));
                    return;
                };
                if let Some(chat) = self.chat_mut(res.chat_id) {
                    chat.title = title.clone();
                    self.mark_dirty(res.chat_id);
                    self.emit_chat_list();
                    let _ = self.evt_tx.send(AppEvent::ChatRenamed {
                        id: res.chat_id,
                        title,
                    });
                }
            }
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(msg));
            }
        }
    }
}

/// Запускает фоновую задачу авто-названия чата: один независимый запрос к модели
/// (без истории/инструментов), сбор текста, отправка результата в `title_tx`.
/// Лимит времени — [`TITLE_TIMEOUT`].
fn spawn_title(
    backend: Arc<dyn EngineBackend>,
    request: ChatRequest,
    chat_id: Uuid,
    title_tx: UnboundedSender<TitleResult>,
) {
    tokio::spawn(async move {
        let cancel = CancellationToken::new();
        let collect = async {
            let mut stream = backend.chat_stream(request, cancel.clone()).await?;
            let mut text = String::new();
            let mut thoughts = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    // Копим «мысли» как запасной источник: если модель так и не
                    // «завершила мысль» (выдала только reasoning), вытащим заголовок
                    // из последней содержательной строки рассуждений.
                    ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                    ChatChunk::Finished(_) => break,
                    ChatChunk::ToolCall(_) | ChatChunk::Usage(_) => {}
                }
            }
            Ok::<(String, String), anyhow::Error>((text, thoughts))
        };
        let text = match tokio::time::timeout(TITLE_TIMEOUT, collect).await {
            Ok(Ok((text, thoughts))) => Ok(salvage_title_source(text, thoughts)),
            Ok(Err(err)) => Err(format!("Ошибка генерации названия: {err}")),
            Err(_) => {
                cancel.cancel();
                Err("Генерация названия превысила лимит времени".to_string())
            }
        };
        let _ = title_tx.send(TitleResult { chat_id, text });
    });
}

/// Выбирает сырой источник заголовка: основной ответ модели, а если он пуст
/// (модель не «завершила мысль» в рамках бюджета) — последнюю содержательную
/// строку рассуждений. Финальную нормализацию делает `clean_generated_title`.
pub(super) fn salvage_title_source(text: String, thoughts: String) -> String {
    if !text.trim().is_empty() {
        return text;
    }
    thoughts
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}
