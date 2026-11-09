# Research: faster backup and restore

**Status:** research, measured 2026-11-09; every fork decided by the user the
same day. **F1** parallel zip, not 7z; **F2** the level is a setting, 6 by
default; **F3** the internal copies take the level the setting holds; **F4** the
thread count is a setting, never more than the machine's cores. The plan in §7
is written for these answers.

**Related:** spec §12.3, architecture §7,
[docs/history/backup-password.md](../history/backup-password.md) (the encryption
format and its limits), [docs/journal/storage.md](../journal/storage.md),
`src/features/backup.rs`.

The questions asked: is backup/restore single-threaded, would it pay to move to
7-Zip's format (which also handles encryption better), are there 7z
implementations entirely in Rust, and do they support multithreading and
encryption.

## 1. Answers in short

- **Yes, both paths are single-threaded.** `write_zip` and `extract_archive` are
  one loop each: one entry at a time, on one core (92 % CPU measured).
- **Most of a restore is not unpacking.** It is the pre-restore copy of the old
  data, made at level 9 before anything is cleared: ~4.1 s of a 5.3 s restore;
  the unpacking itself is ~0.7 s.
- **zip parallelizes well without a format change.** Entries compressed on four
  threads and appended raw: **4.11 s → 1.10 s**, the same 54.4 MB; unpacking on
  four threads **0.70 s → 0.20 s**. No new dependency, the archive is a standard
  zip, third-party readers verified.
- **7z does not make it faster.** `sevenz-rust2` (pure Rust, multithreaded LZMA2,
  AES-256 with an encrypted header) writes an archive 20–45 % smaller, but takes
  10.4 s on four threads at level 6 — ten times the parallel zip. Its encryption
  hides file names and has a heavier key derivation, but it is **not
  authenticated**.

## 2. Where the time goes today

**Test root.** Synthetic, shaped like the owner's real one (321 chats, 77 MB of
chats, a 26 MB database — journal, storage.md): 222 chats, each `.json` with its
near-identical `.bak` (76 MB together), `data.db` 27.7 MB (3 600 RAG chunks
carrying 1024-float unit vectors, which compress as badly as real embeddings,
plus 800 notes), six story databases, 25 incompressible pictures in `files/`, a
dictionary. 110.5 MB, 482 files. The text is paragraphs of this repository's
docs and the strings of `locales/ru.json`, so both scripts are in it.

**How measured.** A 4-vCPU Linux droplet; the 0.24.0 release binary; a warm page
cache; the best of two runs.

| Operation | Time | CPU |
|---|---|---|
| `backup` (level 9, the default) | 4.2 s | 3.77 s user — one core |
| `backup --password` | 4.5 s | |
| `restore` over existing data | 5.3 s | 4.0 s user |
| … of it, the pre-restore copy (level 9) | ~4.1 s | |
| `restore` into an empty root, encrypted archive | 1.4 s | |
| `VACUUM INTO` of the 27.7 MB database, on both paths | 0.35 s | |

Three callers make a full level-9 archive: `backup` itself, the pre-restore copy
(`restore_backup` step 2), and the **pre-migration backup**, which
`data_migration` makes silently at start-up, before the TUI appears — so an
upgrade's first start waits for it too.

## 3. Parallel zip

### 3.1 Method

- **Writing.** Entries sorted largest first; each worker compresses whole entries,
  each into a one-entry zip in memory with the archive's own options (deflate
  level, AES-256); the writer appends it with `ZipWriter::raw_copy_file`, which
  copies the compressed (and encrypted) bytes and their header fields without
  recompressing. Workers are `std::thread::scope` threads — nothing new in
  `Cargo.toml`. The probe merged in entry order; §7 merges in completion order.
- **Reading.** Each worker opens its own `ZipArchive` over the same file (the
  central directory is parsed once per worker — negligible for 482 entries) and
  takes entries largest first from a shared counter.

### 3.2 Measurements

With a password (AES-256), the same root:

| Variant | Time | Archive |
|---|---|---|
| zip L9, 1 thread — today | 4.11 s | 54.4 MB |
| zip L9, 2 threads | 2.13 s | 54.4 MB |
| zip L9, 4 threads | **1.10 s** | 54.4 MB |
| zip L6, 1 thread | 3.39 s | 54.4 MB |
| zip L6, 4 threads | 0.91 s | 54.4 MB |
| zip L1, 1 thread | 1.06 s | 69.0 MB |
| zip L1, 4 threads | 0.34 s | 69.0 MB |
| zip L0 (store), 1 thread | 0.45 s | 110.6 MB |
| unzip, 1 → 2 → 4 threads | 0.70 → 0.38 → **0.20 s** | |

