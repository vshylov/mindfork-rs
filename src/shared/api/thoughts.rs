//! A streaming splitter for "thoughts" (CoT). Extracts `<think>…</think>` blocks
//! from text arriving in chunks, correctly handling tags cut across a chunk
//! boundary. See spec §6.5 and docs/xinfer-contract.md §3.3.
//!
//! Used as a fallback path when the server returns reasoning inline in
//! `content` (external mode without `XINFER_STREAM_AS_REASONING_CONTENT`). With
//! no tags present, the whole text is treated as regular.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

/// A fragment of the parsed stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Regular reply text.
    Text(String),
    /// Content of a reasoning block.
    Thoughts(String),
}

/// An incremental splitter. State persists across [`push`] calls.
///
/// [`push`]: ThoughtsParser::push
#[derive(Debug, Default)]
pub struct ThoughtsParser {
    in_think: bool,
    /// The unprocessed tail, which may contain a partial tag.
    buf: String,
}

impl ThoughtsParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next chunk, returns the fragments that are ready.
    pub fn push(&mut self, input: &str) -> Vec<Piece> {
        self.buf.push_str(input);
        let mut out = Vec::new();
        loop {
            let (tag, make): (&str, fn(String) -> Piece) = if self.in_think {
                (CLOSE, Piece::Thoughts)
            } else {
                (OPEN, Piece::Text)
            };

            if let Some(i) = self.buf.find(tag) {
                if i > 0 {
                    out.push(make(self.buf[..i].to_string()));
                }
                self.buf.drain(..i + tag.len());
                self.in_think = !self.in_think;
                continue;
            }

            // No full tag present: hold back a tail that might be the start of one.
            let hold = held_suffix_len(&self.buf, tag);
            let emit_to = self.buf.len() - hold;
            if emit_to > 0 {
                out.push(make(self.buf[..emit_to].to_string()));
                self.buf.drain(..emit_to);
            }
            break;
        }
        out
    }

    /// Finishes parsing: the remaining buffer is emitted as-is (a partial tag —
    /// as regular text of the corresponding mode).
    pub fn finish(&mut self) -> Vec<Piece> {
        if self.buf.is_empty() {
            return Vec::new();
        }
        let rest = std::mem::take(&mut self.buf);
        let piece = if self.in_think {
            Piece::Thoughts(rest)
        } else {
            Piece::Text(rest)
        };
        vec![piece]
    }
}

/// The length of the longest suffix of `s` that is a proper prefix of `tag`
/// (a full tag occurrence is handled separately via `find`). Tags are ASCII,
/// so a byte-wise comparison is safe for UTF-8 (the matched tail is ASCII).
fn held_suffix_len(s: &str, tag: &str) -> usize {
    let max_k = (tag.len() - 1).min(s.len());
    let sb = s.as_bytes();
    let tb = tag.as_bytes();
    for k in (1..=max_k).rev() {
        if sb[s.len() - k..] == tb[..k] {
            return k;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Piece {
        Piece::Text(s.to_string())
    }
    fn thoughts(s: &str) -> Piece {
        Piece::Thoughts(s.to_string())
    }

    /// Runs a sequence of chunks and collects the fragments, merging
    /// adjacent ones of the same kind (the parser is allowed to split — the consumer concatenates).
    fn run(chunks: &[&str]) -> Vec<Piece> {
        let mut p = ThoughtsParser::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(p.push(c));
        }
        out.extend(p.finish());
        merge(out)
    }

    #[test]
    fn plain_text_without_tags() {
        assert_eq!(run(&["hello ", "world"]), vec![text("hello world")]);
    }

    #[test]
    fn single_think_block_in_one_chunk() {
        assert_eq!(
            run(&["before<think>reason</think>after"]),
            vec![text("before"), thoughts("reason"), text("after")]
        );
    }

    #[test]
    fn open_tag_split_across_chunks() {
        assert_eq!(
            run(&["a<thi", "nk>b</think>c"]),
            vec![text("a"), thoughts("b"), text("c")]
        );
    }

    #[test]
    fn close_tag_split_across_chunks() {
        assert_eq!(
            run(&["<think>x", "y</thi", "nk>z"]),
            vec![thoughts("xy"), text("z")]
        );
    }

    #[test]
    fn tag_split_at_every_byte() {
        let full = "p<think>q</think>r";
        let mut p = ThoughtsParser::new();
        let mut out = Vec::new();
        for ch in full.chars() {
            out.extend(p.push(&ch.to_string()));
        }
        out.extend(p.finish());
        // Adjacent identical fragments may arrive as separate pieces — merge them.
        assert_eq!(merge(out), vec![text("p"), thoughts("q"), text("r")]);
    }

    #[test]
    fn unterminated_think_is_flushed_as_thoughts() {
        assert_eq!(run(&["a<think>oops"]), vec![text("a"), thoughts("oops")]);
    }

    #[test]
    fn stray_lt_is_literal_text() {
        assert_eq!(run(&["1 < 2 and 3<4"]), vec![text("1 < 2 and 3<4")]);
    }

    #[test]
    fn multibyte_text_around_tags() {
        assert_eq!(
            run(&["привет<think>мысль</think>пока"]),
            vec![text("привет"), thoughts("мысль"), text("пока")]
        );
    }

    fn merge(pieces: Vec<Piece>) -> Vec<Piece> {
        let mut out: Vec<Piece> = Vec::new();
        for p in pieces {
            match (out.last_mut(), &p) {
                (Some(Piece::Text(a)), Piece::Text(b)) => a.push_str(b),
                (Some(Piece::Thoughts(a)), Piece::Thoughts(b)) => a.push_str(b),
                _ => out.push(p),
            }
        }
        out
    }
}
