//! `youtube_watch` (spec §9.9): what a YouTube video says **and shows**.
//!
//! Two halves, and the second one degrades into the first:
//!
//! - **metadata** — title, channel, length, the author's description — from the
//!   watch page (with oEmbed as a fallback). Free, no key, works with a local
//!   model;
//! - **watching** — the URL is handed to Gemini, which ingests it directly and
//!   describes the frames and the audio. Measured 2026-08-01 as the only working
//!   path to "what is shown": every free caption route is now behind YouTube's
//!   PoToken gate, `captions.download` needs the video owner's OAuth, and no
//!   other provider takes video at all. See docs/research/youtube-integration.md.
//!
//! Following `fetch_url`, the tool returns **the answer, not the material**: a
//! 10-minute video costs ~62k tokens at Google and a few hundred in the
//! conversation, which is what makes this usable from a local 8k-context model.
//!
//! `transcript: true` (stage 2, docs/youtube-transcript.md) also asks for the
//! words. It costs **exactly the same** — it is the same request with a longer
//! prompt, and on the 3.x models audio is not even billed apart from video — so
//! it is one provider call for both halves, split on a marker. A transcript
//! larger than the per-file attachment budget is not handed back inline: it
//! becomes a **chat attachment** (spec §9.7), which is already paged
//! (`attachment_read`) and searchable (`attachment_search`), where a tool result
//! would go into the context whole.
//!
//! With no Gemini key the tool still exists and returns the metadata plus a
//! plain statement of what is missing (fork R5a) — a tool that vanishes leaves
//! the model unable to explain itself to the user.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::entities::attachment::{Attachment, decide_mode, inline_tokens_excluding};
use crate::entities::profile::ToolId;
use crate::shared::i18n::Locale;
use crate::shared::video::{VideoRequest, VideoUnderstanding};

use super::web::{ACCEPT_HTML, ACCEPT_LANGUAGE, USER_AGENT};
use super::{ChatEffect, Tool, ToolContext, ToolOutcome};

/// Metadata-fetch timeout. Short: it is a best-effort extra, and a slow page
/// must not delay the actual answer.
const META_TIMEOUT: Duration = Duration::from_secs(15);
/// Ceiling on a single watch request. A 30-minute video takes the provider well
/// under this; the token is what really ends it early.
const WATCH_TIMEOUT: Duration = Duration::from_secs(300);
/// Answer budget. Enough for a summary plus a timestamped outline.
const ANSWER_MAX_TOKENS: usize = 2000;
/// Answer budget with `transcript: true` — the words themselves are the bulk of
/// it (~25 minutes of dense speech). A constant rather than a setting: what makes
/// that acceptable is that the ceiling is **reported** when it is hit, the way
/// the length ceiling already is, instead of quietly returning a prefix.
const TRANSCRIPT_MAX_TOKENS: usize = 8000;
/// How much of the author's description to show in the degraded answer.
const DESCRIPTION_CHARS: usize = 1200;
/// The line the transcript prompt asks for between the two halves of the answer.
/// Deliberately an ASCII token and **not** localized: the model is asked to emit
/// it verbatim, and a translated marker would be one more thing to get wrong.
const TRANSCRIPT_MARKER: &str = "---TRANSCRIPT---";
/// Cap on the video title inside the attachment's display name — it has to stay
/// readable in `/file list` and typeable in `attachment_read`.
const NAME_TITLE_CHARS: usize = 60;

/// What the free paths can tell us about a video.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoMeta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub duration_secs: Option<u32>,
    pub description: Option<String>,
}

impl VideoMeta {
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.author.is_none() && self.duration_secs.is_none()
    }
}

/// Extracts the 11-character video id from any URL form YouTube serves — and
/// from a bare id, which is what a model often passes. Pure.
///
/// Deliberately permissive about the *host*: `youtube.com`, `www.`, `m.`,
/// `music.` and `youtu.be` are all the same video.
pub fn video_id(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if is_bare_id(s) {
        return Some(s.to_string());
    }
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let (host, path_and_query) = rest.split_once('/')?;
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);

    // `split_once('/')` above already ate the leading slash, so paths here are
    // `watch`, `shorts/<id>` — no leading `/`.
    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_and_query, None),
    };

    // youtu.be/<id>
    if host == "youtu.be" {
        return first_segment(path)
            .filter(|s| is_bare_id(s))
            .map(Into::into);
    }
    if !(host == "youtube.com"
        || host == "m.youtube.com"
        || host == "music.youtube.com"
        || host == "youtube-nocookie.com")
    {
        return None;
    }
    // watch?v=<id>
    if path == "watch" {
        return query
            .and_then(|q| query_param(q, "v"))
            .filter(|s| is_bare_id(s))
            .map(Into::into);
    }
    // shorts/<id>, embed/<id>, live/<id>, v/<id>
    for prefix in ["shorts/", "embed/", "live/", "v/"] {
        if let Some(tail) = path.strip_prefix(prefix) {
            return first_segment(tail)
                .filter(|s| is_bare_id(s))
                .map(Into::into);
        }
    }
    None
}

/// Whether this looks like a YouTube link at all — used by `fetch_url` to stop
/// being a dead end for them (fork R6).
pub fn is_youtube_url(url: &str) -> bool {
    video_id(url).is_some() && !is_bare_id(url.trim())
}

/// The canonical watch URL for an id. Gemini accepts every form, but one shape
/// keeps results and logs comparable.
pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

