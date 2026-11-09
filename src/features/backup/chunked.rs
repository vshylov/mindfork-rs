//! One big entry compressed on many threads (spec §12.3,
//! docs/research/backup-big-entry.md F1, §3–§4).
//!
//! The pool gives an entry one thread, and a database of a few hundred megabytes
//! was most of a backup. A big entry is cut into chunks; each is compressed on a
//! worker as raw deflate ending in a **sync flush** — an empty stored block that
//! leaves the stream byte-aligned and open — and the last one ends the stream.
//! Joined in order, the pieces are one ordinary deflate stream (pigz's method):
//! every inflater reads it, and nothing about the archive changes for a reader.
//!
//! `zip` cannot take such a stream into an encrypted entry (its `ZipFileBuilder`
//! refuses encryption; its AES writer is private), so the writer assembles the
//! entry itself, as the chunks land, into a zip of one entry in a scratch file —
//! [`Assembly`] — and appends that raw like any other entry. With a password the
//! body is **WinZip AE-2**, as `zip` writes it: a random 16-byte salt, PBKDF2 with
//! HMAC-SHA1 over 1000 rounds for 66 bytes of keys (the AES key, the HMAC key, a
//! 2-byte verifier), AES-256-CTR with a little-endian counter from 1, and the first
//! 10 bytes of an HMAC-SHA1 of the ciphertext. AE-2 carries no CRC; a plain entry
//! carries the CRC combined from the chunks'.
//!
//! An entry this writes is under 4 GiB (no zip64 here); a bigger one stays on the
//! one-thread path, which `zip` writes.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};

use aes::cipher::{BlockCipherEncrypt, KeyInit};
use flate2::{Compress, Compression, FlushCompress, Status};
use hmac::{Mac, SimpleHmacReset};
use sha1::Sha1;
use zeroize::Zeroizing;

use super::ScratchFile;

/// The chunk a big entry is cut into: 1 MiB cost +0.16 % of size on the
/// research's database, against one stream (§3).
pub(super) const CHUNK: usize = 1 << 20;

/// From this size an entry is compressed in chunks; below it one thread does it
/// as fast.
pub(super) const CHUNKED_FROM: u64 = 16 << 20;

/// Below this an entry's assembled zip fits the 32-bit fields of its header, with
/// room for what deflate and AE-2 add to a stream that does not compress.
pub(super) const CHUNKED_BELOW: u64 = (4 << 30) - (64 << 20);

/// Whether an entry of `len` bytes is compressed in chunks at `level`, with
/// chunks of `from` bytes and up: compressed at all (level 0 stores), big enough
/// to gain, small enough for this writer.
pub(super) fn chunked(len: u64, level: i64, from: u64) -> bool {
    level > 0 && len >= from && len < CHUNKED_BELOW
}

/// Raw deflate of one chunk at `level`. `last` ends the stream; any other chunk
/// ends with a sync flush, so the next chunk's deflate continues the stream.
pub(super) fn deflate_chunk(data: &[u8], level: u32, last: bool) -> io::Result<Vec<u8>> {
    let mut compress = Compress::new(Compression::new(level), false);
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    let flush = if last {
        FlushCompress::Finish
    } else {
        FlushCompress::Sync
    };
    let mut input = data;
    loop {
        if out.capacity() - out.len() < 1024 {
            out.reserve(out.capacity().max(64 * 1024));
        }
        let before = compress.total_in();
        let status = compress
            .compress_vec(input, &mut out, flush)
            .map_err(io::Error::other)?;
        let consumed = usize::try_from(compress.total_in() - before).map_err(io::Error::other)?;
        input = &input[consumed..];
        let room_left = out.capacity() > out.len();
        match status {
            Status::StreamEnd => break,
            // The flush is complete once the input is in and the output had room
            // to spare: deflate stops writing only when it has nothing to write.
            Status::Ok | Status::BufError if !last && input.is_empty() && room_left => break,
            _ => {}
        }
    }
    Ok(out)
}

