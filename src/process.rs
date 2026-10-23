//! Facts about other processes on this machine, from `ps` and the kernel,
//! and the one thing rexurrect changes about its own.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::error::{Context, Error, Result};

/// A process's full command line as `ps` prints it: its arguments joined by
/// spaces, with any quoting lost and control characters escaped. Good for
/// spotting a `rex server run`, never for typing (see [`arguments`]).
/// `None` if the process has already exited.
pub fn command_line(pid: u32) -> Result<Option<String>> {
    ps(&["-ww", "-o", "args=", "-p", &pid.to_string()])
}

/// A process's arguments, its program's name first, as the kernel holds
/// them: never `ps`'s text, which joins them with spaces and prints control
/// characters as octal escapes (a line break as `\012`). `None` if they
/// can't be read as they are: the process has exited, is another user's (a
/// `sudo vim`) or a zombie, or an argument isn't UTF-8.
#[cfg(target_os = "macos")]
pub fn arguments(pid: u32) -> Result<Option<Vec<String>>> {
    let Ok(raw_pid) = libc::c_int::try_from(pid) else {
        return Ok(None);
    };
    let mut argmax = [0u8; size_of::<libc::c_int>()];
    sysctl(&mut [libc::CTL_KERN, libc::KERN_ARGMAX], &mut argmax)
        .with_context(|| "reading the size limit of a process's arguments")?;
    let argmax = usize::try_from(libc::c_int::from_ne_bytes(argmax))
        .map_err(|_| Error::new("the size limit of a process's arguments is negative"))?;
    let mut buffer = vec![0u8; argmax];
    let Ok(size) = sysctl(
        &mut [libc::CTL_KERN, libc::KERN_PROCARGS2, raw_pid],
        &mut buffer,
    ) else {
        return Ok(None);
    };
    buffer.truncate(size);
    Ok(procargs2_arguments(&buffer))
}

/// Linux reads `/proc/<pid>/cmdline` (#39). Till then, no arguments: a block
/// without a recorder file keeps its default last command.
#[cfg(not(target_os = "macos"))]
pub fn arguments(_pid: u32) -> Result<Option<Vec<String>>> {
    Ok(None)
}

/// Reads the value `mib` names into `buffer`, and says how many bytes it is.
#[cfg(target_os = "macos")]
fn sysctl(mib: &mut [libc::c_int], buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut size = buffer.len();
    let length = libc::c_uint::try_from(mib.len()).map_err(std::io::Error::other)?;
    // SAFETY: `mib` holds `length` integers, `buffer` holds `size` bytes, and
    // nothing new is set.
    let found = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            length,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    match found {
        0 => Ok(size),
        _ => Err(std::io::Error::last_os_error()),
    }
}

/// The arguments in a `KERN_PROCARGS2` buffer: `argc` as a native-endian
/// int, the executable's path, NULs padding it out, then `argc` arguments,
/// each ending in a NUL (the environment follows). `None` if it holds fewer,
/// or one isn't UTF-8: changed to fit, it wouldn't be what ran.
#[cfg(any(target_os = "macos", test))]
fn procargs2_arguments(buffer: &[u8]) -> Option<Vec<String>> {
    let (argc, rest) = buffer.split_first_chunk::<4>()?;
    let argc = usize::try_from(i32::from_ne_bytes(*argc)).ok()?;
    let path_end = rest.iter().position(|&byte| byte == 0)?;
    let rest = &rest[path_end..];
    let start = rest.iter().position(|&byte| byte != 0)?;
    let mut fields = rest[start..].split(|&byte| byte == 0);
    let arguments = (&mut fields)
        .take(argc)
        .map(|argument| String::from_utf8(argument.to_vec()).ok())
        .collect::<Option<Vec<_>>>()?;
    // Only the last argument may run to the end without a NUL after it,
    // where `split` yields one more (empty) field.
    (arguments.len() == argc && fields.next().is_some()).then_some(arguments)
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

    fn procargs2(argc: i32, rest: &[u8]) -> Vec<u8> {
        let mut buffer = argc.to_ne_bytes().to_vec();
        buffer.extend_from_slice(rest);
        buffer
    }

    #[test]
    fn arguments_are_read_from_procargs2_as_they_are() {
        let buffer = procargs2(
            3,
            b"/bin/sh\0\0\0\0sh\0-c\0tail -f /dev/null\ntrue\0PATH=/bin\0",
        );
        assert_eq!(
            procargs2_arguments(&buffer).unwrap(),
            ["sh", "-c", "tail -f /dev/null\ntrue"]
        );
    }

    #[test]
    fn an_empty_argument_is_kept() {
        let buffer = procargs2(2, b"/usr/bin/vim\0vim\0\0");
        assert_eq!(procargs2_arguments(&buffer).unwrap(), ["vim", ""]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_processs_own_arguments_are_read() {
        let arguments = arguments(std::process::id()).unwrap().unwrap();
        let expected: Vec<String> = std::env::args().collect();
        assert_eq!(arguments, expected);
    }

    /// launchd, pid 1, is root's. Its arguments can't be read, as a `sudo
    /// vim`'s can't, and that mustn't fail a save.
    #[cfg(target_os = "macos")]
    #[test]
    fn another_users_process_has_no_arguments_to_read() {
        // SAFETY: geteuid can't fail and has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(arguments(1).unwrap(), None);
        }
    }

    #[test]
    fn arguments_that_are_not_utf8_are_not_read() {
        let buffer = procargs2(2, b"/bin/cat\0cat\0caf\xe9\0");
        assert_eq!(procargs2_arguments(&buffer), None);
    }

    #[test]
    fn a_buffer_with_fewer_arguments_than_it_counts_is_malformed() {
        assert_eq!(
            procargs2_arguments(&procargs2(3, b"/bin/sh\0sh\0-c\0")),
            None
        );
        assert_eq!(procargs2_arguments(&procargs2(-1, b"/bin/sh\0sh\0")), None);
        assert_eq!(procargs2_arguments(b"\x01\0"), None);
    }

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
