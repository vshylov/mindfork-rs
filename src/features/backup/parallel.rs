//! The worker pool behind packing and unpacking an archive (spec §12.3,
//! docs/research/backup-speed.md §3, §7).
//!
//! **Packing.** The work is a list of jobs that `threads` workers take in order:
//! the database first — its compaction and its packing are the longest chain, so
//! it starts before anything else —, then the scan of the schemas the manifest
//! records, then the files, largest first. A worker compresses (and, with a
//! password, encrypts) one file whole into a zip of one entry, held in memory or,
//! from [`ZipSpec::spill_over`] up, in a scratch file beside the archive. The
//! calling thread appends each raw (`raw_copy_file`: the bytes, the CRC, the AES
//! header — nothing compressed twice) in the order they land, and writes the
//! manifest last. The channel between them holds `threads` entries, so what
//! waits in memory is bounded by the pool, not by the data root.
//!
//! **Unpacking.** Every worker opens the archive on a handle of its own and takes
//! entries largest first; the calling thread counts them for the progress line.
//!
//! **A failure stops the pool and is the run's error.** A worker that fails sends
//! its error and quits; the others see the stop flag at their next job. A writer
//! that fails drops its end of the channel, which fails every worker's next send.
//! For a restore that is the same `Err` the single loop returned, so the rollback
//! is unchanged. Whatever a failed run left in scratch files is removed with them.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};
use zip::write::{FileOptions, FullFileOptions, SimpleFileOptions};
use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter};

use super::chunked::{self, Assembly, Chunk};
use super::{
    BackupManifest, DB_FILES, Entry, EntryProgress, MANIFEST_NAME, SchemaVersions, ScratchFile,
    ZipSpec, compacted_db, dated, entry_name, file_len, normalize,
};
use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
use crate::shared::storage::db;

/// From this size an entry's compressed copy waits in a scratch file beside the
/// archive rather than in memory. With the channel's `threads` entries and one in
/// each worker's hands, a pool of eight holds 17 × 8 MiB at most.
pub(super) const SPILL_OVER: u64 = 8 << 20;

/// A data entry's options: the level (`0` — stored) and, with a password,
/// AES-256. The manifest's are the same without the password.
type EntryOptions<'a> = FileOptions<'a, 'static, ()>;

/// One entry compressed into a zip of its own, waiting to be appended raw.
enum Packed {
    Memory { name: String, bytes: Vec<u8> },
    Spilled { name: String, file: ScratchFile },
}

/// A unit of the packing pool's work, in the order the workers take them.
enum Job<'a> {
    /// The schemas the files hold, for the manifest ([`SchemaVersions::held_by`]):
    /// a read of every chat, so a job of its own rather than the writer's wait.
    Schemas,
    /// Chunk `index` of big entry `entry` ([`chunked`]).
    Chunk { entry: usize, index: usize },
    /// A file, as gathered.
    File(&'a Entry),
    /// The database's compacted copy, held in memory, too small to chunk.
    Bytes { entry: &'a Entry, bytes: &'a [u8] },
}

/// What a finished job hands the writer.
enum Landed {
    /// An entry to append whole.
    Packed(Packed),
    /// A chunk of big entry `entry`, compressed.
    Chunk { entry: usize, chunk: Chunk },
    /// The schema scan, `db` still `0`.
    Schemas(SchemaVersions),
}

/// Where a pool's scratch files go: beside the archive — the volume it is
/// written to, normally `backups/`, never the data root a restore clears — each
/// named after the archive and numbered.
struct Scratch<'a> {
    dir: &'a Path,
    stem: String,
    seq: AtomicUsize,
    spill_over: u64,
}

impl Scratch<'_> {
    fn next(&self) -> ScratchFile {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        ScratchFile(
            self.dir
                .join(format!("{}.part-{}-{n}.tmp", self.stem, std::process::id())),
        )
    }
}

/// The database, made ready before the pool starts: its compacted copy in memory,
/// the same copy in a scratch file when it is over the cap or memory failed, or
/// the raw files with their sidecars when it cannot be compacted (the module doc
/// of `backup`). Ready first because its chunks are the pool's longest work, and
/// a copy in memory must outlive the pool that reads it.
enum Database {
    None,
    Memory {
        copy: db::MemoryCopy,
        entry: Entry,
        schema: u32,
    },
    File {
        // Held so the copy outlives the pool; removed on drop.
        _compact: ScratchFile,
        entry: Entry,
        schema: u32,
    },
    Raw {
        entries: Vec<Entry>,
        schema: u32,
    },
}