AES costs ~0.35 s of the 4.11 s (3.76 s without a password). Level 9 buys
nothing over level 6 on this data (54.4 MB both) and costs ~20 % more time. The
peak memory of the parallel writer was 61 MB, because the probe holds every
compressed entry in memory until the merge (§7 bounds it).

Estimated from these, **not measured end to end**: `backup` ≈ 1.5 s (1.1 s of
packing, the rest the serial `VACUUM INTO` and the walk); `restore` over data
≈ 2 s with a parallel level-9 pre-restore copy and parallel unpacking.

### 3.3 Interoperability

The parallel-built encrypted archive was read by an independent implementation,
`pyzipper` 0.4.0: 482 entries, **0** SHA-256 mismatches against the source
files. Nothing about the archive changes for a reader — it is the same WinZip
AES-256 zip that docs/history/backup-password.md §1 №9 checked against 7-Zip, so
old backups, `mindfork stats <archive>`, `--compare` and the companion export
read it the way they read today's.

### 3.4 Limits

- **One entry is one thread.** The largest file, `data.db`, bounds the speed-up.
  At 27.7 MB it does not show (four threads came within 7 % of a perfect 4×), but
  a database of gigabytes would become the whole backup's duration. Splitting
  one entry needs a pigz-style chunked deflate, which `zip` has no API to write
  (§5).
- **What stays serial**: `VACUUM INTO` (0.35 s), the directory walk, and
  `SchemaVersions::held_by` reading every chat's version — together ~0.45 s,
  about a third of the parallel backup. `VACUUM INTO` can run beside the
  compression of the other entries (§7).
- **Windows is not measured.** NTFS and Defender make creating many small files
  slower there; parallel unpacking usually gains more from that, but this is
  what the go/no-go in §7 is for.

## 4. 7z

### 4.1 Implementations in pure Rust

| Crate | State | Multithreading | Encryption |
|---|---|---|---|
| `sevenz-rust2` 0.25.0 | Active: 1.46 M downloads in 90 days. 45 versions since 2025-02-24, a breaking 0.x release every 2–4 weeks (0.23 → 0.25 between 2026-09-18 and 2026-11-06) | LZMA2 compression (`Lzma2WriterMt`) and decompression (`Lzma2ReaderMt`); `prepare_block` compresses a block on any thread, `push_prepared_block` appends it | AES-256 and header encryption (on by default). **The key derivation defaults to 2^8 rounds** where 7-Zip writes 2^19 — it has to be set by hand (`num_cycles_power`) |
| `sevenz-turbo` 0.29.6 | A fork of `sevenz-rust2` created 2026-09-17, 19.5 k downloads | the same, tuned for speed | default crypto is `aws-lc-rs` — C and assembly, so not "entirely Rust" unless switched to `native-crypto` |
| `zesven` 3.2.0 | Since 2026-01-19, 68 k downloads, 8 versions | `parallel` feature (rayon) — read from its feature list, not measured | AES |
| `sevenz-rust` 0.6.1 | Unmaintained since 2024-07; superseded by `sevenz-rust2` | not examined | AES |

All the living ones use the same codec, `lzma-rust2`: its `optimization` feature
(on by default) uses unsafe code and hand-written assembly. Turned off, the code
is 100 % safe Rust, at some cost in speed.

### 4.2 Measurements

`sevenz-rust2` 0.25.0, `aes256` + `compress` only, AES at 2^19 rounds, the same
root. The second size column is a variant of the root where every paragraph's
words are shuffled, so the chats share little text with each other. The zip
archive is 54.4 MB on the first root and 55.4 MB on the second; LZMA2's gain
depends on how much the files repeat each other, deflate's 32 KB window barely
does.

| Variant | Time | Archive | Shuffled root |
|---|---|---|---|
| solid, LZMA2 L6, 1 thread | 39.9 s | 29.3 MB | — |
| solid, LZMA2 L6, 4 threads, 8 MB chunks | 10.4 s | 33.4 MB | 13.7 s, 42.7 MB |
| solid, LZMA2 L3, 4 threads | 6.1 s | 37.1 MB | 7.3 s, 47.3 MB |
| solid, LZMA2 L1, 4 threads | 3.7 s | 43.3 MB | 4.5 s, 50.2 MB |
| 16 MB blocks, L6, 4 workers (`prepare_block`) | 14.0 s | 31.2 MB | — |
| unpack an MT-written archive, 1 → 4 threads | 0.95 → 0.35 s | | |
| unpack a 1-thread-written archive, 4 threads | 0.92 s | | |

- **Slower at every level.** Even LZMA2 level 1 on four threads (3.7 s) is
  slower than the parallel zip at level 9 (1.1 s).
