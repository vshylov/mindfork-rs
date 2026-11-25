//! The dates an archive's entries carry (spec §12.3).
//!
//! A zip entry keeps its date in two places. The **DOS date-time** in its header
//! is what every reader shows: local time, in steps of two seconds, 1980–2107.
//! The **extended timestamp** extra field (`UT`, 0x5455) is Unix seconds in UTC,
//! which 7-Zip and Info-ZIP read first and this module reads first too — a date
//! that means the same instant on the machine it is restored on, whatever its
//! time zone.
//!
//! Until 0.24.0 every entry carried neither: `zip` is built without a time
//! feature, so its default date — 1980-01-01 00:00 — was written, and 7-Zip showed
//! it. Now an entry is written with its file's modification time in both places,
//! and a restore gives the file that time back. The 1980 default reads as **no
//! date**, so a file from an older archive is dated when it is restored, as it
//! always was.
//!
//! **A backup's entries reach the archive through a raw copy, and keep both.**
//! The pool appends each entry with `raw_copy_file`. Up to `zip` 9.5 that copy
//! dropped the `UT` field (`ExtraField::write` serialized none), so a backup's
//! entries carried the DOS date alone — local time, rounded down to an even
//! second — and a file restored in another time zone was off by the difference.
//! Reported upstream, <https://github.com/zip-rs/zip2/issues/1285>, and fixed in
//! 9.6.0–9.7.1; `zip` is required at 9.8.2 since, and
//! `a_raw_copy_keeps_the_extended_timestamp` holds it. A copy keeps the
//! modification time alone, which is all this module writes.
//!
//! The big entry's headers are written by hand (`chunked`), so they take the field
//! from [`header_date`]: both forms of it are the same nine bytes when only the
//! modification time is set.

use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Timelike};
use zip::extra_fields::ExtraField;
use zip::read::ZipFile;
use zip::write::{FileOptions, FullFileOptions};

/// The extended timestamp's header id.
const EXTENDED_TIMESTAMP: u16 = 0x5455;

/// The extended timestamp's flag for "a modification time follows".
const MODIFIED: u8 = 0x01;

/// `options` dated `mtime`: the DOS date-time in local time and the `UT` field.
/// `None` — no date is known, and the format's default stands. A time the DOS
/// field cannot hold (before 1980, after 2107) keeps its default there; one the
/// `UT` field cannot hold (past 2038 — the field is a signed 32-bit count) goes
/// without it.
pub(crate) fn dated<'k>(
    options: FileOptions<'k, 'static, ()>,
    mtime: Option<SystemTime>,
) -> FullFileOptions<'k, 'static> {
    let Some(mtime) = mtime else {
        return options.into_full_options();
    };
    let options = match dos_time(mtime) {
        Some(dos) => options.last_modified_time(dos),
        None => options,
    };
    let mut options = options.into_full_options();
    if let Some(seconds) = unix_seconds(mtime) {
        let field = extended_timestamp_data(seconds).to_vec();
        if let Err(e) = options.add_extra_field(EXTENDED_TIMESTAMP, field, false) {
            // The field is well-formed by construction; were it refused, the
            // DOS date still dates the entry.
            tracing::warn!(error = %e, "backup: the extended timestamp was refused");
        }
    }
    options
}

/// When the entry's file was last modified, as the archive says it: the `UT`
/// field when there is one, else the DOS date read as local time — unless it is
/// the format's default, which every archive before dates were written carries
/// and which says "no date".
pub(crate) fn entry_mtime<R: Read>(entry: &ZipFile<'_, R>) -> Option<SystemTime> {
    let extended = entry.extra_data_fields().find_map(|field| match field {
        ExtraField::ExtendedTimestamp(ts) => ts.mod_time(),
        _ => None,
    });
    if let Some(seconds) = extended {
        return Some(UNIX_EPOCH + Duration::from_secs(u64::from(seconds)));
    }
    let dos = entry.last_modified()?;
    if dos == zip::DateTime::default() {
        return None;
    }
    let local = chrono::NaiveDate::from_ymd_opt(
        i32::from(dos.year()),
        u32::from(dos.month()),
        u32::from(dos.day()),
    )?
    .and_hms_opt(
        u32::from(dos.hour()),
        u32::from(dos.minute()),
        u32::from(dos.second()),
    )?
    .and_local_timezone(chrono::Local)
    .earliest()?;
    Some(local.into())
}

