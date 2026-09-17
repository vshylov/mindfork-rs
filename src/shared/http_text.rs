//! A fetched page's body → text (spec §9.3.1): the content-coding a server applied is
//! undone, and the bytes go to `shared::text_decode`, which finds the encoding the way a
//! browser finds it — and a step past where a browser stops. One seam for both readers of
//! model-chosen pages, `fetch_url` and `web_search`'s result fetches.
//!
//! **Why it exists.** `reqwest` is built without its default features, which took its
//! `charset` feature with them, so `Response::text()` is `String::from_utf8_lossy`
//! whatever the page declares: every letter of a windows-1251 page came back as U+FFFD,
//! the attachment made of it was unreadable, and its search index was built from the
//! replacement character. Without the decompression features, a body a server gzipped
//! unasked arrives as gzip bytes (measured on www.163.com). See
//! docs/research/page-charset.md.
//!
//! What this module adds to the order in `text_decode` is the transport's part: the
//! `Content-Type` the server sent, and the host's top-level domain as the detector's hint.

use std::io::Read;

use encoding_rs::Encoding;

use crate::shared::text_decode::{self, EncodingSource};

/// The most a compressed body may inflate to — and, since
/// docs/research/safe-defaults.md N3, the most a body may be **read** as: the bytes now
/// arrive through the stream and the count is checked as they do, so a model-chosen
/// address cannot pull gigabytes into memory in the twenty seconds the request has. Far
/// above any page (extraction keeps 400 000 characters of text) and far below what a
/// decompression bomb would take.
pub const MAX_INFLATED_MB: usize = 32;
const MAX_INFLATED_BYTES: usize = MAX_INFLATED_MB * 1024 * 1024;

/// A response body as text, with what decided its encoding (for the log).
#[derive(Debug)]
pub struct BodyText {
    pub text: String,
    /// The response's `Content-Type`, empty when it sent none.
    pub content_type: String,
    pub encoding: &'static Encoding,
    pub source: EncodingSource,
}

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error(transparent)]
    Read(#[from] reqwest::Error),
    #[error("the body is compressed as `{0}`, which is not supported")]
    Unsupported(String),
    #[error("the `{coding}` body is damaged: {source}")]
    Corrupt {
        coding: String,
        source: std::io::Error,
    },
    #[error("the `{coding}` body inflates past {limit} bytes")]
    TooLarge { coding: String, limit: usize },
    /// The response itself is past the ceiling — counted as the bytes arrive, because a
    /// server may send no `Content-Length` and one that does is not obliged to be honest
    /// (the same reasoning as `features::image_fetch`).
    #[error("the response is larger than {limit} bytes")]
    BodyTooLarge { limit: usize },
}

impl BodyError {
    /// The content-coding a decompression failure is about; `None` for a failed read.
    pub fn coding(&self) -> Option<&str> {
        match self {
            Self::Read(_) => None,
            Self::Unsupported(coding)
            | Self::Corrupt { coding, .. }
            | Self::TooLarge { coding, .. } => Some(coding),
            Self::BodyTooLarge { .. } => None,
        }
    }

    /// Whether this is the size ceiling rather than a transport or coding failure — the
    /// caller says so in words instead of "the request failed".
    pub fn too_large(&self) -> bool {
        matches!(self, Self::BodyTooLarge { .. })
    }
}

/// Reads `resp` to its end and decodes it by the order in `text_decode`.
pub async fn read(resp: reqwest::Response) -> Result<BodyText, BodyError> {
    let content_type = header_value(&resp, reqwest::header::CONTENT_TYPE);
    let coding = header_value(&resp, reqwest::header::CONTENT_ENCODING);
    let tld = resp.url().domain().and_then(tld_label);
    let raw = read_capped(resp, MAX_INFLATED_BYTES).await?;
    let body = undo_content_coding(&coding, raw, MAX_INFLATED_BYTES)?;
    let (text, encoding, source) = text_decode::decode(&body, &content_type, tld.as_deref());
    Ok(BodyText {
        text,
        content_type,
        encoding,
        source,
    })
}

