//! The change journal of a chat's code workspace (spec §9.12,
//! [docs/history/code-workspace.md](../../docs/history/code-workspace.md) §3.5): the **pre-image**
//! of every file the assistant touched, so the changes screen can show what
//! changed and put it back.
//!
//! Why a journal of our own rather than git: the question the user asks is "what
//! did the assistant do", not "what differs from HEAD". The two are different in
//! both directions — an attached directory need not be a repository at all, and a
//! repository routinely carries the user's own uncommitted work, which is not
//! the assistant's doing and must not appear in its diff.
//!
//! It lives under the **app's** data root (`data/workspace/<chat-id>/`), never
//! inside the user's project: a tool that scattered its bookkeeping through a
//! checkout would show up in the user's own `git status`.
//!
//! Two properties are load-bearing:
//!
//! - **Only the first touch is recorded.** The baseline is "the file as it was
//!   before the assistant first changed it in this chat", so a second edit of
//!   the same file must not overwrite it — otherwise the diff would shrink to
//!   the last edit and revert would restore a half-finished state.
//! - **The baselines are not recomputable.** Everything else this track stores
//!   can be rebuilt (an index, a listing); these bytes exist nowhere else once
//!   the file is overwritten. That is why an edit refuses when the journal
//!   cannot be written (`features/tools/code.rs`) instead of proceeding
//!   unjournaled.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One journaled file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Project-relative path with forward slashes — the spelling the tools use.
    pub path: String,
    /// Whether the file existed before the assistant first touched it. `false`
    /// means reverting deletes it rather than restoring bytes.
    pub existed: bool,
    /// File name of the stored pre-image under `baseline/`; `None` when the file
    /// did not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    /// When the assistant first touched this file — what [`Journal::entries`]
    /// orders by.
    ///
    /// `#[serde(default)]` for the reason every stored field in this project
    /// has it: a manifest is JSON on the user's disk, and a row missing this
    /// key fails the whole `Manifest` — which `Journal::load` turns into an
    /// *empty* journal, silently dropping the change list and orphaning the
    /// pre-images it points at. The default is the Unix epoch, and it sorts
    /// first on purpose: a row whose time was never recorded is the oldest
    /// thing known about the file, which is truer than inventing "now".
    #[serde(default)]
    pub first_touched_at: DateTime<Utc>,
}

/// What `manifest.json` holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Manifest {
    /// The workspace root these entries belong to. A different root means a
    /// different project, and the journal starts over (design fork F13).
    #[serde(default)]
    root: String,
    #[serde(default)]
    files: Vec<JournalEntry>,
}

/// The journal of one chat.
pub struct Journal {
    dir: PathBuf,
}

