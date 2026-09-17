//! JSON storage: config (`settings.json`), profiles (`profiles.json`),
//! chats (`chats/{id}.json`). Atomic write (write-temp + rename) with a backup.
//! See spec §5.2.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::entities::chat::Chat;
use crate::entities::profile::Profile;
use crate::shared::config::AppConfig;
use crate::shared::paths::Paths;

/// A chat file as seen by a stat-only walk: its id and the cheap change signal
/// the search index reconciles against (docs/research/chat-content-search.md §3).
///
/// `(mtime_ms, size)` is the standard cheap signal. A content hash would be
/// exact, but it needs reading every byte, and the failure mode it guards
/// against — a same-size edit within the mtime resolution — is not realistic
/// for a JSON file the app itself rewrites wholesale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatFileInfo {
    pub id: Uuid,
    /// Last-modified time in milliseconds since the Unix epoch (0 when the
    /// platform cannot report it — then the `size` half still catches edits).
    pub mtime_ms: i64,
    pub size: u64,
}

/// Last-modified time in milliseconds since the epoch. Unsupported/unavailable
/// times, and times before the epoch, degrade to `0` rather than failing: this
/// only feeds a "did it change?" comparison, and a constant simply makes the
/// `size` half do the work.
fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// File-based JSON storage of config, profiles, and chats.
pub struct JsonStore {
    paths: Paths,
}

impl JsonStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// The app's own directories, out of every file and code tool's reach
    /// ([`Paths::app_dirs`]).
    pub fn app_dirs(&self) -> Vec<std::path::PathBuf> {
        self.paths.app_dirs()
    }

    /// Python sandbox directory (`sandbox/`) — for the tool registry
    /// (`python_exec`, Wasmer mode). See [`Paths::sandbox_dir`].
    pub fn sandbox_dir(&self) -> std::path::PathBuf {
        self.paths.sandbox_dir()
    }

    /// Change journals of chats' code workspaces (spec §9.12).
    pub fn workspace_dir(&self) -> std::path::PathBuf {
        self.paths.workspace_dir()
    }

    /// Files stored with chats (`files/`). See [`Paths::files_dir`].
    pub fn files_dir(&self) -> std::path::PathBuf {
        self.paths.files_dir()
    }

    // ---------- config ----------

    /// Loads the config; returns the default if the file is missing.
    pub fn load_config(&self) -> Result<AppConfig> {
        Ok(read_json(&self.paths.settings_file())?.unwrap_or_default())
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        write_json(&self.paths.settings_file(), config)
    }

    // ---------- profiles (one file holding the list) ----------

    /// All profiles (including hidden ones).
    pub fn load_profiles(&self) -> Result<Vec<Profile>> {
        Ok(read_json(&self.paths.profiles_file())?.unwrap_or_default())
    }

    pub fn save_profiles(&self, profiles: &[Profile]) -> Result<()> {
        write_json(&self.paths.profiles_file(), &profiles)
    }

    /// Adds or replaces a profile by `id`.
    pub fn upsert_profile(&self, profile: &Profile) -> Result<()> {
        let mut profiles = self.load_profiles()?;
        match profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile.clone(),
            None => profiles.push(profile.clone()),
        }
        self.save_profiles(&profiles)
    }

    /// Marks a profile hidden. Returns `true` if the profile was found.
    pub fn hide_profile(&self, id: Uuid) -> Result<bool> {
        let mut profiles = self.load_profiles()?;
        let Some(p) = profiles.iter_mut().find(|p| p.id == id) else {
            return Ok(false);
        };
        p.is_hidden = true;
        self.save_profiles(&profiles)?;
        Ok(true)
    }

    // ---------- chats (one file per chat) ----------

    pub fn save_chat(&self, chat: &Chat) -> Result<()> {
        write_json(&self.paths.chat_file(&chat.id.to_string()), chat)
    }

    pub fn load_chat(&self, id: Uuid) -> Result<Option<Chat>> {
        read_json(&self.paths.chat_file(&id.to_string()))
    }

    /// All chats (including hidden ones); does not guarantee any sort order.
    pub fn load_chats(&self) -> Result<Vec<Chat>> {
        let dir = self.paths.chats_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut chats = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // A corrupt chat file is skipped with a warning instead of failing the
            // whole startup (release-engineering.md F11): one broken JSON file must
            // not block the app and must not be silently lost — the file stays on
            // disk for manual repair.
            match read_json::<Chat>(&path) {
                Ok(Some(chat)) => chats.push(chat),
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(file = %path.display(), error = %err,
                        "skipped a corrupted chat file");
                }
            }
        }
        Ok(chats)
    }

    /// A **stat-only** listing of the chat files: id + the change signal
    /// `(mtime_ms, size)`. Nothing is parsed — the whole point is that
    /// answering "did anything change since we indexed?" costs a directory
    /// walk (~0.3 ms on the real corpus) instead of a 94 ms parse of every
    /// chat. See docs/research/chat-content-search.md §3.
    ///
    /// Files that are not `*.json`, or whose stem is not a UUID, are skipped:
    /// the chat id *is* the file name, so an unparseable name is not a chat.
    /// A file whose metadata cannot be read is skipped too — the reconciliation
    /// simply leaves that chat as it is.
    pub fn chat_files(&self) -> Result<Vec<ChatFileInfo>> {
        let dir = self.paths.chats_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                continue;
            };
            // `DirEntry::metadata` is cheaper than `fs::metadata` on Windows:
            // it reuses what the directory scan already returned.
            let Ok(meta) = entry.metadata() else { continue };
            out.push(ChatFileInfo {
                id,
                mtime_ms: mtime_ms(&meta),
                size: meta.len(),
            });
        }
        Ok(out)
    }

    /// Metadata of one chat file (see [`JsonStore::chat_files`]) — for the
    /// post-save index update, which knows the id already.
    pub fn chat_file_info(&self, id: Uuid) -> Option<ChatFileInfo> {
        let meta = fs::metadata(self.paths.chat_file(&id.to_string())).ok()?;
        Some(ChatFileInfo {
            id,
            mtime_ms: mtime_ms(&meta),
            size: meta.len(),
        })
    }

    /// Marks a chat hidden. Returns `true` if the chat was found.
    pub fn hide_chat(&self, id: Uuid) -> Result<bool> {
        let Some(mut chat) = self.load_chat(id)? else {
            return Ok(false);
        };
        chat.is_hidden = true;
        self.save_chat(&chat)?;
        Ok(true)
    }

    /// Hides all chats of a profile (a cascade when hiding the profile). Returns the count.
    pub fn hide_chats_of_profile(&self, profile_id: Uuid) -> Result<usize> {
        let mut count = 0;
        for mut chat in self.load_chats()? {
            if chat.profile_id == profile_id && !chat.is_hidden {
                chat.is_hidden = true;
                self.save_chat(&chat)?;
                count += 1;
            }
        }
        Ok(count)
    }
}

