//! Bytes → text in the encoding they are in (spec §9.3.1): the order that decides a
//! fetched page's encoding, as a function of the bytes and of what was declared about
//! them. `shared::http_text` feeds it a response; a local file's reader is the next
//! caller (docs/research/local-file-encoding.md §3, fork F5b).
//!
//! **The order**, first match wins (docs/research/page-charset.md §3):
//!
//! 1. a byte-order mark;
//! 2. **the bytes, when they read as UTF-8** — some non-ASCII, and more characters that
//!    decode than sequences that do not. Legacy text does not form valid UTF-8 by
//!    accident (GBK, the worst case measured, decodes 59 against 218 broken), while a
//!    site moved to UTF-8 routinely keeps its old `<meta charset=windows-1251>`;
//! 3. pure ASCII: whatever is declared (ISO-2022-JP is seven-bit), else UTF-8;
//! 4. a declared encoding the bytes do not refute — the header's or the document's own
//!    (`<meta>` before `<body>`, or an XML declaration); where the two disagree, the one
//!    the detector agrees with, else the header's. A declared UTF-8 is refuted by step 2
//!    having failed, which is what a server-wide `charset=utf-8` in front of a legacy
//!    page needs;
//! 5. nothing usable declared: the detector — `chardetng`, the one Firefox runs on
//!    unlabelled pages — hinted by a top-level domain.

use encoding_rs::{
    EncoderResult, Encoding, REPLACEMENT, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252, X_USER_DEFINED,
};

/// Which step of the order in the module docs chose the encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingSource {
    Bom,
    /// The bytes read as UTF-8 (pure ASCII with nothing declared included).
    Utf8Bytes,
    /// The `charset` parameter of `Content-Type`.
    Header,
    /// The document's own `<meta>` or XML declaration.
    Document,
    Detected,
}

/// The text of `bytes`, its encoding and the step that chose it (module docs), for a
/// body that arrived with a `Content-Type`.
pub(crate) fn decode(
    bytes: &[u8],
    content_type: &str,
    tld: Option<&str>,
) -> (String, &'static Encoding, EncodingSource) {
    decode_declared(
        bytes,
        header_charset(content_type),
        is_markup(content_type),
        tld,
    )
}

/// The order itself: `header` is what a transport declared, `markup` whether the
/// document may declare its own encoding.
fn decode_declared(
    bytes: &[u8],
    header: Option<&'static Encoding>,
    markup: bool,
    tld: Option<&str>,
) -> (String, &'static Encoding, EncodingSource) {
    if let Some((encoding, bom)) = Encoding::for_bom(bytes) {
        return finish(encoding, &bytes[bom..], EncodingSource::Bom);
    }
    let utf8 = Utf8Evidence::of(bytes);
    if utf8.decoded > utf8.broken {
        return finish(UTF_8, bytes, EncodingSource::Utf8Bytes);
    }
    let document = markup.then(|| document_charset(bytes)).flatten();
    if utf8.broken == 0 {
        // Pure ASCII: every ASCII-compatible encoding reads it alike, so a declaration
        // only matters for the seven-bit ones.
        return match (header, document) {
            (Some(declared), _) => finish(declared, bytes, EncodingSource::Header),
            (None, Some(declared)) => finish(declared, bytes, EncodingSource::Document),
            (None, None) => finish(UTF_8, bytes, EncodingSource::Utf8Bytes),
        };
    }
    // Not UTF-8, so a declaration of UTF-8 is refuted rather than obeyed.
    let legacy = |declared: Option<&'static Encoding>| declared.filter(|e| *e != UTF_8);
    match (legacy(header), legacy(document)) {
        (Some(header), Some(document)) if header != document => {
            if reads_alike(detect(bytes, tld), document, bytes) {
                finish(document, bytes, EncodingSource::Document)
            } else {
                finish(header, bytes, EncodingSource::Header)
            }
        }
        (Some(header), _) => finish(header, bytes, EncodingSource::Header),
        (None, Some(document)) => finish(document, bytes, EncodingSource::Document),
        (None, None) => finish(detect(bytes, tld), bytes, EncodingSource::Detected),
    }
}

