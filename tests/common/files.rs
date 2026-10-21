//! Properties of the files in a directory that tests check without reading
//! their formats or relying on their names: who can access them, how much
//! space they take, and whether they changed.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Every directory and file under `dir` (and `dir` itself) that its group or
/// other users can access, with its mode. Symlinks are skipped: their own
/// mode means nothing.
pub fn accessible_to_others(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for (path, meta) in walk(dir) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            found.push(format!("{} ({mode:o})", path.display()));
        }
    }
    found.sort();
    found
}

/// Whether `dir` exists and holds anything.
pub fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// The total size of the files under `dir`, in bytes.
pub fn disk_usage(dir: &Path) -> u64 {
    walk(dir)
        .into_iter()
        .filter(|(_, meta)| meta.is_file())
        .map(|(_, meta)| meta.len())
        .sum()
}

/// Every file under `dir`, with its contents: compare two to check that
/// nothing was written there in between.
pub fn contents(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    walk(dir)
        .into_iter()
        .filter(|(_, meta)| meta.is_file())
        .map(|(path, _)| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

/// `dir` and everything under it except symlinks, with their metadata.
fn walk(dir: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        }
        found.push((path, meta));
    }
    found
}
