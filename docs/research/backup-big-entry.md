# Research: one big file on many threads

**Status:** research, measured 2026-11-09. Every fork in §8 decided by the user the
same day, each at the recommendation: **F1(a)**, **F2(a)**, **F3(a)**, **F4(a)**,
**F5(a)**. The plan in §9 is built in that order.

**Related:** [backup-speed.md](backup-speed.md) — the pool this builds on, and its
§3.4, the limit studied here; spec §12.3; `src/features/backup/parallel.rs`;
[docs/journal/storage.md](../journal/storage.md).

**Why now.** The owner's Windows run of the parallel backup (i9-9900K, real data,
a debug build) went from 95.3 s to 21.8 s. Two thirds of that root is one file, a
194 MB `data.db`, and the pool gives one file one thread. The question: can one
big file be compressed on many threads, and what else stands in its way?

## 1. Answers in short

- **One entry can be compressed on many threads and stay a standard zip entry.**
  Deflate it in 1 MiB chunks with a sync flush between them — pigz's method — and
  join the pieces into one stream. On a 285 MB database this took **6.68 s → 1.69 s**
  on 4 threads, at +0.16 % size. Every reader tried reads the stream: the `zip`
  crate, `pyzipper`, Info-ZIP.
- **`zip` cannot take such a stream into an encrypted entry, so we write the entry
  ourselves.** That means a one-entry zip — the header, the `0x9901` field and
  WinZip AE-2 around the stream — appended with `raw_copy_file`, as the pool already
  does. The crypto crates are already in the tree. The `zip` crate reads it and
  checks its HMAC, `pyzipper` reads it, and a wrong password is refused.
- **Compression is only half of the database's chain.**
  - At backup, `VACUUM INTO` a file took **4.06 s** for that database on this
    machine. Into SQLite's in-memory VFS it took **0.24 s**, plus 0.30 s to hand
    the bytes over.
  - At restore, an in-place `VACUUM` took **8.05 s** for a database with no free
    page — and since compaction exists, every archive this app writes holds exactly
    such a database.
- **Today, a 352 MB root with that database:** backup **11.0 s**, restore over
  existing data **34.6 s**, restore into an empty root **9.0 s**. With the levers
  below, the estimate is ~2.5 s, ~7 s and ~4 s — an estimate, not a measurement.
- **Two cheap levers on the side:**
  - `zlib-rs` compresses 2.1× faster in one thread, but it would become the backend
    of every `flate2` user, and it is unsafe code.
  - Optimizing the compression crates in the dev profile cut the debug build's
    backup from 3.64 s to 1.32 s — the build the owner measured with.

## 2. Where the time goes with a big database

**Test data.** A synthetic `data.db` of 285 MB:
- 40 000 chunks of text — the repository's docs and Russian strings, half of them
  word-shuffled;
- 40 MB of float32 embeddings;
- 40 MB of zero-padded vector chunks (what `vec0` leaves in a part-filled chunk).

It compresses 2.28× at level 6. The owner's compresses 5× — more text, which
deflates faster per MB, so this is the harder case. For the root it was put into
the 110 MB root of [backup-speed.md](backup-speed.md) §2: 352 MB in all.

**How measured.** A 4-vCPU droplet, the release binary of `main`, a warm page
cache.

| Step | Time | What it is |
|---|---|---|
| `backup`, whole | 11.0–11.7 s | CPU 84–89 %: one thread most of the time |
| — `VACUUM INTO` the scratch file | 4.06 s | the database job's first half |
| — deflate of `data.db`, one thread, L6 | 6.68 s | its second half |
| `restore` over existing data, whole | 34.6 s | CPU 36 %: waiting on the disk |
| — the pre-restore copy | ~11 s | a backup, as above |
| — in-place `VACUUM` after unpacking | 8.05 s | for a database with no free page |
| `restore` into an empty root | 9.0 s | unpacking and the in-place `VACUUM` |

The rest of the root — 482 small files — takes about a second on the pool. The
database's chain is the job.

## 3. Deflate in chunks

**Method.** Split the input into chunks; compress each as raw deflate on a worker,
ending every chunk but the last with a **sync flush**: an empty stored block that
leaves the stream byte-aligned and not finished. The last chunk ends the stream.
Concatenated in order, the pieces are one valid deflate stream. Each chunk starts
with an empty window, which is the only cost. `flate2` can preset a dictionary only
with a zlib backend, and none was needed. The CRC-32 of the whole is combined from
the chunks' (`crc32fast::Hasher::combine`).

