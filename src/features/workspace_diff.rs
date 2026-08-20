//! What the assistant changed in the attached project, as a diff the changes
//! screen can draw — and putting one file back (spec §9.12,
//! [docs/code-workspace.md](../../docs/code-workspace.md) §3.5).
//!
//! The input is the change journal (`features/workspace_journal.rs`): the bytes
//! of every file as it stood **before** the assistant first touched it in this
//! chat. Against the file as it stands now, that is the answer to "what did the
//! assistant do" — which is deliberately not "what differs from HEAD": an
//! attached directory need not be a repository, and a repository routinely
//! carries the user's own uncommitted work.
//!
//! **The diff is built here, not in the screen.** The plan sketched a screen
//! that diffs the selected file lazily from baseline and current text; the
//! codebase's own rule is stronger and cheaper — a screen is a pure projection
//! of a snapshot the orchestrator built off the runtime (the message-search
//! screen says so in as many words). It also decides the size of the event: a
//! rendered diff is a fraction of the two files it came from, and it is computed
//! once instead of on every `↑`.
//!
//! Everything that can go wrong with a file is a **state**, not an error: gone,
//! binary, too large, unchanged after all. Each of those has a different next
//! move for the user, so each says which it is rather than showing an empty
//! diff (docs/lessons.md §4).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::features::workspace_journal::Journal;

/// Files past this are described rather than diffed. A diff of a megabyte of
/// text is not something anyone reads in a terminal pane, and building it would
/// stall the frame it is drawn in.
const MAX_DIFF_BYTES: u64 = 1024 * 1024;

/// How many unchanged lines surround each hunk.
const CONTEXT_LINES: usize = 3;

/// What one line of a rendered diff is, so the screen colours it without
/// re-parsing the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// `@@ … @@` — where in the file the next lines are.
    Hunk,
    /// A line both versions have.
    Context,
    Added,
    Removed,
}

/// One line of a rendered diff.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// The text **without** its `+`/`-`/space marker: the screen draws the
    /// marker itself, in the colour that goes with it, and a marker baked into
    /// the string would be indistinguishable from a `+` the file's own line
    /// starts with.
    pub text: String,
}

/// What became of a journaled file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// It existed and was changed.
    Modified,
    /// The assistant created it — reverting deletes it.
    Created,
    /// Journaled, but it is not on disk now. Nothing here can delete a file, so
    /// this is the user's own doing (or a revert that already happened), and
    /// saying so beats showing the whole file as removed.
    Gone,
    /// Binary, or larger than [`MAX_DIFF_BYTES`]: no diff, but the revert still
    /// works — it is a byte copy either way.
    NotShown,
    /// Touched, then put back to exactly what it was. Kept in the list rather
    /// than hidden: "I changed it and changed it back" is an answer, and a file
    /// vanishing from the list would look like the journal lost it.
    Unchanged,
}

/// One file of the change set.
#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    /// Project-relative, forward slashes — the spelling the tools use.
    pub path: String,
    pub state: FileState,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
}

/// Everything the assistant changed in this chat's project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChangeSet {
    /// The project root, for the screen's title. Empty when nothing is
    /// journaled and there is no root to name.
    pub root: String,
    pub files: Vec<FileChange>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Builds the change set for one chat: every journaled file, diffed against what
/// is on disk now.
///
/// Blocking file I/O — the caller runs it off the async runtime.
pub fn build(journal_dir: &Path, root: &str) -> ChangeSet {
    let journal = Journal::new(journal_dir.to_path_buf());
    let files = journal
        .entries()
        .into_iter()
        .map(|entry| {
            let baseline = journal.baseline_of(&entry.path).unwrap_or_default();
            diff_file(root, &entry.path, entry.existed, &baseline)
        })
        .collect();
    ChangeSet {
        root: root.to_string(),
        files,
    }
}

/// Resolves a project-relative path against the root.
///
/// The journal only ever holds paths the tools produced, which are already
/// confined — but this reads and *writes* the user's files, so it re-checks
/// rather than trusting a stored string: a guard that assumes its input is
/// clean is one edited `manifest.json` from being no guard at all.
fn resolve(root: &str, rel: &str) -> Option<PathBuf> {
    let root = PathBuf::from(root).canonicalize().ok()?;
    let joined = root.join(rel);
    // The file may be gone, so the path itself need not canonicalize; what has
    // to hold is that no component climbs out.
    if joined
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    joined.starts_with(&root).then_some(joined)
}