/// Compacts the database for packing (docs/research/backup-big-entry.md F2): in
/// memory when it and its `-wal` are at most `memory_cap`, else — or when that
/// fails — through a scratch file beside the archive, else raw. The copy is
/// dated by the newest of the files it was made of: a write that reached only
/// the `-wal` is in the copy, and so is its time. Never fails: a backup that
/// happens is worth more than a compact one.
fn prepare_database(paths: &Paths, out_path: &Path, memory_cap: u64) -> Database {
    let live = paths.data_db();
    if !live.is_file() {
        return Database::None;
    }
    let raw: Vec<Entry> = DB_FILES
        .iter()
        .map(|name| (paths.root().join(name), name))
        .filter(|(abs, _)| abs.is_file())
        .map(|(abs, name)| {
            let meta = fs::metadata(&abs).ok();
            Entry::of(abs, (*name).to_string(), meta.as_ref())
        })
        .collect();
    let mtime = raw.iter().filter_map(|e| e.mtime).max();
    let size: u64 = raw.iter().map(|e| e.len).sum();
    let data_db = |abs: PathBuf, len| Entry {
        abs,
        name: "data.db".into(),
        len,
        mtime,
    };
    if size <= memory_cap {
        let in_memory = db::vacuum_into_memory(&live).and_then(|mut copy| {
            let bytes = copy.bytes()?;
            let (len, schema) = (bytes.len() as u64, db::user_version_in_header(&bytes));
            drop(bytes);
            Ok((copy, len, schema.unwrap_or(0)))
        });
        match in_memory {
            Ok((copy, len, schema)) => {
                tracing::info!(
                    before = size,
                    after = len,
                    "backup: database compacted in memory"
                );
                let entry = data_db(live, len);
                return Database::Memory {
                    copy,
                    entry,
                    schema,
                };
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "backup: compacting in memory failed; through a file");
            }
        }
    }
    if let Some(compact) = compacted_db(paths, out_path) {
        let schema = db::header_user_version(compact.path()).unwrap_or(0);
        let entry = data_db(compact.path().to_path_buf(), file_len(compact.path()));
        return Database::File {
            _compact: compact,
            entry,
            schema,
        };
    }
    let schema = db::header_user_version(&live).unwrap_or(0);
    Database::Raw {
        entries: raw,
        schema,
    }
}

/// Where a big entry's bytes are read from: the database's copy in memory, or a
/// file.
enum Source<'a> {
    Memory(&'a [u8]),
    File(&'a Path),
}

/// An entry compressed in chunks: what the workers read, what the writer
/// assembles.
struct Big<'a> {
    name: &'a str,
    len: u64,
    mtime: Option<SystemTime>,
    source: Source<'a>,
    chunks: usize,
}

