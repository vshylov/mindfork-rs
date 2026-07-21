//! Озвучивание текста (TTS): контракт синтеза + клиенты провайдеров +
//! воспроизведение. Слот озвучивания **независим от chat-движка** (у Anthropic TTS
//! нет вовсе), поэтому провайдер конфигурируется отдельно — как выделенный
//! embedding-сервер (ADR 0002). Клиенты **stateless**: строятся из снимка конфига
//! на вызов, менеджер-слот не нужен.
//!
//! Раскладка (docs/research/tts.md §8):
//! - [`openai`] — `POST /v1/audio/speech`: облако OpenAI **и** любой сторонний
//!   OpenAI-совместимый TTS-сервер (разные base URL/ключ/формат ответа);
//! - [`gemini`] — нативный `generateContent` c `responseModalities:["AUDIO"]`;
//! - [`playback`] — воспроизведение: очередь источников `rodio` + отмена;
//! - [`sidecar`] — локальный движок (managed): сайдкар `piper` из `data/tts/`,
//!   без сети и ключей (ADR 0009).

pub mod gemini;
pub mod openai;
pub mod playback;
pub mod sidecar;

use std::path::Path;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::shared::config::{TtsMode, TtsSettings};

/// Синтезированный фрагмент речи.
///
/// Облака отдают **сырой PCM без заголовка** (OpenAI `response_format:"pcm"` —
/// 24 кГц s16le mono; Gemini `audio/L16;codec=pcm;rate=…`), поэтому декодер им не
/// нужен вовсе — сэмплы идут прямо в буфер воспроизведения. Сторонние серверы
/// отвечают контейнером (`wav` — самый переносимый), его разбирает декодер.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioClip {
    /// Сырой знаковый 16-битный little-endian PCM без заголовка.
    Pcm {
        sample_rate: u32,
        channels: u16,
        bytes: Vec<u8>,
    },
    /// Аудио в контейнере (wav/mp3) — с заголовком, разбирается декодером.
    Encoded(Vec<u8>),
}

impl AudioClip {
    /// Пустой ли клип (нечего проигрывать).
    pub fn is_empty(&self) -> bool {
        match self {
            AudioClip::Pcm { bytes, .. } => bytes.len() < 2,
            AudioClip::Encoded(bytes) => bytes.is_empty(),
        }
    }
}

/// Провайдер синтеза речи. Реализации — тонкие HTTP-клиенты (см. модуль).
#[async_trait::async_trait]
pub trait TtsEngine: Send + Sync {
    /// Синтезирует речь для куска текста. Отменяемо: по `cancel` запрос
    /// прерывается (ошибкой), накопленное не используется.
    async fn synthesize(&self, text: &str, cancel: &CancellationToken) -> Result<AudioClip>;

    /// Потолок длины текста одного запроса (символов) — по нему режется текст на
    /// чанки. Жёсткий лимит провайдера, а не предпочтение.
    fn max_input_chars(&self) -> usize;
}

/// Почему озвучивание не настроено. Структурная ошибка (не текст): сообщение
/// формирует UI на языке интерфейса — ось B, прецедент `supervisor::ApiKeyError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsSetupError {
    /// Не задано имя модели.
    Model,
    /// Нет API-ключа (ни введённого в настройках, ни в env-переменной).
    ApiKey,
    /// Не задан URL внешнего сервера.
    Url,
    /// Локальный движок не установлен (нет бинаря `piper`) — нужен `tts setup`.
    Binary,
    /// Голос локального движка не найден (нет `*.onnx` или его `*.onnx.json`).
    Voice,
}