/// Reads the body, refusing past `limit` **as the bytes arrive**. A `Content-Length`
/// above the ceiling ends it before a byte is read; without one, or with a false one, the
/// count in the loop is what bounds the memory (docs/research/safe-defaults.md N3).
async fn read_capped(resp: reqwest::Response, limit: usize) -> Result<Vec<u8>, BodyError> {
    use futures_util::StreamExt;

    if resp.content_length().is_some_and(|len| len > limit as u64) {
        return Err(BodyError::BodyTooLarge { limit });
    }
    let mut body: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len() + chunk.len() > limit {
            return Err(BodyError::BodyTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A response header as text, empty when absent or not visible ASCII.
fn header_value(resp: &reqwest::Response, name: reqwest::header::HeaderName) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Undoes the codings `Content-Encoding` lists, the last applied first. The request never
/// asks for one, so any here was applied unasked — www.163.com gzips regardless
/// (page-charset.md §2.2) — and `gzip`/`deflate` are what such a server sends.
fn undo_content_coding(
    header: &str,
    mut body: Vec<u8>,
    limit: usize,
) -> Result<Vec<u8>, BodyError> {
    for coding in header.rsplit(',').map(|c| c.trim().to_ascii_lowercase()) {
        body = match coding.as_str() {
            "" | "identity" => continue,
            "gzip" | "x-gzip" => {
                inflate(flate2::read::MultiGzDecoder::new(&body[..]), &coding, limit)?
            }
            // RFC 9110's `deflate` is zlib-wrapped; some servers send the raw stream under
            // that name.
            "deflate" => match inflate(flate2::read::ZlibDecoder::new(&body[..]), &coding, limit) {
                Err(BodyError::Corrupt { .. }) => {
                    inflate(flate2::read::DeflateDecoder::new(&body[..]), &coding, limit)?
                }
                other => other?,
            },
            _ => return Err(BodyError::Unsupported(coding)),
        };
    }
    Ok(body)
}

fn inflate(decoder: impl Read, coding: &str, limit: usize) -> Result<Vec<u8>, BodyError> {
    let mut out = Vec::new();
    decoder
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|source| BodyError::Corrupt {
            coding: coding.to_string(),
            source,
        })?;
    if out.len() > limit {
        return Err(BodyError::TooLarge {
            coding: coding.to_string(),
            limit,
        });
    }
    Ok(out)
}

/// A host's rightmost label in the only form `chardetng` accepts — it panics on upper
/// case, a period or non-ASCII — or `None`.
fn tld_label(host: &str) -> Option<String> {
    let label = host
        .trim_end_matches('.')
        .rsplit('.')
        .next()?
        .to_ascii_lowercase();
    let usable = !label.is_empty()
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    usable.then_some(label)
}

#[cfg(test)]
pub(crate) mod testkit {
    use std::io::Write;

    /// `bytes`, gzipped.
    pub(crate) fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    /// The response the defect needed, and one more: a windows-1251 page that declares
    /// its encoding only in `<meta>` (reqwest's `charset` feature, which reads the header
    /// alone, would not have read it either), gzipped though nothing asked for it.
    /// `title` goes in `<h1>`, `prose` in a paragraph.
    pub(crate) fn legacy_page_response(title: &str, prose: &str) -> Vec<u8> {
        let html = format!(
            "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1251\">\
             </head><body><h1>{title}</h1><p>{prose}</p></body></html>"
        );
        let body = gzip(&encoding_rs::WINDOWS_1251.encode(&html).0);
        let mut out = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Encoding: gzip\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(&body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::gzip;
    use super::*;
    use crate::shared::text_decode::{
        Utf8Evidence, detect, document_charset, header_charset, reads_alike,
    };
    use encoding_rs::UTF_8;
    use std::io::Write;

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    fn raw_deflate(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn content_codings_are_undone_last_applied_first() {
        let page = b"<p>the page</p>".repeat(50);
        let codings = [
            ("gzip", gzip(&page)),
            ("x-gzip", gzip(&page)),
            ("deflate", zlib(&page)),
            ("deflate", raw_deflate(&page)),
            ("deflate, gzip", gzip(&zlib(&page))),
            ("identity", page.clone()),
            ("", page.clone()),
        ];
        for (header, body) in codings {
            let undone = undo_content_coding(header, body, MAX_INFLATED_BYTES).unwrap();
            assert_eq!(undone, page, "{header}");
        }
    }

    #[test]
    fn a_coding_that_cannot_be_undone_is_an_error_naming_it() {
        let page = b"<p>the page</p>".repeat(50);
        let unsupported = undo_content_coding("br", page.clone(), MAX_INFLATED_BYTES).unwrap_err();
        assert!(
            matches!(&unsupported, BodyError::Unsupported(c) if c == "br"),
            "{unsupported}"
        );
        let corrupt = undo_content_coding("gzip", page, MAX_INFLATED_BYTES).unwrap_err();
        assert!(matches!(corrupt, BodyError::Corrupt { .. }), "{corrupt}");
        let bomb = undo_content_coding("gzip", gzip(&[0u8; 4096]), 1024).unwrap_err();
        assert!(
            matches!(bomb, BodyError::TooLarge { limit: 1024, .. }),
            "{bomb}"
        );
        assert_eq!(bomb.coding(), Some("gzip"));
    }

    #[test]
    fn the_tld_hint_only_ever_takes_a_form_the_detector_accepts() {
        let cp1251 = encoding_rs::WINDOWS_1251
            .encode("Старый журнал о компьютерах хранил статьи в той кодировке.")
            .0;
        let hosts = [
            ("sector.biz.ua", Some("ua")),
            ("Example.RU.", Some("ru")),
            ("xn--p1ai", Some("xn--p1ai")),
            ("localhost", Some("localhost")),
            ("under_score", None),
            ("", None),
        ];
        for (host, expected) in hosts {
            let label = tld_label(host);
            assert_eq!(label.as_deref(), expected, "{host}");
            // `chardetng` panics on a label in any other form.
            detect(&cp1251, label.as_deref());
        }
    }

    /// The corpus of docs/research/page-charset.md §2, through the client `fetch_url`
    /// uses: per page, what it declares, how its bytes fare as UTF-8, which step chose
    /// what, and what the detector alone would have guessed — with the TLD and without.
    /// The measurement behind steps 2, 4 and 5, and a smoke whose assertion can fail: a
    /// legacy decision reads like the detector's guess, a UTF-8 one holds no
    /// replacement character.
    /// `cargo test live_charset_corpus -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "requires network access"]
    async fn live_charset_corpus() {
        // web::USER_AGENT's value: sector.biz.ua answers a bare client with a 503.
        const AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
            (KHTML, like Gecko) Chrome/124.0 Safari/537.36";
        const CORPUS: &[&str] = &[
            "https://sector.biz.ua/mycomp/mid203/aid5.html",
            "https://www.opennet.ru/",
            "http://lib.ru/",
            "http://www.kulichki.com/",
            "http://abehiroshi.la.coocan.jp/",
            "http://www.newsmth.net/",
            "https://www.fourmilab.ch/",
            "http://citforum.ru/",
            "http://www.people.com.cn/",
            "https://www.163.com/",
            "https://www.ixbt.com/news/",
            "https://habr.com/ru/articles/",
            "https://ru.wikipedia.org/wiki/Windows-1251",
            "https://www.lemonde.fr/",
        ];
        let client = crate::shared::net::GuardedClient::new(
            crate::shared::net::AddressPolicy::PublicOnly,
            std::time::Duration::from_secs(20),
        );
        let mut disputed = Vec::new();
        for url in CORPUS {
            let sent = client
                .get(url)
                .unwrap()
                .header(reqwest::header::USER_AGENT, AGENT)
                .send();
            let resp = match sent.await {
                Ok(resp) => resp,
                Err(err) => {
                    eprintln!("{url} | request failed: {err}");
                    continue;
                }
            };
            let status = resp.status();
            let content_type = header_value(&resp, reqwest::header::CONTENT_TYPE);
            let coding = header_value(&resp, reqwest::header::CONTENT_ENCODING);
            let tld = resp.url().domain().and_then(tld_label);
            let body = undo_content_coding(
                &coding,
                resp.bytes().await.unwrap().into(),
                MAX_INFLATED_BYTES,
            )
            .unwrap();
            let evidence = Utf8Evidence::of(&body);
            let declared_in_document = document_charset(&body).map(Encoding::name);
            let (text, encoding, step) = text_decode::decode(&body, &content_type, tld.as_deref());
            let replaced = text.matches('\u{FFFD}').count();
            eprintln!(
                "{url} | {status} | coding {coding:?} | header {:?} | document {declared_in_document:?} \
                 | utf-8 {}/{} | {} by {step:?} | U+FFFD {replaced} | detector {} (tld {tld:?}), {} (none)",
                header_charset(&content_type).map(Encoding::name),
                evidence.decoded,
                evidence.broken,
                encoding.name(),
                detect(&body, tld.as_deref()).name(),
                detect(&body, None).name(),
            );
            // A count of U+FFFD alone would be vacuous: a single-byte decoder never
            // produces one, right or wrong.
            let agrees = if encoding == UTF_8 {
                replaced == 0
            } else {
                reads_alike(encoding, detect(&body, tld.as_deref()), &body)
            };
            if status.is_success() && !agrees {
                disputed.push(*url);
            }
        }
        assert!(
            disputed.is_empty(),
            "the decision and the evidence disagree on: {disputed:?}"
        );
    }
}
