//! Speech recognition (speech-to-text): what hears a voice note from the
//! messenger (ADR 0013, stage C, D6 (a)) — a slot of its own, built like the
//! speech slot it mirrors (ADR 0009), and like it **stateless**: a client is
//! built from a snapshot of the settings for each note.
//!
//! One route, `POST …/audio/transcriptions`, in two shapes: OpenAI's multipart
//! form — the OpenAI cloud and any server of the person's own (vLLM, speaches,
//! LocalAI, whisper.cpp's server) — and the OpenRouter gateway's JSON, the audio
//! in base64. A note goes as Telegram sent it, OGG/Opus, which every recogniser
//! measured took (docs/research/companion-platform.md §3.8). A server that
//! refuses it is sent the same note once more as 16 kHz WAV, decoded here; a WAV
//! that is heard is remembered for the session ([`WavMemo`]), so the refused
//! request is paid once, not once a note — the speech slot's [`FormatMemo`]
//! pattern.
//!
//! [`FormatMemo`]: crate::shared::tts::FormatMemo

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use reqwest::StatusCode;

use crate::shared::config::{CloudProvider, SttMode, SttSettings};
use crate::shared::tts::{non_empty, resolve_key};
use crate::shared::voice_note::{self, DECODE_RATE, VoiceNoteError};

/// How long one note may take: a long note on a slow server.
const TIMEOUT: Duration = Duration::from_secs(180);

/// What a channel is handed to hear its notes with — plain data, so the
/// bridge can move into a program of its own (ADR 0013, rule 1): the slot's
/// settings, the key they read, and the gateway's attribution switch.
#[derive(Clone, PartialEq, Default)]
pub struct Hearing {
    pub stt: SttSettings,
    /// The stored key of whatever the active mode reads (the provider's, shared
    /// with chat, or the `external` slot's own — ADR 0008), already decrypted.
    pub key: Option<String>,
    /// `openrouter.attribution`: the gateway is told which application asks.
    pub attribution: bool,
}

/// The key is never printed — a log line must not hold it (the video slot's
/// `VideoConfig` precedent).
impl std::fmt::Debug for Hearing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hearing")
            .field("stt", &self.stt)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .field("attribution", &self.attribution)
            .finish()
    }
}

/// Why recognition isn't set up. Structured, so the message is built in the
/// interface's language (the speech slot's precedent, `TtsSetupError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttSetupError {
    /// No model name set.
    Model,
    /// No API key (neither entered in settings nor in an env variable).
    ApiKey,
    /// No external server URL set.
    Url,
}

/// Why a note was not heard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SttError {
    /// The server answered, and not with words: its status and what it said.
    #[error("the recogniser refused the note ({status}): {said}")]
    Refused { status: u16, said: String },
    /// No answer: the network, a timeout, a body that is not the API's.
    #[error("the recogniser did not answer: {0}")]
    Network(String),
    /// The note could not be turned into WAV for a server that wanted it.
    #[error("{0}")]
    Unreadable(#[from] VoiceNoteError),
    /// The answer held no words: a silent note, or speech it could not make out.
    #[error("nothing was heard in the note")]
    Nothing,
}

/// Which servers wanted WAV rather than the note as it came, by address and
/// model. Lives as long as the session, outside the clients (a client is built
/// per note).
#[derive(Debug, Default)]
pub struct WavMemo(Mutex<HashSet<String>>);

impl WavMemo {
    fn wants_wav(&self, key: &str) -> bool {
        let known = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        known.contains(key)
    }

    fn keep(&self, key: String) {
        let mut known = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        known.insert(key);
    }
}

/// The two shapes of the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    /// A multipart form with the file (OpenAI, a server of the person's own).
    Form,
    /// JSON with the audio in base64 (the gateway).
    Json,
}

/// The audio of one request.
enum Audio<'a> {
    Ogg(&'a [u8]),
    Wav(&'a [u8]),
}

impl Audio<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Audio::Ogg(b) | Audio::Wav(b) => b,
        }
    }
    fn format(&self) -> &'static str {
        match self {
            Audio::Ogg(_) => "ogg",
            Audio::Wav(_) => "wav",
        }
    }
    fn mime(&self) -> &'static str {
        match self {
            Audio::Ogg(_) => "audio/ogg",
            Audio::Wav(_) => "audio/wav",
        }
    }
}

/// A transcription client.
pub struct Transcriber {
    http: reqwest::Client,
    /// `{base}/audio/transcriptions`.
    endpoint: String,
    wire: Wire,
    key: Option<String>,
    model: Option<String>,
    language: Option<String>,
    attribution: bool,
    memo: Arc<WavMemo>,
}

/// A client for the settings `hearing` holds; `memo` is the session's.
pub fn transcriber(hearing: &Hearing, memo: Arc<WavMemo>) -> Result<Transcriber, SttSetupError> {
    let stt = &hearing.stt;
    let (base, key, model, wire) = match stt.mode {
        SttMode::OpenAi | SttMode::OpenRouter => {
            let (section, provider, wire) = match stt.mode {
                SttMode::OpenRouter => (&stt.openrouter, CloudProvider::OpenRouter, Wire::Json),
                _ => (&stt.openai, CloudProvider::OpenAi, Wire::Form),
            };
            let model = non_empty(section.model_name.clone()).ok_or(SttSetupError::Model)?;
            let key = resolve_key(hearing.key.clone(), section.api_key_env.as_deref())
                .ok_or(SttSetupError::ApiKey)?;
            let base = non_empty(section.url.clone())
                .unwrap_or_else(|| provider.chat_base_url().to_string());
            (base, Some(key), Some(model), wire)
        }
        SttMode::External => {
            let ext = &stt.external;
            let base = non_empty(ext.url.clone()).ok_or(SttSetupError::Url)?;
            // A local server needs no key, so having none is not refused.
            let key = resolve_key(hearing.key.clone(), ext.api_key_env.as_deref());
            (base, key, non_empty(ext.model_name.clone()), Wire::Form)
        }
    };
    Ok(Transcriber {
        http: reqwest::Client::new(),
        endpoint: format!("{}/audio/transcriptions", base.trim_end_matches('/')),
        wire,
        key,
        model,
        language: non_empty(stt.language.clone()),
        attribution: stt.mode == SttMode::OpenRouter && hearing.attribution,
        memo,
    })
}

