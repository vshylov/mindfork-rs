//! Озвучивание сообщений чата (команда `/tts [N|all|stop]`, spec §11.9).
//!
//! Устроено по образцу [`super::rag`]: команда → снимок данных у оркестратора
//! (он единственный владелец `Chat`) → отменяемая фоновая задача. Задача идёт
//! **конвейером**: пока играет чанк N, синтезируется N+1 — первый звук приходит
//! быстро, а вперёд синтезируется не больше одного чанка (не платим за то, что
//! пользователь оборвёт). Чат при этом не мутируется, генерация не гейтится:
//! озвучивается **снимок** переписки на момент команды.
//!
//! Точки остановки собраны в один хелпер [`Orchestrator::stop_tts`] (прецедент —
//! `reset_rag_cancel`): две по настройке (переключение чата, начало генерации) и
//! три **безусловные** — удаление обмена, перегенерация, удаление чата: текста,
//! который озвучивается, больше не существует. См. docs/research/tts.md §7, §8.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::message::{Message, MessageRole};
use crate::features::tts_command::TtsScope;
use crate::shared::i18n::Locale;
use crate::shared::tts::{TtsEngine, TtsSetupError, engine_from_config, playback::Playback};

use super::Orchestrator;

/// Сколько чанков держать в очереди воспроизведения (играющий + один готовый).
/// Больше — платим за синтез, который может не понадобиться; меньше — рискуем
/// паузой между чанками.
const QUEUE_AHEAD: usize = 2;

/// Пауза опроса очереди воспроизведения (у `rodio` нет асинхронного уведомления
/// «очередь опустела», а `sleep_until_end` блокирующий).
const POLL_INTERVAL: Duration = Duration::from_millis(80);

impl Orchestrator {
    /// Озвучивает сообщения активного чата (команда `/tts`, `/tts N`, `/tts all`).
    pub(super) fn handle_tts(&mut self, scope: TtsScope) {
        // Новая команда всегда прерывает прежнее воспроизведение.
        self.stop_tts();
        let Some(chat) = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
        else {
            self.fail_tts(self.ui_locale().t("ui.err.tts_no_active_chat"));
            return;
        };
        // Пометки и префиксы ролей — речевой контент, поэтому на языке **профиля**
        // (ось A, docs/history/i18n.md), а не интерфейса.
        let speech_loc = self.profile_locale(chat.profile_id);
        let chunks = match build_utterances(
            &chat.messages,
            scope,
            self.config.tts.speak_roles,
            speech_loc,
        ) {
            Some(text) => text,
            None => {
                self.fail_tts(self.ui_locale().t("ui.err.tts_nothing_to_speak"));
                return;
            }
        };

        // Клиент строим из снимка настроек: сохранённый ключ провайдера общий с
        // чатом (ADR 0008) — вводить его заново не нужно.
        let stored = self
            .config
            .tts
            .mode
            .cloud_provider()
            .and_then(|p| crate::shared::secrets::stored_key(&self.config.api_keys, p.key()));
        let engine = match engine_from_config(&self.config.tts, stored) {
            Ok(engine) => engine,
            Err(err) => {
                self.fail_tts(self.ui_locale().t(setup_error_key(err)));
                return;
            }
        };
        let chunks = chunk_utterances(&chunks, engine.max_input_chars());
        if chunks.is_empty() {
            self.fail_tts(self.ui_locale().t("ui.err.tts_nothing_to_speak"));
            return;
        }

        let cancel = CancellationToken::new();
        let task_id = Uuid::new_v4();
        self.tts_cancel = Some(cancel.clone());
        self.tts_gen = Some(task_id);
        let _ = self.evt_tx.send(AppEvent::TtsActive(true));
        spawn_tts(TtsTask {
            engine,
            chunks,
            cancel,
            task_id,
            loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
            done_tx: self.tts_done_tx.clone(),
        });
    }

    /// Останавливает озвучивание (команда `/tts stop` и все точки остановки).
    /// Идемпотентно: если ничего не играет — no-op.
    pub(super) fn stop_tts(&mut self) {
        if let Some(token) = self.tts_cancel.take() {
            token.cancel();
        }
        // Гасим чип сразу и забываем поколение: поздний `done` уже отменённой
        // задачи будет отброшен (иначе он погасил бы чип новой озвучки).
        if self.tts_gen.take().is_some() {
            let _ = self.evt_tx.send(AppEvent::TtsActive(false));
        }
    }

