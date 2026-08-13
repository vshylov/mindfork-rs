//! Downloading an image named by URL (`/image attach <url>`, spec §9.10).
//!
//! The counterpart of `std::fs::read` for the third [`ImageSource`](crate::app::orchestrator)
//! variant: it turns a URL into bytes, and everything below the staging seam —
//! [`super::image_prepare::prepare`], the size cap, the capability probe, the chip — is the
//! path a file attach already takes. The bytes are always downloaded here rather than
//! handed to the provider as a URL (fork F1, docs/research/image-url-attach.md): one code
//! path serves all five engines including Gemini, the normalization every provider matrix
//! needs still happens, and the pixels live in the chat, so a link that dies later cannot
//! break a stored conversation.
//!
//! What this module *does* enforce, and why each one is here (fork F2 — the URL is typed
//! by the user, so the address itself is not filtered; these are the bounds that hold
//! regardless of who typed it):
//!
//! - **`http`/`https` only**, re-checked on every redirect — a redirect is the one place a
//!   scheme can change after the user approved it.
//! - **At most [`MAX_REDIRECTS`] hops**, followed by hand: `reqwest`'s own policy would
//!   follow them silently, and the errors here have to name which bound was hit.
//! - **A hard byte ceiling while streaming.** `Content-Length` is checked first when it is
//!   there, but it may be absent or a lie, so the stream is what actually enforces the cap.
//! - **Timeouts**, so an endpoint that accepts a connection and then says nothing cannot
//!   hang the attach.
//!
//! Deliberately *not* here: judging the content type. The bytes are sniffed downstream by
//! the decoder, which is the truth; the served `Content-Type` is carried along only so the
//! refusal can say "that URL served text/html" instead of "unsupported format" (fork F6).

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Url;

use super::tools::web::USER_AGENT;

/// How many redirects are followed before giving up. Five is above what an image CDN
/// legitimately needs (host → canonical host → signed URL) and well below a loop.
pub const MAX_REDIRECTS: usize = 5;
/// How long to wait for the TCP/TLS connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Ceiling on the whole request, download included. Longer than `fetch_url`'s 20 s: this
/// transfers megabytes rather than a page of text.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Ceiling on the display name derived from the URL.
const NAME_CHARS: usize = 64;
/// The longest trailing `.xxx` still treated as an extension worth preserving when a long
/// name is trimmed.
const EXT_CHARS: usize = 10;

/// What a successful download hands back.
pub struct FetchedImage {
    pub bytes: Vec<u8>,
    /// The served `Content-Type`, lowercased and without parameters (`""` when absent).
    /// Never used to decide *whether* these are pixels — only to explain a refusal.
    pub content_type: String,
    /// The URL the bytes actually came from (after redirects) — what the name is derived
    /// from, so a redirect to `/photo-1234.png` names the image rather than the short link.
    pub final_url: Url,
}

/// Why a URL could not be turned into image bytes. Localized by the caller (axis B), so
/// this layer stays locale-free.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// Not an `http`/`https` URL, or a redirect led out of those schemes.
    #[error("only http and https URLs can be fetched")]
    Scheme,
    /// The string is not a URL at all, or a redirect pointed at something unparseable.
    #[error("malformed URL")]
    Malformed,
    /// More than [`MAX_REDIRECTS`] hops.
    #[error("too many redirects")]
    TooManyRedirects,
    /// Transport-level failure: DNS, connection, TLS, timeout, a stream that broke.
    #[error("request failed: {0}")]
    Request(String),
    /// The server answered, and not with success.
    #[error("HTTP status {0}")]
    Status(u16),
    /// Bigger than the caller's ceiling — by `Content-Length` or measured on the stream.
    #[error("larger than the size limit")]
    TooBig,
    /// A success status with nothing in the body.
    #[error("empty response")]
    Empty,
}

