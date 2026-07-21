//! Клиент озвучивания Gemini. Ключевая удача: TTS у Gemini живёт **в том же
//! протоколе**, на котором уже сидит [`crate::shared::api::gemini`] — обычный
//! `generateContent` с `responseModalities:["AUDIO"]` и заголовком `x-goog-api-key`;
//! новый транспорт не нужен. Ответ — base64 сырого PCM **без WAV-заголовка**;
//! частоту дискретизации берём из `mimeType` (`audio/L16;codec=pcm;rate=24000`),
//! а не хардкодим. См. docs/research/tts.md §3.2.

use anyhow::{Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{AudioClip, TtsEngine, error_body};

/// Потолок длины текста одного запроса (символов). У TTS-моделей Gemini контекст
/// 32k токенов — упираемся не в него, а в желание быстрее получить первый звук.
const GEMINI_MAX_INPUT_CHARS: usize = 2000;

/// Частота дискретизации, если `mimeType` её не сообщил (документированный дефолт).
const DEFAULT_PCM_RATE: u32 = 24_000;

/// Клиент нативного `generateContent` в режиме аудио-ответа.
pub struct GeminiTts {
    http: reqwest::Client,
    /// Базовый URL (`…/v1beta`); путь `/models/{model}:generateContent` добавляем сами.
    base_url: String,
    api_key: String,
    model: String,
    voice: Option<String>,
    /// Указания по стилю. У Gemini отдельного поля нет — стиль задаётся
    /// естественным языком в самом тексте запроса (док провайдера).
    instructions: Option<String>,
}

impl GeminiTts {
    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        voice: Option<String>,
        instructions: Option<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            // Имя модели может прийти с префиксом `models/` — путь его уже несёт.
            model: model.trim().trim_start_matches("models/").to_string(),
            voice,
            instructions,
        }
    }

    /// Текст запроса: указания по стилю (если заданы) идут префиксом к фразе —
    /// отдельного поля для них в API нет.
    fn prompt(&self, text: &str) -> String {
        match &self.instructions {
            Some(style) => format!("{style}: {text}"),
            None => text.to_string(),
        }
    }

    fn body(&self, text: &str) -> GenerateRequest {
        GenerateRequest {
            contents: vec![Content {
                parts: vec![Part {
                    text: self.prompt(text),
                }],
            }],
            generation_config: GenerationConfig {
                response_modalities: vec!["AUDIO"],
                speech_config: self.voice.as_ref().map(|v| SpeechConfig {
                    voice_config: VoiceConfig {
                        prebuilt_voice_config: PrebuiltVoiceConfig {
                            voice_name: v.clone(),
                        },
                    },
                }),
            },
        }
    }
}

// ── тело запроса ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GenerateRequest {
    contents: Vec<Content>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Debug, Serialize)]
struct Content {
    parts: Vec<Part>,
}

#[derive(Debug, Serialize)]
struct Part {
    text: String,
}

#[derive(Debug, Serialize)]
struct GenerationConfig {
    #[serde(rename = "responseModalities")]
    response_modalities: Vec<&'static str>,
    #[serde(rename = "speechConfig", skip_serializing_if = "Option::is_none")]
    speech_config: Option<SpeechConfig>,
}

#[derive(Debug, Serialize)]
struct SpeechConfig {
    #[serde(rename = "voiceConfig")]
    voice_config: VoiceConfig,
}

#[derive(Debug, Serialize)]
struct VoiceConfig {
    #[serde(rename = "prebuiltVoiceConfig")]
    prebuilt_voice_config: PrebuiltVoiceConfig,
}

#[derive(Debug, Serialize)]
struct PrebuiltVoiceConfig {
    #[serde(rename = "voiceName")]
    voice_name: String,
}

// ── тело ответа ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GenerateResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(rename = "promptFeedback", default)]
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    #[serde(default)]
    content: Option<RespContent>,
}