    /// Фоновая задача озвучивания завершилась сама (доиграла/ошибка). Гасим чип,
    /// только если это текущая задача.
    pub(super) fn handle_tts_done(&mut self, task_id: Uuid) {
        if self.tts_gen == Some(task_id) {
            self.tts_gen = None;
            self.tts_cancel = None;
            let _ = self.evt_tx.send(AppEvent::TtsActive(false));
        }
    }

    /// Сообщает об ошибке озвучивания заметкой в ленту (язык интерфейса, ось B).
    fn fail_tts(&self, msg: &str) {
        let _ = self.evt_tx.send(AppEvent::Error(msg.to_string()));
    }
}

/// Ключ бандла для структурной ошибки настройки озвучивания.
fn setup_error_key(err: TtsSetupError) -> &'static str {
    match err {
        TtsSetupError::Model => "ui.err.tts_no_model",
        TtsSetupError::ApiKey => "ui.err.tts_no_api_key",
        TtsSetupError::Url => "ui.err.tts_no_url",
    }
}

/// Отбирает сообщения по объёму команды и превращает их в реплики для синтеза.
///
/// Что считается сообщением: `user`/`assistant` с непустым текстом (system/tool
/// пропускаются — те же правила, что в `F5`-экспорте). Порядок хронологический.
/// `None` — озвучивать нечего (пустой чат / только служебные сообщения).
pub(super) fn build_utterances(
    messages: &[Message],
    scope: TtsScope,
    speak_roles: bool,
    loc: &'static Locale,
) -> Option<Vec<String>> {
    let spoken: Vec<&Message> = messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .filter(|m| !m.text.trim().is_empty())
        .collect();
    let take = match scope {
        TtsScope::Last => 1,
        TtsScope::Recent(n) => n,
        TtsScope::All => spoken.len(),
    };
    let start = spoken.len().saturating_sub(take);
    let out: Vec<String> = spoken[start..]
        .iter()
        .filter_map(|m| {
            // «Мысли» (CoT) и tool-блоки не озвучиваются никогда: первые лежат в
            // отдельном поле `Message.thoughts`, вторых нет в `text`.
            let text = crate::shared::markdown::speakable_text(&m.text, loc);
            if text.is_empty() {
                return None;
            }
            if !speak_roles {
                return Some(text);
            }
            let role = match m.role {
                MessageRole::User => loc.t("speak.role.user"),
                _ => loc.t("speak.role.assistant"),
            };
            Some(format!("{role} {text}"))
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

/// Режет реплики на чанки не длиннее `max_chars` символов — по границам
/// предложений (переиспользуем чанкер RAG), чтобы стык чанков не приходился на
/// середину фразы. Реплики не склеиваются между собой: граница сообщения — это
/// и естественная пауза, и точка отмены.
pub(super) fn chunk_utterances(utterances: &[String], max_chars: usize) -> Vec<String> {
    let max = max_chars.max(1);
    let mut out = Vec::new();
    for utterance in utterances {
        for block in utterance.lines() {
            let block = block.trim();
            if block.is_empty() {
                continue;
            }
            pack_sentences(block, max, &mut out);
        }
    }
    out
}

/// Упаковывает предложения блока в чанки до `max` символов.
fn pack_sentences(block: &str, max: usize, out: &mut Vec<String>) {
    let mut cur = String::new();
    for sentence in crate::features::tools::rag::split_sentences(block) {
        for piece in split_long(&sentence, max) {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            let extra = if cur.is_empty() { 0 } else { 1 };
            if !cur.is_empty() && cur.chars().count() + extra + piece.chars().count() > max {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(piece);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
}

/// Режет по символам предложение, которое само длиннее лимита (редкий случай —
/// текст без пунктуации). Граница слова предпочтительнее середины слова.
fn split_long(sentence: &str, max: usize) -> Vec<String> {
    if sentence.chars().count() <= max {
        return vec![sentence.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in sentence.split_whitespace() {
        let wlen = word.chars().count();
        if wlen > max {
            // Слово длиннее лимита — режем посимвольно (иначе чанк не влезет).
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let chars: Vec<char> = word.chars().collect();
            for part in chars.chunks(max) {
                out.push(part.iter().collect());
            }
            continue;
        }
        let extra = if cur.is_empty() { 0 } else { 1 };
        if cur.chars().count() + extra + wlen > max {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Параметры фоновой задачи озвучивания.
struct TtsTask {
    engine: Box<dyn TtsEngine>,
    chunks: Vec<String>,
    cancel: CancellationToken,
    /// Поколение задачи — по нему оркестратор отличает свой `done` от устаревшего.
    task_id: Uuid,
    /// Язык интерфейса (ось B) — тексты ошибок для человека.
    loc: &'static Locale,
    evt_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
    done_tx: tokio::sync::mpsc::UnboundedSender<Uuid>,
}

/// Запускает фоновую озвучку: открывает аудио-устройство, затем синтезирует чанки
/// в очередь воспроизведения, придерживая синтез, пока очередь заполнена.
fn spawn_tts(task: TtsTask) {
    let TtsTask {
        engine,
        chunks,
        cancel,
        task_id,
        loc,
        evt_tx,
        done_tx,
    } = task;

    tokio::spawn(async move {
        let fail = |msg: String| {
            let _ = evt_tx.send(AppEvent::Error(msg));
        };
        // Устройство открываем **лениво** (по команде, а не на старте приложения):
        // на Linux libasound шумит в stderr при энумерации (cpal#384), а stdout/
        // stderr заняты TUI. Нет звука → понятная заметка, не паника.
        let playback = match Playback::open() {
            Ok(p) => p,
            Err(err) => {
                fail(loc.tf("ui.err.tts_no_audio", &[("err", &err.to_string())]));
                let _ = done_tx.send(task_id);
                return;
            }
        };

        for chunk in chunks {
            if cancel.is_cancelled() {
                break;
            }
            // Конвейер: не синтезируем вперёд больше, чем нужно очереди.
            while playback.queued() >= QUEUE_AHEAD && !cancel.is_cancelled() {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            if cancel.is_cancelled() {
                break;
            }
            match engine.synthesize(&chunk, &cancel).await {
                Ok(clip) if !clip.is_empty() => {
                    if let Err(err) = playback.enqueue(clip) {
                        fail(loc.tf("ui.err.tts_playback", &[("err", &err.to_string())]));
                        break;
                    }
                }
                // Пустой клип — просто нечего играть (не ошибка).
                Ok(_) => {}
                Err(err) => {
                    // Отмена — не ошибка: пользователь сам остановил.
                    if !cancel.is_cancelled() {
                        fail(loc.tf("ui.err.tts_synth", &[("err", &err.to_string())]));
                    }
                    break;
                }
            }
        }

        // Дожидаемся, пока очередь доиграет (или отмены).
        while !playback.is_drained() && !cancel.is_cancelled() {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        if cancel.is_cancelled() {
            playback.stop();
        }
        let _ = done_tx.send(task_id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    fn chat_messages() -> Vec<Message> {
        vec![
            Message::user("первое от пользователя"),
            Message::assistant("первый ответ"),
            Message::user("второе от пользователя"),
            Message::assistant("второй ответ"),
        ]
    }

    #[test]
    fn last_scope_takes_only_final_message() {
        let out = build_utterances(&chat_messages(), TtsScope::Last, false, ru()).unwrap();
        assert_eq!(out, vec!["второй ответ.".to_string()]);
    }

    #[test]
    fn recent_scope_takes_tail_in_chronological_order() {
        let out = build_utterances(&chat_messages(), TtsScope::Recent(3), false, ru()).unwrap();
        assert_eq!(out.len(), 3);
        assert!(
            out[0].contains("первый ответ"),
            "порядок хронологический: {out:?}"
        );
        assert!(out[2].contains("второй ответ"));
        // Запрос больше, чем есть, отдаёт всё (кламп, а не ошибка).
        let all = build_utterances(&chat_messages(), TtsScope::Recent(99), false, ru()).unwrap();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn all_scope_takes_everything_spoken() {
        let out = build_utterances(&chat_messages(), TtsScope::All, false, ru()).unwrap();
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn service_messages_and_empty_text_are_skipped() {
        let messages = vec![
            Message::new(MessageRole::System, "системная инструкция"),
            Message::user("   "),
            Message::new(MessageRole::Tool, "результат инструмента"),
            Message::assistant("настоящий ответ"),
        ];
        let out = build_utterances(&messages, TtsScope::All, false, ru()).unwrap();
        assert_eq!(out.len(), 1, "озвучиваются только user/assistant: {out:?}");
        assert!(out[0].contains("настоящий ответ"));
        // Совсем нечего озвучивать — None (вызывающий покажет понятную ошибку).
        assert!(build_utterances(&[], TtsScope::All, false, ru()).is_none());
        assert!(
            build_utterances(
                &[Message::new(MessageRole::System, "только системное")],
                TtsScope::All,
                false,
                ru()
            )
            .is_none()
        );
    }

    #[test]
    fn role_prefixes_apply_to_every_scope_including_single() {
        // Тумблер «Озвучивать роли» действует и при одиночном `/tts` (решение Р6).
        let one = build_utterances(&chat_messages(), TtsScope::Last, true, ru()).unwrap();
        assert!(
            one[0].starts_with(ru().t("speak.role.assistant")),
            "префикс роли и у одного сообщения: {one:?}"
        );
        let many = build_utterances(&chat_messages(), TtsScope::Recent(2), true, ru()).unwrap();
        assert!(many[0].starts_with(ru().t("speak.role.user")));
        assert!(many[1].starts_with(ru().t("speak.role.assistant")));
    }

    #[test]
    fn message_whose_text_is_all_skippable_is_dropped() {
        // Сообщение из одного код-блока даёт только пометку — она озвучивается,
        // а вот пустой результат экстрактора сообщение бы отбросил.
        let messages = vec![Message::assistant("```rust\nfn main() {}\n```")];
        let out = build_utterances(&messages, TtsScope::All, false, ru()).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].contains(ru().t("speak.skip.code")), "{out:?}");
    }

    #[test]
    fn chunking_respects_limit_and_sentence_boundaries() {
        let text = "Первое предложение. Второе предложение! Третье предложение?".to_string();
        let chunks = chunk_utterances(&[text], 25);
        assert!(
            chunks.iter().all(|c| c.chars().count() <= 25),
            "лимит соблюдён: {chunks:?}"
        );
        // Границы — по предложениям (пунктуация сохранена в конце чанка).
        assert!(
            chunks
                .iter()
                .all(|c| c.ends_with('.') || c.ends_with('!') || c.ends_with('?')),
            "чанк заканчивается концом предложения: {chunks:?}"
        );
        // Ничего не потеряно.
        assert_eq!(
            chunks.join(" ").replace("  ", " "),
            "Первое предложение. Второе предложение! Третье предложение?"
        );
    }

    #[test]
    fn short_text_stays_single_chunk() {
        let chunks = chunk_utterances(&["Коротко.".to_string()], 4096);
        assert_eq!(chunks, vec!["Коротко.".to_string()]);
    }

    #[test]
    fn utterances_are_not_merged_across_messages() {
        // Граница сообщения — естественная пауза и точка отмены: не склеиваем даже
        // короткие реплики.
        let chunks = chunk_utterances(&["Раз.".to_string(), "Два.".to_string()], 4096);
        assert_eq!(chunks, vec!["Раз.".to_string(), "Два.".to_string()]);
    }

    #[test]
    fn overlong_sentence_is_split_by_words_then_chars() {
        let long_words = "слово ".repeat(20);
        let chunks = chunk_utterances(&[long_words.trim().to_string()], 20);
        assert!(chunks.iter().all(|c| c.chars().count() <= 20), "{chunks:?}");
        assert!(chunks.len() > 1);
        // Одно слово длиннее лимита режется посимвольно, а не теряется.
        let giant = "я".repeat(50);
        let chunks = chunk_utterances(std::slice::from_ref(&giant), 20);
        assert!(chunks.iter().all(|c| c.chars().count() <= 20), "{chunks:?}");
        assert_eq!(chunks.concat().chars().count(), giant.chars().count());
    }

    #[test]
    fn setup_error_keys_exist_in_bundle() {
        for err in [
            TtsSetupError::Model,
            TtsSetupError::ApiKey,
            TtsSetupError::Url,
        ] {
            let key = setup_error_key(err);
            assert!(ru().has_key(key), "ключ {key} должен быть в бандле");
        }
    }
}