fn finish(
    encoding: &'static Encoding,
    bytes: &[u8],
    source: EncodingSource,
) -> (String, &'static Encoding, EncodingSource) {
    let (text, _) = encoding.decode_without_bom_handling(bytes);
    (text.into_owned(), encoding, source)
}

/// A local file's text in its own encoding (docs/research/local-file-encoding.md §3).
#[derive(Debug)]
pub struct FileText {
    pub text: String,
    pub encoding: &'static Encoding,
    /// Whether the file opened with a byte-order mark — written back as it was.
    pub bom: bool,
}

/// A local file's text: the order without a transport — nothing declared from outside,
/// the document's own declaration only for markup — with `hint` the TLD of the user's
/// language ([`tld_hint`]). `None` for a binary file: a NUL byte, unless a BOM announces
/// UTF-16 and the rest decodes whole with no NUL character. That comes before anything
/// else, so Notepad's "Unicode" is text and a blob that happens to open `FF FE` is not.
pub fn decode_file(bytes: &[u8], markup: bool, hint: Option<&str>) -> Option<FileText> {
    if let Some((encoding, bom)) = Encoding::for_bom(bytes)
        && (encoding == UTF_16LE || encoding == UTF_16BE)
    {
        let text = encoding.decode_without_bom_handling_and_without_replacement(&bytes[bom..])?;
        return (!text.contains('\0')).then(|| FileText {
            text: text.into_owned(),
            encoding,
            bom: true,
        });
    }
    if bytes.contains(&0) {
        return None;
    }
    let (text, encoding, source) = decode_declared(bytes, None, markup, hint);
    Some(FileText {
        text,
        encoding,
        bom: source == EncodingSource::Bom,
    })
}

/// Whether a file on disk is markup that may declare its own encoding — by extension, since
/// a local file has no `Content-Type`, and a Markdown note *about* `<meta charset>` must not
/// declare an encoding it does not use.
pub fn is_markup_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|e| {
            ["html", "htm", "xhtml", "xml"]
                .iter()
                .any(|m| e.eq_ignore_ascii_case(m))
        })
}

/// `text` in `encoding`, or the first character that encoding cannot store. `encoding_rs`
/// has no UTF-16 encoder — it encodes those as UTF-8, as a browser's form submission does —
/// so UTF-16 is written here.
pub fn encode(text: &str, encoding: &'static Encoding) -> Result<Vec<u8>, char> {
    if encoding == UTF_16LE {
        return Ok(text.encode_utf16().flat_map(u16::to_le_bytes).collect());
    }
    if encoding == UTF_16BE {
        return Ok(text.encode_utf16().flat_map(u16::to_be_bytes).collect());
    }
    let mut encoder = encoding.new_encoder();
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        out.reserve(
            encoder
                .max_buffer_length_from_utf8_without_replacement(rest.len())
                .unwrap_or(rest.len() * 4 + 16),
        );
        let (result, read) =
            encoder.encode_from_utf8_to_vec_without_replacement(rest, &mut out, true);
        rest = &rest[read..];
        match result {
            EncoderResult::InputEmpty => return Ok(out),
            EncoderResult::Unmappable(c) => return Err(c),
            EncoderResult::OutputFull => {}
        }
    }
}

/// Whether `bytes` — past any BOM — come back byte for byte through `encoding`: decoded
/// without replacement, then encoded again. What makes writing an edit back in a file's
/// own encoding safe, since whatever the edit does not touch returns exactly
/// (local-file-encoding.md §2.4). A lossy read — invalid UTF-8 read as UTF-8 — fails it.
pub fn round_trips(bytes: &[u8], encoding: &'static Encoding) -> bool {
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .and_then(|text| encode(&text, encoding).ok())
        .is_some_and(|back| back == bytes)
}

