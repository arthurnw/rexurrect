//! Short-path temporary directories under `/tmp`.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A private (0700) directory directly under `/tmp`, removed on drop.
///
/// The path is deliberately short (`/tmp/rxt.<pid>-<n>`): a throwaway server's
/// unix socket lives inside it, and macOS rejects socket paths longer than
/// about 104 bytes (`bind: invalid argument`). `std::env::temp_dir()` is far
/// too long on macOS.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> TempDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let pid = std::process::id();
        loop {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from(format!("/tmp/rxt.{pid}-{n}"));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return TempDir { path },
                // Left over from an earlier run whose process had the same pid.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("creating {}: {e}", path.display()),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