/// Downloads `url`, refusing anything over `max_bytes`.
pub async fn fetch(url: &str, max_bytes: u64) -> Result<FetchedImage, FetchError> {
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        // Redirects are followed by hand below: the default policy would hide both the
        // scheme change and the hop count, which are exactly the two bounds this enforces.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| FetchError::Request(e.to_string()))?;

    let mut target = parse_http(url)?;
    // One initial request plus MAX_REDIRECTS hops.
    for _ in 0..=MAX_REDIRECTS {
        let resp = client
            .get(target.clone())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "image/*,*/*;q=0.8")
            .send()
            .await
            .map_err(|e| FetchError::Request(e.to_string()))?;

        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchError::Malformed)?;
            // Relative locations are the common case (`/images/a.png`), hence `join`.
            let next = target.join(location).map_err(|_| FetchError::Malformed)?;
            check_scheme(&next)?;
            target = next;
            continue;
        }
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }
        if resp.content_length().is_some_and(|len| len > max_bytes) {
            return Err(FetchError::TooBig);
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| {
                ct.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let final_url = target.clone();

        let mut bytes: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| FetchError::Request(e.to_string()))?;
            // The cap is enforced here rather than only on the header: a server may send
            // no `Content-Length` at all, and one that sends a small one is not obliged to
            // tell the truth. Refusing mid-stream is what bounds the memory.
            if bytes.len() as u64 + chunk.len() as u64 > max_bytes {
                return Err(FetchError::TooBig);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(FetchError::Empty);
        }
        return Ok(FetchedImage {
            bytes,
            content_type,
            final_url,
        });
    }
    Err(FetchError::TooManyRedirects)
}

/// Whether an argument to `/image attach` is a URL rather than a path (fork F3). No path
/// on Windows or unix starts with these, so the two forms cannot be confused.
pub fn looks_like_url(arg: &str) -> bool {
    let lower = arg.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The display name for a downloaded image: the last path segment, percent-decoded, or the
/// host plus `ext` when the URL has no usable segment (fork F4).
pub fn display_name(url: &Url, ext: &str) -> String {
    let segment = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or_default();
    let decoded = percent_encoding::percent_decode_str(segment).decode_utf8_lossy();
    let name = decoded.trim();
    if name.is_empty() {
        return format!("{}.{ext}", url.host_str().unwrap_or("image"));
    }
    trim_name(name)
}

/// Trims a name to [`NAME_CHARS`] while keeping a short trailing extension, so a very long
/// URL segment still shows what kind of file it is.
fn trim_name(name: &str) -> String {
    if name.chars().count() <= NAME_CHARS {
        return name.to_string();
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| !ext.is_empty() && ext.chars().count() <= EXT_CHARS)
        .unwrap_or_default();
    if ext.is_empty() {
        // Nothing worth preserving: the whole budget goes to the name itself.
        return name.chars().take(NAME_CHARS).collect();
    }
    let stem: String = name
        .chars()
        .take(NAME_CHARS.saturating_sub(ext.chars().count() + 1))
        .collect();
    format!("{stem}.{ext}")
}

fn parse_http(url: &str) -> Result<Url, FetchError> {
    let parsed = Url::parse(url.trim()).map_err(|_| FetchError::Malformed)?;
    check_scheme(&parsed)?;
    Ok(parsed)
}

fn check_scheme(url: &Url) -> Result<(), FetchError> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        _ => Err(FetchError::Scheme),
    }
}

/// The HTTP stub these tests run against — and the orchestrator's URL-attach tests too,
/// so the codebase has one such server rather than two drifting copies.
#[cfg(test)]
pub(crate) mod stub {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A one-shot HTTP server: it answers each connection with one of `responses` in order
    /// and closes. Hand-rolled rather than a mock crate — the project's idiom
    /// (`shared/api/http.rs`, `openai/client.rs`), and what a redirect chain needs.
    pub(crate) fn serve(responses: Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        (format!("http://{addr}"), handle)
    }

