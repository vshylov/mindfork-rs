//! A chat's stored files on disk (`data/files/<chat-id>/`): writing an output under a free
//! name, finding what the chat does not list, deleting our copy. See
//! docs/history/sandbox-file-exchange.md (F2, F4, F10, §11 S5, S6, S11), spec §9.7.
//!
//! The listing is [`ChatFile`] in `Chat.files`, and the orchestrator owns it; this module
//! only touches bytes. Every name it is handed is checked to be one plain component before
//! it is joined to the folder, so a name edited into a chat file cannot reach outside it
//! (§6.2).

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::entities::chat_file::{
    ChatFile, FileOrigin, mime_for, same_name, sha256_hex, versioned,
};

/// How many versions of one name storing tries before it gives up.
const MAX_VERSIONS: u32 = 10_000;

/// What storing one output did.
#[derive(Debug, Clone, PartialEq)]
pub enum Stored {
    /// Written, under this listing's name: the one asked for, or its next free version.
    New(ChatFile),
    /// Nothing written: the chat already lists a file of this name's family with the same
    /// bytes, **and** the copy is there.
    Unchanged(ChatFile),
    /// The listing was right and the bytes were gone: the chat lists this file with these
    /// contents, nothing was on disk under its name, and it has just been written back.
    ///
    /// Listed-but-missing is a state the app supports and reports (`StoredInfo.missing`,
    /// `/file list`), and the way out of it is to produce the bytes again — a call that
    /// re-renders the same chart, or the user re-attaching the same document, which is
    /// what `tool.python_exec.err.files_missing` tells the model to ask for. Matching on
    /// the listing alone made that advice a dead end: the name and the digest agreed, so
    /// nothing was written and the copy stayed missing however many times it was offered.
    Restored(ChatFile),
}

/// Writes `content` into `dir` as `name` (already sanitized) or, when the name is taken —
/// by a listed file with other bytes, or by any file on disk, compared case-insensitively
/// — as its next free version, `name (2).ext`… A listed file of the family with the same
/// SHA-256 comes back as [`Stored::Unchanged`] with nothing written **when its copy is on
/// disk**, and as [`Stored::Restored`] when it is not: the listing is not evidence that the
/// bytes are there, and offering them again is the documented way back from
/// listed-but-missing. The file is opened with `create_new`, so an existing one is never
/// overwritten whatever raced, and synced before the listing is returned: the listing must
/// not name bytes a crash could lose.
pub fn store(dir: &Path, listed: &[ChatFile], name: &str, content: &[u8]) -> io::Result<Stored> {
    store_as(dir, listed, name, content, FileOrigin::Sandbox)
}

/// [`store`], for a file that is not a call's output: `/file attach` keeping the user's
/// own bytes beside the extracted text ([`FileOrigin::Attached`], fork F8a). The writing
/// is the same in every respect — the origin is only what the listing records.
pub fn store_as(
    dir: &Path,
    listed: &[ChatFile],
    name: &str,
    content: &[u8],
    origin: FileOrigin,
) -> io::Result<Stored> {
    confined(dir, name)?;
    std::fs::create_dir_all(dir)?;
    let sha256 = sha256_hex(content);
    let on_disk = names_in(dir);
    for n in 1..=MAX_VERSIONS {
        let candidate = versioned(name, n);
        if let Some(existing) = listed.iter().find(|f| same_name(&f.name, &candidate)) {
            if existing.sha256 == sha256 {
                // The listing is not evidence that the bytes are there. `on_disk` is
                // already in hand, so this costs nothing and turns "nothing to do" into
                // "put it back" exactly when the copy has gone missing.
                if on_disk.iter().any(|d| same_name(d, &candidate)) {
                    return Ok(Stored::Unchanged(existing.clone()));
                }
                match write_new(&dir.join(&candidate), content) {
                    // Written between the listing and now: the bytes are there either way,
                    // and they are the same bytes — the digest already agreed.
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        return Ok(Stored::Unchanged(existing.clone()));
                    }
                    other => other?,
                }
                return Ok(Stored::Restored(existing.clone()));
            }
            continue;
        }
        if on_disk.iter().any(|d| same_name(d, &candidate)) {
            continue;
        }
        match write_new(&dir.join(&candidate), content) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
        return Ok(Stored::New(ChatFile {
            id: uuid::Uuid::new_v4(),
            mime: mime_for(&candidate, content).to_string(),
            name: candidate,
            origin,
            bytes: content.len() as u64,
            sha256,
            added_at: chrono::Utc::now(),
        }));
    }
    Err(io::Error::other("no free version of the name"))
}