/// The byte-order mark `encoding` writes, for a file that opened with one.
pub fn bom_of(encoding: &'static Encoding) -> &'static [u8] {
    if encoding == UTF_8 {
        &[0xEF, 0xBB, 0xBF]
    } else if encoding == UTF_16LE {
        &[0xFF, 0xFE]
    } else if encoding == UTF_16BE {
        &[0xFE, 0xFF]
    } else {
        &[]
    }
}

/// The TLD `chardetng` weighs for text in `lang`: a local file has no host, but it has a
/// user, and the interface language is theirs (local-file-encoding.md §2.2 — the `ru` hint
/// lifts short Russian files from 21 of 32 to 27; fork F2c). `None` for English and for a
/// language the detector has no regional prior for. Every value is a lower-case label with
/// no period, the only form `chardetng` does not panic on.
pub fn tld_hint(lang: crate::shared::i18n::Lang) -> Option<&'static str> {
    Some(match lang.code() {
        "ru" => "ru",
        "uk" => "ua",
        "be" => "by",
        "kk" => "kz",
        "bg" => "bg",
        "sr" => "rs",
        "mk" => "mk",
        "pl" => "pl",
        "cs" => "cz",
        "sk" => "sk",
        "sl" => "si",
        "hr" => "hr",
        "hu" => "hu",
        "ro" => "ro",
        "de" => "de",
        "fr" => "fr",
        "es" => "es",
        "it" => "it",
        "pt" => "pt",
        "nl" => "nl",
        "el" => "gr",
        "tr" => "tr",
        "he" => "il",
        "ar" => "sa",
        "fa" => "ir",
        "lt" => "lt",
        "lv" => "lv",
        "et" => "ee",
        "ja" => "jp",
        "zh" => "cn",
        "ko" => "kr",
        "th" => "th",
        "vi" => "vn",
        _ => return None,
    })
}

/// Whether two encodings turn `bytes` into the same text. The detector names a family's
/// superset — KOI8-U for every KOI8-R page measured (page-charset.md §2.3) — so agreeing
/// with a declaration means reading like it, not being it.
pub(crate) fn reads_alike(a: &'static Encoding, b: &'static Encoding, bytes: &[u8]) -> bool {
    a == b || a.decode_without_bom_handling(bytes).0 == b.decode_without_bom_handling(bytes).0
}

/// How bytes fare read as UTF-8: characters above ASCII that decode, and sequences that
/// do not.
#[derive(Debug, Default)]
pub(crate) struct Utf8Evidence {
    pub(crate) decoded: usize,
    pub(crate) broken: usize,
}

impl Utf8Evidence {
    pub(crate) fn of(mut bytes: &[u8]) -> Self {
        let mut evidence = Self::default();
        loop {
            let (valid, error) = match std::str::from_utf8(bytes) {
                Ok(_) => (bytes, None),
                Err(e) => (&bytes[..e.valid_up_to()], Some(e)),
            };
            // A lead byte opens every multi-byte character of valid UTF-8.
            evidence.decoded += valid.iter().filter(|b| **b >= 0xC0).count();
            let Some(error) = error else {
                return evidence;
            };
            evidence.broken += 1;
            match error.error_len() {
                Some(len) => bytes = &bytes[error.valid_up_to() + len..],
                // A sequence cut off by the end of the body.
                None => return evidence,
            }
        }
    }
}

/// The encoding `Content-Type`'s `charset` parameter names.
pub(crate) fn header_charset(content_type: &str) -> Option<&'static Encoding> {
    content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        label(
            value
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .as_bytes(),
        )
    })
}

/// An encoding by its WHATWG label. `replacement` — what labels such as `iso-2022-kr` map
/// to, and which decodes any input to a single U+FFFD — counts as no declaration, so the
/// other steps still get to read the page.
fn label(name: &[u8]) -> Option<&'static Encoding> {
    Encoding::for_label(name).filter(|e| *e != REPLACEMENT)
}