impl Journal {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.json")
    }

    fn baseline_dir(&self) -> PathBuf {
        self.dir.join("baseline")
    }

    /// Reads the manifest; a missing or unreadable one reads as empty.
    ///
    /// Best-effort on *read* deliberately: a corrupt manifest must not make the
    /// workspace unusable, and the worst case is that a file is journaled again
    /// as if untouched — which loses history, not data.
    fn load(&self) -> Manifest {
        std::fs::read(self.manifest_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, manifest: &Manifest) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let text = serde_json::to_string_pretty(manifest)?;
        // Written through a temp file and renamed, like every other artifact the
        // app persists: a manifest torn in half is a journal that cannot be read.
        let tmp = self.manifest_path().with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, self.manifest_path())
            .with_context(|| format!("replacing {}", self.manifest_path().display()))?;
        Ok(())
    }

    /// Records the pre-image of `rel` **before** it is changed, if this is the
    /// first time this chat touches it.
    ///
    /// `current` is the file's bytes, or `None` when it does not exist yet.
    /// Returns `Ok(())` once the change is safe to make; an error means the
    /// caller must not write.
    pub fn record(&self, root: &str, rel: &str, current: Option<&[u8]>) -> Result<()> {
        let mut manifest = self.load();
        if !same_root(&manifest.root, root) {
            // A different project: the previous journal describes files this
            // workspace no longer has, and a stale entry would make the changes
            // screen offer a revert into an unrelated directory.
            //
            // This is the *last* line of that defence, not the only one: it
            // fires on the first write in the new project, and `/project
            // attach` clears the journal before that (design fork F13).
            self.clear()?;
            manifest = Manifest {
                root: root.to_string(),
                files: Vec::new(),
            };
        } else if manifest.root.is_empty() {
            // First use of this chat's journal — stamp it with the project it
            // now describes, which is what every later comparison reads.
            manifest.root = root.to_string();
        }
        if manifest.files.iter().any(|e| e.path == rel) {
            return Ok(()); // already journaled — the first touch is the baseline
        }
        let baseline = match current {
            Some(bytes) => {
                let name = baseline_name(rel);
                let dir = self.baseline_dir();
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("creating {}", dir.display()))?;
                let path = dir.join(&name);
                std::fs::write(&path, bytes)
                    .with_context(|| format!("writing {}", path.display()))?;
                Some(name)
            }
            None => None,
        };
        manifest.files.push(JournalEntry {
            path: rel.to_string(),
            existed: current.is_some(),
            baseline,
            first_touched_at: Utc::now(),
        });
        self.save(&manifest)
    }

    /// Does this journal describe the project at `root`?
    ///
    /// The journal belongs to a **chat**, while the root comes from that chat's
    /// **current** workspace — and `/project attach` can move the second. Every
    /// reader has to ask this first: diffing one project's baselines against
    /// another's tree is not merely wrong on screen, a revert would write the
    /// first project's bytes into the second one's file (design fork F13).
    ///
    /// An empty journal describes every root — it has nothing to misattribute,
    /// and answering `false` would make the very first [`Self::record`] look
    /// like a conflict.
    ///
    /// The comparison is deliberately **fail-safe, not fail-open**: see
    /// [`same_root`].
    pub fn describes(&self, root: &str) -> bool {
        same_root(&self.load().root, root)
    }

    /// Everything journaled for this chat, **oldest touch first**.
    ///
    /// The order comes from the rows' own `first_touched_at` rather than from
    /// the order [`Self::record`] happened to append them in. Push order gives
    /// the same answer today, which is exactly why it was not enough: the
    /// promise is made here and kept by a different function, and a manifest is
    /// a JSON file a user can edit. The sort is stable, so rows sharing a
    /// timestamp — two edits inside one second — keep the order they were
    /// written in.
    ///
    /// The reader half of the journal, consumed by
    /// [`crate::features::workspace_diff`]. Written with the writer
    /// deliberately — the two agree on one manifest shape, and splitting them
    /// across stages is how they drift.
    pub fn entries(&self) -> Vec<JournalEntry> {
        let mut files = self.load().files;
        files.sort_by_key(|e| e.first_touched_at);
        files
    }

    /// The stored pre-image of `rel`, or `None` when it was a new file (or is
    /// not journaled at all). Reverting a file is this plus a write
    /// ([`crate::features::workspace_diff::revert`]).
    pub fn baseline_of(&self, rel: &str) -> Option<Vec<u8>> {
        let entry = self.load().files.into_iter().find(|e| e.path == rel)?;
        let name = entry.baseline?;
        std::fs::read(self.baseline_dir().join(name)).ok()
    }

    /// Drops one file's row and its stored pre-image — what a revert does once
    /// the bytes are back where they belong.
    ///
    /// The baseline file goes with the row: it is the only thing referencing it,
    /// and a directory of orphaned pre-images is bytes of the user's source kept
    /// for no reason. Forgetting something that is not there is not an error —
    /// the end state is the one that was asked for.
    pub fn forget(&self, rel: &str) -> Result<()> {
        let mut manifest = self.load();
        let Some(index) = manifest.files.iter().position(|e| e.path == rel) else {
            return Ok(());
        };
        let entry = manifest.files.remove(index);
        if let Some(name) = entry.baseline {
            let path = self.baseline_dir().join(name);
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
            }
        }
        self.save(&manifest)
    }

    /// Drops the whole journal (a new project, or the chat's workspace detached).
    pub fn clear(&self) -> Result<()> {
        if self.dir.exists() {
            std::fs::remove_dir_all(&self.dir)
                .with_context(|| format!("clearing {}", self.dir.display()))?;
        }
        Ok(())
    }
}

/// Do a manifest's recorded root and a workspace root name the same project?
///
/// An empty `owner` is a journal that has never been written and matches
/// anything. Otherwise plain string equality answers it in every normal case:
/// both spellings are produced by canonicalizing helpers that also strip the
/// Windows `\\?\` prefix (`orchestrator::workspace::canonical_dir`,
/// `tools::code::workspace_root`). A manifest written by hand or by an older
/// build gets a second chance through `canonicalize`.
///
/// **Anything else counts as a different project.** The asymmetry is on
/// purpose: guessing "same" wrongly is how the assistant's bytes end up in an
/// unrelated file, while guessing "different" wrongly only hides a change list
/// whose files a revert could not have resolved anyway
/// (`workspace_diff::resolve` canonicalizes the root and refuses when it does
/// not exist).
fn same_root(owner: &str, root: &str) -> bool {
    if owner.is_empty() || owner == root {
        return true;
    }
    matches!(
        (Path::new(owner).canonicalize(), Path::new(root).canonicalize()),
        (Ok(a), Ok(b)) if a == b
    )
}