/// Writes `content` to `path`, creating it and never overwriting: `create_new`, so whatever
/// raced keeps its file, and `sync_all` before the caller lists it — a listing must not
/// name bytes a crash could lose. A half-written file is removed rather than left behind.
/// `AlreadyExists` is passed back untouched: what an occupied name means is the caller's to
/// say, and the two callers here say different things.
fn write_new(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(e) = file.write_all(content).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

/// The regular files in `dir` that `listed` does not name — never following a link — as
/// [`FileOrigin::Recovered`] listings, in name order (§11 S6). Nothing is deleted: a file
/// here that no listing names was written by a call whose chat was not saved, and the
/// user may already have seen it. A name that is not UTF-8 cannot be listed faithfully and
/// is left alone.
pub fn unlisted(dir: &Path, listed: &[ChatFile]) -> Vec<ChatFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<ChatFile> = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if listed
            .iter()
            .chain(&found)
            .any(|f| same_name(&f.name, &name))
        {
            continue;
        }
        if !std::fs::symlink_metadata(entry.path()).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let Ok((sha256, head, bytes)) = hash_file(&entry.path()) else {
            continue;
        };
        found.push(ChatFile {
            id: uuid::Uuid::new_v4(),
            mime: mime_for(&name, &head).to_string(),
            name,
            origin: FileOrigin::Recovered,
            bytes,
            sha256,
            added_at: chrono::Utc::now(),
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Deletes our copy of a stored file. A file already gone counts as deleted.
pub fn remove(dir: &Path, name: &str) -> io::Result<()> {
    match std::fs::remove_file(confined(dir, name)?) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Whether a stored file's bytes are on disk.
pub fn exists(dir: &Path, name: &str) -> bool {
    confined(dir, name).is_ok_and(|path| path.is_file())
}

/// `dir/name` when `name` is one plain component — no separator, no drive, not `.` or
/// `..` — and an error otherwise, rather than a join that could leave the folder.
///
/// Public because it is the folder's invariant, not this module's: a stored name is read
/// back from `chat.json` without being validated again, so **every** join of a stored
/// name into the folder goes through here — `/file open` included
/// ([`chat_inputs::open_path`](crate::features::chat_inputs::open_path)).
pub fn confined(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let plain = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':']);
    if plain {
        Ok(dir.join(name))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a plain file name: {name:?}"),
        ))
    }
}

