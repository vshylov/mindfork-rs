//! The text chunks of a PNG that may carry a character card (spec §10,
//! docs/research/companion-platform.md §4.5): `ccv3` (V3) and `chara` (V2),
//! each the card's JSON in base64. Only the chunks' framing is read — the image
//! itself is never decoded.

use std::io::Read;

/// The eight bytes every PNG opens with.
pub(super) const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The keywords a card travels under, in the order a reader prefers them: the
/// V3 spec asks for `ccv3` over V2's `chara` when both are present.
pub(super) const KEYWORDS: [&str; 2] = ["ccv3", "chara"];

/// The most a compressed chunk may inflate to — a card is text, and a few
/// megabytes of it is already a very large one.
const MAX_INFLATED: u64 = 16 * 1024 * 1024;

/// Why a PNG gave no card.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PngError {
    /// The chunk framing ended early or claimed more bytes than the file has.
    Truncated,
    /// A well-formed PNG with no `ccv3` or `chara` chunk: a picture, not a card
    /// (an image host that re-encodes a card drops the chunk).
    NoCard,
}

/// The text of each card chunk found, keyed by its keyword in lower case, in
/// the file's order. A chunk compressed (`zTXt`, a compressed `iTXt`) is
/// inflated; one that fails to inflate is passed over.
pub(super) fn card_chunks(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, PngError> {
    let mut found = Vec::new();
    let mut at = SIGNATURE.len();
    loop {
        let Some(head) = bytes.get(at..at + 8) else {
            return Err(PngError::Truncated);
        };
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let kind = &head[4..8];
        let start = at + 8;
        let Some(data) = start.checked_add(len).and_then(|end| bytes.get(start..end)) else {
            return Err(PngError::Truncated);
        };
        if let Some(chunk) = text_chunk(kind, data) {
            found.push(chunk);
        }
        if kind == b"IEND" {
            break;
        }
        // The data, then a four-byte CRC that is not checked: a card whose CRC is
        // wrong still says what it says, and a reader has nothing to repair.
        at = start + len + 4;
    }
    if found.is_empty() {
        Err(PngError::NoCard)
    } else {
        Ok(found)
    }
}

/// A card chunk's keyword and text, when `kind`/`data` is one.
fn text_chunk(kind: &[u8], data: &[u8]) -> Option<(String, Vec<u8>)> {
    let nul = data.iter().position(|&b| b == 0)?;
    let keyword = std::str::from_utf8(&data[..nul]).ok()?.to_ascii_lowercase();
    if !KEYWORDS.contains(&keyword.as_str()) {
        return None;
    }
    let rest = &data[nul + 1..];
    let text = match kind {
        b"tEXt" => rest.to_vec(),
        // A method byte (0, deflate), then the compressed text.
        b"zTXt" => inflate(rest.get(1..)?)?,
        // A compression flag and method, a language tag and a translated keyword
        // (each NUL-terminated), then the text — compressed when the flag says so.
        b"iTXt" => {
            let (flag, rest) = (*rest.first()?, rest.get(2..)?);
            let lang_end = rest.iter().position(|&b| b == 0)?;
            let rest = &rest[lang_end + 1..];
            let tr_end = rest.iter().position(|&b| b == 0)?;
            let text = &rest[tr_end + 1..];
            if flag == 1 {
                inflate(text)?
            } else {
                text.to_vec()
            }
        }
        _ => return None,
    };
    Some((keyword, text))
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(MAX_INFLATED)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// Test fixtures: a minimal PNG around the given text chunks — the signature,
/// a 1×1 `IHDR`, the chunks, `IEND`. CRCs are zero: nothing here checks them.
#[cfg(test)]
pub(super) fn fixture(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(kind);
        out.extend(data);
        out.extend([0, 0, 0, 0]);
    }
    let mut out = SIGNATURE.to_vec();
    chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    for (kind, data) in chunks {
        chunk(&mut out, kind, data);
    }
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A `tEXt` chunk's data: `keyword\0text`.
#[cfg(test)]
pub(super) fn text(keyword: &str, value: &[u8]) -> Vec<u8> {
    let mut d = keyword.as_bytes().to_vec();
    d.push(0);
    d.extend(value);
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn finds_both_keywords_in_file_order_and_skips_other_text() {
        let png = fixture(&[
            (b"tEXt", text("Software", b"paint")),
            (b"tEXt", text("chara", b"djI=")),
            (b"tEXt", text("ccv3", b"djM=")),
        ]);
        let found = card_chunks(&png).unwrap();
        assert_eq!(
            found,
            vec![
                ("chara".to_string(), b"djI=".to_vec()),
                ("ccv3".to_string(), b"djM=".to_vec())
            ]
        );
    }

    /// Compressed text — `zTXt`, and an `iTXt` with its flag set — is
    /// inflated; an uncompressed `iTXt` is read as it is; a keyword in another
    /// case still counts.
    #[test]
    fn reads_compressed_and_international_chunks() {
        let mut ztxt = b"chara\0\0".to_vec();
        ztxt.extend(deflate(b"zipped"));
        let mut itxt = b"CCV3\0\x01\0en\0\0".to_vec();
        itxt.extend(deflate(b"intl"));
        let png = fixture(&[(b"zTXt", ztxt), (b"iTXt", itxt)]);
        let found = card_chunks(&png).unwrap();
        assert_eq!(found[0], ("chara".into(), b"zipped".to_vec()));
        assert_eq!(found[1], ("ccv3".into(), b"intl".to_vec()));

        let plain = fixture(&[(b"iTXt", b"chara\0\0\0\0\0plain".to_vec())]);
        assert_eq!(card_chunks(&plain).unwrap()[0].1, b"plain".to_vec());
    }

    #[test]
    fn a_picture_without_a_card_and_a_cut_file_are_told_apart() {
        let png = fixture(&[(b"tEXt", text("Comment", b"hi"))]);
        assert_eq!(card_chunks(&png), Err(PngError::NoCard));
        let whole = fixture(&[(b"tEXt", text("chara", b"abc"))]);
        assert_eq!(
            card_chunks(&whole[..whole.len() - 10]),
            Err(PngError::Truncated)
        );
        // A length that runs past the end is not trusted.
        let mut lying = SIGNATURE.to_vec();
        lying.extend(u32::MAX.to_be_bytes());
        lying.extend(b"tEXt");
        assert_eq!(card_chunks(&lying), Err(PngError::Truncated));
    }
}