/// The date a header written by hand carries — the big entry's (`chunked`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeaderDate {
    /// The DOS (time, date) fields: local time, or the format's default when the
    /// date is not known or out of range.
    pub(crate) dos: (u16, u16),
    /// The `UT` extra field whole — its id, its size, the flag and the seconds.
    /// It holds the modification time alone, so the local header's form and the
    /// central one's are the same bytes. `None` when the date is not known or out
    /// of the field's range.
    pub(crate) extended: Option<[u8; 9]>,
}

/// `mtime` as the date fields of a header written by hand.
pub(crate) fn header_date(mtime: Option<SystemTime>) -> HeaderDate {
    let dos = mtime.and_then(dos_time).unwrap_or_default();
    let extended = mtime.and_then(unix_seconds).map(|seconds| {
        let mut field = [0u8; 9];
        field[..2].copy_from_slice(&EXTENDED_TIMESTAMP.to_le_bytes());
        field[2..4].copy_from_slice(&5u16.to_le_bytes());
        field[4..].copy_from_slice(&extended_timestamp_data(seconds));
        field
    });
    HeaderDate {
        dos: (dos.timepart(), dos.datepart()),
        extended,
    }
}

/// The `UT` field's data for a modification time alone: the flag, the seconds.
fn extended_timestamp_data(seconds: u32) -> [u8; 5] {
    let mut data = [MODIFIED, 0, 0, 0, 0];
    data[1..].copy_from_slice(&seconds.to_le_bytes());
    data
}

/// `mtime` as a DOS date-time in local time; `None` outside 1980–2107.
fn dos_time(mtime: SystemTime) -> Option<zip::DateTime> {
    let local: chrono::DateTime<chrono::Local> = mtime.into();
    zip::DateTime::from_date_and_time(
        u16::try_from(local.year()).ok()?,
        u8::try_from(local.month()).ok()?,
        u8::try_from(local.day()).ok()?,
        u8::try_from(local.hour()).ok()?,
        u8::try_from(local.minute()).ok()?,
        u8::try_from(local.second()).ok()?,
    )
    .ok()
}