/// One file's row: its state, its counts and its rendered diff.
fn diff_file(root: &str, rel: &str, existed: bool, baseline: &[u8]) -> FileChange {
    let row = |state: FileState, lines: Vec<DiffLine>| FileChange {
        path: rel.to_string(),
        state,
        added: 0,
        removed: 0,
        lines,
    };
    let Some(path) = resolve(root, rel) else {
        return row(FileState::Gone, Vec::new());
    };
    let Ok(current) = std::fs::read(&path) else {
        return row(FileState::Gone, Vec::new());
    };
    let too_large = current.len() as u64 > MAX_DIFF_BYTES || baseline.len() as u64 > MAX_DIFF_BYTES;
    let binary = current.contains(&0) || baseline.contains(&0);
    if too_large || binary {
        return row(FileState::NotShown, Vec::new());
    }
    // Normalized to `\n` on both sides, for the reason the editing tools
    // normalize: a CRLF checkout would otherwise show every line as changed.
    let before = String::from_utf8_lossy(baseline).replace("\r\n", "\n");
    let after = String::from_utf8_lossy(&current).replace("\r\n", "\n");
    if before == after {
        let state = if existed {
            FileState::Unchanged
        } else {
            // Created, and the "baseline" is emptiness: an empty file the
            // assistant created really is unchanged against it, and calling
            // that "unchanged" would hide that the file is new.
            FileState::Created
        };
        return row(state, Vec::new());
    }
    let (lines, added, removed) = render(&before, &after);
    FileChange {
        path: rel.to_string(),
        state: if existed {
            FileState::Modified
        } else {
            FileState::Created
        },
        added,
        removed,
        lines,
    }
}

/// Renders a unified diff into lines the screen can colour, plus the counts.
fn render(before: &str, after: &str) -> (Vec<DiffLine>, usize, usize) {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(before, after);
    let mut lines = Vec::new();
    let (mut added, mut removed) = (0usize, 0usize);
    for (i, group) in diff.grouped_ops(CONTEXT_LINES).into_iter().enumerate() {
        if let (Some(first), Some(last)) = (group.first(), group.last()) {
            let (old, new) = (first.old_range().start, first.new_range().start);
            let (old_end, new_end) = (last.old_range().end, last.new_range().end);
            lines.push(DiffLine {
                kind: DiffKind::Hunk,
                text: format!(
                    "@@ -{},{} +{},{} @@",
                    old + 1,
                    old_end - old,
                    new + 1,
                    new_end - new
                ),
            });
        } else if i > 0 {
            continue;
        }
        for op in group {
            for change in diff.iter_changes(&op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => DiffKind::Context,
                    ChangeTag::Insert => {
                        added += 1;
                        DiffKind::Added
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        DiffKind::Removed
                    }
                };
                lines.push(DiffLine {
                    kind,
                    // The trailing newline is the line separator, not content:
                    // left in, it would draw an empty row after every line.
                    text: change.value().trim_end_matches('\n').to_string(),
                });
            }
        }
    }
    (lines, added, removed)
}

