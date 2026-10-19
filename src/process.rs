//! Facts about other processes on this machine, from `ps`, and the one
//! thing rexurrect changes about its own.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::error::{Context, Error, Result};

/// A process's full command line: its arguments joined by spaces, with any
/// quoting lost. `None` if the process has already exited.
pub fn command_line(pid: u32) -> Result<Option<String>> {
    ps(&["-ww", "-o", "args=", "-p", &pid.to_string()])
}

/// A process's parent and the path of its executable. `None` if the process
/// has already exited.
pub fn parent_and_executable(pid: u32) -> Result<Option<(u32, PathBuf)>> {
    let line = ps(&["-o", "ppid=,comm=", "-p", &pid.to_string()])?;
    Ok(line.as_deref().and_then(pid_and_executable))
}

/// The pid and the path of the executable of every process this user runs.
/// Never another user's: whoever owns a process can replace its executable.
pub fn own_executables() -> Result<Vec<(u32, PathBuf)>> {
    let listing = ps(&["-ax", "-o", "uid=,pid=,comm="])?
        .ok_or_else(|| Error::new("`ps -ax` failed listing processes"))?;
    // `ps`'s `uid` is the effective one.
    // SAFETY: geteuid can't fail and has no preconditions.
    let uid = unsafe { libc::geteuid() };
    Ok(executables_of(uid, &listing))
}

/// The processes `uid` runs, from `ps`'s `uid=,pid=,comm=` listing.
fn executables_of(uid: u32, listing: &str) -> Vec<(u32, PathBuf)> {
    listing
        .lines()
        .filter_map(|line| {
            let (owner, process) = line.trim().split_once(char::is_whitespace)?;
            (owner.parse() == Ok(uid)).then_some(process)
        })
        .filter_map(pid_and_executable)
        .collect()
}

/// A process ID (a process's own or its parent's), then the path of an
/// executable, as `ps -o pid=,comm=` or `ps -o ppid=,comm=` prints them.
/// `comm` comes last and may contain spaces (`/Applications/Rex Beta.app/...`).
fn pid_and_executable(line: &str) -> Option<(u32, PathBuf)> {
    let (pid, executable) = line.trim().split_once(char::is_whitespace)?;
    Some((pid.parse().ok()?, executable.trim().into()))
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

/// `ps <args>`'s output, trimmed; `None` if it failed or printed nothing
/// (with `-p <pid>`: there's no such process).
fn ps(args: &[&str]) -> Result<Option<String>> {
    let out = Command::new("ps")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .with_context(|| "running ps")?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((out.status.success() && !line.is_empty()).then_some(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Another user's `rex` is theirs to replace: running it would run their
    /// code as this user.
    #[test]
    fn only_the_users_own_processes_are_listed() {
        let listing = "  0   1 /sbin/launchd
  502 10280 /Users/you/Library/Application Support/com.superlogical.rex/RexServer/Versions/1203/Rex Server.app/Contents/MacOS/rex
  503 20000 /Users/other/bin/rex
";
        assert_eq!(
            executables_of(502, listing),
            [(
                10280,
                PathBuf::from(
                    "/Users/you/Library/Application Support/com.superlogical.rex/RexServer/Versions/1203/Rex Server.app/Contents/MacOS/rex"
                )
            )]
        );
    }
}
