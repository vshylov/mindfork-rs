//! Ollama and LM Studio — the two local servers a user most often already runs
//! (docs/research/local-servers.md): what tells each apart, what each says
//! about the models it holds, and the look on this machine's own ports that
//! finds them for a chat with no engine configured ([`find`], stage 2).
//!
//! Every shape here has a field that is **required**, and that is the point.
//! LM Studio answers every path it does not serve with `200` and
//! `{"error":"Unexpected endpoint or method. (GET /props)"}` (measured on 1.1.7,
//! §2 of the research), so a status, or a body with every field optional,
//! would name LM Studio whatever was asked.

use std::time::Duration;

use serde::Deserialize;

use super::contract::ServerKind;

/// Where each server listens unless told otherwise, on this machine (F6).
pub const OLLAMA_PORT: u16 = 11434;
pub const LM_STUDIO_PORT: u16 = 1234;

/// Ollama's `GET /api/version` — `{"version":"0.35.1"}`.
#[derive(Debug, Deserialize)]
pub(crate) struct OllamaVersion {
    pub(crate) version: String,
}

/// LM Studio's `GET /api/v1/models`: every downloaded model, each with the
/// instances of it that are loaded.
#[derive(Debug, Deserialize)]
pub(crate) struct LmStudioModels {
    models: Vec<LmStudioModel>,
}

#[derive(Debug, Deserialize)]
struct LmStudioModel {
    /// `llm` (or `vlm`) for a chat model, `embedding` for an embedder.
    #[serde(rename = "type", default)]
    kind: String,
    /// What a request names the model by, before it is loaded.
    key: String,
    #[serde(default)]
    loaded_instances: Vec<LmStudioInstance>,
}

#[derive(Debug, Deserialize)]
struct LmStudioInstance {
    /// The instance's identifier — the model's key for the first instance,
    /// `<key>:2` and on for more, or the `--identifier` it was loaded with. A
    /// request naming it is routed to it.
    id: String,
    #[serde(default)]
    config: LmStudioInstanceConfig,
}

#[derive(Debug, Default, Deserialize)]
struct LmStudioInstanceConfig {
    context_length: Option<u32>,
}

impl LmStudioModels {
    /// The window of the instance a request naming `wanted` runs in: the
    /// instance that has that identifier, or else the only instance of the
    /// model with that key. Several instances and none of them named is
    /// silence, like a model that is not loaded (LM Studio loads it on the
    /// first request, so the orchestrator asks again after the first turn) and
    /// a window of zero.
    pub(crate) fn window_of(&self, wanted: &str) -> Option<u32> {
        let instances = || self.models.iter().flat_map(|m| &m.loaded_instances);
        let named = instances().find(|i| i.id == wanted).or_else(|| {
            let model = self.models.iter().find(|m| m.key == wanted)?;
            match model.loaded_instances.as_slice() {
                [only] => Some(only),
                _ => None,
            }
        })?;
        named.config.context_length.filter(|&n| n > 0)
    }
}

/// Ollama's `GET /api/tags`: every pulled model, with what it can do.
#[derive(Debug, Deserialize)]
struct OllamaTags {
    models: Vec<OllamaTag>,
}

#[derive(Debug, Deserialize)]
struct OllamaTag {
    name: String,
    /// `completion` for a chat model, `embedding` for an embedder (measured on
    /// 0.35.1). An older Ollama sends none; then the name decides.
    #[serde(default)]
    capabilities: Vec<String>,
}

/// Ollama's `GET /api/ps`, read for which models are loaded.
#[derive(Debug, Deserialize)]
struct OllamaLoaded {
    models: Vec<OllamaLoadedModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaLoadedModel {
    name: String,
}

/// A model a local server holds, and whether it is loaded now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundModel {
    pub name: String,
    pub loaded: bool,
}

/// A local server that answered, with its chat models and its embedders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub server: ServerKind,
    /// The OpenAI-compatible base, `/v1` included — what the external section
    /// takes.
    pub url: String,
    pub chat: Vec<FoundModel>,
    pub embedders: Vec<FoundModel>,
}

/// One row of the *Local servers* list: a server and a chat model, and the
/// embedder the same pick configures when the embedder is not configured
/// (docs/research/local-servers.md §4, stage 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalOffer {
    pub server: ServerKind,
    pub url: String,
    pub model: String,
    pub loaded: bool,
    pub embedder: Option<String>,
}