/// `mtime` as the `UT` field's seconds; `None` before 1970 or past 2038.
fn unix_seconds(mtime: SystemTime) -> Option<u32> {
    let seconds = mtime.duration_since(UNIX_EPOCH).ok()?.as_secs();
    u32::try_from(seconds)
        .ok()
        .filter(|&s| s <= i32::MAX.unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::{ZipArchive, ZipWriter};

    /// A one-entry archive written with `options`, read back as its entry's date.
    fn round_trip(options: FullFileOptions<'_, 'static>) -> Option<SystemTime> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("a.json", options).unwrap();
        zip.write_all(b"{}").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let entry = zip.by_index(0).unwrap();
        entry_mtime(&entry)
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// A date written is the date read, to the second — through the `UT` field —
    /// and the DOS field holds the same instant in local time, which is what a
    /// reader that knows no `UT` field shows.
    #[test]
    fn a_dated_entry_reads_back_its_date() {
        let mtime = at(1_715_000_000); // 2024-05-06
        let options = dated(SimpleFileOptions::default(), Some(mtime));
        assert_eq!(round_trip(options.clone()), Some(mtime));
        let local: chrono::DateTime<chrono::Local> = mtime.into();
        let dos = dos_time(mtime).unwrap();
        assert_eq!(
            (dos.year(), dos.month(), dos.day(), dos.hour(), dos.minute()),
            (
                u16::try_from(local.year()).unwrap(),
                u8::try_from(local.month()).unwrap(),
                u8::try_from(local.day()).unwrap(),
                u8::try_from(local.hour()).unwrap(),
                u8::try_from(local.minute()).unwrap(),
            )
        );
    }

    /// No date known — the format's default, which reads back as no date. That is
    /// every entry of an archive written before dates were: its files are dated
    /// when they are restored, not set to 1980.
    #[test]
    fn the_default_date_reads_as_none() {
        assert_eq!(round_trip(dated(SimpleFileOptions::default(), None)), None);
    }

    /// An archive with a DOS date and no `UT` field (another tool's) reads the DOS
    /// date as local time.
    #[test]
    fn a_dos_date_alone_reads_as_local_time() {
        let dos = zip::DateTime::from_date_and_time(2023, 3, 4, 5, 6, 8).unwrap();
        let options = SimpleFileOptions::default()
            .last_modified_time(dos)
            .into_full_options();
        let read: chrono::DateTime<chrono::Local> = round_trip(options).unwrap().into();
        assert_eq!(
            (
                read.year(),
                read.month(),
                read.day(),
                read.hour(),
                read.minute(),
                read.second()
            ),
            (2023, 3, 4, 5, 6, 8)
        );
    }

    /// What `zip` does to an entry the backup pool appends with `raw_copy_file`:
    /// the `UT` field is kept, so an odd second comes back as itself — before
    /// 9.6 the field was dropped and the DOS date gave back the even second below
    /// (<https://github.com/zip-rs/zip2/issues/1285>). Plain and encrypted.
    #[test]
    fn a_raw_copy_keeps_the_extended_timestamp() {
        for password in [None, Some("pw")] {
            let mtime = at(1_715_000_001); // an odd second
            let mut options = dated(SimpleFileOptions::default(), Some(mtime));
            if let Some(pw) = password {
                options = options.with_aes_encryption(zip::AesMode::Aes256, pw);
            }
            let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
            zip.start_file("a.json", options).unwrap();
            zip.write_all(b"{}").unwrap();
            let one = zip.finish().unwrap().into_inner();
            let mut one = ZipArchive::new(Cursor::new(one)).unwrap();

            let mut copy = ZipWriter::new(Cursor::new(Vec::new()));
            copy.raw_copy_file(one.by_index_raw(0).unwrap()).unwrap();
            let copy = copy.finish().unwrap().into_inner();
            let mut copy = ZipArchive::new(Cursor::new(copy)).unwrap();
            let entry = copy.by_index_raw(0).unwrap();
            let fields = entry
                .extra_data_fields()
                .filter(|f| matches!(f, ExtraField::ExtendedTimestamp(_)))
                .count();
            assert_eq!(fields, 1, "{password:?}: one UT field, not none or two");
            assert_eq!(entry_mtime(&entry), Some(mtime), "{password:?}");
        }
    }

    /// The field a header written by hand carries is the one `dated` has `zip`
    /// write: `zip` reads the time back from it, in a local and a central header
    /// alike; the DOS fields are `dated`'s too. No date — neither field.
    #[test]
    fn a_header_date_is_the_field_zip_writes() {
        let mtime = at(1_715_000_001);
        let date = header_date(Some(mtime));
        let field = date.extended.expect("a date the field holds");
        assert_eq!(&field[..4], &[0x55, 0x54, 5, 0]);
        assert_eq!(field[4..], extended_timestamp_data(1_715_000_001));
        let dos = dos_time(mtime).unwrap();
        assert_eq!(date.dos, (dos.timepart(), dos.datepart()));

        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("a.json", dated(SimpleFileOptions::default(), Some(mtime)))
            .unwrap();
        zip.write_all(b"{}").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let found = bytes.windows(field.len()).filter(|w| *w == field).count();
        assert_eq!(found, 2, "zip writes the same bytes, local and central");

        assert_eq!(
            header_date(None),
            HeaderDate {
                dos: (0, 0x21),
                extended: None
            }
        );
        assert_eq!(header_date(Some(at(2_600_000_000))).extended, None);
    }

    /// A backup written by 0.25.0–0.28.1, whose entries lost the field in the raw
    /// copy, still restores its dates from the DOS field: an odd second as the
    /// even one below it.
    #[test]
    fn an_entry_without_the_field_reads_the_dos_date() {
        let dos = dos_time(at(1_715_000_001)).unwrap(); // an odd second
        let options = SimpleFileOptions::default()
            .last_modified_time(dos)
            .into_full_options();
        assert_eq!(round_trip(options), Some(at(1_715_000_000)));
    }

    /// Past 2038 the `UT` field cannot hold the time and is left out; the DOS date
    /// still dates the entry (to its two-second step). Before 1980 it is the other
    /// way round: the DOS field keeps its default and the `UT` field dates it.
    #[test]
    fn a_date_one_field_cannot_hold_is_kept_by_the_other() {
        let late = at(2_600_000_000); // 2052, an even second
        assert_eq!(unix_seconds(late), None);
        assert_eq!(
            round_trip(dated(SimpleFileOptions::default(), Some(late))),
            Some(late)
        );

        let early = at(100_000_000); // 1973
        assert_eq!(dos_time(early), None);
        assert_eq!(
            round_trip(dated(SimpleFileOptions::default(), Some(early))),
            Some(early)
        );
    }
}
