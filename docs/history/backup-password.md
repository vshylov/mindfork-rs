# Password-protected backups — design plan

**Status:** forks decided, implementation in `feat/backup-password`.
**User's decision, 2026-08-01:** F1 (a) WinZip AES-256 inside the zip; F3 (a) one
effective password per invocation; F4 yes, the pre-migration backup is encrypted
too; F6 the hidden prompt is added for `restore` only. F2 (a), F5 yes, F7 no and
the F8 confirmations were taken as recommended.
**Request:** a backup password can be given as a command-line argument to
`backup`/`restore`, or set in the settings. When set in the settings, backups are
created encrypted with it. Restore must accept **both** an archive encrypted with
that password and an unencrypted one. The stored password is machine-bound and
encrypted the same way cloud API keys are (ADR 0008).

Related: [ADR 0008](../decisions/0008-api-key-storage.md) (machine-bound secrets),
[ADR 0006](../decisions/0006-data-schema-versioning.md) (the pre-migration backup),
spec §12.3 (backup/restore), `src/features/backup.rs`, `src/shared/secrets.rs`.

## 1. What was measured (not assumed)

A throwaway probe crate (`zip` 2.4.2 + `deflate` + `aes-crypto`) answered the
load-bearing questions before any design was committed to. All results below are
**measured output**, not documentation claims.

| # | Question | Result |
|---|---|---|
| 1 | Can the `zip` crate **write** encrypted entries? | **Yes** — `FileOptions::with_aes_encryption(AesMode::Aes256, pw)`. The plaintext marker is absent from the archive bytes. |
| 2 | Are entry **names** encrypted? | **No** — `chats/a.json` is plainly visible in the file. Content only. |
| 3 | Read matrix — plain archive **with** a password? | **OK.** The crate discards a password an entry doesn't need (`(Some(_), false) => password = None`). |
| 4 | Encrypted archive **without** a password? | `UnsupportedArchive("Password required to decrypt file")` — a distinct, detectable error. |
| 5 | Encrypted archive with the **wrong** password? | `InvalidPassword` — a second distinct error. |
| 6 | When is the password checked — at open, or after reading? | **At open** (`by_index_decrypt` errors immediately; AES stores a 2-byte verifier). |
| 7 | Is the ciphertext **authenticated**? | **Yes.** 66 of 66 single-byte corruptions inside an entry's payload were detected, **0** returned silently wrong data (HMAC-SHA1 auth code, verified at EOF). |
| 8 | Can the manifest stay **unencrypted** inside an encrypted archive? | **Yes** — readable with no password while the data entries stay protected. |
| 9 | Interoperable with 7-Zip/WinZip? | The `0x9901` WinZip-AES extra field is present — standard AE-2, openable by any mainstream zip tool. |

Two of these shape the design directly:

* **№3 makes the "restore either kind" requirement free.** No detection branch,
  no special case — handing the password to a plain archive is already a no-op.
* **№6 keeps the transactional restore intact.** `restore_backup` validates the
  archive *before* any destructive action; because the password is verified at
  entry-open, validation can reject a wrong password **before** the data is
  cleared. Had verification only happened at EOF, we would have had to read every
  entry up front.

## 2. Limits that must be stated honestly

The project has a standing habit of writing down what a security mechanism does
**not** do (see the threat model in `shared/secrets.rs`). Four items:

1. **Entry names, sizes and structure are not protected.** Anyone holding the
   archive sees that it is a mindfork backup, how many chats it has and roughly
   how big each is. Chat titles and message text live *inside* the encrypted
   payloads; chat file names are UUIDs, so no title leaks. Hiding even that would
   require an outer container (fork F1) and would cost zip-tool interoperability.
2. **The key derivation is PBKDF2-HMAC-SHA1, 1000 iterations** — fixed by the
   WinZip AES spec, not a parameter we can raise. By 2026 standards this is weak
   against an *offline* attack: a GPU manages on the order of 10⁶–10⁷ guesses per
   second, so a short or dictionary password is brute-forceable by anyone who
   obtains the file. The mitigation available to the user is a long passphrase;
   the mitigation available to us is fork F1.
