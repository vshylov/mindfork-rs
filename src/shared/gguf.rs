//! GGUF paths, and the one thing a path can say about the model behind it: that
//! the weights are **split across several files**.
//!
//! A model too large for one file (Hugging Face caps a single upload at 50 GB)
//! ships as parts named by llama.cpp's own `gguf-split` convention —
//! `<name>-00001-of-00003.gguf`, `<name>-00002-of-00003.gguf`, … The server is
//! handed the **first** part and reads the rest itself, by name, out of the same
//! directory; llama.cpp refuses a part that is not the first one ("model must be
//! loaded with the first split"). So the application passes the path through
//! untouched (`-m`, spec §3.4) and this module only answers the two questions
//! that path raises: is this a part of a split model, and if so which files must
//! be sitting next to it.
//!
//! It lives in `shared` rather than in `shared/api/managed.rs` because both sides
//! of the same fact need it and they do not import each other: the launcher's
//! preflight (are all the parts here?) and `shared/config.rs`'s model-name
//! display (a model is not called `gpt-oss-120b-Q8_0-00001-of-00003`).

/// The extension every part carries, split or not.
pub const EXT: &str = ".gguf";

/// Length of the `-00001-of-00003` tail: `gguf-split` writes both numbers
/// `%05d`, so it is a fixed 15 ASCII bytes.
const TAIL_LEN: usize = 15;

/// One part of a split GGUF, as read off its file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shard {
    /// 1-based number of this part, as it appears in the name.
    pub index: u32,
    /// How many parts the model has in total.
    pub total: u32,
    /// The path with the `-NNNNN-of-NNNNN.gguf` tail cut off — the directory
    /// included, so [`Shard::path`] rebuilds a path that can be opened.
    pub stem: String,
}

impl Shard {
    /// The path of part `index` (1-based) of the same model.
    pub fn path(&self, index: u32) -> String {
        format!("{}-{index:05}-of-{:05}{EXT}", self.stem, self.total)
    }

    /// The part llama.cpp has to be pointed at.
    pub fn first(&self) -> String {
        self.path(1)
    }

    /// Every part's path, in order — what must be present on disk for the model
    /// to load.
    pub fn all(&self) -> Vec<String> {
        (1..=self.total).map(|i| self.path(i)).collect()
    }
}

/// The one place the tail is recognized: `Some((head_len, index, total))` when
/// `stem` (an extension-less name) ends in `-NNNNN-of-NNNNN`, where `head_len`
/// is the byte length of the name in front of it.
///
/// Deliberately strict — the fixed five-digit shape `gguf-split` produces, a
/// sane numbering (`1 <= index <= total`) and a non-empty name before the tail.
/// A name that only looks similar is better treated as a plain file than
/// misreported as a model with missing parts, and both callers want the same
/// answer to that question.
fn split_tail(stem: &str) -> Option<(usize, u32, u32)> {
    let b = stem.as_bytes();
    let head_len = b.len().checked_sub(TAIL_LEN)?;
    let tail = &b[head_len..];
    if head_len == 0 || tail[0] != b'-' || &tail[6..10] != b"-of-" {
        return None;
    }
    let index = digits(&tail[1..6])?;
    let total = digits(&tail[10..15])?;
    (index > 0 && index <= total).then_some((head_len, index, total))
}

/// Reads a `-NNNNN-of-NNNNN.gguf` tail off `path` ([`split_tail`]); `None` — an
/// ordinary single-file GGUF (or anything else), which is the safe answer: every
/// caller then behaves exactly as it did before split models were understood.
pub fn parse_shard(path: &str) -> Option<Shard> {
    let stem = path.strip_suffix(EXT)?;
    let (head_len, index, total) = split_tail(stem)?;
    // The tail matched, so all of its bytes are ASCII and `head_len` is a char
    // boundary — slicing the `str` here cannot panic.
    Some(Shard {
        index,
        total,
        stem: stem[..head_len].to_string(),
    })
}

