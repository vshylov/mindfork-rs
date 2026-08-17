//! Machine-bound storage for secrets (cloud-provider API keys, the backup
//! password).
//!
//! A secret entered in settings is encrypted with **this machine's key** and put
//! into `settings.json` (see [`crate::shared::config::AppConfig::api_keys`]). The
//! config stays portable: on another machine the entry does not decrypt — the key
//! is entered again and added as **its own** entry; going back to the first
//! machine, its entry is still readable. See docs/research/api-key-storage.md.
//!
//! Encryption schemes (the entry's `scheme` field, a free-form string — an
//! unfamiliar scheme does not break config reading, the entry is simply "not
//! ours"):
//!
//! * **`dpapi`** (Windows) — the system `CryptProtectData`/`CryptUnprotectData`:
//!   a *user* master key managed by the OS. Decryption on another machine or by
//!   another user is impossible. `pOptionalEntropy` is an app constant
//!   ([`ENTROPY`]): not a secret, but it filters out generic "DPAPI dumper" tools.
//! * **`machine-key-v1`** (Linux) — the key is derived from `/etc/machine-id` via
//!   HKDF-SHA256 (the `sd_id128_get_machine_app_specific` pattern — systemd
//!   explicitly instructs against using the raw machine-id), encryption is
//!   ChaCha20-Poly1305 (AEAD, a random nonce prefixed to the ciphertext). The
//!   username goes into `info` → per-user binding, like DPAPI.
//!
//! **Threat model** (docs/research/api-key-storage.md §3): we protect the
//! **file** — a copy/move/backup of the config (outside its "own" machine it is a
//! useless ciphertext). It does not protect against malicious code running under
//! the same user on the same machine — it would call the same DPAPI / derive the
//! same key. This is fundamental for any scheme where "the app decrypts on its
//! own, with no user input" (Chrome and Git Credential Manager work the same
//! way). The previous path (an env variable) was no safer: any process of the
//! user can read it.

use serde::{Deserialize, Serialize};

/// The Windows DPAPI scheme (the value of the `scheme` field).
// Off Windows the scheme is unavailable (an entry with it is "not ours"), so the
// constant does not show up in code there: silence dead_code so the `-D warnings`
// gate stays green on both OSes.
#[cfg_attr(not(windows), allow(dead_code))]
pub const SCHEME_DPAPI: &str = "dpapi";
/// The Linux scheme: HKDF(machine-id) + ChaCha20-Poly1305.
pub const SCHEME_MACHINE_KEY_V1: &str = "machine-key-v1";

/// Reserved entry key for the **backup password** (spec §12.3), stored beside
/// the API keys in the same per-machine entry.
///
/// The hyphen makes a collision with a [`crate::shared::config::CloudProvider`]
/// key (`openai`/`gemini`/`claude`) impossible, so one map can hold both kinds of
/// secret. Reusing the entry rather than adding a second list keeps `put_key`/
/// [`stored_key`]/[`is_ours`] working unchanged — and renaming the `api_keys`
/// field on disk is exactly what the additive-only rule forbids (ADR 0006 F12),
/// so the field's *name* stays while its meaning is "this machine's secrets".
///
/// A dot would read as an i18n bundle key to `tools/cyrillic_scan.py`'s sibling
/// gate over `*.*` literals — a hyphen is just as unambiguous here.
pub const BACKUP_PASSWORD_KEY: &str = "backup-password";

/// One of the settings slots that can point at an **external**
/// OpenAI-compatible server. Each has a URL of its own, so each has a key of its
/// own: the common configuration is a cloud gateway for chat beside a local
/// `llama-server` for embeddings, and one shared key would send the gateway's
/// Bearer token to localhost. See docs/history/external-api-key.md §3, F1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalSlot {
    /// `engine.external` — the assistant's chat server.
    Chat,
    /// `impersonation_engine.external` — the impersonation server (spec §11.8).
    Impersonation,
    /// `embed.external` — the embedding server (ADR 0002).
    Embed,
    /// `tts.external` — the speech server (ADR 0009).
    Tts,
}