3. **A machine-bound password plus a dead machine = unreadable archives.** The
   stored password is encrypted with DPAPI / the machine key, exactly like an API
   key. If the user only ever typed it into the settings and never recorded it,
   and the machine is lost, every backup made with it is **unrecoverable** — the
   archives are the one artifact designed to outlive the machine, so this
   inverts their purpose. This has to be said in the field's hint, not buried in
   docs.
4. **`--password` on the command line is visible** in the process list and lands
   in shell history. Standard for CLI tools, and the reason fork F6 exists.

What it *does* protect, which is the point of the request: a backup that leaves
the machine — a copy on a NAS, a cloud sync folder, a USB stick, an email
attachment — is useless without the password.

## 3. Sketch of the change

Small and mostly mechanical once the forks are settled.

* **`Cargo.toml`** — `zip` features `+ "aes-crypto"`. New transitive crates:
  `pbkdf2`, `sha1`, `constant_time_eq` (`aes`, `hmac`, `zeroize`, `getrandom` are
  already in `Cargo.lock`). All RustCrypto, pure Rust, no C dependency —
  consistent with why `default-features = false` was chosen for `zip` in the
  first place. `deny.toml` licence check needed.
* **`features/backup.rs`** — `create_backup(…, password: Option<&str>)` and
  `restore_backup(…, password: Option<&str>)`; `write_zip` picks the file options;
  `validate_archive` additionally *opens* each encrypted entry with the password,
  turning both failure modes into localized errors **before** the destructive
  phase; `extract_archive` uses `by_index_decrypt`.
* **`shared/secrets.rs`** — no new mechanism. `put_key`/`stored_key` already do
  exactly what is needed; only the *key name* under which the password is filed
  is new (fork F2).
* **CLI (`features/cli.rs`, `main.rs`)** — `-p/--password <PASSWORD>` on `backup`
  and `restore`; `config_fs_root` widens to return the stored password alongside
  `fs_root` (the CLI already loads the full `AppConfig`, so this is a one-line
  change, not new plumbing).