/// The pool's work: the schema scan, every big entry's chunks — the bulk, so
/// first —, then the rest whole, largest first.
fn plan<'a>(
    entries: &'a [Entry],
    database: &[&'a Entry],
    db_bytes: Option<&'a [u8]>,
    spec: &ZipSpec<'_>,
) -> (Vec<Job<'a>>, Vec<Big<'a>>) {
    let chunk = spec.chunk.max(1);
    let mut bigs = Vec::new();
    let mut whole: Vec<Job<'a>> = Vec::new();
    let mut by_size: Vec<(u64, Job<'a>)> = Vec::new();
    for entry in database.iter().copied().chain(entries) {
        let in_memory =
            db_bytes.filter(|_| database.len() == 1 && std::ptr::eq(entry, database[0]));
        if chunked::chunked(entry.len, spec.level, spec.chunked_from) {
            let source = match in_memory {
                Some(bytes) => Source::Memory(bytes),
                None => Source::File(&entry.abs),
            };
            let chunks = usize::try_from(entry.len.div_ceil(chunk as u64)).unwrap_or(usize::MAX);
            bigs.push(Big {
                name: &entry.name,
                len: entry.len,
                mtime: entry.mtime,
                source,
                chunks,
            });
        } else {
            let job = match in_memory {
                Some(bytes) => Job::Bytes { entry, bytes },
                None => Job::File(entry),
            };
            by_size.push((entry.len, job));
        }
    }
    by_size.sort_by_key(|(len, _)| Reverse(*len));
    whole.push(Job::Schemas);
    for (b, big) in bigs.iter().enumerate() {
        whole.extend((0..big.chunks).map(|index| Job::Chunk { entry: b, index }));
    }
    whole.extend(by_size.into_iter().map(|(_, job)| job));
    (whole, bigs)
}

/// Writes the archive at `out_path`: `entries`, the database, and the manifest
/// last. See the module doc.
pub(super) fn write_archive(
    paths: &Paths,
    out_path: &Path,
    entries: &[Entry],
    spec: &ZipSpec<'_>,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let dir = out_path.parent().unwrap_or_else(|| Path::new("."));
    let zip = create_output(dir, out_path, loc)?;
    let (plain, options) = entry_options(spec);
    let scratch = Scratch {
        dir,
        stem: out_path
            .file_name()
            .map_or_else(|| "backup".into(), |n| n.to_string_lossy().into_owned()),
        seq: AtomicUsize::new(0),
        spill_over: spec.spill_over,
    };
    let mut database = prepare_database(paths, out_path, spec.memory_cap);
    let (data, db_entries, db_schema) = match &mut database {
        Database::None => (None, Vec::new(), 0),
        Database::Memory {
            copy,
            entry,
            schema,
        } => (Some(copy.bytes()?), vec![&*entry], *schema),
        Database::File { entry, schema, .. } => (None, vec![&*entry], *schema),
        Database::Raw { entries, schema } => (None, entries.iter().collect(), *schema),
    };
    let (jobs, bigs) = plan(entries, &db_entries, data.as_deref(), spec);
    let password = normalize(spec.password);
    let assemblies = bigs
        .iter()
        .map(|big| {
            let dos = super::dates::dos_fields(big.mtime);
            let file = scratch.next();
            Assembly::new(big.name, big.chunks, dos, password, file)
                .map(Some)
                .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", big.name)]))
        })
        .collect::<Result<Vec<_>>>()?;
    let pool = &Pool {
        paths,
        options: &options,
        scratch: &scratch,
        loc,
        jobs: &jobs,
        bigs: &bigs,
        level: u32::try_from(spec.level.clamp(1, 9)).unwrap_or(6),
        chunk: spec.chunk.max(1),
        next: AtomicUsize::new(0),
        stop: AtomicBool::new(false),
    };
    let mut writer = Writer {
        zip,
        ticker: EntryProgress::new(entries.len() + db_entries.len()),
        schemas: SchemaVersions::default(),
        db_schema,
        assemblies,
    };

    let workers = spec.threads.clamp(1, jobs.len());
    let (tx, rx) = mpsc::sync_channel::<Result<Landed>>(workers);
    thread::scope(|s| -> Result<()> {
        for _ in 0..workers {
            let tx = tx.clone();
            s.spawn(move || pool.work(&tx));
        }
        drop(tx);
        // `rx` moves into this closure: an early return drops it before the scope
        // joins the workers, so none of them waits on a full channel for ever.
        let rx = rx;
        for landed in rx.iter() {
            if let Err(err) = landed.and_then(|landed| writer.land(landed, loc, progress)) {
                pool.stop.store(true, Ordering::Relaxed);
                return Err(err);
            }
        }
        Ok(())
    })?;
    writer.finish(plain, loc)
}

/// Creates the archive file, and the directory it goes in.
fn create_output(dir: &Path, out_path: &Path, loc: &Locale) -> Result<ZipWriter<File>> {
    fs::create_dir_all(dir).with_context(|| {
        loc.tf(
            "backup.ctx.create_dir",
            &[("path", &dir.display().to_string())],
        )
    })?;
    let file = File::create(out_path).with_context(|| {
        loc.tf(
            "backup.ctx.create_file",
            &[("path", &out_path.display().to_string())],
        )
    })?;
    Ok(ZipWriter::new(file))
}

/// The manifest's options — the level, `0` stored — and the data entries':
/// the same, encrypted when a password is set. The manifest carries no user
/// data and stays readable without the password.
fn entry_options<'a>(spec: &ZipSpec<'a>) -> (SimpleFileOptions, EntryOptions<'a>) {
    let level = spec.level.clamp(0, 9);
    let plain = if level == 0 {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    } else {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(level))
    };
    let options = match normalize(spec.password) {
        Some(pw) => plain.with_aes_encryption(AesMode::Aes256, pw),
        None => plain,
    };
    (plain, options)
}

