//! Opening one of the chat's files — or its folder — in the user's desktop
//! (fork F9 of docs/sandbox-file-exchange.md, sub-decisions §13 U3–U6; spec §9.7).
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
    let Some(ext) = Path::new(name).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    DOCUMENTS.contains(&ext.as_str())
}

/// What opens for a file, and the path to hand to [`open`]: the file itself when its type
/// is a document, otherwise the folder it sits in (§13 U3). A path with no parent — a
/// bare name — stands in for its own folder rather than opening nothing.
pub fn decide(path: &Path) -> (Opens, &Path) {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if is_document(name) {
        (Opens::File, path)
    } else {
        (
            Opens::Folder,
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(path),
        )
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
    use std::process::{Command, Stdio};

    let program = launcher(HERE).unwrap_or("xdg-open");
    // Spawned and **not** waited on: the launcher may exec a viewer that lives as long as
    // the user keeps it open, and a blocking-pool thread must not be held for that. The
    // child is reaped when the application exits; the failure that matters — no launcher
    // on the machine — is the spawn error itself.
    Command::new(program)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
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

    #[test]
    fn a_refused_type_opens_its_folder_instead() {
        let dir = Path::new("/data/files/chat");
        let chart = dir.join("chart.png");
        let (opens, at) = decide(&chart);
        assert_eq!(opens, Opens::File);
        assert_eq!(at, chart);

        let script = dir.join("run.bat");
        let (opens, at) = decide(&script);
        assert_eq!(opens, Opens::Folder);
        assert_eq!(at, dir);

        // A bare name has no folder to fall back to: it stands in for its own.
        let bare = Path::new("run.bat");
        assert_eq!(decide(bare), (Opens::Folder, bare));
    }
}