/// Puts one file back to its baseline and forgets it.
///
/// The two halves are one operation on purpose: a restored file still listed as
/// changed would offer a second revert that does nothing, and a forgotten entry
/// whose file was not restored loses the bytes for good. The write happens
/// first — if it fails, the journal still holds the baseline and the user can
/// try again.
pub fn revert(journal_dir: &Path, root: &str, rel: &str) -> Result<()> {
    let journal = Journal::new(journal_dir.to_path_buf());
    let entry = journal
        .entries()
        .into_iter()
        .find(|e| e.path == rel)
        .with_context(|| format!("{rel} is not in this chat's change journal"))?;
    let path = resolve(root, rel).with_context(|| format!("{rel} is outside {root}"))?;
    if entry.existed {
        let baseline = journal
            .baseline_of(rel)
            .with_context(|| format!("the stored original of {rel} is missing"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, &baseline)
            .with_context(|| format!("restoring {}", path.display()))?;
    } else if path.exists() {
        // The assistant created it, so putting it back means removing it. A
        // file already gone is not an error — the end state is what was asked
        // for.
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    journal.forget(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project directory, a journal directory, and a `Journal` over it.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        journal_dir: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("proj");
            std::fs::create_dir_all(&root).unwrap();
            let journal_dir = dir.path().join("journal");
            Self {
                _dir: dir,
                root,
                journal_dir,
            }
        }

        fn root(&self) -> String {
            self.root.to_string_lossy().into_owned()
        }

        fn journal(&self) -> Journal {
            Journal::new(self.journal_dir.clone())
        }

        /// Writes `before` (or nothing), journals it, then writes `after` (or
        /// removes the file) — the assistant's edit, reproduced.
        fn touched(&self, rel: &str, before: Option<&str>, after: Option<&str>) {
            let path = self.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if let Some(before) = before {
                std::fs::write(&path, before).unwrap();
            }
            self.journal()
                .record(&self.root(), rel, before.map(str::as_bytes))
                .unwrap();
            match after {
                Some(after) => std::fs::write(&path, after).unwrap(),
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        fn build(&self) -> ChangeSet {
            build(&self.journal_dir, &self.root())
        }
    }

    #[test]
    fn a_changed_file_is_diffed_with_counts() {
        let f = Fixture::new();
        f.touched(
            "src/a.rs",
            Some("one\ntwo\nthree\n"),
            Some("one\n2\nthree\n"),
        );
        let set = f.build();
        assert_eq!(set.files.len(), 1);
        let file = &set.files[0];
        assert_eq!(file.path, "src/a.rs");
        assert_eq!(file.state, FileState::Modified);
        assert_eq!((file.added, file.removed), (1, 1));
        assert!(
            file.lines.iter().any(|l| l.kind == DiffKind::Hunk),
            "a hunk header is what says where in the file this is: {:?}",
            file.lines
        );
        assert!(
            file.lines
                .iter()
                .any(|l| l.kind == DiffKind::Removed && l.text == "two")
        );
        assert!(
            file.lines
                .iter()
                .any(|l| l.kind == DiffKind::Added && l.text == "2")
        );
        // The marker is the screen's to draw: a line carrying its own `+` could
        // not be told from a line whose text starts with one.
        assert!(
            file.lines.iter().all(|l| !l.text.starts_with('+')),
            "{:?}",
            file.lines
        );
    }

    /// Each of these has a different next move for the user, so each has to be
    /// distinguishable — an empty diff for all of them would say nothing
    /// (docs/lessons.md §4). Looped over the states rather than four
    /// near-identical tests.
    #[test]
    fn every_state_is_reported_as_itself() {
        let f = Fixture::new();
        f.touched("created.rs", None, Some("fn main() {}\n"));
        f.touched("modified.rs", Some("a\n"), Some("b\n"));
        f.touched("gone.rs", Some("a\n"), None);
        f.touched("undone.rs", Some("same\n"), Some("same\n"));
        std::fs::write(f.root.join("binary.rs"), [0u8, 1, 2]).unwrap();
        f.journal()
            .record(&f.root(), "binary.rs", Some(&[0u8, 9]))
            .unwrap();

        let set = f.build();
        let state = |name: &str| {
            set.files
                .iter()
                .find(|c| c.path == name)
                .unwrap_or_else(|| panic!("{name} missing from {:?}", set.files))
                .state
        };
        assert_eq!(state("created.rs"), FileState::Created);
        assert_eq!(state("modified.rs"), FileState::Modified);
        assert_eq!(state("gone.rs"), FileState::Gone);
        assert_eq!(state("undone.rs"), FileState::Unchanged);
        assert_eq!(state("binary.rs"), FileState::NotShown);
    }

    /// A Windows checkout is CRLF and the model writes `\n`. Without
    /// normalization every line of every file would read as changed, and the
    /// screen would be useless on exactly the platform this app targets first.
    #[test]
    fn line_endings_alone_are_not_a_change() {
        let f = Fixture::new();
        f.touched("crlf.rs", Some("a\r\nb\r\n"), Some("a\nb\n"));
        let set = f.build();
        assert_eq!(set.files[0].state, FileState::Unchanged);
        assert_eq!((set.files[0].added, set.files[0].removed), (0, 0));
    }

    /// Reverting restores the bytes **and** drops the row: a file put back but
    /// still listed offers a second revert that does nothing.
    #[test]
    fn reverting_restores_the_original_and_forgets_it() {
        let f = Fixture::new();
        f.touched("src/a.rs", Some("original\n"), Some("changed\n"));
        revert(&f.journal_dir, &f.root(), "src/a.rs").unwrap();
        assert_eq!(
            std::fs::read_to_string(f.root.join("src/a.rs")).unwrap(),
            "original\n"
        );
        assert!(f.build().is_empty(), "the row must be gone too");
    }

    /// The one deletion that exists in v1: a file the assistant created has no
    /// bytes to restore, so putting it back means removing it.
    #[test]
    fn reverting_a_created_file_deletes_it() {
        let f = Fixture::new();
        f.touched("src/new.rs", None, Some("fn main() {}\n"));
        revert(&f.journal_dir, &f.root(), "src/new.rs").unwrap();
        assert!(!f.root.join("src/new.rs").exists());
        assert!(f.build().is_empty());
    }

    /// Reverting something that is not journaled must fail rather than touch a
    /// file: the journal is the whole of what this may write to.
    #[test]
    fn reverting_an_unjournaled_path_is_refused() {
        let f = Fixture::new();
        std::fs::write(f.root.join("untouched.rs"), "mine\n").unwrap();
        assert!(revert(&f.journal_dir, &f.root(), "untouched.rs").is_err());
        assert_eq!(
            std::fs::read_to_string(f.root.join("untouched.rs")).unwrap(),
            "mine\n"
        );
    }

    /// A `manifest.json` is a file on disk and can be edited by hand. Neither
    /// the diff nor the revert may follow a path out of the project — the same
    /// containment the tools apply, applied again where the bytes are written.
    #[test]
    fn a_path_escaping_the_root_is_refused() {
        let f = Fixture::new();
        let outside = f.root.parent().unwrap().join("secret.txt");
        std::fs::write(&outside, "not yours\n").unwrap();
        assert!(resolve(&f.root(), "../secret.txt").is_none());
        assert!(revert(&f.journal_dir, &f.root(), "../secret.txt").is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "not yours\n");
    }

    #[test]
    fn an_empty_journal_is_an_empty_change_set() {
        let f = Fixture::new();
        assert!(f.build().is_empty());
    }
}