impl ExternalSlot {
    /// Every slot, for enumerating the presence list.
    pub const ALL: [ExternalSlot; 4] = [Self::Chat, Self::Impersonation, Self::Embed, Self::Tts];

    /// The slot's part of the storage name (see [`SecretKey::storage_name`]).
    fn key(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Impersonation => "impersonation",
            Self::Embed => "embed",
            Self::Tts => "tts",
        }
    }
}

/// Which secret a storage slot holds. One typed key instead of raw strings: the
/// side effects of storing differ per kind (a provider key re-raises the servers
/// that use it, an MCP one re-spawns that server, a backup password needs
/// nothing), and dispatching those by parsing a name is how they drift. Carried
/// by `AppCommand::SetSecret` and by the settings snapshot's presence list; the
/// storage itself only ever sees [`Self::storage_name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKey {
    /// A cloud provider's API key — shared by chat/impersonation/embeddings of
    /// that provider (ADR 0008 §3).
    Provider(crate::shared::config::CloudProvider),
    /// The Bearer key of one external OpenAI-compatible server (a proxy or a
    /// gateway — LiteLLM, OpenRouter, vLLM…). Addressed by **slot** rather than
    /// by provider, which is what ADR 0008 could not do and is why the external
    /// mode stayed env-only until now: an arbitrary URL cannot be pinned to a
    /// provider, but the sub-section the user is typing the URL into is a
    /// perfectly good address. See docs/history/external-api-key.md.
    External(ExternalSlot),
    /// The backup password (spec §12.3).
    BackupPassword,
    /// The value of one environment variable handed to an MCP server
    /// (docs/history/mcp-server-editor.md §9, S1): the alternative to naming an
    /// OS variable in `env`, and the only one that does not require setting a
    /// variable outside the app.
    McpEnv { server: String, var: String },
}

impl SecretKey {
    /// The key this secret is stored under in the machine entry. Neither `mcp-`
    /// nor `external-` can collide with a provider key
    /// (`openai`/`gemini`/`claude`/`grok`) or with [`BACKUP_PASSWORD_KEY`], and
    /// since a variable name is restricted to `[A-Za-z0-9_]` (only the server id
    /// may contain `-`) the composed MCP name is unambiguous from the right. The
    /// external slots are a closed set, so theirs cannot be ambiguous at all.
    pub fn storage_name(&self) -> String {
        match self {
            Self::Provider(p) => p.key().to_string(),
            Self::External(slot) => format!("external-{}", slot.key()),
            Self::BackupPassword => BACKUP_PASSWORD_KEY.to_string(),
            Self::McpEnv { server, var } => format!("mcp-{server}-{var}"),
        }
    }
}

/// Plaintext of the `check` probe: encrypted alongside the keys; decrypting it
/// successfully identifies an entry as "ours" (we do not store an explicit
/// machine-id in the portable config).
const CHECK_PLAINTEXT: &str = "mindfork-rs api-key check v1";

/// Additional DPAPI entropy / HKDF salt — an app constant (not a secret).
const ENTROPY: &[u8] = b"mindfork-rs/api-keys/v1";

/// The HKDF `info` string (the key domain; the username is appended to it).
const HKDF_INFO: &[u8] = b"mindfork-rs api-key v1 user=";

/// One entry of stored API keys — **for one machine**. Entries of other machines
/// sit alongside and are left untouched (they will "come alive" on their own
/// machines): the config is portable, the keys are per-machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiKeyEntry {
    /// Human-readable label (computer name + date) — for display/diagnostics
    /// only, takes no part in the logic.
    pub label: String,
    /// Encryption scheme: [`SCHEME_DPAPI`] | [`SCHEME_MACHINE_KEY_V1`]. A
    /// free-form string — an entry with an unfamiliar (future) scheme is read
    /// and saved as is.
    pub scheme: String,
    /// Ciphertext of [`CHECK_PLAINTEXT`] — the "is this entry ours" probe
    /// ([`is_ours`]).
    pub check: String,
    /// Ciphertexts of the keys by provider (`openai`/`gemini`/`claude`).
    pub keys: std::collections::BTreeMap<String, String>,
}

