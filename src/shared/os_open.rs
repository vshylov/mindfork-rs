//! Opening one of the chat's files — or its folder — in the user's desktop
//! (fork F9 of docs/history/sandbox-file-exchange.md, sub-decisions §13 U3–U6; spec §9.7).
//!
//! Three platform calls and one policy. The policy is [`is_document`], and it is the
//! reason this module is not simply "hand the path to the shell": the chat's folder is
//! written by `python_exec`, so a name in it is a name the *model* chose, and a `run.bat`,
//! a `.lnk` or a scripted `.html` would **run** under its default handler. Only the
//! document types open directly; [`decide`] sends everything else to the folder it sits
//! in, which is one double-click away from the file and runs nothing.
//!
//! The launch is `ShellExecuteW` on Windows, `xdg-open` on Linux and `open` on macOS:
//! one argument, no shell. Not `cmd /c start`, which re-parses its argument outside
//! Rust's own escaping (docs/lessons.md §6), and not a crate for three calls.
//!
//! **Blocking** (§13 U5): the shell starts the handler before returning and `xdg-open`
//! is a script that execs another, so callers run [`open`] on the blocking pool rather
//! than on the orchestrator's command loop.
//!
//! **The mark** (§13 U11): [`FROM_ELSEWHERE`] is what a file a call wrote carries on
//! Windows, set where it is stored (`features::chat_files`). The allowlist says which types
//! may be handed to a handler; the mark is what that handler is told about the file.

use std::ffi::OsStr;
use std::io;
use std::path::Path;

/// The platforms the launch differs on. A parameter rather than a `cfg!` chain so the
/// mapping is testable on every host (§13 U4) — the spawn itself is the stage's manual
/// gate, but which program is spawned is not left untested.
///
/// Only the non-Windows [`launch`] reads this mapping (Windows opens through the shell,
/// which is what its `None` says), so on a Windows build it is used by the tests alone.
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    Mac,
}

/// The platform this build runs on (tested everywhere, read by [`launch`] off Windows).
#[cfg_attr(windows, allow(dead_code))]
pub const HERE: Platform = if cfg!(windows) {
    Platform::Windows
} else if cfg!(target_os = "macos") {
    Platform::Mac
} else {
    Platform::Linux
};

/// The program that opens a path on `platform` — `None` on Windows, which has no argv
/// launcher of its own: the shell opens the path itself (`ShellExecuteW`).
#[cfg_attr(windows, allow(dead_code))]
pub fn launcher(platform: Platform) -> Option<&'static str> {
    match platform {
        Platform::Windows => None,
        Platform::Linux => Some("xdg-open"),
        Platform::Mac => Some("open"),
    }
}

/// The file types a handler may open directly (§13 U3).
///
/// What is **deliberately absent** is the point of the list: `svg` and `html` — both are
/// shapes a `python_exec` call writes and both are scripted documents a browser executes;
/// the macro-enabled `docm`/`xlsm`; and every executable shape (`bat`, `cmd`, `ps1`,
/// `sh`, `lnk`, `exe`). Those open their folder instead, with the reason said out loud.
pub const DOCUMENTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "pdf", "csv", "tsv", "txt", "md",
    "json", "xlsx", "docx",
];

/// Whether a file name's type may be handed to a handler. The **last** extension decides
/// — `report.pdf.bat` is a batch file — and the comparison is case-insensitive, as both
/// supported platforms' shells are about extensions.
pub fn is_document(name: &str) -> bool {
    let Some(ext) = Path::new(name).extension().and_then(OsStr::to_str) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    DOCUMENTS.contains(&ext.as_str())
}

/// What opens for a file, and the path to hand to [`open`]: the file itself when its type
/// is a document, otherwise the folder it sits in (§13 U3).
///
/// `None` when the type is not a document and the path names no folder — a bare name.
/// This used to fall back onto the path itself, which is exactly the file the allowlist
/// had just refused: `run.bat` with no folder was handed to the shell as `run.bat`. Every
/// caller passes an absolute path today, so the case does not arise; the signature is
/// what keeps it from arising later.
pub fn decide(path: &Path) -> Option<(Opens, &Path)> {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if is_document(name) {
        Some((Opens::File, path))
    } else {
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|dir| (Opens::Folder, dir))
    }
}

/// Which of the two [`decide`] chose — the note tells the user which happened, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opens {
    File,
    Folder,
}