/// Asks Ollama and LM Studio on this machine, both at once, each with a short
/// timeout — a port nothing listens on is refused at once, and a server that
/// hangs costs three seconds, not a start-up. What does not answer, or answers
/// in a shape that is not its own, is left out.
pub async fn find() -> Vec<Found> {
    let http = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
    {
        Ok(http) => http,
        Err(err) => {
            tracing::warn!(error = %err, "no client to look for local servers with");
            return Vec::new();
        }
    };
    find_at(
        &http,
        &format!("http://127.0.0.1:{OLLAMA_PORT}"),
        &format!("http://127.0.0.1:{LM_STUDIO_PORT}"),
    )
    .await
}

/// [`find`] against two given roots — the seam the tests use.
pub(crate) async fn find_at(http: &reqwest::Client, ollama: &str, lm_studio: &str) -> Vec<Found> {
    let (o, l) = tokio::join!(find_ollama(http, ollama), find_lm_studio(http, lm_studio));
    o.into_iter().chain(l).collect()
}

async fn get_json<T: serde::de::DeserializeOwned>(http: &reqwest::Client, url: &str) -> Option<T> {
    let resp = match http.get(url).send().await {
        Ok(r) if r.status().is_success() => r,
        other => {
            tracing::debug!(%url, ok = other.is_ok(), "no local server answered");
            return None;
        }
    };
    resp.json().await.ok()
}

async fn find_ollama(http: &reqwest::Client, root: &str) -> Option<Found> {
    let tags: OllamaTags = get_json(http, &format!("{root}/api/tags")).await?;
    let loaded: Vec<String> = get_json::<OllamaLoaded>(http, &format!("{root}/api/ps"))
        .await
        .map(|ps| ps.models.into_iter().map(|m| m.name).collect())
        .unwrap_or_default();
    let (mut chat, mut embedders) = (Vec::new(), Vec::new());
    for tag in tags.models {
        let embeds = if tag.capabilities.is_empty() {
            tag.name.contains("embed")
        } else {
            tag.capabilities.iter().any(|c| c == "embedding")
                && !tag.capabilities.iter().any(|c| c == "completion")
        };
        let model = FoundModel {
            loaded: loaded.contains(&tag.name),
            name: tag.name,
        };
        if embeds {
            embedders.push(model);
        } else {
            chat.push(model);
        }
    }
    Some(Found {
        server: ServerKind::Ollama,
        url: format!("{root}/v1"),
        chat,
        embedders,
    })
}

async fn find_lm_studio(http: &reqwest::Client, root: &str) -> Option<Found> {
    let listing: LmStudioModels = get_json(http, &format!("{root}/api/v1/models")).await?;
    let (mut chat, mut embedders) = (Vec::new(), Vec::new());
    for model in listing.models {
        let found = FoundModel {
            loaded: !model.loaded_instances.is_empty(),
            name: model.key,
        };
        match model.kind.as_str() {
            "embedding" | "embeddings" => embedders.push(found),
            _ => chat.push(found),
        }
    }
    Some(Found {
        server: ServerKind::LmStudio,
        url: format!("{root}/v1"),
        chat,
        embedders,
    })
}

/// The rows of the list: every server's chat models, a loaded one first, each
/// with the embedder its pick would configure — none when `embed_free` is
/// false, since an embedder already configured is never replaced (F5).
pub fn offers(found: &[Found], embed_free: bool) -> Vec<LocalOffer> {
    found
        .iter()
        .flat_map(|server| {
            let embedder = embed_free
                .then(|| pick_embedder(&server.embedders))
                .flatten();
            let mut chat: Vec<&FoundModel> = server.chat.iter().collect();
            // Stable: a loaded model first, the server's own order otherwise.
            chat.sort_by_key(|m| !m.loaded);
            chat.into_iter().map(move |m| LocalOffer {
                server: server.server,
                url: server.url.clone(),
                model: m.name.clone(),
                loaded: m.loaded,
                embedder: embedder.clone(),
            })
        })
        .collect()
}