/// WinZip AE-2 around a stream written in pieces: CTR keeps its place in the
/// keystream across calls, and the HMAC takes the ciphertext as it goes.
struct Ae2 {
    cipher: aes::Aes256,
    mac: SimpleHmacReset<Sha1>,
    counter: u128,
    keystream: [u8; 16],
    used: usize,
}

impl Ae2 {
    /// Keys for `password` under a fresh salt, and the 18 bytes that open the
    /// entry's body: the salt and the password verifier.
    fn new(password: &str) -> io::Result<(Self, [u8; 18])> {
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(io::Error::other)?;
        let mut keys = Zeroizing::new([0u8; 66]);
        pbkdf2::pbkdf2::<SimpleHmacReset<Sha1>>(password.as_bytes(), &salt, 1000, &mut keys[..])
            .map_err(io::Error::other)?;
        let cipher = aes::Aes256::new_from_slice(&keys[..32]).map_err(io::Error::other)?;
        let mac =
            SimpleHmacReset::<Sha1>::new_from_slice(&keys[32..64]).map_err(io::Error::other)?;
        let mut head = [0u8; 18];
        head[..16].copy_from_slice(&salt);
        head[16..].copy_from_slice(&keys[64..66]);
        let ae2 = Self {
            cipher,
            mac,
            counter: 1,
            keystream: [0; 16],
            used: 16,
        };
        Ok((ae2, head))
    }

    /// Encrypts `data` in place, and takes the ciphertext into the HMAC.
    fn apply(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            if self.used == 16 {
                let mut block = aes::cipher::Block::<aes::Aes256>::from(self.counter.to_le_bytes());
                self.cipher.encrypt_block(&mut block);
                self.keystream.copy_from_slice(&block);
                self.counter += 1;
                self.used = 0;
            }
            *byte ^= self.keystream[self.used];
            self.used += 1;
        }
        self.mac.update(data);
    }

    /// The authentication code that closes the body.
    fn tag(self) -> [u8; 10] {
        let mut tag = [0u8; 10];
        tag.copy_from_slice(&self.mac.finalize().into_bytes()[..10]);
        tag
    }
}

/// One chunk of a big entry, compressed, as it lands.
pub(super) struct Chunk {
    pub(super) index: usize,
    pub(super) deflated: Vec<u8>,
    pub(super) crc: u32,
    pub(super) len: u64,
}

/// A big entry assembled from its chunks, in order, into a zip of one entry in a
/// scratch file — encrypted on the way when there is a password.
pub(super) struct Assembly {
    name: String,
    // Declared before `scratch`, so it is dropped first: the file is closed
    // before an abandoned assembly's scratch file is removed (Windows refuses to
    // remove a file still open).
    out: BufWriter<File>,
    scratch: ScratchFile,
    chunks: usize,
    next: usize,
    pending: BTreeMap<usize, Chunk>,
    crc: crc32fast::Hasher,
    uncompressed: u64,
    body: u64,
    ae2: Option<Ae2>,
    encrypted: bool,
    dos: (u16, u16),
}

/// The local header's fixed fields, up to the name: 30 bytes.
const LOCAL_HEADER: u64 = 30;