/// Opens a file or a directory in the desktop environment. Blocking; the caller is
/// responsible for the path existing (the refusals are worded where the handle is
/// resolved) and for the type being one [`decide`] allowed.
pub fn open(path: &Path) -> io::Result<()> {
    launch(path)
}

/// What Windows writes beside a file that came from elsewhere — a download, a mail
/// attachment — as its `Zone.Identifier` stream: zone 3, the Internet (§13 U11).
///
/// A file a call wrote carries it. Office opens a marked workbook or document in Protected
/// View, where a model-written formula reaches no DDE server, external links are not
/// followed and a document's remote template is not fetched — whether the file was opened
/// by `/file open` or double-clicked in the folder; without it the chat's folder is a
/// local, trusted place to Office. **Not a CSV**: Excel puts a marked `.csv` in Protected
/// View only when "Always open untrusted Text-Based files in Protected View" is on, which
/// it is not by default, and `.tsv` is not a type that setting names — for those the
/// defence is Excel's own, DDE server launch being off by default in Microsoft 365. No
/// `HostUrl`: there is no address to name, and an invented one would be a lie.
pub const FROM_ELSEWHERE: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";

/// The `Zone.Identifier` stream a file carries, if it carries one. `None` off Windows,
/// which has no such stream, and for a file on a volume that holds no alternate streams.
pub fn zone_of(path: &Path) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        std::fs::read(zone_stream(path))
            .ok()
            .filter(|zone| !zone.is_empty())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// Writes `zone` as the file's `Zone.Identifier` stream, replacing one it had.
///
/// Does nothing off Windows: no handler there reads a mark, and there is none to write. On
/// Windows an error is a file that is not there — refused rather than written, because a
/// stream opened for writing creates the file it belongs to — or a volume that cannot hold
/// a stream (FAT, exFAT, some network shares). The file's bytes are untouched either way,
/// so a caller keeps the file and says so in the log.
pub fn set_zone(path: &Path, zone: &[u8]) -> io::Result<()> {
    #[cfg(windows)]
    {
        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "there is no file to mark",
            ));
        }
        std::fs::write(zone_stream(path), zone)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, zone);
        Ok(())
    }
}

/// `path:Zone.Identifier` — a stream is named by its file's name with its own appended.
#[cfg(windows)]
fn zone_stream(path: &Path) -> std::path::PathBuf {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    stream.into()
}

#[cfg(windows)]
fn launch(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let file: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // "open" — the default verb spelled out, so a type whose default verb is `edit` or
    // `runas` still opens for viewing.
    let verb: [u16; 5] = [b'o' as u16, b'p' as u16, b'e' as u16, b'n' as u16, 0];
    // SAFETY: both strings are NUL-terminated and live until the call returns; the
    // remaining arguments are the documented nulls (no parent window, no parameters, no
    // working directory) and a constant show command.
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Documented: a value **at or below 32** is an error code, not an instance handle.
    let code = rc as isize;
    match code {
        c if c > 32 => Ok(()),
        // SE_ERR_NOASSOC — the type has no handler at all, which is the one failure a
        // user can act on (and the reason the note always prints the path).
        31 => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no application is associated with this file type",
        )),
        2 | 3 => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the shell did not find the file",
        )),
        c => Err(io::Error::other(format!("ShellExecuteW failed ({c})"))),
    }
}

#[cfg(not(windows))]
fn launch(path: &Path) -> io::Result<()> {
    launch_with(launcher(HERE).unwrap_or("xdg-open"), path)
}

/// The launch off Windows, with the launcher named. `launch` passes the platform's; a test
/// passes a stub, which is how the shape that matters — one program, one argument, nothing
/// re-parsed on the way (docs/lessons.md §6) — is checked on Linux with no desktop, in CI.
#[cfg(not(windows))]
fn launch_with(program: &str, path: &Path) -> io::Result<()> {
    spawn_reaped(program, path).map(|_| ())
}