/// What every worker of one packing shares: the jobs and the next one to take,
/// the stop flag, and what a job needs.
struct Pool<'a> {
    paths: &'a Paths,
    options: &'a EntryOptions<'a>,
    scratch: &'a Scratch<'a>,
    loc: &'a Locale,
    jobs: &'a [Job<'a>],
    bigs: &'a [Big<'a>],
    /// The deflate level of a chunk, `1..=9`.
    level: u32,
    /// The chunk a big entry is cut into ([`ZipSpec::chunk`]).
    chunk: usize,
    next: AtomicUsize,
    stop: AtomicBool,
}

impl Pool<'_> {
    /// A worker's loop: takes the next job until there are none, the pool is
    /// stopped, its own job fails, or the writer has gone.
    fn work(&self, tx: &mpsc::SyncSender<Result<Landed>>) {
        while !self.stop.load(Ordering::Relaxed) {
            let Some(job) = self.jobs.get(self.next.fetch_add(1, Ordering::Relaxed)) else {
                break;
            };
            let landed = run_job(job, self);
            let failed = landed.is_err();
            if tx.send(landed).is_err() || failed {
                break;
            }
        }
    }
}

/// The calling thread's side: the archive, the progress line, what the manifest
/// will record, and the big entries being assembled.
struct Writer {
    zip: ZipWriter<File>,
    ticker: EntryProgress,
    schemas: SchemaVersions,
    db_schema: u32,
    assemblies: Vec<Option<Assembly>>,
}

impl Writer {
    /// Takes in one finished job: appends its entry, a big entry's chunk — and
    /// the big entry once it is whole —, or keeps what it found.
    fn land(
        &mut self,
        landed: Landed,
        loc: &Locale,
        progress: &mut impl FnMut(&str),
    ) -> Result<()> {
        match landed {
            Landed::Packed(packed) => {
                self.ticker.tick(loc, progress);
                append(&mut self.zip, packed, loc)
            }
            Landed::Chunk { entry, chunk } => {
                let slot = self
                    .assemblies
                    .get_mut(entry)
                    .ok_or_else(|| anyhow!("a chunk of an entry that is not being assembled"))?;
                let assembly = slot
                    .as_mut()
                    .ok_or_else(|| anyhow!("a chunk of an entry already assembled"))?;
                let name = assembly.name().to_string();
                let complete = assembly
                    .land(chunk)
                    .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", &name)]))?;
                if complete && let Some(assembly) = slot.take() {
                    let file = assembly
                        .finish()
                        .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", &name)]))?;
                    self.ticker.tick(loc, progress);
                    append(&mut self.zip, Packed::Spilled { name, file }, loc)?;
                }
                Ok(())
            }
            Landed::Schemas(found) => {
                self.schemas = found;
                Ok(())
            }
        }
    }

    /// Writes the schema-version manifest (metadata, not a user file) last, and
    /// finishes the archive.
    fn finish(mut self, plain: SimpleFileOptions, loc: &Locale) -> Result<()> {
        if let Some(left) = self.assemblies.iter().flatten().next() {
            return Err(anyhow!("{} was left unassembled", left.name()));
        }
        let manifest = BackupManifest::written_now(SchemaVersions {
            db: self.db_schema,
            ..self.schemas
        });
        let manifest =
            serde_json::to_vec_pretty(&manifest).context("serializing backup manifest")?;
        self.zip
            .start_file(MANIFEST_NAME, dated(plain, Some(SystemTime::now())))
            .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", MANIFEST_NAME)]))?;
        io::copy(&mut manifest.as_slice(), &mut self.zip)
            .with_context(|| loc.tf("backup.ctx.pack", &[("path", MANIFEST_NAME)]))?;
        self.zip
            .finish()
            .with_context(|| loc.t("backup.ctx.finalize").to_string())?;
        Ok(())
    }
}

