//! Facts about other processes on this machine, from `ps`, and the one
//! thing rex-resurrect changes about its own.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::error::{Context, Result};

/// A process's full command line: its arguments joined by spaces, with any
/// quoting lost. `None` if the process has already exited.
pub fn command_line(pid: u32) -> Result<Option<String>> {
    ps(pid, &["-ww", "-o", "args="])
}

/// A process's parent and the path of its executable. `None` if the process
/// has already exited.
pub fn parent_and_executable(pid: u32) -> Result<Option<(u32, PathBuf)>> {
    let Some(line) = ps(pid, &["-o", "ppid=,comm="])? else {
        return Ok(None);
    };
    // `comm` comes last and may contain spaces (`/Applications/Rex Beta.app/...`).
    let parsed = line
        .split_once(char::is_whitespace)
        .and_then(|(ppid, executable)| Some((ppid.parse().ok()?, executable.trim().into())));
    Ok(parsed)
}

/// Whether a process with this pid exists.
pub fn is_running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 delivers nothing; kill only checks that the process
    // exists and may be signalled.
    let found = unsafe { libc::kill(pid, 0) } == 0;
    found || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Marks every file descriptor this process has open, other than stdin,
/// stdout and stderr, close-on-exec, so no program it starts holds them.
///
/// A process started from Rex's init.lua inherits some of the server's: a
/// server launched by `rex server start` passes on its start lock, for one.
/// A long-running child holding that lock open could keep the next server
/// from starting.
pub fn stop_passing_on_open_files() {
    let Ok(entries) = std::fs::read_dir("/dev/fd") else {
        return;
    };
    let fds: Vec<libc::c_int> = entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .filter(|&fd| fd > 2)
        .collect();
    for fd in fds {
        // SAFETY: F_GETFD and F_SETFD only read and set a descriptor's flags.
        // A number that isn't open (the listing's own, closed by now) fails
        // with EBADF and is left alone.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

/// `ps <format> -p <pid>`, trimmed; `None` if there's no such process.
fn ps(pid: u32, format: &[&str]) -> Result<Option<String>> {
    let out = Command::new("ps")
        .args(format)
        .args(["-p", &pid.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .with_context(|| "running ps")?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((out.status.success() && !line.is_empty()).then_some(line))
}