fn is_bare_id(s: &str) -> bool {
    s.len() == 11
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn first_segment(path: &str) -> Option<&str> {
    let seg = path.trim_start_matches('/').split(['/', '?', '#']).next()?;
    (!seg.is_empty()).then_some(seg)
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// Pulls the JSON object that starts at or after `from`, tracking string state so
/// a `}` inside a string doesn't end it early. A regex would be shorter and
/// wrong on exactly the pages that matter.
fn balanced_object(s: &str, from: usize) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = s[from..].find('{')? + from;
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    for (offset, &c) in bytes[start..].iter().enumerate() {
        let i = start + offset;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return s.get(start..=i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Metadata from the watch page's `ytInitialPlayerResponse`. Pure — tested on a
/// saved fixture, no network.
pub fn parse_watch_page(html: &str) -> Option<VideoMeta> {
    let at = html.find("ytInitialPlayerResponse")?;
    let json = balanced_object(html, at)?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let d = v.get("videoDetails")?;
    let text = |k: &str| {
        d.get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let meta = VideoMeta {
        title: text("title"),
        author: text("author"),
        duration_secs: d
            .get("lengthSeconds")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse().ok()),
        description: text("shortDescription"),
    };
    (!meta.is_empty()).then_some(meta)
}

/// Title/channel from the oEmbed endpoint — the fallback when the watch page's
/// shape changes. No key, no scraping; gives no duration or description.
pub fn parse_oembed(body: &str) -> Option<VideoMeta> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let meta = VideoMeta {
        title: text("title"),
        author: text("author_name"),
        ..Default::default()
    };
    (!meta.is_empty()).then_some(meta)
}

/// `12:34` / `1:02:03`.
pub fn format_duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Splits an answer asked to carry a description, [`TRANSCRIPT_MARKER`], and
/// then the transcript. Returns the description and the transcript, if any.
///
/// **The fallback is not to guess.** With no marker the whole answer is the
/// description and the transcript is `None` — the caller then says plainly that
/// none came back. Splitting on a heuristic would risk filing half a description
/// as a transcript, which is worse than reporting the miss.
///
/// Tolerant about the marker's dressing (`## TRANSCRIPT`, `**TRANSCRIPT**`,
/// `TRANSCRIPT:`): the model is asked for one exact line, and models decorate.
pub fn split_transcript(answer: &str) -> (String, Option<String>) {
    match answer.lines().position(is_marker_line) {
        Some(i) => {
            let head: Vec<&str> = answer.lines().take(i).collect();
            let tail: Vec<&str> = answer.lines().skip(i + 1).collect();
            let body = tail.join("\n");
            let body = body.trim();
            (
                head.join("\n").trim().to_string(),
                (!body.is_empty()).then(|| body.to_string()),
            )
        }
        None => (answer.trim().to_string(), None),
    }
}

/// Is this the separator line? True for the marker under any decoration the
/// model might add around the word itself.
fn is_marker_line(line: &str) -> bool {
    let stripped: String = line
        .chars()
        .filter(|c| !matches!(c, '-' | '=' | '#' | '*' | ':' | '_' | ' ' | '\t'))
        .collect();
    stripped.eq_ignore_ascii_case("TRANSCRIPT")
}

/// Makes a segment transcript's timestamps absolute — measured from the start of
/// the **video**, which is what the attachment's header says they are.
///
/// Measured live 2026-08-01: asked for a 0:40–1:20 clip, Gemini numbers it from
/// zero **despite the prompt asking for video-relative timestamps**. The
/// research doc left this open ("stating it in the prompt costs nothing and
/// removes the question") — it does not remove it, so the fix is arithmetic we
/// control rather than an instruction we hope is followed. Left alone, a reader
/// would seek to 0:03 for a line that is really at 0:43.
///
/// Whether the provider complied is decided by the **first** timestamp: one
/// below `offset` means the clip was numbered from zero. That is what keeps a
/// future, better-behaved model from being double-shifted; the discriminator is
/// only ambiguous when `offset` is tiny, where so is the error. Lines that carry
/// no timestamp pass through untouched.
fn absolute_timestamps(body: &str, offset: u32) -> String {
    if offset == 0 {
        return body.to_string();
    }
    let first = body.lines().find_map(|l| leading_stamp(l).map(|(_, s)| s));
    match first {
        // Already absolute (or nothing to go on) — leave it be.
        Some(s) if s >= offset => return body.to_string(),
        None => return body.to_string(),
        _ => {}
    }
    body.lines()
        .map(|line| match leading_stamp(line) {
            Some((end, secs)) => {
                format!("[{}]{}", format_duration(secs + offset), &line[end + 1..])
            }
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A leading `[h:mm:ss]` / `[m:ss]` timestamp: the index of the closing bracket
/// and the time in seconds.
fn leading_stamp(line: &str) -> Option<(usize, u32)> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let parts: Vec<&str> = rest[..end].split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let nums: Option<Vec<u32>> = parts.iter().map(|p| p.trim().parse::<u32>().ok()).collect();
    let nums = nums?;
    let secs = nums.iter().fold(0u32, |acc, n| acc * 60 + n);
    Some((end + 1, secs))
}

/// `0:40-1:20` / `from 0:40` — the segment as it goes into the attachment's name
/// and source key. `None` when the whole video was requested.
fn segment_label(start: Option<u32>, end: Option<u32>) -> Option<String> {
    match (start, end) {
        (None, None) => None,
        (s, Some(e)) => Some(format!(
            "{}-{}",
            format_duration(s.unwrap_or(0)),
            format_duration(e)
        )),
        (Some(s), None) => Some(format!("{}-", format_duration(s))),
    }
}

/// The attachment's source key — the field attachments dedupe by. A transcript
/// has no path, so it needs a synthetic one; the **segment is part of it**, so
/// transcribing the same span twice replaces it while two different spans of one
/// video coexist.
fn transcript_source(id: &str, segment: Option<&str>) -> String {
    match segment {
        Some(s) => format!("youtube:{id}#transcript@{s}"),
        None => format!("youtube:{id}#transcript"),
    }
}

/// The display name: meaningful in `/file list`, typeable in `attachment_read`.
fn transcript_name(meta: &VideoMeta, id: &str, segment: Option<&str>, loc: &Locale) -> String {
    let title = meta
        .title
        .as_deref()
        .map(|t| {
            let clipped: String = t.chars().take(NAME_TITLE_CHARS).collect();
            clipped.trim().to_string()
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| format!("YouTube {id}"));
    match segment {
        Some(s) => loc.tf(
            "tool.youtube_watch.attachment.name_segment",
            &[("title", &title), ("segment", s)],
        ),
        None => loc.tf("tool.youtube_watch.attachment.name", &[("title", &title)]),
    }
}

/// The attachment's own header. Read by the model (axis A), and it is what makes
/// the file self-describing when it is read page by page much later — a bare
/// wall of timestamps says nothing about which video it came from, nor that it
/// was produced by a model rather than taken from official captions.
fn transcript_header(
    meta: &VideoMeta,
    url: &str,
    segment: Option<&str>,
    truncated: bool,
    loc: &Locale,
) -> String {
    let title = meta.title.as_deref().unwrap_or_default();
    let mut out = loc.tf(
        "tool.youtube_watch.attachment.header",
        &[("title", title.trim()), ("url", url)],
    );
    if let Some(s) = segment {
        out.push('\n');
        out.push_str(&loc.tf("tool.youtube_watch.attachment.segment", &[("range", s)]));
    }
    out.push('\n');
    out.push_str(loc.t("tool.youtube_watch.attachment.caveat"));
    if truncated {
        out.push('\n');
        out.push_str(loc.t("tool.youtube_watch.attachment.truncated"));
    }
    out
}

/// `youtube_watch` — describe a YouTube video.
pub struct YoutubeWatch {
    http: reqwest::Client,
    /// `None` — no video provider is configured; the tool degrades to metadata.
    video: Option<Arc<dyn VideoUnderstanding>>,
    /// Refuse videos longer than this many minutes (`0` — no ceiling).
    max_minutes: u32,
}

impl YoutubeWatch {
    pub fn new(video: Option<Arc<dyn VideoUnderstanding>>, max_minutes: u32) -> Self {
        let http = reqwest::Client::builder()
            .timeout(META_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            video,
            max_minutes,
        }
    }

    /// Best effort: the watch page, then oEmbed. Never fatal — the answer is
    /// worth more than its header.
    async fn fetch_meta(&self, id: &str) -> Option<VideoMeta> {
        fetch_meta(&self.http, id).await
    }
}

/// Free-function form, so `fetch_url` can reuse it with its own client (R6).
pub async fn fetch_meta(http: &reqwest::Client, id: &str) -> Option<VideoMeta> {
    if let Ok(resp) = http
        .get(watch_url(id))
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, ACCEPT_HTML)
        .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
        .send()
        .await
        && resp.status().is_success()
        && let Ok(body) = resp.text().await
        && let Some(meta) = parse_watch_page(&body)
    {
        return Some(meta);
    }
    let oembed = format!(
        "https://www.youtube.com/oembed?url={}&format=json",
        urlencoding_minimal(&watch_url(id))
    );
    let resp = http.get(oembed).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    parse_oembed(&resp.text().await.ok()?)
}

impl YoutubeWatch {
    /// A one-line header for the answer: `Title — Channel · 3:33`.
    pub(crate) fn header(meta: &VideoMeta, url: &str, loc: &Locale) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = &meta.title {
            parts.push(t.clone());
        }
        if let Some(a) = &meta.author {
            parts.push(a.clone());
        }
        if let Some(d) = meta.duration_secs {
            parts.push(format_duration(d));
        }
        if parts.is_empty() {
            return loc.tf("tool.youtube_watch.result.header_bare", &[("url", url)]);
        }
        loc.tf(
            "tool.youtube_watch.result.header",
            &[("meta", &parts.join(" · ")), ("url", url)],
        )
    }

    /// Everything the free paths know — the answer when we cannot watch, and
    /// what `fetch_url` returns for a YouTube link instead of nothing (R6).
    pub(crate) fn meta_block(meta: &VideoMeta, url: &str, loc: &Locale) -> String {
        let mut out = Self::header(meta, url, loc);
        if let Some(desc) = &meta.description {
            let clipped: String = desc.chars().take(DESCRIPTION_CHARS).collect();
            out.push('\n');
            out.push_str(&loc.tf(
                "tool.youtube_watch.result.description",
                &[("text", clipped.trim())],
            ));
        }
        out
    }
}

/// Percent-encodes just enough for the oEmbed query (`:` `/` `?` `=` `&`). Not a
/// general encoder — the input is a URL we built ourselves.
fn urlencoding_minimal(url: &str) -> String {
    url.chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            '?' => "%3F".to_string(),
            '=' => "%3D".to_string(),
            '&' => "%26".to_string(),
            c => c.to_string(),
        })
        .collect()
}

#[async_trait::async_trait]
impl Tool for YoutubeWatch {
    fn id(&self) -> ToolId {
        super::YOUTUBE_WATCH_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::ExternalWorld
    }
    fn ui_label(&self) -> &'static str {
        "watch a YouTube video"
    }
    fn gate(&self) -> Option<super::meta::ToolGate> {
        // Network access, like web_search/fetch_url.
        Some(super::meta::ToolGate::Web)
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.youtube_watch.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": loc.t("tool.youtube_watch.param.url")},
                "focus": {"type": "string", "description": loc.t("tool.youtube_watch.param.focus")},
                "start": {"type": "integer", "description": loc.t("tool.youtube_watch.param.start")},
                "end": {"type": "integer", "description": loc.t("tool.youtube_watch.param.end")},
                "transcript": {"type": "boolean", "description": loc.t("tool.youtube_watch.param.transcript")}
            },
            "required": ["url"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let raw = args
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.youtube_watch.err.url_empty")))?;
        let Some(id) = video_id(raw) else {
            return Ok(ToolOutcome::text(
                ctx.loc
                    .tf("tool.youtube_watch.err.not_youtube", &[("url", raw)]),
            ));
        };
        let url = watch_url(&id);
        let focus = args
            .get("focus")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let start = args.get("start").and_then(|v| v.as_u64()).map(|v| v as u32);
        let end = args.get("end").and_then(|v| v.as_u64()).map(|v| v as u32);
        let want_transcript = args
            .get("transcript")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if let (Some(s), Some(e)) = (start, end)
            && e <= s
        {
            return Ok(ToolOutcome::text(
                ctx.loc.t("tool.youtube_watch.err.bad_range").to_string(),
            ));
        }

        let meta = self.fetch_meta(&id).await.unwrap_or_default();

        let Some(video) = &self.video else {
            // Not configured: say so plainly and hand over what is free anyway.
            let mut out = Self::meta_block(&meta, &url, ctx.loc);
            out.push('\n');
            out.push_str(ctx.loc.t("tool.youtube_watch.result.not_configured"));
            return Ok(ToolOutcome::text(out));
        };

        // The length gate. Only a *segment* is charged when bounds are given, so
        // that is what gets measured against the ceiling.
        let mut end_secs = end;
        let mut note: Option<String> = None;
        if self.max_minutes > 0 {
            let cap = self.max_minutes * 60;
            match (meta.duration_secs, start, end) {
                (_, s, Some(e)) if e.saturating_sub(s.unwrap_or(0)) > cap => {
                    return Ok(ToolOutcome::text(too_long(
                        ctx.loc,
                        e.saturating_sub(s.unwrap_or(0)),
                        cap,
                    )));
                }
                (Some(d), s, None) if d.saturating_sub(s.unwrap_or(0)) > cap => {
                    return Ok(ToolOutcome::text(too_long(
                        ctx.loc,
                        d.saturating_sub(s.unwrap_or(0)),
                        cap,
                    )));
                }
                (None, s, None) => {
                    // Length unknown (metadata failed): clip to the ceiling rather
                    // than write a blank cheque — and say so, so the answer is not
                    // silently about the first part only.
                    end_secs = Some(s.unwrap_or(0) + cap);
                    note = Some(ctx.loc.tf(
                        "tool.youtube_watch.result.unknown_length",
                        &[("minutes", &self.max_minutes.to_string())],
                    ));
                }
                _ => {}
            }
        }

        let task = match focus {
            Some(f) => ctx
                .loc
                .tf("tool.youtube_watch.prompt.focus", &[("focus", f)]),
            None => ctx.loc.t("tool.youtube_watch.prompt.default").to_string(),
        };
        // One request for both halves (fork F4): the video is ingested either
        // way, so a second call would double the expensive part for a formatting
        // convenience.
        let prompt = if want_transcript {
            ctx.loc.tf(
                "tool.youtube_watch.prompt.transcript",
                &[("task", &task), ("marker", TRANSCRIPT_MARKER)],
            )
        } else {
            task
        };
        let request = VideoRequest {
            url: url.clone(),
            prompt,
            start_secs: start,
            end_secs,
            max_output_tokens: if want_transcript {
                TRANSCRIPT_MAX_TOKENS
            } else {
                ANSWER_MAX_TOKENS
            },
        };

        let answer =
            match tokio::time::timeout(WATCH_TIMEOUT, video.describe(request, &ctx.cancel)).await {
                Ok(Ok(answer)) => answer,
                Ok(Err(err)) => {
                    // Graceful degradation, the `fetch_url` shape: the metadata is
                    // still worth returning, and the model needs to know why.
                    let mut out = Self::meta_block(&meta, &url, ctx.loc);
                    out.push('\n');
                    out.push_str(&ctx.loc.tf(
                        "tool.youtube_watch.result.failed",
                        &[("err", &err.to_string())],
                    ));
                    return Ok(ToolOutcome::text(out));
                }
                Err(_) => {
                    let mut out = Self::meta_block(&meta, &url, ctx.loc);
                    out.push('\n');
                    out.push_str(ctx.loc.t("tool.youtube_watch.result.timeout"));
                    return Ok(ToolOutcome::text(out));
                }
            };

        let mut out = Self::header(&meta, &url, ctx.loc);
        if let Some(n) = note {
            out.push('\n');
            out.push_str(&n);
        }
        if !want_transcript {
            out.push('\n');
            out.push_str(answer.text.trim());
            return Ok(ToolOutcome::text(out));
        }

        let (description, transcript) = split_transcript(&answer.text);
        out.push('\n');
        out.push_str(description.trim());
        let Some(body) = transcript else {
            // The marker never came, or nothing followed it (a video with no
            // speech). Report it — a silent absence reads as "there were no
            // words", which is a different claim.
            out.push('\n');
            out.push_str(ctx.loc.t("tool.youtube_watch.result.no_transcript"));
            return Ok(ToolOutcome::text(out));
        };

        let segment = segment_label(start, end_secs);
        // The provider numbers a clip from zero whatever the prompt asks (measured
        // live), so the header's claim is made true here rather than hoped for.
        let body = absolute_timestamps(&body, start.unwrap_or(0));
        let header = transcript_header(&meta, &url, segment.as_deref(), answer.truncated, ctx.loc);
        let text = format!("{header}\n\n{body}");
        let est = crate::shared::tokens::estimate_text(&text) as usize;

        // Small enough to read at once → straight into the result: the model needs
        // no second call, and attaching would put the same text in the pinned
        // block *and* in the history. The threshold is the attachment budget
        // itself (fork F2), which is also why an attached transcript is always by
        // reference: inline requires `est <= max_file_tokens`, and this branch is
        // exactly the other side of that.
        if est <= ctx.attachment_cfg.max_file_tokens {
            out.push('\n');
            out.push_str(&text);
            if answer.truncated {
                out.push('\n');
                out.push_str(ctx.loc.t("tool.youtube_watch.result.transcript_truncated"));
            }
            return Ok(ToolOutcome::text(out));
        }

        let name = transcript_name(&meta, &id, segment.as_deref(), ctx.loc);
        let source = transcript_source(&id, segment.as_deref());
        let used = inline_tokens_excluding(&ctx.attachments, &source);
        let mode = decide_mode(est, used, &ctx.attachment_cfg);
        let bytes = text.len();
        let attachment = Attachment::new(name.clone(), source, text, bytes, mode);
        let pages = attachment.page_count(ctx.attachment_cfg.page_tokens);

        out.push('\n');
        out.push_str(&ctx.loc.tf(
            "tool.youtube_watch.result.transcript_attached",
            &[("name", &name), ("pages", &pages.to_string())],
        ));
        if answer.truncated {
            out.push('\n');
            out.push_str(ctx.loc.t("tool.youtube_watch.result.transcript_truncated"));
        }
        Ok(ToolOutcome::with_effects(
            out,
            vec![ChatEffect::AddAttachment(Box::new(attachment))],
        ))
    }
}

fn too_long(loc: &Locale, secs: u32, cap: u32) -> String {
    loc.tf(
        "tool.youtube_watch.result.too_long",
        &[
            ("length", &format_duration(secs)),
            ("cap", &format_duration(cap)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};
    use crate::shared::video::mock::MockVideo;
    use uuid::Uuid;

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    fn ctx() -> (tempfile::TempDir, ToolContext) {
        let (dir, _s, ctx) = super::super::testkit::ctx_with_storage(Uuid::new_v4());
        (dir, ctx)
    }

    #[test]
    fn video_id_covers_every_url_form() {
        let id = "dQw4w9WgXcQ";
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "http://youtube.com/watch?v=dQw4w9WgXcQ",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=10",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ",
            // A bare id: what a model passes about as often as a URL.
            "dQw4w9WgXcQ",
        ] {
            assert_eq!(video_id(url).as_deref(), Some(id), "url: {url}");
        }
    }

    #[test]
    fn video_id_rejects_other_links() {
        for url in [
            "https://example.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/results?search_query=x",
            "https://www.youtube.com/@channel",
            "not a url",
            "",
        ] {
            assert!(video_id(url).is_none(), "should not parse: {url}");
        }
        // is_youtube_url is about *links*, so a bare id is not one — otherwise
        // fetch_url would try to route arbitrary 11-char text here.
        assert!(!is_youtube_url("dQw4w9WgXcQ"));
        assert!(is_youtube_url("https://youtu.be/dQw4w9WgXcQ"));
    }

    #[test]
    fn balanced_object_survives_braces_inside_strings() {
        // The reason this isn't a regex: the real page has `}` inside strings.
        let html = r#"var ytInitialPlayerResponse = {"a":"} not the end {","b":{"c":1}};more"#;
        let json = balanced_object(html, html.find("ytInitialPlayerResponse").unwrap()).unwrap();
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["b"]["c"], 1);
    }

    #[test]
    fn parse_watch_page_reads_video_details() {
        let html = r#"<html><script>var ytInitialPlayerResponse = {"videoDetails":{
            "videoId":"dQw4w9WgXcQ","title":"Never Gonna Give You Up",
            "lengthSeconds":"213","author":"Rick Astley",
            "shortDescription":"The official video"}};</script></html>"#;
        let m = parse_watch_page(html).unwrap();
        assert_eq!(m.title.as_deref(), Some("Never Gonna Give You Up"));
        assert_eq!(m.author.as_deref(), Some("Rick Astley"));
        assert_eq!(m.duration_secs, Some(213));
        assert_eq!(m.description.as_deref(), Some("The official video"));
    }

    #[test]
    fn parse_watch_page_none_without_the_blob() {
        assert!(parse_watch_page("<html><body>nothing here</body></html>").is_none());
    }

    #[test]
    fn parse_oembed_gives_title_and_author() {
        let m = parse_oembed(r#"{"title":"T","author_name":"A","type":"video"}"#).unwrap();
        assert_eq!(m.title.as_deref(), Some("T"));
        assert_eq!(m.author.as_deref(), Some("A"));
        assert!(m.duration_secs.is_none());
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(59), "0:59");
        assert_eq!(format_duration(213), "3:33");
        assert_eq!(format_duration(3723), "1:02:03");
    }

    #[tokio::test]
    async fn without_a_provider_it_returns_metadata_and_says_what_is_missing() {
        let (_d, ctx) = ctx();
        let tool = YoutubeWatch::new(None, 30);
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"url": "https://youtu.be/dQw4w9WgXcQ"}),
            )
            .await
            .unwrap();
        // The point of R5a: the tool doesn't vanish, it explains itself.
        assert!(
            out.result
                .contains(ru().t("tool.youtube_watch.result.not_configured")),
            "got: {}",
            out.result
        );
    }

    /// Every degraded answer must **close the door**, not just describe the lock.
    ///
    /// A live run showed why: told only that video understanding was not
    /// configured, the model read that as a local limitation and spent five
    /// rounds rediscovering the dead ends this project had already measured —
    /// scraping `captionTracks` and hitting `timedtext` (200, empty body),
    /// `pip install youtube-transcript-api` (no pip in the sandbox), looking for
    /// `yt-dlp`/`ffmpeg` (absent), and searching the web for a transcript. So the
    /// message names those routes by the tool ids it would reach for. Same defect
    /// class as the by-reference attachment block, fixed the same way.
    #[test]
    fn degraded_answers_name_the_routes_that_do_not_work() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = locale(lang);
            for key in [
                "tool.youtube_watch.result.not_configured",
                "tool.youtube_watch.result.failed",
                "tool.youtube_watch.result.timeout",
            ] {
                let text = loc.t(key);
                for route in [super::super::PYTHON_EXEC_ID, super::super::FETCH_URL_ID] {
                    assert!(
                        text.contains(route),
                        "{lang:?} {key} must say {route} won't get around it: {text}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn a_non_youtube_url_is_reported_not_watched() {
        let (_d, ctx) = ctx();
        let mock = Arc::new(MockVideo::ok("should not be called"));
        let tool = YoutubeWatch::new(Some(mock.clone()), 30);
        let out = tool
            .invoke(&ctx, serde_json::json!({"url": "https://example.com/x"}))
            .await
            .unwrap();
        assert!(mock.taken().is_none(), "the provider must not be called");
        assert!(out.result.contains("example.com"), "got: {}", out.result);
    }

    #[tokio::test]
    async fn the_segment_is_passed_through_to_the_provider() {
        let (_d, ctx) = ctx();
        let mock = Arc::new(MockVideo::ok("described"));
        let tool = YoutubeWatch::new(Some(mock.clone()), 30);
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"url": "dQw4w9WgXcQ", "start": 40, "end": 80,
                                   "focus": "what is on the whiteboard"}),
            )
            .await
            .unwrap();
        let req = mock.taken().expect("the provider was called");
        assert_eq!(req.start_secs, Some(40));
        assert_eq!(req.end_secs, Some(80));
        assert_eq!(req.url, watch_url("dQw4w9WgXcQ"));
        assert!(
            req.prompt.contains("whiteboard"),
            "focus reaches the prompt: {}",
            req.prompt
        );
        assert!(out.result.contains("described"), "got: {}", out.result);
    }

    #[tokio::test]
    async fn a_reversed_range_is_refused_before_spending_anything() {
        let (_d, ctx) = ctx();
        let mock = Arc::new(MockVideo::ok("x"));
        let tool = YoutubeWatch::new(Some(mock.clone()), 30);
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"url": "dQw4w9WgXcQ", "start": 80, "end": 40}),
            )
            .await
            .unwrap();
        assert!(mock.taken().is_none(), "nothing should be requested");
        assert_eq!(out.result, ru().t("tool.youtube_watch.err.bad_range"));
    }

    #[tokio::test]
    async fn a_segment_over_the_ceiling_is_refused_and_names_the_ceiling() {
        let (_d, ctx) = ctx();
        let mock = Arc::new(MockVideo::ok("x"));
        // 5-minute ceiling, a 10-minute segment asked for.
        let tool = YoutubeWatch::new(Some(mock.clone()), 5);
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"url": "dQw4w9WgXcQ", "start": 0, "end": 600}),
            )
            .await
            .unwrap();
        assert!(mock.taken().is_none(), "the ceiling must gate the spend");
        assert!(out.result.contains("5:00"), "names the cap: {}", out.result);
    }

    #[tokio::test]
    async fn a_provider_failure_degrades_to_metadata_with_the_reason() {
        let (_d, ctx) = ctx();
        let mock = Arc::new(MockVideo::failing("quota exhausted"));
        let tool = YoutubeWatch::new(Some(mock), 30);
        let out = tool
            .invoke(&ctx, serde_json::json!({"url": "dQw4w9WgXcQ"}))
            .await
            .unwrap();
        assert!(
            out.result.contains("quota exhausted"),
            "the reason reaches the model: {}",
            out.result
        );
    }

    // ---- stage 2: the words, not just the description (docs/youtube-transcript.md)

    #[test]
    fn split_transcript_takes_both_halves_and_tolerates_decoration() {
        let (d, t) = split_transcript("a summary\n---TRANSCRIPT---\n[0:01] hello\n[0:04] world");
        assert_eq!(d, "a summary");
        assert_eq!(t.as_deref(), Some("[0:01] hello\n[0:04] world"));
        // Models decorate the one line they were asked for.
        for marker in [
            "## Transcript",
            "**TRANSCRIPT**",
            "TRANSCRIPT:",
            "=== transcript ===",
        ] {
            let (d, t) = split_transcript(&format!("desc\n{marker}\n[0:01] hi"));
            assert_eq!(d, "desc", "marker: {marker}");
            assert_eq!(t.as_deref(), Some("[0:01] hi"), "marker: {marker}");
        }
    }

    #[test]
    fn split_transcript_does_not_guess_when_the_marker_is_missing() {
        // Filing half a description as a transcript would be worse than
        // reporting the miss — so everything is the description.
        let (d, t) = split_transcript("just a description, no marker");
        assert_eq!(d, "just a description, no marker");
        assert!(t.is_none());
        // A marker with nothing under it (a video with no speech) is the same.
        let (d, t) = split_transcript("desc\n---TRANSCRIPT---\n   \n");
        assert_eq!(d, "desc");
        assert!(t.is_none());
    }

    #[test]
    fn the_source_key_separates_segments_and_repeats_the_same_one() {
        let id = "dQw4w9WgXcQ";
        assert_eq!(
            transcript_source(id, segment_label(Some(40), Some(80)).as_deref()),
            transcript_source(id, segment_label(Some(40), Some(80)).as_deref()),
            "the same segment replaces the previous transcript"
        );
        assert_ne!(
            transcript_source(id, segment_label(Some(40), Some(80)).as_deref()),
            transcript_source(id, segment_label(Some(80), Some(120)).as_deref()),
            "a different segment is a different attachment"
        );
        assert_ne!(
            transcript_source(id, None),
            transcript_source(id, segment_label(Some(0), Some(80)).as_deref())
        );
    }

    #[test]
    fn segment_timestamps_are_made_absolute_but_only_when_they_are_relative() {
        let clip = "[0:00:00] first line\nno stamp here\n[0:00:37] later line";
        // Measured live: the provider numbers a clip from zero, so a 0:40 offset
        // has to be added or the header's "from the start of the video" is a lie.
        let shifted = absolute_timestamps(clip, 40);
        assert!(shifted.starts_with("[0:40] first line"), "{shifted}");
        assert!(shifted.contains("[1:17] later line"), "{shifted}");
        assert!(shifted.contains("no stamp here"), "untouched: {shifted}");

        // A provider that *does* honour the instruction must not be shifted
        // twice — decided by the first timestamp, not by hope.
        let absolute = "[0:40] first line\n[1:17] later line";
        assert_eq!(absolute_timestamps(absolute, 40), absolute);

        // No segment, nothing to do.
        assert_eq!(absolute_timestamps(clip, 0), clip);
        // Nothing parseable, nothing touched.
        assert_eq!(absolute_timestamps("just prose", 40), "just prose");
    }

    #[tokio::test]
    async fn without_the_flag_nothing_about_the_request_changes() {
        let (_d, ctx) = ctx();
        let mock = Arc::new(MockVideo::ok("a description"));
        let tool = YoutubeWatch::new(Some(mock.clone()), 30);
        let out = tool
            .invoke(&ctx, serde_json::json!({"url": "dQw4w9WgXcQ"}))
            .await
            .unwrap();
        let req = mock.taken().unwrap();
        assert!(!req.prompt.contains(TRANSCRIPT_MARKER));
        assert_eq!(req.max_output_tokens, ANSWER_MAX_TOKENS);
        assert!(out.effects.is_empty());
        assert!(out.result.contains("a description"));
    }

    #[tokio::test]
    async fn a_small_transcript_comes_back_in_the_result_without_attaching() {
        let (_d, ctx) = ctx();
        let mock = Arc::new(MockVideo::ok(
            "a description\n---TRANSCRIPT---\n[0:01] hello there",
        ));
        let tool = YoutubeWatch::new(Some(mock.clone()), 30);
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"url": "dQw4w9WgXcQ", "transcript": true}),
            )
            .await
            .unwrap();
        let req = mock.taken().unwrap();
        // One request for both halves, with room for the words.
        assert!(req.prompt.contains(TRANSCRIPT_MARKER), "{}", req.prompt);
        assert_eq!(req.max_output_tokens, TRANSCRIPT_MAX_TOKENS);
        // Small enough to read at once: no second call needed, nothing attached.
        assert!(out.effects.is_empty(), "nothing should be attached");
        assert!(out.result.contains("hello there"), "got: {}", out.result);
        assert!(out.result.contains("a description"), "got: {}", out.result);
    }

    #[tokio::test]
    async fn a_large_transcript_becomes_a_by_reference_attachment() {
        let (_d, mut ctx) = ctx();
        // The threshold is the attachment budget itself (fork F2).
        ctx.attachment_cfg.max_file_tokens = 5;
        let body: String = (0..40)
            .map(|i| format!("[0:{i:02}] a spoken line number {i}\n"))
            .collect();
        let mock = Arc::new(MockVideo::ok(&format!(
            "a description\n---TRANSCRIPT---\n{body}"
        )));
        let tool = YoutubeWatch::new(Some(mock), 30);
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"url": "dQw4w9WgXcQ", "transcript": true, "start": 40, "end": 80}),
            )
            .await
            .unwrap();

        let [ChatEffect::AddAttachment(att)] = out.effects.as_slice() else {
            panic!(
                "expected exactly one attachment effect, got {:?}",
                out.effects
            );
        };
        // The invariant of fork F2: past the per-file budget there is no other
        // mode, so this path never adds an inline attachment.
        assert_eq!(
            att.mode,
            crate::entities::attachment::AttachMode::ByReference
        );
        assert!(att.text.contains("a spoken line number 39"));
        // Self-describing: which video, which segment, and what it is.
        assert!(att.text.contains(&watch_url("dQw4w9WgXcQ")), "{}", att.text);
        assert!(att.text.contains("0:40-1:20"), "{}", att.text);
        assert!(
            att.text
                .contains(ru().t("tool.youtube_watch.attachment.caveat")),
            "{}",
            att.text
        );
        assert_eq!(
            att.source,
            transcript_source("dQw4w9WgXcQ", Some("0:40-1:20"))
        );

        // The result must be actionable: the words are not in it, so it names
        // the attachment and the two tools that reach it.
        assert!(out.result.contains(&att.name), "got: {}", out.result);
        for tool_id in ["attachment_read", "attachment_search"] {
            assert!(out.result.contains(tool_id), "got: {}", out.result);
        }
        assert!(
            !out.result.contains("a spoken line number 39"),
            "the transcript must not also be dumped into the result: {}",
            out.result
        );
    }

    #[tokio::test]
    async fn a_missing_transcript_is_reported_not_silently_absent() {
        let (_d, ctx) = ctx();
        // The marker never came — "no words came back" and "the video has no
        // speech" are different claims, so it says which one it knows.
        let mock = Arc::new(MockVideo::ok("only a description here"));
        let tool = YoutubeWatch::new(Some(mock), 30);
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"url": "dQw4w9WgXcQ", "transcript": true}),
            )
            .await
            .unwrap();
        assert!(out.effects.is_empty());
        assert!(
            out.result
                .contains(ru().t("tool.youtube_watch.result.no_transcript")),
            "got: {}",
            out.result
        );
    }

    #[tokio::test]
    async fn a_cut_off_transcript_says_so_in_both_places() {
        let (_d, mut ctx) = ctx();
        ctx.attachment_cfg.max_file_tokens = 5;
        let body: String = (0..40).map(|i| format!("[0:{i:02}] line {i}\n")).collect();
        let mock = Arc::new(MockVideo::truncated(&format!(
            "a description\n---TRANSCRIPT---\n{body}"
        )));
        let tool = YoutubeWatch::new(Some(mock), 30);
        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({"url": "dQw4w9WgXcQ", "transcript": true}),
            )
            .await
            .unwrap();
        let [ChatEffect::AddAttachment(att)] = out.effects.as_slice() else {
            panic!("expected an attachment");
        };
        // In the result, because that is what the model reads now…
        assert!(
            out.result
                .contains(ru().t("tool.youtube_watch.result.transcript_truncated")),
            "got: {}",
            out.result
        );
        // …and in the file, because that is what it reads later, when the fact
        // that this is only a prefix is otherwise invisible.
        assert!(
            att.text
                .contains(ru().t("tool.youtube_watch.attachment.truncated")),
            "{}",
            att.text
        );
    }

    #[test]
    fn description_and_parameters_are_localized() {
        // §3.5 docs/history/i18n.md — catches a forgotten `_loc`.
        let tool = YoutubeWatch::new(None, 30);
        let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
        let no_cyr = |s: &str| {
            !s.chars()
                .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё')
        };
        assert_ne!(tool.description(ru), tool.description(en));
        assert!(no_cyr(&tool.description(en)));
        assert!(no_cyr(&tool.parameters(en).to_string()));
        assert!(no_cyr(en.t("tool.youtube_watch.prompt.default")));
    }

    /// The one thing the mocked tests cannot show: that a real Gemini call
    /// through our own request shape actually watches the video — and that the
    /// free metadata path still reads a live watch page.
    ///
    /// The assertion is deliberately about something **only visible on screen**:
    /// the clip is the first 20 s of the video, where a transcript would give
    /// almost nothing (the first sung line starts at ~0:18). If this passes, the
    /// "what is shown" half of the feature is real.
    #[tokio::test]
    #[ignore = "requires a real Gemini key (MINDFORK_GEMINI_KEY) and network"]
    async fn watches_a_real_video_live() {
        let Ok(key) = std::env::var("MINDFORK_GEMINI_KEY") else {
            eprintln!("skip: MINDFORK_GEMINI_KEY is not set");
            return;
        };
        let cfg = crate::shared::video::resolve_config(
            &crate::shared::config::VideoSettings::default(),
            Some(key),
        )
        .expect("a key and the default model are enough to configure the slot");
        let engine = Arc::new(crate::shared::video::gemini::GeminiVideo::new(cfg));
        let (_d, ctx) = ctx();
        let tool = YoutubeWatch::new(Some(engine), 30);

        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({
                    "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                    "start": 0, "end": 20,
                    "focus": "What is shown on screen? Describe the people, their \
                              clothes and the setting."
                }),
            )
            .await
            .unwrap();
        eprintln!("--- youtube_watch result ---\n{}", out.result);

        // The metadata path read the live watch page.
        assert!(
            out.result.contains("Never Gonna Give You Up"),
            "the title from the live watch page is missing: {}",
            out.result
        );
        // Not one of the degraded answers.
        for marker in [
            "tool.youtube_watch.result.not_configured",
            "tool.youtube_watch.result.failed",
            "tool.youtube_watch.result.timeout",
        ] {
            let text = ru().t(marker);
            assert!(!out.result.contains(text), "degraded: {}", out.result);
        }
        // Something that can only come from looking at the frames. Bilingual:
        // the prompt goes out in the profile's scaffold language (Ru by default
        // here), so the answer normally comes back in Russian — but the smoke
        // must not fail merely because the model chose the other one.
        let lower = out.result.to_lowercase();
        let visual = [
            "пиджак",
            "пальто",
            "плащ",
            "рубаш",
            "танц",
            "микрофон",
            "сцен",
            "jacket",
            "coat",
            "shirt",
            "danc",
            "microphone",
            "stage",
        ];
        assert!(
            visual.iter().any(|w| lower.contains(w)),
            "no visual detail in the answer: {}",
            out.result
        );
    }

    /// Stage 2's live half: that a real provider returns usable **words** in the
    /// shape the prompt asks for, and that they land as an attachment.
    ///
    /// The mirror of the smoke above, deliberately: that one asserts on
    /// something only *visible* on screen (what a transcript could never give),
    /// this one on something only *said*. Together they cover both halves of the
    /// question the whole track started from.
    ///
    /// The per-file budget is lowered rather than the video lengthened: the
    /// threshold is a user setting, and one short clip keeps the smoke cheap
    /// while still exercising the attachment path end to end.
    #[tokio::test]
    #[ignore = "requires a real Gemini key (MINDFORK_GEMINI_KEY) and network"]
    async fn transcribes_a_real_video_into_an_attachment_live() {
        let Ok(key) = std::env::var("MINDFORK_GEMINI_KEY") else {
            eprintln!("skip: MINDFORK_GEMINI_KEY is not set");
            return;
        };
        let cfg = crate::shared::video::resolve_config(
            &crate::shared::config::VideoSettings::default(),
            Some(key),
        )
        .expect("a key and the default model are enough to configure the slot");
        let engine = Arc::new(crate::shared::video::gemini::GeminiVideo::new(cfg));
        let (_d, mut ctx) = ctx();
        ctx.attachment_cfg.max_file_tokens = 20;
        let tool = YoutubeWatch::new(Some(engine), 30);

        let out = tool
            .invoke(
                &ctx,
                serde_json::json!({
                    "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                    "start": 40, "end": 80,
                    "transcript": true
                }),
            )
            .await
            .unwrap();
        eprintln!("--- youtube_watch result ---\n{}", out.result);

        let [ChatEffect::AddAttachment(att)] = out.effects.as_slice() else {
            panic!(
                "the transcript should have been attached: {:?}",
                out.effects
            );
        };
        eprintln!("--- attachment {:?} ---\n{}", att.name, att.text);

        // The words themselves — this segment is squarely inside the chorus.
        let lower = att.text.to_lowercase();
        assert!(
            lower.contains("never gonna"),
            "no spoken words in the transcript: {}",
            att.text
        );
        // In the shape the prompt asked for: one line per utterance, timestamped…
        let stamps: Vec<u32> = att
            .text
            .lines()
            .filter_map(|l| leading_stamp(l).map(|(_, s)| s))
            .collect();
        assert!(!stamps.is_empty(), "no timestamped lines: {}", att.text);
        // …and measured from the start of the **video**, as the header claims.
        // The provider numbers the clip from zero regardless of the prompt
        // (measured 2026-08-01), so this asserts our own correction, live.
        assert!(
            stamps.iter().all(|&s| (40..=80 + 5).contains(&s)),
            "timestamps must be absolute, inside the requested 0:40-1:20: {stamps:?}"
        );
        // Self-describing, and pointing the model at the tools that reach it.
        assert!(att.text.contains(&watch_url("dQw4w9WgXcQ")));
        assert_eq!(
            att.mode,
            crate::entities::attachment::AttachMode::ByReference
        );
        for tool_id in ["attachment_read", "attachment_search"] {
            assert!(out.result.contains(tool_id), "got: {}", out.result);
        }
    }
}
