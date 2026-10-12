//! The user's account: their home directory, and their login name and shell
//! as the system's user database (passwd) records them.

use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::error::{Error, Result};

/// The user's home directory (`$HOME`, else their passwd entry's): where a
/// block starts when it has no usable working directory, and where the Rex
/// app's server socket and LaunchAgents live.
pub fn home() -> Result<PathBuf> {
    std::env::home_dir()
        .filter(|home| home.is_absolute())
        .ok_or_else(|| Error::new("can't find the home directory: set HOME"))
}

/// The user's login name and login shell: who and what Rex starts each
/// block's shell as. Rex uses the passwd entry, not `$USER` or `$SHELL`.
#[derive(Debug, Clone)]
pub struct Account {
    pub user: String,
    pub shell: PathBuf,
}

impl Account {
    /// The current user's passwd entry (`getpwuid_r(getuid())`), or `None`
    /// if there's none, or it lacks a name or an absolute shell path.
    pub fn current() -> Option<Account> {
        // SAFETY: getuid can't fail and has no preconditions.
        let uid = unsafe { libc::getuid() };
        // SAFETY: `passwd` is plain C data; all-zero is a valid value.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let mut buf: Vec<libc::c_char> = vec![0; 1024];
        loop {
            // SAFETY: every pointer is valid for the duration of the call, and
            // `buf.len()` is the buffer's real size.
            let rc = unsafe {
                libc::getpwuid_r(uid, &mut entry, buf.as_mut_ptr(), buf.len(), &mut found)
            };
            if rc == libc::ERANGE && buf.len() < 1 << 20 {
                buf.resize(buf.len() * 2, 0);
                continue;
            }
            if rc != 0 || found.is_null() {
                return None;
            }
            break;
        }
        // SAFETY: on success the entry's strings are null or NUL-terminated
        // and point into `buf`, which is still alive.
        let field = |p: *const libc::c_char| {
            (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_bytes().to_vec())
        };
        let user = String::from_utf8(field(entry.pw_name)?)
            .ok()
            .filter(|user| !user.is_empty())?;
        let shell = PathBuf::from(OsStr::from_bytes(&field(entry.pw_shell)?));
        shell.is_absolute().then_some(Account { user, shell })
    }
}
