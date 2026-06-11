//! Process spawning and tree termination.
//!
//! The single most reported sidecar pain is shutdown: sidecars spawn their
//! own children (Python servers fork workers, .NET hosts spawn helpers), and
//! killing only the direct child leaves orphans holding ports and file
//! handles — the app then "won't launch a second time".
//!
//! Strategy:
//! - **Unix**: each sidecar gets its own process group (`setpgid`); shutdown
//!   signals the whole group (`killpg`), so grandchildren die too.
//! - **Windows**: each sidecar is assigned to a Job Object with
//!   `KILL_ON_JOB_CLOSE`; terminating the job (or the app dying and the job
//!   handle closing) takes the whole tree down — even if the supervisor
//!   itself crashes, the OS cleans up. This is the mechanism taskkill
//!   loops try (and fail) to approximate.

use std::path::Path;
use std::process::Stdio;

use tokio::process::{Child, Command};

use crate::domain::error::SidecarError;

/// A spawned sidecar process plus the OS handle that owns its tree.
pub struct SpawnedProcess {
    pub child: Child,
    pub pid: u32,
    /// Owning handle of the Job Object. `KILL_ON_JOB_CLOSE` is set, so
    /// dropping this handle terminates every process in the job — including
    /// the case where the supervisor itself dies and the OS closes it for us.
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

pub fn spawn(
    binary: &Path,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
) -> Result<SpawnedProcess, SidecarError> {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);

    #[cfg(unix)]
    {
        // Own process group → killpg reaches grandchildren.
        cmd.process_group(0);
    }

    #[cfg(windows)]
    {
        // Hide the console window flash for windowless sidecars.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd.spawn().map_err(|e| SidecarError::Spawn {
        binary: binary.to_path_buf(),
        source: e,
    })?;
    let pid = child.id().ok_or(SidecarError::AlreadyExited)?;

    #[cfg(windows)]
    let job = {
        let job = win32job::Job::create().map_err(|e| SidecarError::Os(e.to_string()))?;
        let mut info = job
            .query_extended_limit_info()
            .map_err(|e| SidecarError::Os(e.to_string()))?;
        info.limit_kill_on_job_close();
        job.set_extended_limit_info(&info)
            .map_err(|e| SidecarError::Os(e.to_string()))?;
        // Children created after this point inherit job membership. The gap
        // between spawn and assignment is microseconds; the taskkill sweep in
        // kill_tree covers anything that slipped through it.
        let handle = child.raw_handle().ok_or(SidecarError::AlreadyExited)?;
        job.assign_process(handle as isize)
            .map_err(|e| SidecarError::Os(e.to_string()))?;
        Some(job)
    };

    Ok(SpawnedProcess {
        child,
        pid,
        #[cfg(windows)]
        job,
    })
}

impl SpawnedProcess {
    /// Sends the platform "please exit" signal to the whole tree.
    /// Unix: SIGTERM to the process group. Windows: no-op (no portable
    /// equivalent — use an HTTP graceful hook instead).
    pub fn signal_graceful(&self) {
        #[cfg(unix)]
        // SAFETY: killpg with a valid pid and signal is always sound; a stale
        // pid simply returns ESRCH, which we ignore. No memory is touched.
        unsafe {
            libc::killpg(i32::try_from(self.pid).unwrap_or(i32::MAX), libc::SIGTERM);
        }
    }

    /// Forcibly terminates the entire process tree.
    pub async fn kill_tree(&mut self) {
        #[cfg(unix)]
        // SAFETY: killpg with a valid pid and signal is sound; a dead group
        // returns ESRCH, ignored. No memory is dereferenced.
        unsafe {
            libc::killpg(i32::try_from(self.pid).unwrap_or(i32::MAX), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            // Primary: dropping the only handle to a KILL_ON_JOB_CLOSE job
            // makes the OS terminate every process in it, grandchildren
            // included. This also fires automatically if the supervisor
            // itself dies — the orphan problem solved at the OS level.
            drop(self.job.take());
            // Belt-and-suspenders: a child created in the microsecond window
            // before job assignment could escape the job. `taskkill /T` walks
            // the live parent/child tree from the pid as a second sweep —
            // the battle-tested userland fallback. Either alone usually
            // suffices; together they leave no orphan.
            taskkill_tree(self.pid);
        }
        // Reap the direct child so it doesn't zombie.
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

/// `taskkill /F /T /PID <pid>` — force-kills the process and its child tree.
/// The proven Windows escalation when a clean handle isn't enough.
#[cfg(windows)]
fn taskkill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

/// Best-effort kill of a process tree from a *previous* app run, identified
/// by pid + executable name (see `cleanup.rs`). The name check prevents
/// killing an unrelated process that recycled the pid.
pub fn kill_stale(pid: u32, expected_exe: &str) -> bool {
    let mut system = sysinfo::System::new();
    let sys_pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sys_pid]), true);
    let Some(proc_) = system.process(sys_pid) else {
        return false; // already gone
    };
    let name_matches = proc_
        .exe()
        .and_then(|p| p.file_name())
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(expected_exe));
    if !name_matches {
        return false;
    }
    #[cfg(unix)]
    // SAFETY: killpg/kill take a pid + signal by value and touch no memory;
    // an already-dead target just returns ESRCH, which is fine here.
    unsafe {
        // The stale process was spawned with its own group — try group first,
        // fall back to the single pid.
        if libc::killpg(i32::try_from(pid).unwrap_or(i32::MAX), libc::SIGKILL) != 0 {
            libc::kill(i32::try_from(pid).unwrap_or(i32::MAX), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        // The orphan's Job Object died with the previous app run, so reach for
        // the tree-killer directly: `taskkill /F /T /PID` cleans the orphan and
        // any workers it had forked. `proc_.kill()` only hits the single pid.
        taskkill_tree(pid);
        let _ = proc_; // name check already done; silence unused on this path
    }
    true
}