/// Runs one job on a worker.
fn run_job(job: &Job<'_>, pool: &Pool<'_>) -> Result<Landed> {
    let (options, scratch, loc) = (pool.options, pool.scratch, pool.loc);
    match job {
        Job::Schemas => Ok(Landed::Schemas(SchemaVersions::held_by(pool.paths))),
        Job::File(entry) => Ok(Landed::Packed(pack_file(entry, options, scratch, loc)?)),
        Job::Bytes { entry, bytes } => Ok(Landed::Packed(pack_from(
            entry,
            &mut &bytes[..],
            options,
            scratch,
            loc,
        )?)),
        Job::Chunk { entry, index } => {
            let big = &pool.bigs[*entry];
            let start = (*index as u64) * pool.chunk as u64;
            let end = (start + pool.chunk as u64).min(big.len);
            let data = read_chunk(big, start, end, loc)?;
            let last = index + 1 == big.chunks;
            let deflated = chunked::deflate_chunk(&data, pool.level, last)
                .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", big.name)]))?;
            Ok(Landed::Chunk {
                entry: *entry,
                chunk: Chunk {
                    index: *index,
                    deflated,
                    crc: crc32fast::hash(&data),
                    len: end - start,
                },
            })
        }
    }
}

/// The bytes `start..end` of a big entry: a slice of the copy in memory, or read
/// from its file.
fn read_chunk<'a>(big: &Big<'a>, start: u64, end: u64, loc: &Locale) -> Result<Cow<'a, [u8]>> {
    match big.source {
        Source::Memory(bytes) => {
            let range = usize::try_from(start)?..usize::try_from(end)?;
            let slice = bytes
                .get(range)
                .ok_or_else(|| anyhow!("{}: a chunk past the end", big.name))?;
            Ok(Cow::Borrowed(slice))
        }
        Source::File(path) => {
            let pack = || loc.tf("backup.ctx.pack", &[("path", &path.display().to_string())]);
            let mut file = File::open(path).with_context(|| {
                loc.tf("backup.ctx.open", &[("path", &path.display().to_string())])
            })?;
            file.seek(SeekFrom::Start(start)).with_context(pack)?;
            let mut data = vec![0; usize::try_from(end - start)?];
            file.read_exact(&mut data).with_context(pack)?;
            Ok(Cow::Owned(data))
        }
    }
}

/// Compresses `entry`'s file into a zip holding it alone, under its name and
/// dated by its modification time ([`dated`]).
fn pack_file(
    entry: &Entry,
    options: &EntryOptions<'_>,
    scratch: &Scratch<'_>,
    loc: &Locale,
) -> Result<Packed> {
    let abs = &entry.abs;
    let mut src = File::open(abs)
        .with_context(|| loc.tf("backup.ctx.open", &[("path", &abs.display().to_string())]))?;
    pack_from(entry, &mut src, options, scratch, loc)
}

/// [`pack_file`] from `src` rather than the file — the database's copy in
/// memory. `entry.abs` names the source in an error.
fn pack_from(
    entry: &Entry,
    src: &mut dyn Read,
    options: &EntryOptions<'_>,
    scratch: &Scratch<'_>,
    loc: &Locale,
) -> Result<Packed> {
    let (abs, name, len) = (&entry.abs, entry.name.as_str(), entry.len);
    let options = dated(*options, entry.mtime);
    if len >= scratch.spill_over {
        let file = scratch.next();
        let out = File::create(file.path()).with_context(|| {
            loc.tf(
                "backup.ctx.create_file",
                &[("path", &file.path().display().to_string())],
            )
        })?;
        write_one(ZipWriter::new(out), name, options, src, abs, loc)?;
        Ok(Packed::Spilled {
            name: name.to_string(),
            file,
        })
    } else {
        let capacity = usize::try_from(len / 2).unwrap_or(0);
        let out = Cursor::new(Vec::with_capacity(capacity));
        let bytes = write_one(ZipWriter::new(out), name, options, src, abs, loc)?;
        Ok(Packed::Memory {
            name: name.to_string(),
            bytes: bytes.into_inner(),
        })
    }
}

/// Writes `src` into `zip` as its one entry and finishes it.
fn write_one<W: Write + Seek>(
    mut zip: ZipWriter<W>,
    name: &str,
    options: FullFileOptions<'_, 'static>,
    src: &mut dyn Read,
    abs: &Path,
    loc: &Locale,
) -> Result<W> {
    zip.start_file(name, options)
        .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", name)]))?;
    io::copy(src, &mut zip)
        .with_context(|| loc.tf("backup.ctx.pack", &[("path", &abs.display().to_string())]))?;
    zip.finish()
        .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", name)]))
}

