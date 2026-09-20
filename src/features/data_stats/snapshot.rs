//! Reading **the other copy** for `mindfork stats --compare` (docs/data-stats.md
//! G2, G3): a `--json` snapshot, or a backup archive — told apart by the file's
//! first bytes, never by its name.
//!
//! A snapshot is a [`DataStats`] written out, so reading one back is a
//! deserialization — guarded by the `format` field first, because a snapshot
//! that predates the message ids would deserialize happily into empty id lists
//! and compare as "this copy has everything": the false comfort the whole
//! comparison exists to prevent.

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{DataStats, SNAPSHOT_FORMAT};
use crate::shared::i18n::Locale;
use crate::shared::text_decode;

/// The first format a comparison can work from (the one that added the ids).
const FIRST_COMPARABLE_FORMAT: u32 = 2;

/// What kind of file `--compare` was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtherCopy {
    /// A zip — read as a backup archive (`collect_archive`).
    Archive,
    /// Anything else — read as a snapshot ([`read_snapshot`]), which is where
    /// a file that is neither gets its refusal.
    Snapshot,
}

/// Which of the two `path` is. `Err` — it cannot be opened.
pub fn other_copy(path: &Path, loc: &Locale) -> Result<OtherCopy> {
    let mut head = [0u8; 2];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut head))
        .with_context(|| open_context(path, loc))?;
    // Every zip — an empty one included — starts with `PK`.
    Ok(if read == head.len() && &head == b"PK" {
        OtherCopy::Archive
    } else {
        OtherCopy::Snapshot
    })
}

fn open_context(path: &Path, loc: &Locale) -> String {
    loc.tf(
        "cli.stats.ctx.open_other",
        &[("path", &path.display().to_string())],
    )
}

/// Just enough of a snapshot to decide whether to read the rest.
#[derive(Deserialize)]
struct Probe {
    format: u32,
}

/// Reads a `--json` snapshot. `Err` (already localized, each naming the way
/// out): not a snapshot at all, one made before the comparison existed, or one
/// made by a newer version.
///
/// The text is decoded by its byte-order mark first: Windows PowerShell's `>`
/// writes UTF-16, and `stats --json > this-pc.json` is the command the help
/// shows.
pub fn read_snapshot(path: &Path, loc: &Locale) -> Result<DataStats> {
    let name = path.display().to_string();
    let bytes = fs::read(path).with_context(|| open_context(path, loc))?;
    let not_a_snapshot = || loc.tf("cli.stats.err.not_a_snapshot", &[("path", &name)]);
    let Some(decoded) = text_decode::decode_file(&bytes, false, None) else {
        bail!("{}", not_a_snapshot());
    };
    let text = decoded.text.trim_start_matches('\u{feff}');

    let Ok(probe) = serde_json::from_str::<Probe>(text) else {
        bail!("{}", not_a_snapshot());
    };
    let format = probe.format.to_string();
    if probe.format < FIRST_COMPARABLE_FORMAT {
        bail!(
            "{}",
            loc.tf(
                "cli.stats.err.snapshot_old",
                &[("path", &name), ("format", &format)]
            )
        );
    }
    if probe.format > SNAPSHOT_FORMAT {
        bail!(
            "{}",
            loc.tf(
                "cli.stats.err.snapshot_new",
                &[("path", &name), ("format", &format)]
            )
        );
    }
    serde_json::from_str(text).with_context(not_a_snapshot)
}