impl Assembly {
    /// Starts the entry `name` of `chunks` chunks in `scratch`: the local header,
    /// sizes and CRC to be filled in, and with a password the AE-2 opening.
    pub(super) fn new(
        name: &str,
        chunks: usize,
        dos: (u16, u16),
        password: Option<&str>,
        scratch: ScratchFile,
    ) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(scratch.path())?);
        let encrypted = password.is_some();
        out.write_all(&local_header(name, encrypted, dos, 0, 0, 0))?;
        let mut body = 0;
        let ae2 = match password {
            Some(pw) => {
                let (ae2, head) = Ae2::new(pw)?;
                out.write_all(&head)?;
                body += head.len() as u64;
                Some(ae2)
            }
            None => None,
        };
        Ok(Self {
            name: name.to_string(),
            out,
            scratch,
            chunks,
            next: 0,
            pending: BTreeMap::new(),
            crc: crc32fast::Hasher::new(),
            uncompressed: 0,
            body,
            ae2,
            encrypted,
            dos,
        })
    }

    /// The entry's name in the archive.
    pub(super) fn name(&self) -> &str {
        &self.name
    }

    /// Takes in a chunk, writes every chunk now in order, and says whether the
    /// entry is complete.
    pub(super) fn land(&mut self, chunk: Chunk) -> io::Result<bool> {
        self.pending.insert(chunk.index, chunk);
        while let Some(mut chunk) = self.pending.remove(&self.next) {
            if let Some(ae2) = &mut self.ae2 {
                ae2.apply(&mut chunk.deflated);
            }
            self.out.write_all(&chunk.deflated)?;
            self.body += chunk.deflated.len() as u64;
            self.crc.combine(&crc32fast::Hasher::new_with_initial_len(
                chunk.crc, chunk.len,
            ));
            self.uncompressed += chunk.len;
            self.next += 1;
        }
        Ok(self.next == self.chunks)
    }

    /// Closes the entry — the AE-2 tag, the sizes and CRC in the local header, the
    /// central directory and its end — and hands over the scratch file.
    pub(super) fn finish(mut self) -> io::Result<ScratchFile> {
        let encrypted = self.encrypted;
        if let Some(ae2) = self.ae2.take() {
            let tag = ae2.tag();
            self.out.write_all(&tag)?;
            self.body += tag.len() as u64;
        }
        // AE-2 carries no CRC: the HMAC is the check.
        let compressed = u32::try_from(self.body).map_err(io::Error::other)?;
        let uncompressed = u32::try_from(self.uncompressed).map_err(io::Error::other)?;
        let crc = if encrypted {
            0
        } else {
            self.crc.clone().finalize()
        };
        let mut file = self.out.into_inner().map_err(io::Error::other)?;
        let central_at = file.stream_position()?;
        // The local header again, now with what it could not know at the start.
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&local_header(
            &self.name,
            encrypted,
            self.dos,
            crc,
            compressed,
            uncompressed,
        ))?;
        file.seek(SeekFrom::Start(central_at))?;
        let central = central_header(
            &self.name,
            encrypted,
            self.dos,
            crc,
            compressed,
            uncompressed,
        );
        file.write_all(&central)?;
        file.write_all(&end_of_directory(central.len(), central_at)?)?;
        file.flush()?;
        drop(file);
        Ok(self.scratch)
    }
}

/// The 0x9901 field: AE-2, AES-256, the actual method deflate.
const AES_FIELD: [u8; 11] = [0x01, 0x99, 7, 0, 2, 0, b'A', b'E', 3, 8, 0];

/// The fields a local and a central header share, from "version needed" on.
fn common(
    encrypted: bool,
    dos: (u16, u16),
    crc: u32,
    compressed: u32,
    uncompressed: u32,
    name: &str,
) -> Vec<u8> {
    let (version, flags, method): (u16, u16, u16) = if encrypted {
        (51, 0x0801, 99)
    } else {
        (20, 0x0800, 8)
    };
    let mut h = Vec::with_capacity(32);
    h.extend_from_slice(&version.to_le_bytes());
    h.extend_from_slice(&flags.to_le_bytes());
    h.extend_from_slice(&method.to_le_bytes());
    h.extend_from_slice(&dos.0.to_le_bytes());
    h.extend_from_slice(&dos.1.to_le_bytes());
    h.extend_from_slice(&crc.to_le_bytes());
    h.extend_from_slice(&compressed.to_le_bytes());
    h.extend_from_slice(&uncompressed.to_le_bytes());
    h.extend_from_slice(&(name.len() as u16).to_le_bytes());
    let extra: u16 = if encrypted { AES_FIELD.len() as u16 } else { 0 };
    h.extend_from_slice(&extra.to_le_bytes());
    h
}