/// The model's name for the UI: the file name without the directory, without
/// `.gguf`, and — for a split model — without the `-00001-of-00003` tail, which
/// names a *file* rather than a model. `None` if nothing is left.
pub fn display_name(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let name = name.trim_end_matches(EXT);
    let name = match split_tail(name) {
        Some((head_len, _, _)) => &name[..head_len],
        None => name,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// Parses a fixed-width run of ASCII digits; `None` if anything else is in there.
fn digits(b: &[u8]) -> Option<u32> {
    if !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(b.iter().fold(0, |n, c| n * 10 + u32::from(c - b'0')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_split_part() {
        let s = parse_shard("/models/gpt-oss-120b-Q8_0-00002-of-00003.gguf")
            .expect("a gguf-split name must parse");
        assert_eq!(s.index, 2);
        assert_eq!(s.total, 3);
        assert_eq!(s.stem, "/models/gpt-oss-120b-Q8_0");
        assert_eq!(s.first(), "/models/gpt-oss-120b-Q8_0-00001-of-00003.gguf");
        assert_eq!(
            s.all(),
            vec![
                "/models/gpt-oss-120b-Q8_0-00001-of-00003.gguf".to_string(),
                "/models/gpt-oss-120b-Q8_0-00002-of-00003.gguf".to_string(),
                "/models/gpt-oss-120b-Q8_0-00003-of-00003.gguf".to_string(),
            ]
        );
    }

    /// Windows paths are the common case for this application; the tail sits at
    /// the end of the string, so the separator never matters.
    #[test]
    fn parses_a_windows_path() {
        let s = parse_shard(r"C:\GGUF\model-00001-of-00002.gguf").expect("must parse");
        assert_eq!(s.stem, r"C:\GGUF\model");
        assert_eq!(s.path(2), r"C:\GGUF\model-00002-of-00002.gguf");
    }

    /// Anything that is not exactly the `gguf-split` shape is a plain file: a
    /// false positive would invent missing parts for a model that has none.
    #[test]
    fn a_plain_gguf_is_not_a_split() {
        for path in [
            "gemma-4-it.gguf",
            "model-1-of-3.gguf",             // not five digits
            "model-00001-of-00003.bin",      // not a GGUF
            "model-00001_of_00003.gguf",     // wrong separators
            "model-00000-of-00003.gguf",     // parts are numbered from 1
            "model-00004-of-00003.gguf",     // out of range
            "-00001-of-00003.gguf",          // tail only, no name
            "модель-00001-of-00003.gguf.gz", // not a GGUF either
        ] {
            assert!(parse_shard(path).is_none(), "{path}");
        }
    }

    /// A multi-byte name must not make the byte-level tail check panic.
    #[test]
    fn handles_non_ascii_names() {
        let s = parse_shard("модель-00001-of-00002.gguf").expect("must parse");
        assert_eq!(s.stem, "модель");
        assert_eq!(
            display_name("модель-00001-of-00002.gguf").as_deref(),
            Some("модель")
        );
        assert!(parse_shard("моделька.gguf").is_none());
    }

    #[test]
    fn display_name_drops_directory_extension_and_tail() {
        assert_eq!(
            display_name("/models/gpt-oss-120b-Q8_0-00001-of-00003.gguf").as_deref(),
            Some("gpt-oss-120b-Q8_0")
        );
        assert_eq!(
            display_name(r"C:\GGUF\gemma-4-it.gguf").as_deref(),
            Some("gemma-4-it")
        );
        assert_eq!(
            display_name("bge-m3-Q8_0.gguf").as_deref(),
            Some("bge-m3-Q8_0")
        );
        assert_eq!(display_name(""), None);
        assert_eq!(display_name(".gguf"), None);
        // A tail with no name in front of it stays as-is: cutting it would leave
        // nothing, and the user is better off seeing the file they configured.
        assert_eq!(
            display_name("-00001-of-00003.gguf").as_deref(),
            Some("-00001-of-00003")
        );
    }
}