#[derive(Debug, Deserialize)]
struct RespContent {
    #[serde(default)]
    parts: Vec<RespPart>,
}

#[derive(Debug, Deserialize)]
struct RespPart {
    #[serde(rename = "inlineData", default)]
    inline_data: Option<InlineData>,
}

#[derive(Debug, Deserialize)]
struct InlineData {
    #[serde(rename = "mimeType", default)]
    mime_type: String,
    #[serde(default)]
    data: String,
}

#[derive(Debug, Deserialize)]
struct PromptFeedback {
    #[serde(rename = "blockReason", default)]
    block_reason: Option<String>,
}

/// Частота дискретизации из `mimeType` вида `audio/L16;codec=pcm;rate=24000`.
/// Хардкодить нельзя — провайдер вправе отдать другую (док провайдера).
fn rate_from_mime(mime: &str) -> u32 {
    mime.split(';')
        .filter_map(|p| p.trim().strip_prefix("rate="))
        .find_map(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_PCM_RATE)
}

/// Достаёт аудио из ответа: base64 PCM + частота из `mimeType`. Блокировку фильтром
/// и пустой ответ превращает в понятную ошибку (иначе была бы «тишина без причины»).
fn clip_from_response(resp: GenerateResponse) -> Result<AudioClip> {
    if let Some(reason) = resp.prompt_feedback.and_then(|f| f.block_reason) {
        anyhow::bail!("Gemini отклонил запрос озвучивания (причина: {reason})");
    }
    let data = resp
        .candidates
        .into_iter()
        .filter_map(|c| c.content)
        .flat_map(|c| c.parts)
        .find_map(|p| p.inline_data);
    let Some(data) = data else {
        anyhow::bail!("Gemini не вернул аудио");
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.data.trim())
        .context("разбор base64 аудио от Gemini")?;
    Ok(AudioClip::Pcm {
        sample_rate: rate_from_mime(&data.mime_type),
        channels: 1,
        bytes,
    })
}