/// Whether the body may declare its own encoding: HTML, XML, or untyped.
fn is_markup(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.trim().is_empty() || ct.contains("html") || ct.contains("xml")
}

/// The encoding the document declares: an XML declaration opening it, else the first
/// `<meta>` before `<body>` that names one. Not just the first 1024 bytes a browser's
/// prescan reads — habr.com's `<meta>` sits at byte 1698 (page-charset.md §2.1), and a
/// reader holding the whole body has no reason to stop there. A declared UTF-16 means
/// UTF-8 and `x-user-defined` means windows-1252, as in a browser: a document whose
/// declaration could be read as ASCII is not UTF-16.
pub(crate) fn document_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    let declared = xml_declaration(bytes).or_else(|| meta_charset(bytes))?;
    Some(if declared == UTF_16LE || declared == UTF_16BE {
        UTF_8
    } else if declared == X_USER_DEFINED {
        WINDOWS_1252
    } else {
        declared
    })
}

/// `<?xml … encoding="…"?>` at the start of the body.
fn xml_declaration(bytes: &[u8]) -> Option<&'static Encoding> {
    let rest = bytes.trim_ascii_start().strip_prefix(b"<?xml")?;
    let end = rest.iter().position(|&b| b == b'>')?;
    Attributes(&rest[..end])
        .find(|(name, _)| name.eq_ignore_ascii_case(b"encoding"))
        .and_then(|(_, value)| label(value))
}

/// The first `<meta>` before `<body>` naming a known encoding — WHATWG's prescan
/// (`charset`, or `http-equiv="content-type"` with a `charset=` inside `content`), with
/// comments skipped.
fn meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    let mut rest = bytes;
    while let Some(open) = rest.iter().position(|&b| b == b'<') {
        rest = &rest[open + 1..];
        if let Some(comment) = rest.strip_prefix(b"!--") {
            rest = find(comment, b"-->").map_or(&[][..], |end| &comment[end + 3..]);
            continue;
        }
        let name_len = rest
            .iter()
            .position(|b| !b.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        let (name, attributes) = rest.split_at(name_len);
        if name.eq_ignore_ascii_case(b"body") {
            return None;
        }
        if name.eq_ignore_ascii_case(b"meta")
            && let Some(encoding) = meta_declaration(attributes)
        {
            return Some(encoding);
        }
    }
    None
}

/// What one `<meta>` declares, given what follows its name.
fn meta_declaration(attributes: &[u8]) -> Option<&'static Encoding> {
    let (mut charset, mut pragma, mut content) = (None, false, None);
    for (name, value) in Attributes(attributes) {
        if name.eq_ignore_ascii_case(b"charset") {
            charset = charset.or(Some(value));
        } else if name.eq_ignore_ascii_case(b"http-equiv") {
            pragma |= value.eq_ignore_ascii_case(b"content-type");
        } else if name.eq_ignore_ascii_case(b"content") {
            content = content.or(Some(value));
        }
    }
    match charset {
        Some(value) => label(value),
        None if pragma => content.and_then(charset_in_content).and_then(label),
        None => None,
    }
}

/// The label after `charset=` in a `content` attribute (WHATWG's "extracting a character
/// encoding from a meta element").
fn charset_in_content(content: &[u8]) -> Option<&[u8]> {
    let lower = content.to_ascii_lowercase();
    let mut from = 0;
    loop {
        let at = from + find(&lower[from..], b"charset")?;
        let rest = content[at + 7..].trim_ascii_start();
        let Some(value) = rest.strip_prefix(b"=") else {
            from = at + 7;
            continue;
        };
        let value = value.trim_ascii_start();
        return match value.first() {
            Some(&quote @ (b'"' | b'\'')) => {
                let inner = &value[1..];
                inner
                    .iter()
                    .position(|&b| b == quote)
                    .map(|end| &inner[..end])
            }
            Some(_) => {
                let end = value
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || *b == b';')
                    .unwrap_or(value.len());
                Some(&value[..end])
            }
            None => None,
        };
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A tag's attributes as `(name, value)`, from just past its name to its first unquoted
/// `>`. An attribute without `=` has an empty value.
struct Attributes<'a>(&'a [u8]);