/// Reads and deserializes a JSON file; `None` if the file is missing. `pub(crate)` —
/// migrations (`features::data_migration`) read files as `serde_json::Value` to detect
/// the version, distinguishing "no file" (`Ok(None)`) from "corrupt" (`Err`).
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => {
            let value = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Atomically writes a value to JSON: backing up the existing file → temp → rename.
/// `pub(crate)` — migrations write the migrated `serde_json::Value` through the same
/// atomic path (with a `.bak` of the previous version).
pub(crate) fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating dir {}", parent.display()))?;
    }
    if path.exists() {
        let backup = path.with_extension("bak");
        let _ = fs::copy(path, &backup);
    }
    let data = serde_json::to_vec_pretty(value).context("serializing to JSON")?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &data).with_context(|| format!("writing {}", tmp.display()))?;
    // std::fs::rename replaces an existing file on both Windows and Unix.
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::Message;

    fn store() -> (tempfile::TempDir, JsonStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(Paths::with_root(dir.path()));
        (dir, store)
    }

    #[test]
    fn config_defaults_when_missing_then_roundtrips() {
        let (_d, s) = store();
        let cfg = s.load_config().unwrap();
        assert_eq!(cfg, AppConfig::default());

        let mut changed = cfg;
        changed.max_tool_rounds = 3;
        s.save_config(&changed).unwrap();
        assert_eq!(s.load_config().unwrap().max_tool_rounds, 3);
    }

    #[test]
    fn profile_upsert_and_hide() {
        let (_d, s) = store();
        let mut p = Profile::new("Joyce", "sys");
        s.upsert_profile(&p).unwrap();
        assert_eq!(s.load_profiles().unwrap().len(), 1);

        p.name = "Joyce 2".into();
        s.upsert_profile(&p).unwrap();
        let all = s.load_profiles().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Joyce 2");

        assert!(s.hide_profile(p.id).unwrap());
        assert!(s.load_profiles().unwrap()[0].is_hidden);
        assert!(!s.hide_profile(Uuid::new_v4()).unwrap());
    }

    #[test]
    fn chat_save_load_list_hide() {
        let (_d, s) = store();
        let p = Profile::new("X", "s");
        let chat = Chat::from_profile(&p, "Чат 1");
        s.save_chat(&chat).unwrap();

        assert_eq!(s.load_chat(chat.id).unwrap().unwrap().title, "Чат 1");
        assert_eq!(s.load_chats().unwrap().len(), 1);

        assert!(s.hide_chat(chat.id).unwrap());
        assert!(s.load_chat(chat.id).unwrap().unwrap().is_hidden);
    }

    #[test]
    fn chat_files_lists_ids_with_a_change_signal() {
        let (d, s) = store();
        let p = Profile::new("X", "s");
        let a = Chat::from_profile(&p, "a");
        let b = Chat::from_profile(&p, "b");
        s.save_chat(&a).unwrap();
        s.save_chat(&b).unwrap();
        // Not a chat: a different extension, and a `.json` whose name is not a
        // UUID (the chat id *is* the file name). The `.bak` written by
        // `save_chat` is covered by the first of those.
        // (Names chosen to avoid an i18n-bundle prefix — a literal like
        // `notes.txt` reads as a bundle key to `all_bundle_key_references_in_code_exist`.)
        std::fs::write(d.path().join("chats").join("scratch.txt"), "x").unwrap();
        std::fs::write(d.path().join("chats").join("readme.json"), "{}").unwrap();

        let mut files = s.chat_files().unwrap();
        files.sort_by_key(|f| f.id);
        let mut expected = vec![a.id, b.id];
        expected.sort();
        assert_eq!(files.iter().map(|f| f.id).collect::<Vec<_>>(), expected);
        assert!(files.iter().all(|f| f.size > 0), "{files:?}");

        // The signal actually changes when the file does — that is the whole
        // point of it (a re-index is decided on this comparison alone).
        let before = s.chat_file_info(a.id).unwrap();
        let mut grown = a.clone();
        grown.push_message(Message::user("новое сообщение делает файл длиннее"));
        s.save_chat(&grown).unwrap();
        let after = s.chat_file_info(a.id).unwrap();
        assert_ne!(before, after);
        assert!(after.size > before.size);
    }

    #[test]
    fn chat_files_is_empty_without_a_chats_dir() {
        let dir = tempfile::tempdir().unwrap();
        let s = JsonStore::new(Paths::with_root(dir.path().join("nothing-here")));
        assert!(s.chat_files().unwrap().is_empty());
        assert_eq!(s.chat_file_info(Uuid::new_v4()), None);
    }

    #[test]
    fn hide_chats_of_profile_cascades() {
        let (_d, s) = store();
        let p1 = Profile::new("A", "s");
        let p2 = Profile::new("B", "s");
        s.save_chat(&Chat::from_profile(&p1, "c1")).unwrap();
        s.save_chat(&Chat::from_profile(&p1, "c2")).unwrap();
        s.save_chat(&Chat::from_profile(&p2, "c3")).unwrap();

        assert_eq!(s.hide_chats_of_profile(p1.id).unwrap(), 2);
        let visible = s
            .load_chats()
            .unwrap()
            .into_iter()
            .filter(|c| !c.is_hidden)
            .count();
        assert_eq!(visible, 1);
    }

    #[test]
    fn write_creates_backup_of_previous() {
        let (d, s) = store();
        s.save_config(&AppConfig::default()).unwrap();
        let c = AppConfig {
            max_tool_rounds: 99,
            ..Default::default()
        };
        s.save_config(&c).unwrap();
        assert!(d.path().join("settings.bak").exists());
    }

    #[test]
    fn chat_save_backs_up_previous_version() {
        let (_d, s) = store();
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "Чат");
        s.save_chat(&chat).unwrap();
        // Saving again creates a .bak with the previous version (spec §12.3).
        chat.push_message(Message::user("привет"));
        s.save_chat(&chat).unwrap();
        let bak = s
            .paths
            .chat_file(&chat.id.to_string())
            .with_extension("bak");
        assert!(bak.exists(), "expected a chat file backup");
        // The backup holds the previous (empty) version, not the current one.
        let backed: Chat = serde_json::from_slice(&fs::read(&bak).unwrap()).unwrap();
        assert!(backed.messages.is_empty());
    }
}