/// Строит клиент озвучивания из снимка настроек. `stored_key` — сохранённый ключ
/// провайдера (ADR 0008): он уже расшифрован вызывающим; при его отсутствии
/// ключ читается из env-переменной, имя которой задано в настройках. `tts_dir` —
/// каталог локального движка (`data/tts/`), нужен только режиму `managed`.
pub fn engine_from_config(
    tts: &TtsSettings,
    stored_key: Option<String>,
    tts_dir: Option<&Path>,
) -> std::result::Result<Box<dyn TtsEngine>, TtsSetupError> {
    let speed = tts.speed;
    match tts.mode {
        TtsMode::Managed => Ok(Box::new(sidecar::PiperTts::new(
            tts_dir,
            tts.managed.binary.as_deref(),
            tts.managed.voice.as_deref(),
            speed,
        )?)),
        TtsMode::OpenAi | TtsMode::Gemini => {
            let cloud = tts.cloud().ok_or(TtsSetupError::Model)?;
            let model = non_empty(cloud.model_name.clone()).ok_or(TtsSetupError::Model)?;
            let key = stored_key
                .filter(|k| !k.is_empty())
                .or_else(|| env_key(cloud.api_key_env.as_deref()))
                .ok_or(TtsSetupError::ApiKey)?;
            let provider = tts.mode.cloud_provider().ok_or(TtsSetupError::Model)?;
            let base = non_empty(cloud.url.clone())
                .unwrap_or_else(|| provider.chat_base_url().to_string());
            let voice = non_empty(cloud.voice.clone());
            let instructions = non_empty(cloud.instructions.clone());
            Ok(match tts.mode {
                TtsMode::Gemini => Box::new(gemini::GeminiTts::new(
                    base,
                    key,
                    model,
                    voice,
                    instructions,
                )),
                // OpenAI (облако): просим сырой PCM — декодер не нужен.
                _ => Box::new(openai::OpenAiTts::cloud(
                    base,
                    key,
                    model,
                    voice,
                    instructions,
                    speed,
                )),
            })
        }
        TtsMode::External => {
            let url = non_empty(tts.external.url.clone()).ok_or(TtsSetupError::Url)?;
            Ok(Box::new(openai::OpenAiTts::external(
                url,
                env_key(tts.external.api_key_env.as_deref()),
                non_empty(tts.external.model_name.clone()),
                non_empty(tts.external.voice.clone()),
                speed,
            )))
        }
    }
}

/// Значение env-переменной по её имени (фолбэк к ключу из настроек, ADR 0008).
fn env_key(var: Option<&str>) -> Option<String> {
    let var = var?.trim();
    if var.is_empty() {
        return None;
    }
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// Непустая строка или `None` (в настройках пустое поле хранится как `Some("")`).
fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Не глотает тело ошибки провайдера: статус + причина логируются и попадают в
/// текст ошибки (обрезка до 500 символов) — как в клиентах движка (ADR 0004).
pub(crate) async fn error_body(what: &str, resp: reqwest::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let detail: String = body.trim().chars().take(500).collect();
    tracing::warn!(%status, body = %detail, "{what} вернул статус ошибки");
    if detail.is_empty() {
        anyhow::anyhow!("{what}: статус {status}")
    } else {
        anyhow::anyhow!("{what}: статус {status}: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::config::{TtsCloudSettings, TtsExternalSettings};

    #[test]
    fn cloud_requires_model_and_key() {
        let mut tts = TtsSettings {
            openai: TtsCloudSettings {
                model_name: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            engine_from_config(&tts, Some("sk-x".into()), None).err(),
            Some(TtsSetupError::Model)
        );
        tts.openai.model_name = Some("gpt-4o-mini-tts".into());
        // Ключа нет ни сохранённого, ни в env → понятная структурная ошибка.
        assert_eq!(
            engine_from_config(&tts, None, None).err(),
            Some(TtsSetupError::ApiKey)
        );
        // Сохранённый ключ (ADR 0008) достаточен — вводить заново ничего не нужно.
        assert!(engine_from_config(&tts, Some("sk-x".into()), None).is_ok());
    }

    #[test]
    fn external_requires_url_only() {
        let tts = TtsSettings {
            mode: TtsMode::External,
            external: TtsExternalSettings::default(),
            ..Default::default()
        };
        assert_eq!(
            engine_from_config(&tts, None, None).err(),
            Some(TtsSetupError::Url)
        );
        let tts = TtsSettings {
            mode: TtsMode::External,
            external: TtsExternalSettings {
                url: Some("http://127.0.0.1:8880/v1".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        // Локальному серверу ключ и модель не нужны.
        assert!(engine_from_config(&tts, None, None).is_ok());
    }

    #[test]
    fn blank_fields_count_as_unset() {
        let tts = TtsSettings {
            openai: TtsCloudSettings {
                model_name: Some("   ".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            engine_from_config(&tts, Some("sk-x".into()), None).err(),
            Some(TtsSetupError::Model)
        );
    }

    #[test]
    fn empty_clip_detected() {
        assert!(AudioClip::Encoded(Vec::new()).is_empty());
        assert!(
            AudioClip::Pcm {
                sample_rate: 24000,
                channels: 1,
                bytes: vec![0],
            }
            .is_empty()
        );
        assert!(!AudioClip::Encoded(vec![1, 2, 3]).is_empty());
    }
}