/// Whether a secret-encryption scheme is available on this machine (otherwise
/// stored keys are not supported — the env path remains). On Windows — always;
/// on Linux depends on machine-id being present.
// Consumers: tests (skip on systems without machine-id) and the stage-2 settings
// screen (the "API key" field is hidden/explained when storage is unavailable).
#[allow(dead_code)]
pub fn scheme_available() -> bool {
    local_scheme().is_some()
}

/// Identifies "our" entry — by decrypting the `check` probe (DPAPI returned
/// success / AEAD authentication matched). Entries of other machines and other
/// schemes — `false`.
pub fn is_ours(entry: &ApiKeyEntry) -> bool {
    decrypt(&entry.scheme, &entry.check).as_deref() == Some(CHECK_PLAINTEXT)
}

/// The decrypted provider key from "our" entry; `None` — there is no entry, it
/// belongs to another machine, or this provider's key is not set in it.
pub fn stored_key(entries: &[ApiKeyEntry], provider_key: &str) -> Option<String> {
    let entry = entries.iter().find(|e| is_ours(e))?;
    decrypt(&entry.scheme, entry.keys.get(provider_key)?)
}

/// Puts the provider's key into **this** machine's entry (creates it if absent).
/// An empty `key` removes the key; an emptied entry is removed entirely. Other
/// machines' entries are left untouched. `Err` — the scheme is unavailable on
/// this machine, or encryption failed.
pub fn put_key(
    entries: &mut Vec<ApiKeyEntry>,
    provider_key: &str,
    key: &str,
    label: impl FnOnce() -> String,
) -> Result<(), SecretError> {
    let idx = entries.iter().position(is_ours);
    if key.is_empty() {
        if let Some(i) = idx {
            entries[i].keys.remove(provider_key);
            if entries[i].keys.is_empty() {
                entries.remove(i);
            }
        }
        return Ok(());
    }
    let scheme = local_scheme().ok_or(SecretError::Unavailable)?;
    let cipher = encrypt(scheme, key)?;
    match idx {
        Some(i) => {
            entries[i].keys.insert(provider_key.into(), cipher);
        }
        None => entries.push(ApiKeyEntry {
            label: label(),
            check: encrypt(scheme, CHECK_PLAINTEXT)?,
            scheme: scheme.into(),
            keys: std::collections::BTreeMap::from([(provider_key.into(), cipher)]),
        }),
    }
    Ok(())
}

/// Computer name for the entry's label (diagnostics: whose entry this is). For
/// display only — takes no part in identifying the entry (see [`is_ours`]).
/// Fallback — `?`.
pub fn machine_label() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

/// Error working with stored secrets.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No scheme is available on this machine (Linux without machine-id) —
    /// storing keys is not supported, the env path remains.
    #[error("storing keys is not supported on this machine (no machine-id)")]
    Unavailable,
    /// Platform encryption failure (DPAPI/AEAD).
    #[error("failed to encrypt the secret")]
    Encrypt,
}

/// This machine's encryption scheme (`None` — unavailable).
fn local_scheme() -> Option<&'static str> {
    #[cfg(windows)]
    {
        Some(SCHEME_DPAPI)
    }
    #[cfg(not(windows))]
    {
        machine_ikm().map(|_| SCHEME_MACHINE_KEY_V1)
    }
}

/// Encrypts a secret with the given scheme → a hex string for JSON.
fn encrypt(scheme: &str, plaintext: &str) -> Result<String, SecretError> {
    let bytes = match scheme {
        #[cfg(windows)]
        SCHEME_DPAPI => dpapi::protect(plaintext.as_bytes()).ok_or(SecretError::Encrypt)?,
        SCHEME_MACHINE_KEY_V1 => {
            let key = machine_key().ok_or(SecretError::Unavailable)?;
            encrypt_with_key(&key, plaintext.as_bytes()).ok_or(SecretError::Encrypt)?
        }
        _ => return Err(SecretError::Unavailable),
    };
    Ok(hex_encode(&bytes))
}

