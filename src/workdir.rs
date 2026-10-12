//! A shell block's working directory.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use crate::error::Result;
use crate::rex::Rex;
use crate::rex::api::TerminalProcess;

/// Where the block's shell is: what it last reported through the terminal
/// `pwd` method, or, while it hasn't reported anything, the foreground
/// process's directory from what its terminal `process` method answered
/// (`process`).
pub fn working_directory(
    rex: &Rex,
    session_id: &str,
    block_id: &str,
    process: &TerminalProcess,
) -> Result<PathBuf> {
    let pwd = rex.terminal_pwd(session_id, block_id)?;
    Ok(pwd
        .pwd
        .as_deref()
        .and_then(path_from_file_url)
        .unwrap_or_else(|| process_directory(process)))
}

/// The path in a `file://<host>/<path>` URL, percent-decoded. The host may be
/// anything or empty; raw (unencoded) characters are kept as they are.
fn path_from_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    Some(PathBuf::from(OsString::from_vec(percent_decode(
        path.as_bytes(),
    ))))
}

fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// The foreground process's directory (a physical path), else home.
fn process_directory(process: &TerminalProcess) -> PathBuf {
    process
        .foreground
        .cwd
        .clone()
        .unwrap_or_else(|| process.home.clone())
}