285 MB, 4 threads; `miniz_oxide` (today's backend), then `zlib-rs` for comparison:

| | miniz_oxide | | zlib-rs | |
|---|---|---|---|---|
| | time | size | time | size |
| one thread, L1 | 1.40 s | 148.0 MB | 1.22 s | 162.7 MB |
| one thread, **L6** | **6.68 s** | **124.7 MB** | 3.15 s | 125.5 MB |
| one thread, L9 | 8.69 s | 124.6 MB | 6.26 s | 124.7 MB |
| chunks of 256 KiB, L6 | 1.69 s | 125.7 MB | 0.85 s | 126.5 MB |
| chunks of **1 MiB**, L6 | **1.69 s** | **124.9 MB** | 0.86 s | 125.8 MB |
| chunks of 4 MiB, L6 | 1.74 s | 124.7 MB | 0.86 s | 125.6 MB |
| chunks of 1 MiB, L1 | 0.42 s | 148.0 MB | 0.40 s | 162.9 MB |
| chunks of 1 MiB, L9 | 2.08 s | 124.8 MB | 1.65 s | 124.9 MB |

- **3.95× on 4 threads.** The cost is +0.16 % at 1 MiB and nothing measurable at
  4 MiB.
- **Unpacking is one stream either way.** A chunked stream inflated in 0.74 s, as
  a single-thread one does. Inflating one stream in parallel would need the chunk
  boundaries, which the zip format has nowhere to keep (§7).
- **Memory.** The probe held the whole file. The design streams it: workers take
  chunks from the file, and a reorder window of a few chunks per thread puts them
  back in order.

## 4. Writing the entry ourselves

**What `zip` offers, and why none of it is enough:**
- `raw_copy_file` copies an entry of another archive as it is. It needs that
  archive to exist, so the entry still has to be written somewhere.
- `ZipFileBuilder` / `PreparedZipFile` (9.x; 9.5 adds a rayon pool over them)
  compress elsewhere and append. But the compressor is the crate's own, per file,
  and *"Encrypted files cannot be compressed outside of a ZipWriter"*. There is no
  constructor that takes a stream compressed elsewhere.
- The AES writer (`AesWriter`) is in a private module.

**What we would write**, into a scratch file beside the archive (as the pool spills
today), which the writer then raw-copies:
- a **local header**:
  - method 8, or 99 with a password;
  - flags: UTF-8, plus encrypted with a password;
  - the DOS date (`dates.rs`);
  - sizes patched in by a seek once known;
  - the CRC — 0 under AE-2, which by definition carries none.
- the **`0x9901` field**: AE-2, AES-256, actual method 8.
- **the body.** Plain: the chunked stream. With a password it is wrapped in AE-2:
  - a 16-byte random salt;
  - PBKDF2-HMAC-SHA1 × 1000, giving 66 bytes: the AES key, the HMAC key and a
    2-byte verifier;
  - AES-256-CTR with a little-endian counter from 1;
  - HMAC-SHA1 over the ciphertext, the first 10 bytes appended.

  CTR is a stream and HMAC is incremental, so encryption follows the chunks in
  order. In the probe it took 0.26 s for 125 MB on one thread.
- a **central directory** of one entry, and the end record.

**Measured.** Both entries were raw-copied into an archive the `zip` crate wrote: a
285 MB `data.db` with AE-2, and the same one plain.
- The `zip` crate read both back, CRC equal; with the password it verified the
  HMAC, and a wrong password was refused.
- `pyzipper` read both byte-identical and refused a wrong password.
- Info-ZIP's `unzip -t` passed the plain one.

**What it costs.** About 200 lines: chunking, reordering, the header, AE-2. It is
crypto we would own. AE-2 is small and fully specified, and the reader that checks
it is the independent `zip` crate. The tests would be:
- a round trip through the `zip` crate, encrypted and not;
- a byte flipped in the ciphertext refused by the HMAC (backup-password.md §1 №7);
- a wrong password refused;
- a fixture pinned once 7-Zip has opened it by hand.

**What it leaves to the existing path.**
- An entry of 4 GiB or more would need zip64 fields in the hand-written header.
  Such an entry stays on the single-thread path, which `zip` already handles.
- Entries below a threshold (16 MiB) gain nothing from chunks.

The `UT` timestamp is dropped by `raw_copy_file` here as everywhere (`dates.rs`),
so this entry carries the DOS date like the rest.

## 5. The database's compaction

`VACUUM INTO` the scratch file was the other half of the chain.

| 285 MB database, nothing to free | Time |
|---|---|
| `VACUUM INTO` a file (today's backup) | 4.06 s |
| … with a 256 MB cache, temp store in memory | 4.22–4.31 s |
| Online Backup API into a file | 4.01 s |
| plain file copy | 2.85 s |
| **`VACUUM INTO` SQLite's in-memory VFS (`memdb`)** | **0.24 s** |
| … and `serialize` to hand over its bytes | 0.54 s in all |
| in-place `VACUUM` (today's restore) | 8.05 s |

- **On this machine it is the write that costs.** `strace` counted 0.6 s in
  `pwrite`/`fdatasync`, and the file copy alone took 2.85 s. On the owner's NVMe
  the file path will cost less — but the copy need not be written at all.
- **Compaction in memory.** Hold a `memdb` connection open, `VACUUM INTO` it, and
  `serialize` it. `rusqlite`'s `serialize` feature is already enabled for
  `mindfork stats`, and on a `memdb` it can hand over its pages without copying.
  The chunks are then compressed straight from memory.
  - It costs RAM equal to the database: 194 MB for the owner. Above a cap the file
    path stays (§8 F2).
  - The source connection must be opened with `SQLITE_OPEN_URI` for the target's
    URI to be read as one.
  - The existing test that a `VACUUM INTO` keeps the `vec0` index joined
    (`vacuum_into_preserves_the_vector_index`) applies unchanged.
- **The restore's in-place `VACUUM` has nothing to do on our own archives.**
  - A backup packs a compacted copy, so the database it unpacks has no free page.
    The header says so — its free-page count, offset 36 — read with no SQLite open,
    as `header_user_version` reads offset 60.
  - An archive from before compaction, or from another tool, has free pages and is
    still compacted.
  - The 8 s this saves are the single largest item of a restore here.

## 6. Two levers on the side

- **`zlib-rs` as the deflate backend.** 2.1× in one thread, 2× in chunks, about
  +0.6 % size at L6. It is the backend of **every** `flate2` user (features unify):
  fetched pages, PDFs, downloads. `Cargo.toml` deliberately keeps `miniz_oxide`
  there, because `zlib-rs` is unsafe code. With chunks on 8–16 cores the database's
  deflate is already under a second, so the gain is small for what it moves.
- **The dev profile.** `cargo run` builds dependencies unoptimized, so deflate,
  AES, SHA-1 and SQLite run at debug speed. `[profile.dev.package.<crate>]
  opt-level = 3` for `miniz_oxide`, `flate2`, `zip`, `crc32fast`, `aes`, `sha1`,
  `hmac` and `libsqlite3-sys` cost one 83 s rebuild. A debug backup of the 110 MB
  root then took 1.32 s, where it had taken 3.64 s (the release binary: 1.07 s).
  It touches no shipped binary, and it speeds up every test that compresses or
  touches SQLite too.

## 7. Not pursued

- **Splitting a big file into several entries** (`data.db.001`, …). The pool would
  take them as they are, but 0.24.0 and older would unpack the parts and no
  `data.db`, a restore by hand would need a concatenation, and `mindfork stats`
  reads `data.db` from the archive by name.
- **Inflating one stream in parallel.** It needs the chunk boundaries, and the zip
  format has no place for them that other readers would ignore safely. Inflate is
  0.7 s for 285 MB in any case; the disk write after it costs more.
- **Compressing `VACUUM INTO`'s output while it is written.** The pages are
  written in order today, but nothing promises it. `memdb` makes the question moot.
- **zstd / 7z for the big entry** — weighed and left in
  [backup-speed.md](backup-speed.md) §4–§5.

## 8. Forks

### F1. Chunked deflate of a big entry

- **(a) Yes** *(recommended)*: entries of 16 MiB up to 4 GiB, 1 MiB chunks, in the
  same pool, through the hand-written entry of §4.
- **(b) No**: one entry, one thread, and the database's deflate stays the chain.

**User's decision (2026-11-09): (a).**

### F2. Where the database is compacted at backup

- **(a) In memory** *(recommended)*: `VACUUM INTO` a `memdb`, chunks compressed
  from it. The cap is the smaller of 1 GiB and a quarter of physical memory; above
  it, the file path of today.
- **(b) A file, as today.**

**User's decision (2026-11-09): (a).**

### F3. The restore's in-place `VACUUM`

- **(a) Only when the header counts free pages** *(recommended)*. Our own archives
  skip it; older and foreign ones are compacted as now.
- **(b) Always, as today.**

**User's decision (2026-11-09): (a).**

### F4. The deflate backend

- **(a) Keep `miniz_oxide`** *(recommended)*: with F1 the remaining gain is under
  a second, and the backend is every `flate2` user's.
- **(b) `zlib-rs`**: 2× more, unsafe code in every inflate the app does.

**User's decision (2026-11-09): (a).**

### F5. Optimized compression crates in debug builds

- **(a) Yes** *(recommended)*: the eight crates of §6 at `opt-level = 3` in the
  dev profile. Nothing shipped changes.
- **(b) No.**

**User's decision (2026-11-09): (a).**

## 9. Plan, for the decisions

Stages, each its own PR:

1. **F5 and F3** — small, independent, measurable at once.
2. **F2** — compaction in memory, with its cap.
3. **F1** — the chunked entry. The writer gains jobs that are a chunk of an entry;
   the entry's scratch file is assembled in order as they land; AE-2 follows. The
   tests are the ones in §4.

**Go/no-go, on the owner's machine with real data, release build:**
- `backup` and `restore` over existing data each at least 2× faster than `main`
  today on the same machine;
- 7-Zip opens the new archive, the encrypted one included;
- the restored root is identical.

## Appendix: the probes

Throwaway crates, not committed (as in backup-password.md §1):
- **A chunked-deflate and AE-2 probe.** `flate2` raw `Compress` with
  `FlushCompress::Sync`; AE-2 from `aes`, `hmac`, `sha1` and `pbkdf2`; a one-entry
  zip written by hand; `zip` 9.1.3 to raw-copy and read back.
- **The SQLite timings**, taken with the system SQLite through Python.
- **The dev-profile timing**, taken with the overrides appended to `Cargo.toml`
  and reverted.

The database generator is a short Python script; §2 gives its shape.