/// Starts the launcher and hands it to a thread of its own that waits for it; returns its
/// pid and that thread.
///
/// Not waited on **here**: the launcher may exec a viewer that lives as long as the user
/// keeps it open, and the blocking-pool thread this runs on must not be held for that. It
/// used to be waited on nowhere, which left one zombie per `/file open` until the
/// application exited, on the reasoning that the alternative holds a thread for the
/// viewer's lifetime. It mostly does not: `xdg-open` on a desktop hands the file to
/// `gio`/`kde-open` and exits within milliseconds, taking the waiting thread with it, and
/// only a launcher that runs the viewer itself keeps one parked thread — not a pool slot —
/// for as long as the viewer is open. No launcher on the machine is still the spawn error;
/// a waiting thread that cannot be started is not reported as a failed open, because by
/// then the open has happened.
#[cfg(not(windows))]
fn spawn_reaped(
    program: &str,
    path: &Path,
) -> io::Result<(u32, Option<std::thread::JoinHandle<()>>)> {
    use std::process::{Command, Stdio};

    let mut child = Command::new(program)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = child.id();
    let reaper = std::thread::Builder::new()
        .name("open-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .ok();
    Ok((pid, reaper))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_has_its_launcher() {
        assert_eq!(launcher(Platform::Linux), Some("xdg-open"));
        assert_eq!(launcher(Platform::Mac), Some("open"));
        // Windows opens through the shell, not through a program of its own.
        assert_eq!(launcher(Platform::Windows), None);
        assert_eq!(
            HERE,
            if cfg!(windows) {
                Platform::Windows
            } else if cfg!(target_os = "macos") {
                Platform::Mac
            } else {
                Platform::Linux
            }
        );
    }

    #[test]
    fn documents_open_and_everything_else_does_not() {
        for name in [
            "chart.png",
            "chart.PNG",
            "photo.JPEG",
            "report.pdf",
            "sales.csv",
            "sales.xlsx",
            "notes.md",
            "data.json",
            "letter.docx",
        ] {
            assert!(is_document(name), "{name} should open directly");
        }
        for name in [
            // Scripted documents a browser executes — and both are shapes a call writes.
            "plot.svg",
            "report.html",
            // Macro-enabled office files.
            "book.xlsm",
            "letter.docm",
            // Executable shapes: the attack fork F9 named.
            "run.bat",
            "run.cmd",
            "run.ps1",
            "run.sh",
            "link.lnk",
            "installer.exe",
            // The last extension decides, so a document name in front changes nothing.
            "report.pdf.bat",
            // No extension at all.
            "Makefile",
            "",
        ] {
            assert!(!is_document(name), "{name} must not be handed to a handler");
        }
    }

    /// The Linux half, checked where it can be: a stub launcher on the path of the call
    /// records what it was given. The argument must arrive whole — spaces, brackets and
    /// all — which is the property `cmd /c start` cannot offer (docs/lessons.md §6), and
    /// a launcher that is not installed must come back as an error, since that is the
    /// failure the note turns into a path the user can copy (§13 U6).
    #[cfg(unix)]
    #[test]
    fn the_unix_launch_hands_over_one_whole_argument() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("a temp dir");
        let log = dir.path().join("argv.txt");
        let stub = dir.path().join("xdg-open");
        let mut f = std::fs::File::create(&stub).expect("the stub");
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "printf '%s\\n' \"$#\" \"$1\" > '{}'", log.display()).unwrap();
        drop(f);
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let target = dir.path().join("my chart (1).png");
        std::fs::write(&target, b"x").unwrap();
        launch_with(stub.to_str().unwrap(), &target).expect("the stub launcher started");

        // The child is waited on by a thread of its own, not by the call (a viewer outlives
        // the call), so the recording is polled for — a second is orders of magnitude more
        // than `/bin/sh` needs.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let recorded = loop {
            if let Ok(text) = std::fs::read_to_string(&log)
                && text.lines().count() >= 2
            {
                break text;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the stub launcher recorded nothing"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let mut lines = recorded.lines();
        assert_eq!(lines.next(), Some("1"), "exactly one argument: {recorded}");
        assert_eq!(lines.next(), target.to_str(), "the path arrived split");

        let missing = launch_with("mindfork-no-such-launcher", &target);
        assert!(
            missing.is_err(),
            "a machine with no launcher must report it"
        );
    }

    /// A launcher that has exited is reaped, not left in the process table until the
    /// application exits. A zombie keeps its `/proc` entry until its parent waits for it, so
    /// the entry's absence once the waiting thread is done is the property itself.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_unix_launcher_is_reaped_rather_than_left_a_zombie() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let target = dir.path().join("chart.png");
        std::fs::write(&target, b"x").unwrap();

        let (pid, reaper) = spawn_reaped("true", &target).expect("`true` started");
        reaper
            .expect("the waiting thread started")
            .join()
            .expect("the waiting thread finished");
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "the launcher (pid {pid}) was left a zombie"
        );
    }

    /// The mark is a stream beside the file's bytes (§13 U11): read back as written, the
    /// bytes left alone, one entry in the folder, a second mark replacing the first, and no
    /// file created where there was none to mark.
    #[cfg(windows)]
    #[test]
    fn a_zone_is_written_beside_the_bytes_and_read_back() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join("sales.csv");
        std::fs::write(&file, b"=1+1\n").unwrap();
        assert_eq!(
            zone_of(&file),
            None,
            "a file we wrote carries no mark of its own"
        );

        set_zone(&file, b"[ZoneTransfer]\r\nZoneId=0\r\n").unwrap();
        set_zone(&file, FROM_ELSEWHERE).unwrap();
        assert_eq!(zone_of(&file).as_deref(), Some(FROM_ELSEWHERE));
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"=1+1\n",
            "the bytes are untouched"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        let missing = dir.path().join("gone.csv");
        assert!(set_zone(&missing, FROM_ELSEWHERE).is_err());
        assert!(
            !missing.exists(),
            "marking a missing file must not create it"
        );
    }

    /// Off Windows there is no mark to read or write, and asking for one is not an error —
    /// storing a file must not fail on a platform where the mark does not exist.
    #[cfg(not(windows))]
    #[test]
    fn off_windows_there_is_no_mark_and_setting_one_succeeds() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join("sales.csv");
        std::fs::write(&file, b"=1+1\n").unwrap();
        assert!(set_zone(&file, FROM_ELSEWHERE).is_ok());
        assert_eq!(zone_of(&file), None);
    }

    /// The stage's manual gate (§13 U10): the only part no automated test can cover is
    /// whether a real desktop actually brings something up. Run on a machine with one:
    ///
    /// ```text
    /// MINDFORK_OPEN_LIVE=1 cargo test -- --ignored --nocapture opens_a_document_and_a_folder_live
    /// ```
    ///
    /// It opens a viewer on a text file and a file manager on its folder, and prints both
    /// paths — watch for two windows. Guarded by the variable so the `--ignored` suite on a
    /// headless machine cannot start one (CLAUDE.md §Commands).
    #[test]
    #[ignore = "opens windows on the desktop; needs MINDFORK_OPEN_LIVE"]
    fn opens_a_document_and_a_folder_live() {
        if std::env::var("MINDFORK_OPEN_LIVE").is_err() {
            println!("skipped: set MINDFORK_OPEN_LIVE=1 to open windows on this desktop");
            return;
        }
        let dir = tempfile::tempdir().expect("a temp dir");
        let doc = dir.path().join("mindfork open gate.txt");
        std::fs::write(&doc, b"stage 4: this file was opened by /file open\n").unwrap();

        let (opens, at) = decide(&doc).expect("a document opens");
        assert_eq!(opens, Opens::File);
        println!("opening the document: {}", at.display());
        open(at).expect("the document opened");

        // And the fallback half: a type no handler may run opens the folder it sits in.
        let script = dir.path().join("run.bat");
        std::fs::write(&script, b"@echo off\n").unwrap();
        let (opens, at) = decide(&script).expect("its folder opens");
        assert_eq!(opens, Opens::Folder);
        println!("opening the folder instead of run.bat: {}", at.display());
        open(at).expect("the folder opened");

        // The handler and the file manager read the files after this returns, so the
        // temporary directory has to outlive the call by more than nothing.
        std::thread::sleep(std::time::Duration::from_secs(5));
    }

    #[test]
    fn a_refused_type_opens_its_folder_instead() {
        let dir = Path::new("/data/files/chat");
        let chart = dir.join("chart.png");
        let (opens, at) = decide(&chart).expect("a document opens");
        assert_eq!(opens, Opens::File);
        assert_eq!(at, chart);

        let script = dir.join("run.bat");
        let (opens, at) = decide(&script).expect("its folder opens");
        assert_eq!(opens, Opens::Folder);
        assert_eq!(at, dir);

        // A bare name has no folder to fall back to, and the file itself is the one thing
        // that may not stand in for it: nothing opens.
        assert_eq!(decide(Path::new("run.bat")), None);
        // A bare *document* is still a document.
        let bare = Path::new("report.pdf");
        assert_eq!(decide(bare), Some((Opens::File, bare)));
    }
}