/// Appends a packed entry to the archive as it is — compressed and encrypted
/// already. A spilled one's scratch file is removed when it goes out of scope.
fn append(zip: &mut ZipWriter<File>, packed: Packed, loc: &Locale) -> Result<()> {
    let (name, copied) = match packed {
        Packed::Memory { name, bytes } => {
            let copied = copy_raw(zip, Cursor::new(bytes));
            (name, copied)
        }
        Packed::Spilled { name, file } => {
            let copied = File::open(file.path())
                .map_err(anyhow::Error::from)
                .and_then(|one| copy_raw(zip, one));
            (name, copied)
        }
    };
    copied.with_context(|| loc.tf("backup.ctx.write_entry", &[("name", &name)]))
}

/// Copies the single entry of the zip in `one` into `zip`, raw.
fn copy_raw<R: Read + Seek>(zip: &mut ZipWriter<File>, one: R) -> Result<()> {
    let mut one = ZipArchive::new(one)?;
    zip.raw_copy_file(one.by_index_raw(0)?)?;
    Ok(())
}

/// Unpacks `archive` into the data root on `threads` workers (entry names are
/// already considered safe — `enclosed_name` rejects escaping outside the root,
/// and is asked again here).
///
/// A `password` given for an unencrypted archive is harmlessly discarded by the
/// zip layer, so one code path restores both kinds.
pub(super) fn extract_archive(
    paths: &Paths,
    archive: &Path,
    password: Option<&str>,
    threads: usize,
    loc: &Locale,
    progress: &mut impl FnMut(&str),
) -> Result<()> {
    let mut zip = open_archive(archive, loc)?;
    // Largest first, by what each entry costs to read.
    let mut order: Vec<(usize, u64)> = (0..zip.len())
        .map(|i| (i, zip.by_index_raw(i).map_or(0, |e| e.compressed_size())))
        .collect();
    drop(zip);
    order.sort_by_key(|&(_, size)| Reverse(size));

    let mut ticker = EntryProgress::new(order.len());
    let workers = threads.clamp(1, order.len().max(1));
    let (next, stop, order) = (&AtomicUsize::new(0), &AtomicBool::new(false), &order);
    let (tx, rx) = mpsc::channel::<Result<()>>();
    thread::scope(|s| {
        for _ in 0..workers {
            let tx = tx.clone();
            s.spawn(move || {
                let mut zip = match open_archive(archive, loc) {
                    Ok(zip) => zip,
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                };
                while !stop.load(Ordering::Relaxed) {
                    let Some(&(i, _)) = order.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let done = extract_entry(&mut zip, i, paths, password, loc);
                    let failed = done.is_err();
                    if tx.send(done).is_err() || failed {
                        break;
                    }
                }
            });
        }
        drop(tx);
        // Every message is read, so no worker is left waiting; the first error
        // is the run's, the rest are what the stop flag did not reach in time.
        let mut first = None;
        for done in rx {
            match done {
                Ok(()) => ticker.tick(loc, progress),
                Err(err) => {
                    stop.store(true, Ordering::Relaxed);
                    first.get_or_insert(err);
                }
            }
        }
        first.map_or(Ok(()), Err)
    })
}