/// A loaded embedder first, then `bge-m3` — the model the app is calibrated
/// against — then the first listed.
fn pick_embedder(embedders: &[FoundModel]) -> Option<String> {
    embedders
        .iter()
        .find(|m| m.loaded)
        .or_else(|| embedders.iter().find(|m| m.name.contains("bge-m3")))
        .or_else(|| embedders.first())
        .map(|m| m.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LM Studio's `/api/v1/models` as measured on 1.1.7 with one model loaded
    /// at 4096, trimmed to what is read and a little of what is not.
    const LOADED: &str = r#"{"models":[
        {"type":"llm","publisher":"bartowski","key":"google_gemma-4-e4b-it",
         "display_name":"Google Gemma 4 E4B Instruct","max_context_length":131072,
         "loaded_instances":[{"id":"google_gemma-4-e4b-it",
            "config":{"context_length":4096,"parallel":4,"flash_attention":true}}]},
        {"type":"embedding","key":"text-embedding-nomic-embed-text-v1.5",
         "loaded_instances":[]}]}"#;

    fn parse(body: &str) -> LmStudioModels {
        serde_json::from_str(body).expect("the listing parses")
    }

    #[test]
    fn the_configured_models_window() {
        assert_eq!(parse(LOADED).window_of("google_gemma-4-e4b-it"), Some(4096));
    }

    /// A second instance of the same model has an identifier of its own, and a
    /// request naming it runs there, not in the first.
    #[test]
    fn an_instance_named_wins() {
        const TWO: &str = r#"{"models":[{"key":"qwen","loaded_instances":[
            {"id":"qwen","config":{"context_length":8192}},
            {"id":"qwen:2","config":{"context_length":32768}}]}]}"#;
        assert_eq!(parse(TWO).window_of("qwen:2"), Some(32768));
        assert_eq!(parse(TWO).window_of("qwen"), Some(8192));
    }

    /// An instance loaded under an identifier of the user's own is still the
    /// model's only one, and a request naming the key runs in it.
    #[test]
    fn the_only_instance_of_the_key() {
        const ALIASED: &str = r#"{"models":[{"key":"qwen","loaded_instances":[
            {"id":"my-qwen","config":{"context_length":16384}}]}]}"#;
        assert_eq!(parse(ALIASED).window_of("qwen"), Some(16384));
        assert_eq!(parse(ALIASED).window_of("my-qwen"), Some(16384));
    }

    /// Silence, each way: a model that is not loaded, a model the server does
    /// not hold, a window of zero, two instances and neither named.
    #[test]
    fn silence_is_never_a_guess() {
        const COLD: &str = r#"{"models":[{"key":"gemma","loaded_instances":[]}]}"#;
        const ZERO: &str = r#"{"models":[{"key":"gemma","loaded_instances":[{"id":"gemma","config":{"context_length":0}}]}]}"#;
        const TWO: &str = r#"{"models":[{"key":"gemma","loaded_instances":[
            {"id":"a","config":{"context_length":8192}},
            {"id":"b","config":{"context_length":4096}}]}]}"#;
        assert_eq!(parse(COLD).window_of("gemma"), None);
        assert_eq!(parse(LOADED).window_of("absent"), None);
        assert_eq!(parse(ZERO).window_of("gemma"), None);
        assert_eq!(parse(TWO).window_of("gemma"), None);
    }

    /// LM Studio's answer to a path it does not serve is a `200` — and must not
    /// parse as any of the shapes here, or it would be taken for Ollama, or for
    /// itself when another server sent it.
    #[test]
    fn an_error_body_is_neither_server() {
        const ERROR: &str = r#"{"error":"Unexpected endpoint or method. (GET /api/version)"}"#;
        assert!(serde_json::from_str::<OllamaVersion>(ERROR).is_err());
        assert!(serde_json::from_str::<LmStudioModels>(ERROR).is_err());
        // Ollama's own answer to the version question.
        let v: OllamaVersion = serde_json::from_str(r#"{"version":"0.35.1"}"#).unwrap();
        assert_eq!(v.version, "0.35.1");
    }

    /// A server on a free port that answers `n` requests from `routes` — a
    /// path and its `200` body; anything else is a `404` — one per connection.
    fn serve(routes: &'static [(&'static str, &'static str)], n: usize) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..n {
                let Ok((mut sock, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0u8; 2048];
                let read = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..read]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or_default();
                let (status, body) = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, b)| ("200 OK", *b))
                    .unwrap_or(("404 Not Found", "{}"));
                let _ = write!(
                    sock,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}")
    }

    /// A port nothing listens on.
    fn closed() -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        format!("http://127.0.0.1:{port}")
    }

    /// Ollama's `/api/tags` as measured on 0.35.1, a chat model and an
    /// embedder, trimmed to what is read and a little of what is not.
    const TAGS: &str = r#"{"models":[
        {"name":"gemma4:e4b","model":"gemma4:e4b","size":6583656505,
         "details":{"family":"gemma4","context_length":131072},
         "capabilities":["completion","vision","audio","tools","thinking"]},
        {"name":"nomic-embed-text:latest","capabilities":["embedding"]}]}"#;
    const PS: &str = r#"{"models":[{"name":"gemma4:e4b","context_length":4096}]}"#;

    #[tokio::test]
    async fn ollama_is_found_with_its_chat_models_and_its_embedders() {
        let ollama = serve(&[("/api/tags", TAGS), ("/api/ps", PS)], 2);
        let found = find_at(&reqwest::Client::new(), &ollama, &closed()).await;
        assert_eq!(
            found,
            [Found {
                server: ServerKind::Ollama,
                url: format!("{ollama}/v1"),
                chat: vec![FoundModel {
                    name: "gemma4:e4b".into(),
                    loaded: true
                }],
                embedders: vec![FoundModel {
                    name: "nomic-embed-text:latest".into(),
                    loaded: false
                }],
            }]
        );
    }

    #[tokio::test]
    async fn lm_studio_is_found_by_its_listing() {
        let lm_studio = serve(&[("/api/v1/models", LOADED)], 1);
        let found = find_at(&reqwest::Client::new(), &closed(), &lm_studio).await;
        assert_eq!(found.len(), 1, "{found:?}");
        let found = &found[0];
        assert_eq!(found.server, ServerKind::LmStudio);
        assert_eq!(found.url, format!("{lm_studio}/v1"));
        assert_eq!(
            found.chat,
            [FoundModel {
                name: "google_gemma-4-e4b-it".into(),
                loaded: true
            }]
        );
        assert_eq!(
            found.embedders[0].name,
            "text-embedding-nomic-embed-text-v1.5"
        );
    }

    /// A server on the port that is not the one expected is not taken for it:
    /// LM Studio's `200 {"error":…}` where Ollama's tags were asked for, a
    /// `404` where LM Studio's listing was. Nothing answering is nothing found.
    #[tokio::test]
    async fn only_the_servers_own_answer_counts() {
        const ERROR: &str = r#"{"error":"Unexpected endpoint or method. (GET /api/tags)"}"#;
        let not_ollama = serve(&[("/api/tags", ERROR), ("/api/ps", ERROR)], 1);
        let not_lm_studio = serve(&[], 1);
        let found = find_at(&reqwest::Client::new(), &not_ollama, &not_lm_studio).await;
        assert!(found.is_empty(), "{found:?}");
        let found = find_at(&reqwest::Client::new(), &closed(), &closed()).await;
        assert!(found.is_empty(), "{found:?}");
    }

    /// An Ollama older than its `capabilities` field: the name decides.
    #[tokio::test]
    async fn without_capabilities_an_embedder_is_told_by_its_name() {
        const OLD: &str =
            r#"{"models":[{"name":"llama3:8b"},{"name":"mxbai-embed-large:latest"}]}"#;
        let ollama = serve(&[("/api/tags", OLD)], 2);
        let found = find_at(&reqwest::Client::new(), &ollama, &closed()).await;
        assert_eq!(found[0].chat[0].name, "llama3:8b");
        assert_eq!(found[0].embedders[0].name, "mxbai-embed-large:latest");
    }

    fn model(name: &str, loaded: bool) -> FoundModel {
        FoundModel {
            name: name.into(),
            loaded,
        }
    }

    /// Every server's chat models become rows, a loaded one first; each row
    /// carries its server's embedder only when the embedder is free (F5).
    #[test]
    fn the_rows_put_a_loaded_model_first_and_carry_the_embedder() {
        let found = [
            Found {
                server: ServerKind::Ollama,
                url: "http://127.0.0.1:11434/v1".into(),
                chat: vec![model("qwen3.5:9b", false), model("gemma4:e4b", true)],
                embedders: vec![
                    model("nomic-embed-text:latest", false),
                    model("bge-m3:latest", false),
                ],
            },
            Found {
                server: ServerKind::LmStudio,
                url: "http://127.0.0.1:1234/v1".into(),
                chat: vec![model("google_gemma-4-e4b-it", false)],
                embedders: vec![],
            },
        ];
        let rows = offers(&found, true);
        let named: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|r| (r.model.as_str(), r.embedder.as_deref()))
            .collect();
        assert_eq!(
            named,
            [
                ("gemma4:e4b", Some("bge-m3:latest")),
                ("qwen3.5:9b", Some("bge-m3:latest")),
                ("google_gemma-4-e4b-it", None),
            ]
        );
        assert!(
            offers(&found, false).iter().all(|r| r.embedder.is_none()),
            "a configured embedder is never replaced"
        );
    }

    #[test]
    fn a_loaded_embedder_then_bge_m3_then_the_first() {
        assert_eq!(
            pick_embedder(&[
                model("a-embed", false),
                model("bge-m3", false),
                model("b-embed", true)
            ]),
            Some("b-embed".into())
        );
        assert_eq!(
            pick_embedder(&[model("a-embed", false), model("bge-m3", false)]),
            Some("bge-m3".into())
        );
        assert_eq!(
            pick_embedder(&[model("a-embed", false)]),
            Some("a-embed".into())
        );
        assert_eq!(pick_embedder(&[]), None);
    }
}