#[async_trait::async_trait]
impl TtsEngine for GeminiTts {
    async fn synthesize(&self, text: &str, cancel: &CancellationToken) -> Result<AudioClip> {
        // Не-стриминговый `generateContent`: `streamGenerateContent` для TTS
        // поддержан только моделями 3.1+, а конвейер по предложениям и так даёт
        // быстрый первый звук (docs/research/tts.md §3.2, §8).
        let url = format!("{}/models/{}:generateContent", self.base_url, self.model);
        let rb = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .json(&self.body(text));
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("озвучивание отменено"),
            r = rb.send() => r.with_context(|| format!("POST {url}"))?,
        };
        if !resp.status().is_success() {
            return Err(error_body("Gemini TTS", resp).await);
        }
        let parsed: GenerateResponse = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("озвучивание отменено"),
            j = resp.json() => j.context("разбор ответа Gemini TTS")?,
        };
        clip_from_response(parsed)
    }

    fn max_input_chars(&self) -> usize {
        GEMINI_MAX_INPUT_CHARS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> GeminiTts {
        GeminiTts::new(
            "https://generativelanguage.googleapis.com/v1beta/".into(),
            "key".into(),
            "models/gemini-2.5-flash-preview-tts".into(),
            Some("Kore".into()),
            None,
        )
    }

    #[test]
    fn request_asks_for_audio_modality_and_voice() {
        let e = engine();
        let v = serde_json::to_value(e.body("привет")).unwrap();
        assert_eq!(v["contents"][0]["parts"][0]["text"], "привет");
        assert_eq!(v["generationConfig"]["responseModalities"][0], "AUDIO");
        assert_eq!(
            v["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
            "Kore"
        );
        // Префикс `models/` и хвостовой слэш сняты — путь их уже несёт.
        assert_eq!(e.model, "gemini-2.5-flash-preview-tts");
        assert_eq!(
            e.base_url,
            "https://generativelanguage.googleapis.com/v1beta"
        );
    }

    #[test]
    fn instructions_go_as_style_prefix() {
        let e = GeminiTts::new(
            "https://x/v1beta".into(),
            "k".into(),
            "m".into(),
            None,
            Some("скажи бодро".into()),
        );
        let v = serde_json::to_value(e.body("текст")).unwrap();
        assert_eq!(v["contents"][0]["parts"][0]["text"], "скажи бодро: текст");
        // Голос не задан — секции speechConfig в теле нет вовсе.
        assert!(
            v["generationConfig"].get("speechConfig").is_none(),
            "незаданный голос не отправляем: {v}"
        );
    }

    #[test]
    fn parses_inline_pcm_and_rate_from_mime() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {"parts": [{"inlineData": {
                    "mimeType": "audio/L16;codec=pcm;rate=16000",
                    "data": "AAECAw=="
                }}]},
                "finishReason": "STOP"
            }]
        });
        let resp: GenerateResponse = serde_json::from_value(json).unwrap();
        assert_eq!(
            clip_from_response(resp).unwrap(),
            AudioClip::Pcm {
                sample_rate: 16_000,
                channels: 1,
                bytes: vec![0, 1, 2, 3],
            }
        );
    }

    #[test]
    fn rate_defaults_when_mime_says_nothing() {
        assert_eq!(rate_from_mime("audio/L16;codec=pcm"), DEFAULT_PCM_RATE);
        assert_eq!(rate_from_mime(""), DEFAULT_PCM_RATE);
        assert_eq!(rate_from_mime("audio/L16; rate=48000 "), 48_000);
    }

    #[test]
    fn block_and_empty_answer_become_clear_errors() {
        let blocked: GenerateResponse = serde_json::from_value(
            serde_json::json!({"promptFeedback": {"blockReason": "SAFETY"}}),
        )
        .unwrap();
        let err = clip_from_response(blocked).unwrap_err().to_string();
        assert!(err.contains("SAFETY"), "причина блокировки в ошибке: {err}");

        let empty: GenerateResponse =
            serde_json::from_value(serde_json::json!({"candidates": []})).unwrap();
        assert!(clip_from_response(empty).is_err(), "пустой ответ — ошибка");
    }

    /// Живой смоук Gemini: синтез короткой русской фразы. Требует
    /// `MINDFORK_GEMINI_KEY`; без неё тихо пропускается.
    #[tokio::test]
    #[ignore = "требует ключ Gemini (MINDFORK_GEMINI_KEY)"]
    async fn gemini_synthesizes_russian_speech_live() {
        let Ok(key) = std::env::var("MINDFORK_GEMINI_KEY") else {
            eprintln!("MINDFORK_GEMINI_KEY не задан — смоук пропущен");
            return;
        };
        let e = GeminiTts::new(
            crate::shared::config::CloudProvider::Gemini
                .chat_base_url()
                .to_string(),
            key,
            std::env::var("MINDFORK_TTS_MODEL")
                .unwrap_or_else(|_| crate::shared::config::DEFAULT_TTS_GEMINI_MODEL.into()),
            Some(crate::shared::config::DEFAULT_TTS_GEMINI_VOICE.into()),
            None,
        );
        let clip = e
            .synthesize(
                "Проверка озвучивания. Латинская вставка: API, JSON.",
                &CancellationToken::new(),
            )
            .await
            .expect("синтез должен пройти");
        let AudioClip::Pcm {
            sample_rate,
            channels,
            bytes,
        } = clip
        else {
            panic!("Gemini должен отдавать сырой PCM");
        };
        eprintln!("получено PCM: {} байт, {sample_rate} Гц", bytes.len());
        assert_eq!(channels, 1);
        assert!(sample_rate >= 8000, "разумная частота дискретизации");
        assert!(bytes.len() > 10_000, "клип подозрительно короткий");
    }
}