fn local_header(
    name: &str,
    encrypted: bool,
    dos: (u16, u16),
    crc: u32,
    compressed: u32,
    uncompressed: u32,
) -> Vec<u8> {
    let mut h = 0x0403_4b50u32.to_le_bytes().to_vec();
    h.extend(common(encrypted, dos, crc, compressed, uncompressed, name));
    h.extend_from_slice(name.as_bytes());
    if encrypted {
        h.extend_from_slice(&AES_FIELD);
    }
    debug_assert_eq!(
        h.len() as u64,
        LOCAL_HEADER + name.len() as u64 + if encrypted { 11 } else { 0 }
    );
    h
}

fn central_header(
    name: &str,
    encrypted: bool,
    dos: (u16, u16),
    crc: u32,
    compressed: u32,
    uncompressed: u32,
) -> Vec<u8> {
    let mut h = 0x0201_4b50u32.to_le_bytes().to_vec();
    // Made by: Unix, spec 6.3 — the attributes below are a Unix mode.
    h.extend_from_slice(&0x033fu16.to_le_bytes());
    h.extend(common(encrypted, dos, crc, compressed, uncompressed, name));
    h.extend_from_slice(&0u16.to_le_bytes()); // comment length
    h.extend_from_slice(&0u16.to_le_bytes()); // disk number
    h.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
    h.extend_from_slice(&(0o100_644u32 << 16).to_le_bytes()); // -rw-r--r--
    h.extend_from_slice(&0u32.to_le_bytes()); // the local header's offset
    h.extend_from_slice(name.as_bytes());
    if encrypted {
        h.extend_from_slice(&AES_FIELD);
    }
    h
}