impl<'a> Iterator for Attributes<'a> {
    type Item = (&'a [u8], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let start = self
            .0
            .iter()
            .position(|b| !b.is_ascii_whitespace() && *b != b'/')?;
        let tag = &self.0[start..];
        if tag[0] == b'>' {
            self.0 = &[];
            return None;
        }
        // At least one byte, so a stray `=` is a name and the walk always advances.
        let name_len = tag
            .iter()
            .skip(1)
            .position(|b| b.is_ascii_whitespace() || matches!(b, b'=' | b'>' | b'/'))
            .map_or(tag.len(), |p| p + 1);
        let (name, after) = tag.split_at(name_len);
        let Some(value) = after.trim_ascii_start().strip_prefix(b"=") else {
            self.0 = after;
            return Some((name, &[]));
        };
        let value = value.trim_ascii_start();
        let (value, rest) = match value.first() {
            Some(&quote @ (b'"' | b'\'')) => {
                let inner = &value[1..];
                match inner.iter().position(|&b| b == quote) {
                    Some(end) => (&inner[..end], &inner[end + 1..]),
                    None => (inner, &[][..]),
                }
            }
            _ => value.split_at(
                value
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || *b == b'>')
                    .unwrap_or(value.len()),
            ),
        };
        self.0 = rest;
        Some((name, value))
    }
}