/// The names in `dir` (lossy), for the case-insensitive collision check.
fn names_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// A file's SHA-256 (lowercase hex), its first bytes (enough to recognise an image) and
/// its size — streamed, so a large file is not read into memory whole.
fn hash_file(path: &Path) -> io::Result<(String, Vec<u8>, u64)> {
    use sha2::{Digest, Sha256};
    const HEAD: usize = 64;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut head = Vec::with_capacity(HEAD);
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if head.len() < HEAD {
            head.extend_from_slice(&buf[..n.min(HEAD - head.len())]);
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    let hex = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((hex, head, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n-a-chart-";

    fn listing(name: &str, content: &[u8]) -> ChatFile {
        ChatFile::new(name, FileOrigin::Sandbox, content)
    }

    fn new_name(stored: Stored) -> String {
        match stored {
            Stored::New(file) => file.name,
            Stored::Unchanged(file) => panic!("expected a new file, got unchanged {}", file.name),
            Stored::Restored(file) => panic!("expected a new file, got restored {}", file.name),
        }
    }

    #[test]
    fn stores_under_the_name_and_lists_what_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let Stored::New(file) = store(dir.path(), &[], "chart.png", PNG).unwrap() else {
            panic!("expected a new file");
        };
        assert_eq!(file.name, "chart.png");
        assert_eq!(file.mime, "image/png");
        assert_eq!(file.bytes, PNG.len() as u64);
        assert_eq!(file.sha256, sha256_hex(PNG));
        assert_eq!(file.origin, FileOrigin::Sandbox);
        assert_eq!(std::fs::read(dir.path().join("chart.png")).unwrap(), PNG);
    }

    #[test]
    fn a_listed_name_with_other_bytes_gets_the_next_version_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let listed = [listing("Chart.PNG", b"older")];
        std::fs::write(dir.path().join("Chart.PNG"), b"older").unwrap();
        let name = new_name(store(dir.path(), &listed, "chart.png", PNG).unwrap());
        assert_eq!(name, "chart (2).png");
        assert_eq!(
            std::fs::read(dir.path().join("Chart.PNG")).unwrap(),
            b"older"
        );
    }

    #[test]
    fn the_same_bytes_under_a_listed_name_of_the_family_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let listed = [
            listing("chart.png", b"first"),
            listing("chart (2).png", PNG),
        ];
        // The copy is where the listing says it is, so there is genuinely nothing to do.
        std::fs::write(dir.path().join("chart.png"), b"first").unwrap();
        std::fs::write(dir.path().join("chart (2).png"), PNG).unwrap();

        let stored = store(dir.path(), &listed, "chart.png", PNG).unwrap();
        assert_eq!(stored, Stored::Unchanged(listed[1].clone()));
        assert_eq!(names_in(dir.path()).len(), 2, "nothing was written");
    }

    /// Listed-but-missing is a supported state, and the way out of it is to offer the
    /// bytes again — the refusal `python_exec` hands the model says exactly that. Matching
    /// on the listing alone made that advice a dead end: name and digest agreed, so nothing
    /// was written and the copy stayed gone however many times it was offered.
    #[test]
    fn the_same_bytes_put_a_listed_copy_back_when_it_has_gone_missing() {
        let dir = tempfile::tempdir().unwrap();
        let listed = [listing("chart.png", PNG)];
        // The listing stands, the folder is empty — a pruned `data/files/`, a partial sync,
        // a chat file restored without its folder.
        assert_eq!(names_in(dir.path()).len(), 0);

        let stored = store(dir.path(), &listed, "chart.png", PNG).unwrap();
        assert_eq!(
            stored,
            Stored::Restored(listed[0].clone()),
            "the listing keeps its identity; only the bytes came back"
        );
        assert_eq!(
            std::fs::read(dir.path().join("chart.png")).unwrap(),
            PNG,
            "the copy has to be readable again, which is the point"
        );
        // And offering them once more is then the no-op it claims to be.
        let again = store(dir.path(), &listed, "chart.png", PNG).unwrap();
        assert_eq!(again, Stored::Unchanged(listed[0].clone()));
    }

    /// An unlisted file already on disk — say, one a crashed session left — is never
    /// overwritten: the output moves on to the next version.
    #[test]
    fn a_file_on_disk_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chart.png"), b"kept").unwrap();
        let name = new_name(store(dir.path(), &[], "chart.png", PNG).unwrap());
        assert_eq!(name, "chart (2).png");
        assert_eq!(
            std::fs::read(dir.path().join("chart.png")).unwrap(),
            b"kept"
        );
    }

    #[test]
    fn a_name_that_is_not_one_component_is_refused_and_nothing_is_written() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("chat");
        for name in ["../escape.txt", r"..\escape.txt", "..", "", "C:escape.txt"] {
            let err = store(&dir, &[], name, b"x").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{name:?}");
            assert!(remove(&dir, name).is_err(), "{name:?}");
        }
        assert!(!root.path().join("escape.txt").exists());
        assert!(!dir.exists(), "a refused store creates nothing");
    }

    #[test]
    fn unlisted_files_are_found_with_their_hash_and_type_and_nothing_else_is() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("listed.csv"), b"a,b").unwrap();
        std::fs::write(dir.path().join("orphan.png"), PNG).unwrap();
        std::fs::create_dir(dir.path().join("a directory")).unwrap();
        let listed = [listing("LISTED.csv", b"a,b")];
        let found = unlisted(dir.path(), &listed);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "orphan.png");
        assert_eq!(found[0].origin, FileOrigin::Recovered);
        assert_eq!(found[0].mime, "image/png");
        assert_eq!(found[0].sha256, sha256_hex(PNG));
        assert_eq!(found[0].bytes, PNG.len() as u64);
        assert!(
            dir.path().join("orphan.png").exists(),
            "adopting deletes nothing"
        );
    }

    #[test]
    fn an_absent_folder_holds_nothing_unlisted() {
        let dir = tempfile::tempdir().unwrap();
        assert!(unlisted(&dir.path().join("none"), &[]).is_empty());
    }

    #[test]
    fn remove_deletes_our_copy_and_a_missing_one_counts_as_deleted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chart.png"), PNG).unwrap();
        assert!(exists(dir.path(), "chart.png"));
        remove(dir.path(), "chart.png").unwrap();
        assert!(!exists(dir.path(), "chart.png"));
        remove(dir.path(), "chart.png").unwrap();
    }
}
