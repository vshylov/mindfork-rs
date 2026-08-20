//! Killing a **process tree**, not a process.
//!
//! `Child::kill` and `kill_on_drop` end the process this application spawned and
//! nothing else. That is enough for the Python sandbox, where the interpreter
//! runs *inside* the `wasmer` process (ADR 0005), and it is not enough for
//! anything that shells out to a build system: `cargo build` spawns `rustc`
//! children of its own, and killing `cargo` leaves them compiling. On a timeout
//! or an `Esc` the user is entitled to expect the machine to go quiet.
//!
//! Two mechanisms, one per platform:
//!
//! - **Windows** — a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. The
//!   whole tree dies when the job handle closes (a clean exit, or the process
//!   crashing), and [`TreeGuard::kill`] ends it on demand with
//!   `TerminateJobObject`. This half was written for the MCP host (`cmd /c npx`
//!   → `node`) and lived in `shared/mcp.rs`; the code workspace needs the same
//!   thing, so it was hoisted here rather than copied (docs/lessons.md §2).
//! - **Unix** — a new process **group** (`process_group(0)`) and `killpg`. MCP
//!   never had this and still does not: giving a server its own group changes
//!   how signals reach it, which is a behaviour change unrelated to running a
//!   build. So the group is opt-in — [`TreeGuard::assign`] keeps the old
//!   behaviour exactly, [`TreeGuard::assign_group`] is the one that can `killpg`.
//!
//! There is a race on Windows that the MCP precedent already accepts and this
//! module inherits: the job is created *after* `spawn`, so a child that forks
//! within those few microseconds escapes it. Closing it properly needs
//! `CREATE_SUSPENDED` plus `ResumeThread`, and no real build system starts a
//! grandchild before it has finished loading.

use tokio::process::{Child, Command};

/// Prepares `cmd` so the process it spawns can later be killed as a tree.
///
/// Call it **before** `spawn`, and pair it with [`TreeGuard::assign_group`]
/// afterwards — the two halves are what make [`TreeGuard::kill`] reach
/// grandchildren on unix. On Windows it does nothing (the Job Object is created
/// after the spawn and needs no preparation).
pub fn prepare_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        // A group of its own, with the child as its leader: `killpg` then
        // reaches everything the command started, and — just as important —
        // nothing this application started. A build that spawns compilers is
        // the whole point; the TUI's own process group is not in it.
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

/// Owns whatever the platform needs in order to end a spawned process tree.
///
/// Holding it keeps the guarantee alive; **dropping it ends the tree** on both
/// platforms — the Windows job handle closes (kill-on-close) and unix signals
/// the group. That is what covers cancellation, where the agentic loop drops the
/// tool's future and no cleanup code of ours runs at all. A process that has
/// already been reaped must be [`disarmed`](Self::disarm) first, or the drop
/// would signal a pid the OS has since given to somebody else.
pub struct TreeGuard {
    #[cfg(windows)]
    job: Option<JobHandle>,
    /// The child's pid, which is also its process-group id — but only while the
    /// guard is **armed**, and only when [`prepare_group`] actually made a
    /// group. `None` means "nothing to kill here", which covers two different
    /// situations that must both be safe: no group was requested (the child
    /// then shares **ours**, and killing that group would kill the application),
    /// and the process has already been reaped ([`Self::disarm`] — after which
    /// the pid may belong to somebody else entirely).
    #[cfg(unix)]
    pgid: Option<i32>,
}

impl Drop for TreeGuard {
    fn drop(&mut self) {
        // Dropping the guard is how a **cancelled** command is torn down: the
        // agentic loop drops the tool's future on `Esc`, so nothing downstream
        // gets to run cleanup code. On Windows that is already handled — the
        // job handle closes and kill-on-close ends the tree — and on unix it is
        // this. Reaped processes are disarmed first, so no dead pid is signalled.
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // SAFETY: a plain syscall against the group we asked the child to lead.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
struct JobHandle(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a Job Object HANDLE is just a kernel handle; sending it across threads is safe.
#[cfg(windows)]
unsafe impl Send for JobHandle {}

#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was created by us in `assign` and hasn't been closed yet.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

impl TreeGuard {
    /// Places the process into a kill-on-close Job Object (Windows) and nothing
    /// more — the behaviour the MCP host has always had.
    ///
    /// "Best effort": a failure is only logged, and the process keeps running
    /// without a job, as it does on unix.
    pub fn assign(child: &Child) -> Self {
        Self {
            #[cfg(windows)]
            job: create_job(child),
            #[cfg(unix)]
            pgid: {
                let _ = child;
                None
            },
        }
    }

    /// Like [`Self::assign`], and on unix additionally remembers the process
    /// group so [`Self::kill`] can reach the whole tree. Only correct when
    /// [`prepare_group`] was called on the command before it was spawned.
    pub fn assign_group(child: &Child) -> Self {
        Self {
            #[cfg(windows)]
            job: create_job(child),
            #[cfg(unix)]
            pgid: child.id().map(|id| id as i32),
        }
    }

    /// Stops the guard from killing anything on drop. Call it once the child
    /// has been **reaped**: from that moment its pid is free for the OS to hand
    /// to another process, and a late `killpg` would reach a stranger.
    pub fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.pgid = None;
        }
    }

    /// Ends the tree now: `TerminateJobObject` on Windows, `killpg(SIGKILL)` on
    /// unix. Safe to call on a process that has already exited — both calls
    /// simply fail, and a caller reacting to a timeout cannot know which it is.
    pub fn kill(&self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            // SAFETY: our own handle, still open (we are holding it).
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job.0, 1);
            }
        }
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // SAFETY: a plain syscall; `pgid` is the group we asked the child to
            // lead, so this cannot reach the application's own group.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