fn end_of_directory(central_len: usize, central_at: u64) -> io::Result<Vec<u8>> {
    let mut e = 0x0605_4b50u32.to_le_bytes().to_vec();
    e.extend_from_slice(&[0, 0, 0, 0]); // this disk, the directory's disk
    e.extend_from_slice(&1u16.to_le_bytes());
    e.extend_from_slice(&1u16.to_le_bytes());
    e.extend_from_slice(
        &u32::try_from(central_len)
            .map_err(io::Error::other)?
            .to_le_bytes(),
    );
    e.extend_from_slice(
        &u32::try_from(central_at)
            .map_err(io::Error::other)?
            .to_le_bytes(),
    );
    e.extend_from_slice(&0u16.to_le_bytes()); // comment length
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Data that is half text and half noise, as a database is.
    fn sample(len: usize) -> Vec<u8> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        (0..len)
            .map(|i| {
                if (i / 4096) % 2 == 0 {
                    b"the quick brown fox jumps over the lazy dog "[i % 44]
                } else {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                }
            })
            .collect()
    }

    /// The chunks of `data`, compressed apart, in the order `order` lands them.
    fn chunks_of(data: &[u8], size: usize, order: &[usize]) -> Vec<Chunk> {
        let pieces: Vec<&[u8]> = data.chunks(size).collect();
        order
            .iter()
            .map(|&index| Chunk {
                index,
                deflated: deflate_chunk(pieces[index], 6, index + 1 == pieces.len()).unwrap(),
                crc: crc32fast::hash(pieces[index]),
                len: pieces[index].len() as u64,
            })
            .collect()
    }

    /// The entry an assembly wrote, read back by the `zip` crate — its CRC check
    /// for a plain entry, its HMAC check for an encrypted one.
    fn read_back(scratch: &ScratchFile, password: Option<&str>) -> zip::result::ZipResult<Vec<u8>> {
        let mut zip = zip::ZipArchive::new(File::open(scratch.path()).unwrap()).unwrap();
        let mut entry = match password {
            Some(pw) => zip.by_index_decrypt(0, pw.as_bytes())?,
            None => zip.by_index(0)?,
        };
        let mut out = Vec::new();
        entry.read_to_end(&mut out)?;
        Ok(out)
    }

    fn assemble(dir: &std::path::Path, data: &[u8], password: Option<&str>) -> ScratchFile {
        let order = [2, 0, 3, 1, 4];
        let mut assembly = Assembly::new(
            "data.db",
            order.len(),
            (0, 0x5b69),
            password,
            ScratchFile(dir.join("one.zip")),
        )
        .unwrap();
        let landed: Vec<bool> = chunks_of(data, data.len().div_ceil(order.len()), &order)
            .into_iter()
            .map(|chunk| assembly.land(chunk).unwrap())
            .collect();
        assert_eq!(
            landed,
            [false, false, false, false, true],
            "complete with the last one"
        );
        assembly.finish().unwrap()
    }

    /// Chunks compressed apart, joined in order, are one deflate stream that any
    /// inflater reads back whole — the sync flush keeps it open, the last chunk ends it.
    #[test]
    fn chunks_compressed_apart_are_one_stream() {
        let data = sample(300_000);
        let mut stream = Vec::new();
        for chunk in chunks_of(&data, 64 * 1024, &[0, 1, 2, 3, 4]) {
            stream.extend(chunk.deflated);
        }
        let mut back = Vec::new();
        flate2::read::DeflateDecoder::new(&stream[..])
            .read_to_end(&mut back)
            .unwrap();
        assert_eq!(back, data);
        assert!(stream.len() < data.len(), "the text half compresses");
    }

    /// An entry assembled from chunks that landed out of order reads back whole,
    /// its CRC checked by the `zip` crate — and its name and date are the ones given.
    #[test]
    fn a_plain_assembled_entry_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let data = sample(200_000);
        let scratch = assemble(dir.path(), &data, None);
        assert_eq!(read_back(&scratch, None).unwrap(), data);
        let mut zip = zip::ZipArchive::new(File::open(scratch.path()).unwrap()).unwrap();
        let entry = zip.by_index_raw(0).unwrap();
        assert_eq!(entry.name().unwrap(), "data.db");
        assert_eq!(entry.last_modified().unwrap().datepart(), 0x5b69);
        assert!(!entry.encrypted());
    }

    /// With a password the entry is WinZip AE-2: it reads back with it — the HMAC
    /// checked by the `zip` crate —, a wrong one is refused, a flipped byte of the
    /// ciphertext is caught, and the plaintext is nowhere in the file.
    #[test]
    fn an_encrypted_assembled_entry_is_ae2() {
        let dir = tempfile::tempdir().unwrap();
        let data = sample(200_000);
        let scratch = assemble(dir.path(), &data, Some("correct horse"));
        assert_eq!(read_back(&scratch, Some("correct horse")).unwrap(), data);
        assert!(read_back(&scratch, Some("wrong horse")).is_err());
        let bytes = fs_read(scratch.path());
        assert!(
            !bytes.windows(20).any(|w| w == &data[..20]),
            "plaintext in the file"
        );

        let mut flipped = bytes.clone();
        let at = flipped.len() / 2;
        flipped[at] ^= 0x40;
        std::fs::write(scratch.path(), &flipped).unwrap();
        assert!(
            read_back(&scratch, Some("correct horse")).is_err(),
            "a flipped byte read as good"
        );
    }

    fn fs_read(path: &std::path::Path) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }

    /// What is chunked: compressed at all, big enough, small enough for this writer.
    #[test]
    fn only_a_big_compressed_entry_is_chunked() {
        assert!(chunked(CHUNKED_FROM, 6, CHUNKED_FROM));
        assert!(!chunked(CHUNKED_FROM - 1, 6, CHUNKED_FROM));
        assert!(!chunked(CHUNKED_FROM, 0, CHUNKED_FROM), "stored");
        assert!(!chunked(CHUNKED_BELOW, 6, CHUNKED_FROM), "4 GiB and up");
    }
}