/// Decrypts a hex string with the given scheme. `None` — a foreign machine, an
/// unfamiliar scheme, or data corruption (all cases equivalent: "cannot be
/// read").
fn decrypt(scheme: &str, hex: &str) -> Option<String> {
    let bytes = hex_decode(hex)?;
    let plain = match scheme {
        #[cfg(windows)]
        SCHEME_DPAPI => dpapi::unprotect(&bytes)?,
        SCHEME_MACHINE_KEY_V1 => decrypt_with_key(&machine_key()?, &bytes)?,
        _ => return None,
    };
    String::from_utf8(plain).ok()
}

// ── The `machine-key-v1` scheme: HKDF(machine-id) + ChaCha20-Poly1305 ──────────

/// ChaCha20-Poly1305 nonce length (ciphertext prefix).
const NONCE_LEN: usize = 12;

/// Derives a 32-byte key from the input keying material (machine-id) and the
/// username. A pure function — testable on any OS by injecting `ikm`.
fn derive_key(ikm: &[u8], user: &str) -> [u8; 32] {
    let mut info = HKDF_INFO.to_vec();
    info.extend_from_slice(user.as_bytes());
    let mut okm = [0u8; 32];
    // `expand` into 32 bytes (= the SHA-256 output size) cannot exceed the HKDF limit.
    hkdf::Hkdf::<sha2::Sha256>::new(Some(ENTROPY), ikm)
        .expand(&info, &mut okm)
        .expect("HKDF: 32 bytes is always a valid length");
    okm
}

/// Encrypts `plaintext`: the result = `nonce || ciphertext+tag`. A pure function.
fn encrypt_with_key(key: &[u8; 32], plaintext: &[u8]) -> Option<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, OsRng};
    use chacha20poly1305::{AeadCore, ChaCha20Poly1305, KeyInit};

    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut out = nonce.to_vec();
    out.extend_from_slice(&cipher.encrypt(&nonce, plaintext).ok()?);
    Some(out)
}

/// Decrypts `nonce || ciphertext+tag`. `None` — a foreign key (AEAD did not
/// authenticate), corruption, or too-short input. A pure function.
fn decrypt_with_key(key: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    use chacha20poly1305::aead::Aead;
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit};

    if data.len() <= NONCE_LEN {
        return None;
    }
    let (nonce, ct) = data.split_at(NONCE_LEN);
    ChaCha20Poly1305::new(key.into())
        .decrypt(nonce.into(), ct)
        .ok()
}

/// This machine's key for the `machine-key-v1` scheme (`None` — no machine-id).
fn machine_key() -> Option<[u8; 32]> {
    Some(derive_key(&machine_ikm()?, &current_user()))
}

/// Key input material: the OS instance identifier. Primary source —
/// `/etc/machine-id` (systemd), fallback — `/var/lib/dbus/machine-id`. `None` —
/// a non-systemd system without either (the scheme is unavailable, the env path
/// remains). Not used on Windows (DPAPI is used there).
fn machine_ikm() -> Option<Vec<u8>> {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = std::fs::read_to_string(path) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.as_bytes().to_vec());
            }
        }
    }
    None
}

/// The current username (the HKDF `info` component → per-user binding). An
/// empty name is acceptable — the binding is then to the machine only.
fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

// ── The `dpapi` scheme (Windows) ────────────────────────────────────────────────

#[cfg(windows)]
mod dpapi {
    use super::ENTROPY;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
    };

    /// Encrypts data with DPAPI (a user key managed by the OS). `None` — a
    /// winapi failure.
    pub(super) fn protect(data: &[u8]) -> Option<Vec<u8>> {
        crypt(data, true)
    }

    /// Decrypts DPAPI data. `None` — a different machine/user, corruption, or
    /// different entropy (all cases equivalent: "cannot be read").
    pub(super) fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        crypt(data, false)
    }

    /// Shared wrapper: both DPAPI calls have the same shape (blob in → blob
    /// out, the OS allocates the output buffer and it must be freed with
    /// `LocalFree`).
    fn crypt(data: &[u8], protect: bool) -> Option<Vec<u8>> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: `input`/`entropy` point to live slices for the whole call;
        // the other pointers are null (we do not use description/reserved/prompt);
        // `out` is filled by the OS, we free it with `LocalFree` exactly once below.
        let ok = unsafe {
            if protect {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut out,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut out,
                )
            }
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        // SAFETY: on success the OS guarantees a valid buffer of length `cbData`.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        // SAFETY: `pbData` was allocated by the OS specifically for `LocalFree`; not used afterward.
        unsafe { LocalFree(out.pbData as *mut core::ffi::c_void) };
        Some(bytes)
    }

    /// Wraps a slice into a DPAPI blob (a "length + pointer" structure).
    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }
}