    pub(crate) fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let buf = image::ImageBuffer::from_fn(width, height, |x, _| {
            image::Rgb([(x % 256) as u8, 30, 60])
        });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(buf)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// A 200 with a body and, optionally, a truthful `Content-Length`.
    pub(crate) fn ok_response(content_type: &str, body: &[u8], with_length: bool) -> Vec<u8> {
        let mut head = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n");
        if with_length {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        // Without a length the client needs the close to know the body ended; the stub
        // closes the connection when the thread drops the stream.
        head.push_str("Connection: close\r\n\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(body);
        out
    }

    pub(crate) fn redirect_response(location: &str) -> Vec<u8> {
        format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
            .into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::stub::*;
    use super::*;

    #[tokio::test]
    async fn downloads_an_image_and_reports_its_content_type() {
        let png = png_bytes(20, 10);
        let (base, _h) = serve(vec![ok_response("image/png", &png, true)]);
        let fetched = fetch(&format!("{base}/pics/shot.png"), 10_000_000)
            .await
            .unwrap();
        assert_eq!(fetched.bytes, png);
        assert_eq!(fetched.content_type, "image/png");
        assert_eq!(fetched.final_url.path(), "/pics/shot.png");
    }

    /// `Content-Type: image/png; charset=binary` must not defeat the comparison the
    /// refusal message depends on.
    #[tokio::test]
    async fn content_type_parameters_are_stripped() {
        let (base, _h) = serve(vec![ok_response(
            "IMAGE/PNG; charset=binary",
            &png_bytes(4, 4),
            true,
        )]);
        let fetched = fetch(&base, 10_000_000).await.unwrap();
        assert_eq!(fetched.content_type, "image/png");
    }

    #[tokio::test]
    async fn follows_a_redirect_chain_within_the_cap() {
        let png = png_bytes(8, 8);
        let (base, _h) = serve(vec![
            redirect_response("/one"),
            redirect_response("/two"),
            ok_response("image/png", &png, true),
        ]);
        let fetched = fetch(&base, 10_000_000).await.unwrap();
        assert_eq!(fetched.bytes, png);
        // The name comes from where the bytes really came from, not from what was typed.
        assert_eq!(fetched.final_url.path(), "/two");
    }

    #[tokio::test]
    async fn a_chain_over_the_cap_is_refused() {
        // One more redirect than the client will follow, and never a body.
        let responses = (0..=MAX_REDIRECTS + 1)
            .map(|i| redirect_response(&format!("/hop{i}")))
            .collect();
        let (base, _h) = serve(responses);
        assert_eq!(
            fetch(&base, 10_000_000).await.err(),
            Some(FetchError::TooManyRedirects)
        );
    }

    #[tokio::test]
    async fn a_redirect_out_of_http_is_refused() {
        let (base, _h) = serve(vec![redirect_response("file:///etc/passwd")]);
        assert_eq!(
            fetch(&base, 10_000_000).await.err(),
            Some(FetchError::Scheme)
        );
    }

    #[tokio::test]
    async fn a_non_http_url_is_refused_before_any_request() {
        assert_eq!(
            fetch("file:///C:/secrets/id_rsa", 10_000_000).await.err(),
            Some(FetchError::Scheme)
        );
        assert_eq!(
            fetch("ftp://example.com/a.png", 10_000_000).await.err(),
            Some(FetchError::Scheme)
        );
        assert_eq!(
            fetch("not a url", 10_000_000).await.err(),
            Some(FetchError::Malformed)
        );
    }

    #[tokio::test]
    async fn a_failing_status_is_reported_with_its_code() {
        let (base, _h) = serve(vec![
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ]);
        assert_eq!(
            fetch(&base, 10_000_000).await.err(),
            Some(FetchError::Status(404))
        );
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused_by_its_content_length() {
        let (base, _h) = serve(vec![ok_response("image/png", &png_bytes(100, 100), true)]);
        assert_eq!(fetch(&base, 64).await.err(), Some(FetchError::TooBig));
    }

    /// The header check is the cheap half; this is the one that actually bounds memory.
    /// Without a `Content-Length` there is nothing to pre-check, so a stub that sends none
    /// is what distinguishes a working stream cap from a deleted one.
    #[tokio::test]
    async fn an_oversized_body_without_a_content_length_is_refused_on_the_stream() {
        let (base, _h) = serve(vec![ok_response("image/png", &png_bytes(100, 100), false)]);
        assert_eq!(fetch(&base, 64).await.err(), Some(FetchError::TooBig));
    }

    #[tokio::test]
    async fn an_empty_body_is_refused_rather_than_handed_on_as_zero_pixels() {
        let (base, _h) = serve(vec![ok_response("image/png", &[], true)]);
        assert_eq!(
            fetch(&base, 10_000_000).await.err(),
            Some(FetchError::Empty)
        );
    }

    /// A page served where an image was expected still downloads: the decoder is what
    /// judges the bytes (fork F6). What matters here is that the served type survives, so
    /// the refusal can name it.
    #[tokio::test]
    async fn html_is_downloaded_and_its_type_is_carried_for_the_message() {
        let (base, _h) = serve(vec![ok_response("text/html", b"<html>nope</html>", true)]);
        let fetched = fetch(&base, 10_000_000).await.unwrap();
        assert_eq!(fetched.content_type, "text/html");
    }

    #[test]
    fn a_url_is_told_apart_from_a_path() {
        assert!(looks_like_url("https://example.com/a.png"));
        assert!(looks_like_url("HTTP://example.com/a.png"));
        assert!(looks_like_url("  https://example.com/a.png  "));
        assert!(!looks_like_url("D:\\pics\\a.png"));
        assert!(!looks_like_url("/home/user/a.png"));
        assert!(!looks_like_url("a.png"));
        // A path that merely mentions the scheme is still a path.
        assert!(!looks_like_url("./https://weird.png"));
    }

    #[test]
    fn the_name_comes_from_the_last_path_segment() {
        let name = |u: &str| display_name(&Url::parse(u).unwrap(), "png");
        assert_eq!(name("https://example.com/pics/chart.png"), "chart.png");
        // A query string is not part of the name.
        assert_eq!(name("https://example.com/a/b.jpg?w=800&h=600"), "b.jpg");
        // Percent-encoding is decoded, because the name is shown to a human.
        assert_eq!(name("https://example.com/my%20chart.png"), "my chart.png");
    }

    #[test]
    fn a_url_with_no_usable_segment_falls_back_to_the_host() {
        let name = |u: &str| display_name(&Url::parse(u).unwrap(), "jpeg");
        assert_eq!(name("https://example.com/"), "example.com.jpeg");
        assert_eq!(name("https://example.com"), "example.com.jpeg");
        // An image endpoint that carries everything in the query has no segment either.
        assert_eq!(
            name("https://cdn.example.com/?id=42"),
            "cdn.example.com.jpeg"
        );
    }

    #[test]
    fn a_very_long_name_is_trimmed_but_keeps_its_extension() {
        let long = format!("https://e.com/{}.png", "a".repeat(200));
        let name = display_name(&Url::parse(&long).unwrap(), "png");
        assert_eq!(name.chars().count(), NAME_CHARS);
        assert!(name.ends_with(".png"), "{name}");
        // Nothing to preserve: no extension at all, and a suffix too long to be one.
        let long = format!("https://e.com/{}", "b".repeat(200));
        assert_eq!(
            display_name(&Url::parse(&long).unwrap(), "png")
                .chars()
                .count(),
            NAME_CHARS
        );
        let long = format!("https://e.com/{}.{}", "c".repeat(100), "d".repeat(40));
        let name = display_name(&Url::parse(&long).unwrap(), "png");
        assert_eq!(name.chars().count(), NAME_CHARS);
        assert!(!name.contains('.'), "{name}");
    }
}