* **Settings UI** — one field, a mirror of the existing "API key" row: masked
  editor, shows a *status* ("set (this computer)" / "not set" / "not supported on
  this system"), never the secret; `Del` clears it. `emit_settings` already
  strips secrets from the snapshot and sends presence flags — the same treatment,
  one more flag.
* **i18n** — CLI help/errors and the settings field/hint, both bundles.
* **Docs** — spec §12.3, README, `docs/install.md`, CHANGELOG (Added +
  **Security**), CLAUDE.md journal; an ADR if F1 goes the non-standard way.

**A live run is not required** (AGENTS.md §3): no engine, memory, tool or
provider path is touched. What replaces it, as in the database-compaction work,
is exercising the real CLI against a copy of the real dev data root.

## 4. Forks — the user's decision needed before implementation

### F1. Encryption format

| Option | Protects | Costs |
|---|---|---|
| **(a) WinZip AES-256 inside the zip** *(recommended)* | Content of every entry, authenticated (measurement №7) | Entry names/sizes visible; PBKDF2-SHA1/1000 KDF is not raisable |
| (b) Our own outer container: encrypt the finished zip with ChaCha20-Poly1305 + Argon2id, `.mfbak` | Everything, including names; a modern KDF we control | No zip tool can open it; a new dependency (`argon2`); we own the format forever; the manifest can no longer be read without the password |
| (c) Both — (a) by default, (b) opt-in | — | Two code paths, two formats to support |

**Recommendation: (a).** The threat the request describes is a backup file
leaving the machine, and (a) closes it with a format the user can also open by
hand in 7-Zip — which matters for an artifact whose whole job is to be
recoverable when the app is not available. (b) is strictly stronger
cryptographically but makes the archive dependent on this program continuing to
exist and to run, which is a bad trade for a backup. The KDF weakness is real and
is answered by the hint recommending a passphrase.

### F2. Where the stored password lives in `settings.json`

| Option | Notes |
|---|---|
| **(a) Reuse `AppConfig.api_keys`** under a reserved name `"backup.password"` *(recommended)* | Zero new machinery — `put_key`/`stored_key`/`is_ours` work unchanged; one entry per machine keeps holding everything that machine knows. The dot makes collision with a `CloudProvider` key (`openai`/`gemini`/`claude`) impossible. Cost: the field's *name* on disk stays `api_keys` while its meaning widens to "machine-bound secrets" — a doc-comment change, no migration. |
| (b) A new `AppConfig.backup_password: Vec<ApiKeyEntry>` | Cleaner naming; cost: a second per-machine entry list, duplicated `is_ours`/label logic, more UI plumbing. |
| (c) Rename `api_keys` → `secrets` | Cleanest name; **breaks existing configs** — renaming a serialized field is exactly what the additive-only rule (ADR 0006 F12) forbids. |

**Recommendation: (a).** (c) is disqualified outright.

### F3. Which password the **pre-restore copy** is encrypted with

`restore_backup` auto-saves the existing data before replacing it. Which password
does *that* archive get?

| Option | Behaviour |
|---|---|
| **(a) One effective password per invocation** *(recommended)*: `--password` if given, else the stored one, else none — used both to read the input archive and to write the pre-restore copy | One concept, easy to state. Oddity: restoring a foreign archive with `--password foreign` encrypts your pre-restore copy with `foreign` — recoverable, since the user just typed it. |
| (b) `--password` reads the input; the **stored** password always governs writing | Arguably more correct; two concepts to explain, and on a machine with no stored password the pre-restore copy silently comes out unencrypted. |

**Recommendation: (a)** — and the copy's path is printed, so nothing is lost
either way.

### F4. Is the **pre-migration** backup encrypted?

ADR 0006 makes the app create `backups/pre-migrate-*.zip` at startup when a schema
migration is pending. It contains the same data as any other backup.

**Recommendation: yes** — encrypt it with the stored password when one is set.
A setting that says "my backups are encrypted" should not have an exception that
quietly writes a plaintext copy of everything. Cost: `data_migration::run` reads
`settings.json` as a `serde_json::Value` before storage opens, so it needs to pull
`api_keys` out of that value (a few lines) rather than from a typed `AppConfig`.

### F5. Is `manifest.json` left unencrypted inside an encrypted archive?

**Recommendation: yes.** It carries app version, schema versions and a timestamp —
no user data. Leaving it readable keeps `read_manifest` (and the "this backup is
from a newer version" warning) working without a password, and makes the archive
self-describing. Measurement №8 confirms the mix works.

### F6. An interactive hidden prompt when a password is needed but absent

`crossterm` is already a dependency, so a hidden prompt costs no new crate.

**Recommendation: yes, for `restore`** — restoring a foreign archive on a fresh
machine is precisely the case where there is no stored password, and the only
alternative is `--password` on the command line, which leaks into shell history.
For `backup` the prompt matters less (the stored setting covers the normal
flow). Say the word and I will scope it to `restore` only, both, or neither.

### F7. Also read `MINDFORK_BACKUP_PASSWORD` from the environment?

**Recommendation: no**, unless you want scripted/CI backups. It is trivial to add
later; adding a third source now widens the "where did this password come from"
question for no current need. (Precedent: the API-key env fallback exists because
it predates the settings storage — it was not added for symmetry.)

### F8. Confirmations (no real alternative, listed so nothing is assumed)

* An **empty** password means "no encryption" — clearing the setting returns to
  plain archives, and existing encrypted archives still restore with an explicit
  `--password`.
* A wrong or missing password on restore is a **hard error before any data is
  touched** (measurement №6 makes this free).
* Precedence is **argument > setting > none**.
* After restoring an archive from *another* machine, the restored `settings.json`
  carries that machine's secret entries, which do not decrypt here — so the
  backup password (like every API key) has to be entered again on this machine.
  Inherent to ADR 0008, worth a line in the docs.

## 5. Rough size

Assuming the recommended options: ~2 focused PRs' worth of work but no track —
one PR is appropriate. Roughly: `backup.rs` +~80 lines and its tests +~120;
CLI/`main.rs` +~60; settings UI +~50; i18n ~12 keys ×2 bundles; docs. New tests
cover the four read-matrix cases, wrong-password-before-destruction,
round-trip with the stored password, an empty password meaning plain, and the
manifest staying readable.