impl Transcriber {
    /// The words of `note`, an OGG/Opus voice note.
    pub async fn transcribe(&self, note: &[u8]) -> Result<String, SttError> {
        let remembered = format!("{} {}", self.endpoint, self.model.as_deref().unwrap_or(""));
        if self.memo.wants_wav(&remembered) {
            let wav = as_wav(note).await?;
            return heard(self.send(Audio::Wav(&wav)).await?);
        }
        match self.send(Audio::Ogg(note)).await {
            Ok(text) => heard(text),
            // A refusal of the request's shape may be a refusal of the format:
            // the note is asked once more as WAV, and what that comes to is
            // kept — a refusal that was not about the format is told as it was.
            Err(SttError::Refused { status, said }) if matches!(status, 400 | 415 | 422) => {
                let first = SttError::Refused { status, said };
                let Ok(wav) = as_wav(note).await else {
                    return Err(first);
                };
                match self.send(Audio::Wav(&wav)).await {
                    Ok(text) => {
                        tracing::info!(endpoint = %self.endpoint, "stt: the server takes WAV, not the note as it came");
                        self.memo.keep(remembered);
                        heard(text)
                    }
                    Err(_) => Err(first),
                }
            }
            Err(err) => Err(err),
        }
    }

    async fn send(&self, audio: Audio<'_>) -> Result<String, SttError> {
        let mut rb = self.http.post(&self.endpoint).timeout(TIMEOUT);
        if let Some(key) = &self.key {
            rb = rb.bearer_auth(key);
        }
        if self.attribution {
            rb = crate::shared::api::openai::attributed(rb);
        }
        rb = match self.wire {
            Wire::Form => {
                let file = reqwest::multipart::Part::bytes(audio.bytes().to_vec())
                    .file_name(format!("voice.{}", audio.format()))
                    .mime_str(audio.mime())
                    .map_err(|e| SttError::Network(e.to_string()))?;
                let mut form = reqwest::multipart::Form::new()
                    .part("file", file)
                    .text("response_format", "json");
                if let Some(model) = &self.model {
                    form = form.text("model", model.clone());
                }
                if let Some(language) = &self.language {
                    form = form.text("language", language.clone());
                }
                rb.multipart(form)
            }
            Wire::Json => {
                let mut body = serde_json::json!({
                    "input_audio": {
                        "data": base64::engine::general_purpose::STANDARD.encode(audio.bytes()),
                        "format": audio.format(),
                    },
                });
                if let Some(model) = &self.model {
                    body["model"] = model.clone().into();
                }
                if let Some(language) = &self.language {
                    body["language"] = language.clone().into();
                }
                rb.json(&body)
            }
        };
        let resp = rb
            .send()
            .await
            .map_err(|e| SttError::Network(e.without_url().to_string()))?;
        let status = resp.status();
        let label = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = resp
            .text()
            .await
            .map_err(|e| SttError::Network(e.without_url().to_string()))?;
        if !status.is_success() {
            return Err(refusal(status, &body));
        }
        words_of(&label, &body)
    }
}

/// The words of a `200`: `{"text": …}`, or the body itself where a server
/// answers in plain text.
fn words_of(label: &str, body: &str) -> Result<String, SttError> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        return match json.get("text").and_then(|t| t.as_str()) {
            Some(text) => Ok(text.to_string()),
            None => Err(SttError::Network(format!(
                "an answer with no text: {}",
                excerpt(body)
            ))),
        };
    }
    if label.starts_with("text/plain") {
        return Ok(body.to_string());
    }
    Err(SttError::Network(format!(
        "an answer that is not the API's: {}",
        excerpt(body)
    )))
}

/// Words, or [`SttError::Nothing`] when there are none.
fn heard(text: String) -> Result<String, SttError> {
    let text = text.trim().to_string();
    if text.is_empty() {
        Err(SttError::Nothing)
    } else {
        Ok(text)
    }
}

/// A refusal in the server's words: the message of an error envelope, else the
/// body's first characters.
fn refusal(status: StatusCode, body: &str) -> SttError {
    let said = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message").or(Some(e)))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| excerpt(body));
    tracing::warn!(%status, said = %said, "stt: the recogniser refused a note");
    SttError::Refused {
        status: status.as_u16(),
        said,
    }
}

fn excerpt(body: &str) -> String {
    body.trim().chars().take(300).collect()
}

/// The note as 16 kHz WAV, decoded off the async threads.
async fn as_wav(note: &[u8]) -> Result<Vec<u8>, SttError> {
    let note = note.to_vec();
    let samples = tokio::task::spawn_blocking(move || voice_note::decode(&note))
        .await
        .map_err(|e| SttError::Network(e.to_string()))??;
    Ok(voice_note::wav(&samples, DECODE_RATE))
}

#[cfg(test)]
mod tests;
