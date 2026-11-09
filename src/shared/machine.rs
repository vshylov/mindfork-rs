//! What the machine has: its physical memory. One question so far — whether a
//! backup may compact the database in memory rather than through a scratch file
//! (docs/research/backup-big-entry.md F2) — so one call per platform, through the
//! dependencies already here: `libc` on unix, `windows-sys` on Windows.

/// The machine's physical memory in bytes; `None` when the system will not say.
pub fn physical_memory() -> Option<u64> {
    imp::physical_memory().filter(|&bytes| bytes > 0)
}

#[cfg(unix)]
mod imp {
    /// Pages times page size — `_SC_PHYS_PAGES` is there on Linux and macOS.
    pub(super) fn physical_memory() -> Option<u64> {
        // SAFETY: `sysconf` takes a name and returns a value or -1; it touches no
        // memory of ours.
        let (pages, size) = unsafe {
            (
                libc::sysconf(libc::_SC_PHYS_PAGES),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        let pages = u64::try_from(pages).ok()?;
        let size = u64::try_from(size).ok()?;
        pages.checked_mul(size)
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    pub(super) fn physical_memory() -> Option<u64> {
        let mut status = MEMORYSTATUSEX {
            dwLength: u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).ok()?,
            ..Default::default()
        };
        // SAFETY: `status` is a `MEMORYSTATUSEX` with `dwLength` set, which is
        // all the call asks of its one argument.
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        (ok != 0).then_some(status.ullTotalPhys)
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub(super) fn physical_memory() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every platform the app ships on says how much memory it has, and it is
    /// more than a toy's: what the CI runners and the droplet have is gigabytes.
    #[test]
    fn the_machine_says_how_much_memory_it_has() {
        let bytes = physical_memory().expect("the platform reports its memory");
        assert!(bytes >= 512 << 20, "{bytes}");
    }
}