/// The detector's guess, UTF-8 excluded: the bytes were already found not to be. `tld`
/// must be a lower-case ASCII label with no period — `chardetng` panics on anything else.
pub(crate) fn detect(bytes: &[u8], tld: Option<&str>) -> &'static Encoding {
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    detector.guess(tld.map(str::as_bytes), chardetng::Utf8Detection::Deny)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Russian prose, long enough for the detector to be sure of its alphabet, and
    /// holding nothing KOI8-R lacks (no dashes, no guillemets).
    const RU: &str = "Старый журнал о компьютерах хранил статьи в той кодировке, которой \
        пользовались тогда почти все сайты. Читатель открывал страницу, и браузер сам \
        понимал, как показать буквы. Программа, которая читает такие страницы сегодня, \
        должна поступать так же: смотреть, что объявил сервер, что написано в самом \
        документе, и только потом угадывать по частоте букв.";
    const ZH: &str = "旧的网站常常使用本地编码保存网页，浏览器根据声明来显示文字。\
        程序读取这些网页时，也应该先看服务器和文档的声明，然后再根据字节猜测编码。";
    const JA: &str = "古いウェブサイトはシフトJISで書かれていることが多く、ブラウザは宣言を見て文字を表示します。";
    const EN: &str =
        "An old page written in plain ASCII reads the same under every encoding it could declare.";
    /// Latin prose whose only non-ASCII letter is the one fourmilab.ch carries
    /// (page-charset.md §2.3): too little for the detector to tell windows-1252 from
    /// windows-1257 without the TLD.
    const FR: &str = "Fourmilab is written in Neuchâtel, Switzerland, by the lake of Neuchâtel.";

    /// The decision table (page-charset.md §3). Columns: case | prose | the bytes as |
    /// Content-Type | the document's head | TLD | chosen encoding | the step that chose.
    /// `{pad}` in a head is a 1200-byte description pushing what follows past the first
    /// 1024 bytes; a head opening with `<?xml` makes the document a feed. The detector
    /// names KOI8-U for KOI8-R text — the superset, the same Russian letters (§2.3).
    const CASES: &str = r#"
        header names it                         | ru | windows-1251 | text/html; charset=windows-1251   | -                                                                          | -  | windows-1251 | Header
        quoted, cased header label              | ru | windows-1251 | text/html; Charset="Windows-1251" | -                                                                          | -  | windows-1251 | Header
        meta http-equiv only                    | ru | windows-1251 | text/html                         | <meta http-equiv="Content-Type" content="text/html; charset=windows-1251"> | -  | windows-1251 | Document
        description mentioning charset= ignored | ru | windows-1251 | text/html                         | <meta name="description" content="about charset=koi8-r"><meta charset=windows-1251> | -  | windows-1251 | Document
        meta past byte 1024                     | ru | windows-1251 | text/html                         | {pad}<meta charset=windows-1251>                                           | -  | windows-1251 | Document
        utf-8 header refuted by the bytes       | ru | windows-1251 | text/html; charset=utf-8          | <meta charset="windows-1251">                                              | -  | windows-1251 | Document
        utf-8 header alone, refuted             | ru | windows-1251 | text/html; charset=utf-8          | -                                                                          | ua | windows-1251 | Detected
        meta inside a comment is ignored        | ru | windows-1251 | text/html                         | <!-- <meta charset=koi8-r> --><meta charset='windows-1251'>                | -  | windows-1251 | Document
        meta after body is ignored              | ru | koi8-r       | text/html                         | </head><body><meta charset=windows-1251>                                   | ru | KOI8-U       | Detected
        declarations disagree, detector: meta   | ru | windows-1251 | text/html; charset=iso-8859-1     | <meta charset=windows-1251>                                                | ru | windows-1251 | Document
        declarations disagree, detector: header | ru | koi8-r       | text/html; charset=koi8-r         | <meta charset=windows-1251>                                                | ru | KOI8-R       | Header
        disagree, detector names the superset   | ru | koi8-r       | text/html; charset=windows-1251   | <meta charset=koi8-r>                                                      | ru | KOI8-R       | Document
        nothing declared, windows-1251          | ru | windows-1251 | text/html                         | -                                                                          | ua | windows-1251 | Detected
        nothing declared, koi8-r                | ru | koi8-r       | text/html                         | -                                                                          | ru | KOI8-U       | Detected
        nothing declared, the tld decides       | fr | windows-1252 | text/html                         | -                                                                          | ch | windows-1252 | Detected
        replacement label counts as none        | ru | windows-1251 | text/html; charset=iso-2022-kr    | -                                                                          | ru | windows-1251 | Detected
        text/plain is not scanned for meta      | ru | windows-1251 | text/plain                        | <meta charset=koi8-r>                                                      | ru | windows-1251 | Detected
        migrated site keeps its old meta        | ru | utf-8        | text/html; charset=windows-1251   | <meta charset=windows-1251>                                                | -  | UTF-8        | Utf8Bytes
        utf-8 with stray bytes                  | ru | utf-8+stray  | text/html; charset=windows-1251   | -                                                                          | -  | UTF-8        | Utf8Bytes
        gbk outvotes its accidental utf-8 pairs | zh | gbk          | text/html                         | <meta charset=gbk>                                                         | -  | GBK          | Document
        shift_jis in a content attribute        | ja | shift_jis    | text/html                         | <meta http-equiv="content-type" content="text/html;charset=Shift_JIS">     | -  | Shift_JIS    | Document
        seven-bit iso-2022-jp                   | ja | iso-2022-jp  | text/html                         | <meta charset=iso-2022-jp>                                                 | -  | ISO-2022-JP  | Document
        meta utf-16 means utf-8                 | en | utf-8        | text/html                         | <meta charset=utf-16>                                                      | -  | UTF-8        | Document
        pure ascii, nothing declared            | en | utf-8        | -                                 | -                                                                          | -  | UTF-8        | Utf8Bytes
        utf-8 bom beats the header              | ru | utf-8+bom    | text/html; charset=windows-1251   | -                                                                          | -  | UTF-8        | Bom
        utf-16le with a bom                     | ru | utf-16le+bom | text/html                         | -                                                                          | -  | UTF-16LE     | Bom
        utf-16le named by the header            | zh | utf-16le     | text/html; charset=utf-16le       | -                                                                          | -  | UTF-16LE     | Header
        feed with an xml declaration            | ru | windows-1251 | application/rss+xml               | <?xml version="1.0" encoding="windows-1251"?>                              | -  | windows-1251 | Document
    "#;

    /// The test document: `head` in `<head>` and `prose` in a paragraph — or, for an XML
    /// declaration, a feed carrying `prose` as its title.
    fn document(head: &str, prose: &str) -> String {
        let pad = format!(
            "<meta name=\"description\" content=\"{}\">",
            "x".repeat(1200)
        );
        let head = head.replace("{pad}", &pad);
        if head.starts_with("<?xml") {
            format!("{head}<rss><channel><title>{prose}</title></channel></rss>")
        } else {
            format!("<html><head>{head}</head><body><p>{prose}</p></body></html>")
        }
    }

    /// `doc` as a server would send it: by a label, or in a shape `encoding_rs` has no
    /// encoder for (UTF-16) or that no encoder makes (a BOM, stray bytes).
    fn encoded(shape: &str, doc: &str) -> Vec<u8> {
        let utf16le = || {
            doc.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        match shape {
            "utf-8+bom" => [&[0xEF, 0xBB, 0xBF][..], doc.as_bytes()].concat(),
            "utf-16le+bom" => [vec![0xFF, 0xFE], utf16le()].concat(),
            "utf-16le" => utf16le(),
            "utf-8+stray" => {
                let mut bytes = doc.as_bytes().to_vec();
                // A byte no UTF-8 sequence can hold, twice, each between two characters;
                // the later position first, so the earlier one still stands.
                for at in [doc.len() * 3 / 4, doc.len() / 2] {
                    let at = (at..).find(|i| doc.is_char_boundary(*i)).unwrap();
                    bytes.insert(at, 0xFF);
                }
                bytes
            }
            label => {
                let encoding = Encoding::for_label(label.as_bytes()).unwrap();
                let (bytes, _, unmappable) = encoding.encode(doc);
                assert!(!unmappable, "{label} cannot hold the fixture");
                bytes.into_owned()
            }
        }
    }

    #[test]
    fn the_decision_table() {
        let dash = |s: &'static str| (s != "-").then_some(s);
        for row in CASES.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let cols: Vec<&'static str> = row.split('|').map(str::trim).collect();
            let [case, prose, shape, content_type, head, tld, encoding, step] = cols[..] else {
                panic!("a malformed row: {row}");
            };
            let prose = match prose {
                "ru" => RU,
                "zh" => ZH,
                "ja" => JA,
                "fr" => FR,
                _ => EN,
            };
            let bytes = encoded(shape, &document(dash(head).unwrap_or_default(), prose));
            let (text, chosen, by) =
                decode(&bytes, dash(content_type).unwrap_or_default(), dash(tld));
            assert_eq!(
                (chosen.name(), format!("{by:?}").as_str()),
                (encoding, step),
                "{case}"
            );
            let stray = if shape == "utf-8+stray" { 2 } else { 0 };
            assert_eq!(text.matches('\u{FFFD}').count(), stray, "{case}: {text}");
            assert!(
                text.replace('\u{FFFD}', "").contains(prose),
                "{case}: {text}"
            );
        }
    }

    /// The `gbk` row above only means something if its bytes really do hold valid UTF-8
    /// pairs — otherwise the majority rule was never tested against them.
    #[test]
    fn gbk_prose_does_form_some_valid_utf8_and_is_outvoted() {
        let bytes = encoding_rs::GBK.encode(ZH).0;
        let evidence = Utf8Evidence::of(&bytes);
        assert!(evidence.decoded > 0, "{evidence:?}");
        assert!(evidence.decoded <= evidence.broken, "{evidence:?}");
    }

    /// Notepad's "Unicode" is full of NUL bytes and is text; a blob is not, even one that
    /// happens to open `FF FE` — the check runs before any decoding.
    #[test]
    fn a_file_is_text_unless_it_holds_a_nul_a_whole_utf16_file_excepted() {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("Выручка\r\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let file = decode_file(&utf16, false, None).expect("UTF-16 with its BOM is text");
        assert_eq!(
            (file.text.as_str(), file.encoding, file.bom),
            ("Выручка\r\n", UTF_16LE, true)
        );
        assert!(decode_file(&[0xFF, 0xFE, 0x00, 0x01, 0x80], false, None).is_none());
        assert!(decode_file(b"PK\x03\x04\x00\x00", false, None).is_none());
        let bom8 = [&[0xEF, 0xBB, 0xBF][..], "текст".as_bytes()].concat();
        let file = decode_file(&bom8, false, None).unwrap();
        assert_eq!(
            (file.text.as_str(), file.encoding, file.bom),
            ("текст", UTF_8, true)
        );
    }

    /// A local file has no `Content-Type`: its own `<meta>` counts only when it is markup.
    #[test]
    fn a_file_declares_its_encoding_only_when_it_is_markup() {
        let body = format!("<meta charset=koi8-r>\n{RU}");
        let bytes = encoding_rs::WINDOWS_1251.encode(&body).0;
        let note = decode_file(&bytes, false, Some("ru")).unwrap();
        assert_eq!(
            note.encoding.name(),
            "windows-1251",
            "detected from the bytes"
        );
        let page = decode_file(&bytes, true, Some("ru")).unwrap();
        assert_eq!(page.encoding.name(), "KOI8-R", "taken from its own <meta>");
        assert!(is_markup_path(std::path::Path::new("a/Page.HTML")));
        assert!(!is_markup_path(std::path::Path::new("notes.md")));
    }

    #[test]
    fn writing_back_names_the_character_the_encoding_lacks() {
        let cp1251 = encoding_rs::WINDOWS_1251.encode("Выручка").0.into_owned();
        assert_eq!(encode("Выручка", encoding_rs::WINDOWS_1251), Ok(cp1251));
        assert_eq!(encode("цена — 5", encoding_rs::KOI8_R), Err('—'));
        assert_eq!(encode("итог 🙂", encoding_rs::WINDOWS_1251), Err('🙂'));
        let le: Vec<u8> = "Ёж".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(encode("Ёж", UTF_16LE), Ok(le));
        assert_eq!(bom_of(UTF_16LE), [0xFF, 0xFE]);
        assert!(bom_of(encoding_rs::WINDOWS_1251).is_empty());
    }

    /// A lossy read cannot be written back: the invalid byte became `U+FFFD`, and that is
    /// what would go to disk.
    #[test]
    fn only_a_lossless_read_round_trips() {
        let cp1251 = encoding_rs::WINDOWS_1251.encode(RU).0;
        assert!(round_trips(&cp1251, encoding_rs::WINDOWS_1251));
        let stray = [RU.as_bytes(), &[0xFF]].concat();
        assert!(!round_trips(&stray, UTF_8));
        let utf16: Vec<u8> = RU.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(round_trips(&utf16, UTF_16LE));
    }

    #[test]
    fn the_language_hint_is_a_label_the_detector_accepts() {
        use crate::shared::i18n::Lang;
        assert_eq!(tld_hint(Lang::Ru), Some("ru"));
        assert_eq!(tld_hint(Lang::En), None);
        assert_eq!(tld_hint(Lang::from_code("uk")), Some("ua"));
        assert_eq!(tld_hint(Lang::from_code("xx")), None);
        let cp1251 = encoding_rs::WINDOWS_1251.encode(RU).0;
        for code in ["ru", "uk", "el", "ja", "zh", "vi"] {
            // `chardetng` panics on a label in any other form.
            detect(&cp1251, tld_hint(Lang::from_code(code)));
        }
    }
}