- **Blocks lose to the MT encoder.** The `data.db` block keeps one worker busy,
  while the MT encoder splits the file itself.
- **Unpacking runs in parallel only for archives the MT encoder wrote** — they
  are cut into independent chunks; a single-thread archive is one stream.
- **Memory.** The 7z runs peaked at 443 MB, against 61 MB for the parallel zip.

**Interoperability:** both 7z archives were extracted by `py7zr` 1.1.4 (liblzma
inside) with 0 mismatches of 482; without a password it refuses even to list the
names (`PasswordRequired`).

### 4.3 Encryption compared

| | zip, WinZip AES-256 (today) | 7z, 7zAES |
|---|---|---|
| Cipher | AES-256-CTR | AES-256-CBC |
| Integrity | HMAC-SHA1 per entry: 66 of 66 corruptions caught (backup-password.md §1 №7) | **None cryptographic**: a CRC32 of the plaintext, checked after decompression |
| Names, sizes, structure | visible | hidden when the header is encrypted |
| Key derivation | PBKDF2-HMAC-SHA1 × 1000, fixed by the format | SHA-256 × 2^NumCyclesPower, stored in the archive: 2^19 from 7-Zip, 2^8 from `sevenz-rust2` unless set |
| Work per guess | 66 derived bytes = 4 blocks × 1000 × 2 = 8 000 SHA-1 compressions | 2^19 × (password as UTF-16 + 8-byte counter + salt): ~393 000 SHA-256 compressions for a 20-character password — about **50× more**; at 2^8 with its salt, ~260 — about **30× less** than zip |
| Salt | 16 bytes per entry | none from 7-Zip; 16 random bytes from `sevenz-rust2` |
| Memory-hard KDF | no | no |
| `manifest.json` readable without the password | yes (backup-password.md F5) | no, once the header is encrypted |

The counts are this note's arithmetic over each format's definition, not a
cracking benchmark.

So "7z encrypts better" holds for **name hiding** and the **cost of a guess**. It
does not hold for **integrity**: 7z drops the per-entry authentication the
current format has. Neither format has a memory-hard KDF; the only route to one
is the outer container backup-password.md F1(b) weighed and rejected.

### 4.4 What a switch would cost

- **zip has to stay readable anyway.** Every existing backup is a zip, so 7z
  would be a second format, not a replacement.
- **Every reader of the archive changes**: `backup.rs` (create, validate,
  `check_password`, `read_manifest`, `ArchiveReader`), `main.rs` (the password
  check before the prompt, the "newer version" warning), `data_migration.rs`,
  `data_stats` and its timeline, `companion_carry.rs`.
- **Random access to an entry is lost in a solid archive**: `mindfork stats
  <archive>` reads `data.db` straight out of the archive, and in a solid block
  that means decoding everything before it.
- **The manifest needs the password**, so the "this backup is from a newer
  version" warning and `stats` would no longer work without it.
- **Windows Explorer opens a plain `.zip`** everywhere, a plain `.7z` only from
  Windows 11 23H2. Encrypted archives need 7-Zip either way: Explorer does not
  read WinZip AES.
- **A dependency on a 0.x crate** that breaks its API every few weeks, with a
  weak KDF default to override.

### 4.5 Verdict

7z is not a way to speed up backups. It is a candidate for a different goal —
hiding names in an archive that leaves the machine — and, if that ever becomes a
requirement, belongs beside zip as an export format, weighed against its loss of
authentication. **User's decision (2026-11-09): parallel zip; 7z is not
pursued.**

## 5. Not pursued

- **`zlib-rs` as the deflate backend.** It is faster than `miniz_oxide`, but
  `Cargo.toml` deliberately keeps every `flate2` user on `miniz_oxide`, and
  features unify across the binary. Not measured; a decision of its own.
- **zstd inside the zip.** The `zstd` crate is C, the pure-Rust encoders are
  young, and stock tools (Windows Explorer) cannot open such an entry.
- **Moving the old data aside instead of a pre-restore archive.** A rename is
  instant on one volume, but the copy would no longer be one restorable file,
  and it would not be encrypted — backup-password.md F3 makes the copy never
  weaker than what the user asked for.
- **A chunked (pigz-style) deflate of one large entry.** It only matters for a
  database of gigabytes, and `zip` has no API to write a pre-compressed stream.
  Revisit if such a root is measured.

## 6. Forks

### F1. The format — decided

**(a) parallel zip**, **(b)** 7z, **(c)** both. **User's decision, 2026-11-09:
(a).**

### F2. The default compression level

| Option | Effect on the test root, 4 threads |
|---|---|
| (a) keep 9 | 1.10 s, 54.4 MB |
| **(b) 6** *(recommended)* | 0.91 s, 54.4 MB — the same size |
| (c) 1 | 0.34 s, 69.0 MB (+27 %) |