// ── hex codec (ciphertext in JSON) ───────────────────────────────────────────────
// A hand-rolled implementation instead of the base64 crate — a precedent is
// `features::sandbox_setup::hex_lower`; the string-size difference is negligible
// for a config, no dependency needed.

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b = s.as_bytes();
    (0..b.len() / 2)
        .map(|i| {
            let hi = (b[i * 2] as char).to_digit(16)?;
            let lo = (b[i * 2 + 1] as char).to_digit(16)?;
            Some(((hi << 4) | lo) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind of secret has to occupy its own slot in the one per-machine
    /// `keys` map: a collision would make two unrelated fields overwrite each
    /// other's value. The external names are also pinned literally — they are on
    /// disk now, so renaming one silently orphans a stored key.
    #[test]
    fn storage_names_are_distinct_across_kinds() {
        use crate::shared::config::CloudProvider;
        let mut names: Vec<String> = CloudProvider::ALL
            .into_iter()
            .map(SecretKey::Provider)
            .chain(ExternalSlot::ALL.into_iter().map(SecretKey::External))
            .chain([
                SecretKey::BackupPassword,
                SecretKey::McpEnv {
                    server: "chat".into(), // a server named after an external slot
                    var: "TOKEN".into(),
                },
            ])
            .map(|k| k.storage_name())
            .collect();
        let total = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), total, "storage names collide: {names:?}");
        assert_eq!(
            SecretKey::External(ExternalSlot::Chat).storage_name(),
            "external-chat"
        );
        assert_eq!(
            SecretKey::External(ExternalSlot::Impersonation).storage_name(),
            "external-impersonation"
        );
        assert_eq!(
            SecretKey::External(ExternalSlot::Embed).storage_name(),
            "external-embed"
        );
        assert_eq!(
            SecretKey::External(ExternalSlot::Tts).storage_name(),
            "external-tts"
        );
    }

    #[test]
    fn hex_round_trip_and_rejects_malformed() {
        let data = vec![0u8, 1, 15, 16, 200, 255];
        assert_eq!(hex_decode(&hex_encode(&data)).unwrap(), data);
        assert_eq!(hex_encode(&[0xab, 0x0f]), "ab0f");
        assert!(hex_decode("abc").is_none()); // odd length
        assert!(hex_decode("zz").is_none()); // not hex
    }

    #[test]
    fn aead_round_trip_with_derived_key() {
        let key = derive_key(b"machine-id-abc", "user1");
        let enc = encrypt_with_key(&key, b"sk-secret-value").unwrap();
        assert_ne!(&enc[NONCE_LEN..], b"sk-secret-value"); // not plaintext
        assert_eq!(decrypt_with_key(&key, &enc).unwrap(), b"sk-secret-value");
    }

    #[test]
    fn aead_rejects_foreign_key_and_tampering() {
        let mine = derive_key(b"machine-A", "user1");
        let theirs = derive_key(b"machine-B", "user1");
        let other_user = derive_key(b"machine-A", "user2");
        let enc = encrypt_with_key(&mine, b"secret").unwrap();
        // A different machine and a different user on the same machine cannot read it.
        assert!(decrypt_with_key(&theirs, &enc).is_none());
        assert!(decrypt_with_key(&other_user, &enc).is_none());
        // Ciphertext corruption is caught by AEAD authentication.
        let mut bad = enc.clone();
        *bad.last_mut().unwrap() ^= 0xff;
        assert!(decrypt_with_key(&mine, &bad).is_none());
        // Too-short input (not even a nonce) — not a panic, but `None`.
        assert!(decrypt_with_key(&mine, &[0u8; NONCE_LEN]).is_none());
    }

    #[test]
    fn nonce_is_random_so_ciphertexts_differ() {
        let key = derive_key(b"machine-id", "u");
        let a = encrypt_with_key(&key, b"same").unwrap();
        let b = encrypt_with_key(&key, b"same").unwrap();
        assert_ne!(
            a, b,
            "identical plaintext must not produce identical ciphertext"
        );
    }

    #[test]
    fn derive_key_is_deterministic_and_domain_separated() {
        assert_eq!(derive_key(b"m", "u"), derive_key(b"m", "u"));
        assert_ne!(derive_key(b"m", "u"), derive_key(b"m", "v"));
        assert_ne!(derive_key(b"m", "u"), derive_key(b"n", "u"));
    }

    /// Full round trip over a config entry — on this machine's platform scheme
    /// (Windows: DPAPI; Linux: machine-id if present — otherwise the test is
    /// skipped).
    #[test]
    fn entry_round_trip_on_local_scheme() {
        if !scheme_available() {
            return; // non-systemd Linux without machine-id: key storage is not supported
        }
        let mut entries: Vec<ApiKeyEntry> = vec![];
        put_key(&mut entries, "openai", "sk-test-123", || "test".into()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(is_ours(&entries[0]));
        // The secret is not stored in plaintext.
        let json = serde_json::to_string(&entries).unwrap();
        assert!(
            !json.contains("sk-test-123"),
            "plaintext leaked into serialization: {json}"
        );
        assert_eq!(
            stored_key(&entries, "openai").as_deref(),
            Some("sk-test-123")
        );
        assert_eq!(stored_key(&entries, "claude"), None);
        // A second provider goes into the same entry.
        put_key(&mut entries, "claude", "sk-ant-9", || "test".into()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(stored_key(&entries, "claude").as_deref(), Some("sk-ant-9"));
        // An empty key removes the provider; an emptied entry disappears.
        put_key(&mut entries, "openai", "", || "test".into()).unwrap();
        assert_eq!(stored_key(&entries, "openai"), None);
        assert_eq!(entries.len(), 1);
        put_key(&mut entries, "claude", "", || "test".into()).unwrap();
        assert!(entries.is_empty());
    }

    /// An entry from a foreign machine (undecryptable) and an entry with an
    /// unfamiliar scheme are not recognized as ours, do not yield keys, and are
    /// **left untouched** when editing.
    #[test]
    fn foreign_entries_are_ignored_and_preserved() {
        if !scheme_available() {
            return;
        }
        let foreign = ApiKeyEntry {
            label: "other-pc".into(),
            scheme: SCHEME_MACHINE_KEY_V1.into(),
            check: hex_encode(&[7u8; 40]), // a foreign ciphertext
            keys: std::collections::BTreeMap::from([("openai".into(), hex_encode(&[9u8; 40]))]),
        };
        let future = ApiKeyEntry {
            label: "future-pc".into(),
            scheme: "keychain-v9".into(), // a scheme we do not know
            check: "00".into(),
            keys: std::collections::BTreeMap::from([("openai".into(), "00".into())]),
        };
        let mut entries = vec![foreign.clone(), future.clone()];
        assert!(!is_ours(&entries[0]) && !is_ours(&entries[1]));
        assert_eq!(stored_key(&entries, "openai"), None);

        put_key(&mut entries, "openai", "sk-mine", || "mine".into()).unwrap();
        assert_eq!(
            entries.len(),
            3,
            "our entry is added, foreign ones are not replaced"
        );
        assert_eq!(entries[0], foreign, "the foreign entry is untouched");
        assert_eq!(
            entries[1], future,
            "the entry with an unfamiliar scheme is untouched"
        );
        assert_eq!(stored_key(&entries, "openai").as_deref(), Some("sk-mine"));
    }
}