/// Creates the kill-on-close Job Object and puts `child` in it.
#[cfg(windows)]
fn create_job(child: &Child) -> Option<JobHandle> {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    let Some(raw) = child.raw_handle() else {
        tracing::warn!("no process handle — Job Object not assigned");
        return None;
    };
    // SAFETY: `raw` is a valid handle of the just-spawned process; the struct is
    // zero-initialized; the job closes via JobHandle::drop.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            tracing::warn!("CreateJobObjectW failed");
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 || AssignProcessToJobObject(job, raw as _) == 0 {
            tracing::warn!("failed to assign the kill-on-close Job Object");
            windows_sys::Win32::Foundation::CloseHandle(job);
            return None;
        }
        Some(JobHandle(job))
    }
}

/// A python interpreter, for tests that need a program behaving the same on
/// both platforms — a subprocess fixture, or a grandchild that keeps writing to
/// a file so its death is observable from outside.
///
/// Python rather than a shell because the two shells spell such a fixture
/// differently enough to be two fixtures, and this repository already depends on
/// python for its own gates. `None` when there is none: a test then says so out
/// loud rather than reporting `ok` (docs/lessons.md §9).
///
/// Lives here, next to the process machinery, because two test modules wrote it
/// minutes apart — which is the point at which an opening is a fixture rather
/// than part of a test (docs/lessons.md §2).
#[cfg(test)]
pub fn test_python() -> Option<String> {
    ["python", "python3"].into_iter().find_map(|c| {
        std::process::Command::new(c)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()
            .filter(|st| st.success())
            .map(|_| c.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// The property the whole module exists for: killing the process we spawned
    /// must also kill what *it* spawned. A `kill_on_drop`/`Child::kill` would
    /// pass the first half of this test and fail the second — which is exactly
    /// what `cargo build` leaving `rustc` behind looks like.
    #[tokio::test]
    async fn a_grandchild_dies_with_the_tree() {
        let Some(py) = test_python() else {
            println!("SKIP: no python interpreter — the grandchild fixture needs one");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("alive.txt");
        let marker_arg = marker.to_string_lossy().into_owned();
        // The parent spawns a grandchild that appends to `marker` forever, then
        // sleeps. Killing only the parent leaves the file growing.
        let script = "import subprocess,sys,time\n\
             child=\"import sys,time\\nwhile True:\\n open(sys.argv[1],'a').write('x')\\n time.sleep(0.05)\"\n\
             subprocess.Popen([sys.executable,'-c',child,sys.argv[1]])\n\
             time.sleep(120)\n";
        let mut cmd = Command::new(&py);
        cmd.arg("-c")
            .arg(script)
            .arg(&marker_arg)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        prepare_group(&mut cmd);
        let mut child = cmd.spawn().expect("spawn the fixture");
        let guard = TreeGuard::assign_group(&child);

        // Wait for the grandchild to actually be writing — otherwise the test
        // could pass by killing a tree that never grew one.
        let mut grew = false;
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if marker.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
                grew = true;
                break;
            }
        }
        assert!(grew, "the grandchild never started writing");

        guard.kill();
        let _ = child.start_kill();
        let _ = child.wait().await;
        let mut guard = guard;
        guard.disarm();

        // Give the kill a moment to land, then measure: the file must stop
        // growing. Size is the assertion rather than "the process is gone",
        // because the symptom is what the user cares about (docs/lessons.md §2).
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let settled = marker.metadata().map(|m| m.len()).unwrap_or(0);
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        let after = marker.metadata().map(|m| m.len()).unwrap_or(0);
        assert_eq!(
            settled, after,
            "the grandchild survived the tree kill: {settled} → {after} bytes"
        );
    }
}