**Recommendation: (b).** On this data level 9 compresses no better than 6, and
6 is the level `companion_carry` already uses. `-c 9` stays available. The change
is `cli::DEFAULT_COMPRESSION`; no help text names the default.

**User's decision (2026-11-09): a setting, 6 by default.** The level lives in
`settings.json` and on the settings screen; `-c` overrides it for one run.

### F3. The level of the internal copies (pre-restore, pre-migration)

Both are hard-coded at 9 today (`restore_backup`, `data_migration`).

- **(a) the default level of F2** *(recommended)*. One constant, and the copy
  that persists in `backups/` is as compact as a hand-made one; parallel packing
  already brings it to ~1 s here.
- **(b) level 1.** About 0.6 s faster on this root, at +27 % disk for a file
  that stays.

**User's decision (2026-11-09): (a)** — the level the setting holds.

### F4. The number of threads

- **(a) `std::thread::available_parallelism()`, capped at 8, no setting**
  *(recommended)*. Past a few threads the largest entry dominates, and every
  worker holds a deflate state and a buffer. The cap of 8 is a guess: nothing
  above 4 threads was measured.
- **(b) the same, plus a `--threads` flag** on `backup`/`restore`, for a machine
  that should keep cores free.
- **(c) a setting** in the settings screen.

**User's decision (2026-11-09): a setting, and never more than the cores.** `0`
means every core; a number above what the machine has counts as all of them, so
a portable data root moved to a smaller machine keeps working at its size.

## 7. Plan, for the decisions

One PR, `feat/backup-parallel`.

- **Settings** (`settings.json`, additive — no schema step): `backup.compression`
  (0–9, default 6) and `backup.threads` (0 = all cores), two rows in the "Data"
  section beside the password. `mindfork backup` without `-c`, the pre-restore
  copy and the pre-migration backup take the level from there; packing and
  unpacking take the thread count, capped at the cores. The backup commands read
  both from the raw JSON, like the password (`backup_settings`): a settings file
  the app refuses to start on can still be backed up.

- **Writer** (`write_zip`):
  - Workers compress entries largest first, each into a one-entry zip.
  - A bounded channel hands each finished entry to the writer, which appends it
    with `raw_copy_file` **in completion order** — to check: no reader depends on
    the order of entries; the manifest is still written last.
  - An entry above a threshold (e.g. 64 MiB) is spilled to a scratch file beside
    the archive rather than held in memory, so memory is bounded by the channel,
    not by the root. With a password the spill is ciphertext; without one it is
    what the archive holds anyway. It is removed on drop, like `TempDb`.
  - Progress ticks on the writer thread, as entries land.
- **`VACUUM INTO` on its own thread.** The compacted `data.db` joins the queue
  when it is ready, instead of the whole backup waiting for it.
- **Reader** (`extract_archive`):
  - Workers each hold their own `ZipArchive<File>` and take entries largest
    first.
  - The first error raises a stop flag and is returned, so `restore_backup`'s
    contract and its rollback are unchanged. `create_dir_all` is safe
    concurrently.
  - The `files/` origin mark (`chat_files::mark`) is applied per entry, as
    today.
- **Tests:**
  - The parallel archive holds the same names and bytes as the sequential one,
    with and without a password.
  - The manifest stays unencrypted.
  - A failure injected mid-extract still rolls back.
  - A wrong password is still refused before anything is cleared.
  - A 0.24.0-made archive still restores.
- **Go/no-go, on the owner's Windows machine and real data:**
  - `backup` and a restore over existing data are each at least 2× faster than
    0.24.0 on the same machine.
  - 7-Zip opens the new archive, the encrypted one included.
  - The restored root matches the source.

  A live model run is not required (AGENTS.md §3): no engine, memory or tool
  path is touched.
- **Docs:** spec §12.3 (one line on parallelism and the default level), the
  journal entry in storage.md, CHANGELOG (*Changed*: faster backup and restore;
  the new default level if F2(b)).

## Appendix: the probe

A throwaway crate, not committed (as in backup-password.md §1):
- the same `zip` (9.1.3, `deflate-flate2` + `aes-crypto`) as the app, plus
  `sevenz-rust2` 0.25.0 (`aes256`, `compress`);
- built `opt-level = 3`, one codegen unit, thin LTO;
- the generator of the test root is a short Python script.

To reproduce, regenerate a root of the shape in §2 and time:
- the app's `write_zip` loop;
- the worker/`raw_copy_file` variant of §3.1;
- per-worker `ZipArchive` extraction;
- `ArchiveWriter` with `Lzma2Options::from_level_mt` and `AesEncoderOptions`
  (`num_cycles_power = 19`).