/// Opens `archive` as a zip, the refusals in the user's language.
fn open_archive(archive: &Path, loc: &Locale) -> Result<ZipArchive<File>> {
    let file = File::open(archive).with_context(|| {
        loc.tf(
            "backup.ctx.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    ZipArchive::new(file).with_context(|| loc.t("backup.ctx.read_archive").to_string())
}

/// Unpacks entry `i` of `zip` into the data root. The manifest is archive
/// metadata, not user data, and is not written into the root.
fn extract_entry(
    zip: &mut ZipArchive<File>,
    i: usize,
    paths: &Paths,
    password: Option<&str>,
    loc: &Locale,
) -> Result<()> {
    let mut entry = match password {
        Some(pw) => zip.by_index_decrypt(i, pw.as_bytes())?,
        None => zip.by_index(i)?,
    };
    let rel = entry.enclosed_name().ok_or_else(|| {
        anyhow!(
            "{}",
            loc.tf("backup.err.unsafe_entry", &[("name", &entry_name(&entry))])
        )
    })?;
    if rel == Path::new(MANIFEST_NAME) {
        return Ok(());
    }
    let dest = paths.root().join(&rel);
    if entry.is_dir() {
        return fs::create_dir_all(&dest).with_context(|| {
            loc.tf(
                "backup.ctx.create_dir",
                &[("path", &dest.display().to_string())],
            )
        });
    }
    if let Some(parent) = dest.parent() {
        // Safe when another worker creates the same directory at the same time:
        // `create_dir_all` treats a directory that appeared as made.
        fs::create_dir_all(parent).with_context(|| {
            loc.tf(
                "backup.ctx.create_dir",
                &[("path", &parent.display().to_string())],
            )
        })?;
    }
    let mtime = super::dates::entry_mtime(&entry);
    let mut out = File::create(&dest).with_context(|| {
        loc.tf(
            "backup.ctx.create_file",
            &[("path", &dest.display().to_string())],
        )
    })?;
    io::copy(&mut entry, &mut out).with_context(|| {
        loc.tf(
            "backup.ctx.extract",
            &[("path", &dest.display().to_string())],
        )
    })?;
    // An archive carries no stored file's origin, and it came from wherever the user
    // kept it: a restored `files/` entry carries the mark a call's output does (§13
    // U11), a user's own copy included.
    if rel.starts_with("files") {
        drop(out);
        crate::features::chat_files::mark(&dest, Some(crate::shared::os_open::FROM_ELSEWHERE));
        if let Some(mtime) = mtime {
            // After the mark: on NTFS writing its stream would date the file anew.
            let reopened = fs::OpenOptions::new().write(true).open(&dest);
            set_mtime(reopened, &dest, mtime);
        }
    } else if let Some(mtime) = mtime {
        set_mtime(Ok(out), &dest, mtime);
    }
    Ok(())
}

/// Gives a restored file the date its entry carries. Best effort: a date is not
/// data, and a restore that put every byte back must not fail over one.
fn set_mtime(file: io::Result<File>, dest: &Path, mtime: SystemTime) {
    if let Err(e) = file.and_then(|f| f.set_modified(mtime)) {
        tracing::warn!(path = %dest.display(), error = %e, "restore: the file's date was not set");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn scratch(dir: &Path, spill_over: u64) -> Scratch<'_> {
        Scratch {
            dir,
            stem: "a.zip".into(),
            seq: AtomicUsize::new(0),
            spill_over,
        }
    }

    /// From `spill_over` up an entry's compressed copy waits in a scratch file
    /// beside the archive — named after it, gone once the entry is dropped —
    /// and below it in memory; both copies hold the same entry.
    #[test]
    fn an_entry_waits_in_memory_or_in_a_scratch_file_by_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("chat.json");
        fs::write(&src, "word ".repeat(1000)).unwrap();
        let len = file_len(&src);
        let loc = locale(Lang::En);
        let options: EntryOptions<'_> = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .with_aes_encryption(AesMode::Aes256, "pw");
        let unpack = |reader: &mut dyn FnMut() -> ZipArchive<Box<dyn ReadSeek>>| {
            let mut zip = reader();
            let mut entry = zip.by_name_decrypt("chats/a.json", b"pw").unwrap();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            text
        };

        let entry = Entry {
            abs: src.clone(),
            name: "chats/a.json".into(),
            len,
            mtime: None,
        };
        let below = scratch(dir.path(), len + 1);
        let Packed::Memory { name, bytes } = pack_file(&entry, &options, &below, loc).unwrap()
        else {
            panic!("an entry below the threshold waits in memory");
        };
        assert_eq!(name, "chats/a.json");
        let in_memory =
            unpack(&mut || ZipArchive::new(Box::new(Cursor::new(bytes.clone())) as _).unwrap());

        let at = scratch(dir.path(), len);
        let Packed::Spilled { file, .. } = pack_file(&entry, &options, &at, loc).unwrap() else {
            panic!("an entry at the threshold waits in a scratch file");
        };
        let path = file.path().to_path_buf();
        assert_eq!(path.parent(), Some(dir.path()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("a.zip.part-"),
            "{path:?}"
        );
        let spilled =
            unpack(&mut || ZipArchive::new(Box::new(File::open(&path).unwrap()) as _).unwrap());
        assert_eq!(in_memory, spilled);
        assert_eq!(spilled, "word ".repeat(1000));
        drop(file);
        assert!(!path.exists(), "the scratch file outlived its entry");
    }

    trait ReadSeek: Read + Seek {}
    impl<T: Read + Seek> ReadSeek for T {}
}