/// The baseline file's name: a hash of the project-relative path.
///
/// Hashed rather than sanitized, because a path is not a file name — it carries
/// separators, and on Windows characters that cannot appear in one at all. The
/// mapping only has to be stable and collision-free, which is what a digest is.
fn baseline_name(rel: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    // The first 16 bytes, 32 hex characters — the name this has always written.
    // Spelled out a byte at a time because `digest` 0.11 answers with a
    // `hybrid_array::Array`, which, unlike the `GenericArray` it replaced, has no
    // `LowerHex`; `the_baseline_name_does_not_move_between_builds` pins the result.
    let digest = Sha256::digest(rel.as_bytes());
    digest[..16]
        .iter()
        .fold(String::with_capacity(32), |mut name, b| {
            let _ = write!(name, "{b:02x}");
            name
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal() -> (tempfile::TempDir, Journal) {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::new(dir.path().join("chat"));
        (dir, journal)
    }

    #[test]
    fn records_the_pre_image_and_reads_it_back() {
        let (_d, j) = journal();
        j.record("/proj", "src/a.rs", Some(b"before")).unwrap();
        assert_eq!(j.baseline_of("src/a.rs").as_deref(), Some(&b"before"[..]));
        let entries = j.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "src/a.rs");
        assert!(entries[0].existed);
    }

    /// The property the whole diff rests on: the baseline is the file as it was
    /// before the **first** change, so a second edit must not move it.
    #[test]
    fn only_the_first_touch_is_recorded() {
        let (_d, j) = journal();
        j.record("/proj", "src/a.rs", Some(b"original")).unwrap();
        j.record("/proj", "src/a.rs", Some(b"after the first edit"))
            .unwrap();
        assert_eq!(j.baseline_of("src/a.rs").as_deref(), Some(&b"original"[..]));
        assert_eq!(j.entries().len(), 1);
    }

    /// A file the assistant created has no bytes to restore; reverting it means
    /// deleting it, and the entry has to say so.
    #[test]
    fn a_created_file_is_journaled_as_absent() {
        let (_d, j) = journal();
        j.record("/proj", "src/new.rs", None).unwrap();
        let entries = j.entries();
        assert!(!entries[0].existed);
        assert_eq!(entries[0].baseline, None);
        assert_eq!(j.baseline_of("src/new.rs"), None);
    }

    /// Fork F13: a different project starts a fresh journal, or the changes
    /// screen would offer to revert files this workspace does not have.
    #[test]
    fn attaching_another_project_starts_over() {
        let (_d, j) = journal();
        j.record("/proj-one", "src/a.rs", Some(b"one")).unwrap();
        j.record("/proj-two", "src/b.rs", Some(b"two")).unwrap();
        let entries = j.entries();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].path, "src/b.rs");
        assert_eq!(j.baseline_of("src/a.rs"), None);
    }

    /// The changes screen's row order is this module's promise, so it rests on
    /// the rows' own timestamps rather than on the order they were appended.
    #[test]
    fn entries_come_back_oldest_touch_first() {
        let (_d, j) = journal();
        j.record("/p", "second.rs", Some(b"2")).unwrap();
        j.record("/p", "third.rs", Some(b"3")).unwrap();

        // A manifest is a file on disk: reordered by hand, or written by some
        // future caller in another order. The reader must not care.
        let path = j.manifest_path();
        let mut doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let files = doc["files"].as_array_mut().unwrap();
        files.reverse();
        files.push(serde_json::json!({
            "path": "first.rs",
            "existed": true,
            "first_touched_at": DateTime::<Utc>::default(),
        }));
        std::fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();

        let order: Vec<String> = j.entries().into_iter().map(|e| e.path).collect();
        assert_eq!(order, ["first.rs", "second.rs", "third.rs"], "{order:?}");
    }

    /// A row missing the timestamp costs that row its place in the order — not
    /// the whole journal.
    ///
    /// Without `#[serde(default)]` the `Manifest` fails to parse, `load`
    /// swallows the error by design ("a corrupt manifest must not make the
    /// workspace unusable"), and the user's entire change list disappears while
    /// its baselines stay on disk with nothing referencing them.
    #[test]
    fn a_row_without_a_timestamp_does_not_blank_the_journal() {
        let (_d, j) = journal();
        j.record("/p", "a.rs", Some(b"x")).unwrap();

        let path = j.manifest_path();
        let mut doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(
            doc["files"][0]
                .as_object_mut()
                .unwrap()
                .remove("first_touched_at")
                .is_some(),
            "the fixture has to remove a field that was actually there"
        );
        std::fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();

        let entries = j.entries();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].path, "a.rs");
        assert_eq!(entries[0].first_touched_at, DateTime::<Utc>::default());
        assert_eq!(
            j.baseline_of("a.rs").as_deref(),
            Some(&b"x"[..]),
            "and the pre-image is still reachable"
        );
    }

    /// The question every reader of the journal has to ask before it trusts a
    /// baseline (design fork F13): whose project is this?
    #[test]
    fn a_journal_says_which_project_it_describes() {
        let (_d, j) = journal();
        assert!(
            j.describes("/anywhere"),
            "an empty journal has nothing to misattribute"
        );
        j.record("/proj-one", "src/a.rs", Some(b"one")).unwrap();
        assert!(j.describes("/proj-one"));
        assert!(
            !j.describes("/proj-two"),
            "a different project must not be able to read these baselines"
        );
        // Recording stamps the root even though the first call takes the
        // "same root" branch — everything later compares against it.
        assert!(!j.describes(""), "an unnamed root is not this project");
    }

    /// Two spellings of one directory are one project. The everyday case is
    /// plain equality; this covers a manifest written by hand or by an older
    /// build, where the fallback through `canonicalize` is what keeps a real
    /// change list from silently emptying.
    #[test]
    fn one_directory_spelled_two_ways_is_still_the_same_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let j = Journal::new(dir.path().join("journal"));
        j.record(&root.to_string_lossy(), "a.rs", Some(b"x"))
            .unwrap();

        let roundabout = root.join("sub").join("..").to_string_lossy().into_owned();
        assert_ne!(roundabout, root.to_string_lossy(), "a different spelling");
        assert!(
            j.describes(&roundabout),
            "…of the same directory: {roundabout}"
        );
    }

    #[test]
    fn separators_and_case_do_not_collide() {
        assert_ne!(baseline_name("src/a.rs"), baseline_name("src/b.rs"));
        assert_ne!(baseline_name("src/a.rs"), baseline_name("SRC/A.RS"));
        assert_eq!(baseline_name("src/a.rs"), baseline_name("src/a.rs"));
        // A name, not a path: nothing in it can escape the baseline directory.
        let name = baseline_name("../../etc/passwd");
        assert!(
            !name.contains('/') && !name.contains('\\') && !name.contains('.'),
            "got: {name}"
        );
    }

    /// The name is on disk. A baseline written by an earlier build has to be
    /// found by a later one, so the mapping is pinned rather than merely stable
    /// within a run — a digest crate that changed its output would otherwise
    /// orphan every baseline in silence.
    #[test]
    fn the_baseline_name_does_not_move_between_builds() {
        assert_eq!(
            baseline_name("src/a.rs"),
            "bdfc5619650b795d1ffec5e8af3154a9"
        );
    }

    /// An unwritable journal must be an error, not a shrug: these bytes exist
    /// nowhere else once the file is overwritten, so the caller has to be able
    /// to refuse the edit.
    #[test]
    fn a_journal_that_cannot_be_written_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        // A *file* where the journal directory should be — portable across
        // platforms, unlike permission juggling.
        let blocked = dir.path().join("chat");
        std::fs::write(&blocked, "not a directory").unwrap();
        let j = Journal::new(&blocked);
        assert!(j.record("/proj", "src/a.rs", Some(b"x")).is_err());
    }

    /// Forgetting one file takes its stored pre-image with it: nothing else
    /// references those bytes, and they are the user's source.
    #[test]
    fn forget_drops_the_row_and_its_baseline() {
        let (_d, j) = journal();
        j.record("/proj", "src/a.rs", Some(b"a")).unwrap();
        j.record("/proj", "src/b.rs", Some(b"b")).unwrap();
        j.forget("src/a.rs").unwrap();
        let entries = j.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "src/b.rs");
        assert_eq!(j.baseline_of("src/a.rs"), None);
        // The other file's baseline must survive — a shared directory means a
        // careless delete takes a neighbour with it.
        assert_eq!(j.baseline_of("src/b.rs").as_deref(), Some(&b"b"[..]));
        // Forgetting what is not there is the end state already.
        assert!(j.forget("src/a.rs").is_ok());
    }

    #[test]
    fn clear_removes_everything() {
        let (_d, j) = journal();
        j.record("/proj", "src/a.rs", Some(b"x")).unwrap();
        j.clear().unwrap();
        assert!(j.entries().is_empty());
        assert_eq!(j.baseline_of("src/a.rs"), None);
        // Clearing an absent journal is not an error — detaching twice is fine.
        assert!(j.clear().is_ok());
    }
}
