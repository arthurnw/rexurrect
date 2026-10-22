//! rexurrect's state directory: snapshots, the lineage map and anything else
//! rexurrect has to remember between runs, and the file operations every
//! module uses on it.
//!
//! Location, first match wins:
//! 1. `$REXURRECT_STATE_DIR`
//! 2. `$XDG_STATE_HOME/rexurrect` (only if `XDG_STATE_HOME` is absolute)
//! 3. `$HOME/.local/state/rexurrect`
//!
//! Each module owns the names of its files inside it.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Context, Error, Result};
use crate::settings;

pub const STATE_DIR_VAR: &str = "REXURRECT_STATE_DIR";

#[derive(Debug, Clone)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    pub fn from_env() -> Result<StateDir> {
        let var = settings::env_non_empty;
        resolve(var(STATE_DIR_VAR), var("XDG_STATE_HOME"), var("HOME"))
            .map(|root| StateDir { root })
            .ok_or_else(|| {
                Error::new(format!(
                    "can't find a state directory: set {STATE_DIR_VAR}, XDG_STATE_HOME or HOME"
                ))
            })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// A path inside the state directory.
    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.root.join(name)
    }

    /// Creates the state directory if needed and makes it private (`0700`):
    /// snapshots and recorder files hold commands, and commands can hold
    /// secrets. An existing directory others can access is tightened.
    pub fn make_private(&self) -> Result<()> {
        create_private_dirs(&self.root)?;
        let mode = fs::metadata(&self.root)
            .with_context(|| format!("reading {}", self.root.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("making {} private", self.root.display()))?;
        }
        Ok(())
    }
}

fn resolve(
    override_dir: Option<OsString>,
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = override_dir {
        return Some(PathBuf::from(dir));
    }
    if let Some(xdg) = xdg_state_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Some(xdg.join("rexurrect"));
    }
    home.map(|home| PathBuf::from(home).join(".local/state/rexurrect"))
}

/// Writes a state file atomically (see [`write_atomically`]), readable and
/// writable by the user only (`0600`). Missing parent directories are
/// created private (`0700`). Every state file is written through here.
pub fn write_file(path: &Path, contents: &[u8]) -> Result<()> {
    create_private_dirs(parent_of(path))?;
    write_atomically(path, contents, 0o600)
}

/// Writes `contents` to `path` with permissions `mode`, atomically: they go
/// to a temporary file next to it (see [`temp_path`]), which is then
/// renamed over `path`, so neither a reader nor a crash ever sees a partial
/// file. The directory must exist.
pub fn write_atomically(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let temp = temp_path(path);
    let written = write_new_file(&temp, contents, mode).and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written.with_context(|| format!("writing {}", path.display()))
}

/// Points the symlink `link` at `target`, atomically: a new link is renamed
/// over the old one, so `link` never goes missing.
pub fn write_symlink(link: &Path, target: &Path) -> Result<()> {
    create_private_dirs(parent_of(link))?;
    let temp = temp_path(link);
    let _ = fs::remove_file(&temp);
    let written = std::os::unix::fs::symlink(target, &temp).and_then(|()| fs::rename(&temp, link));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written.with_context(|| format!("pointing {} at {}", link.display(), target.display()))
}

/// Where [`write_atomically`] stages `path`: `.<name>.<pid>.tmp` in the
/// same directory. Hidden and without the real file's extension, so
/// listings that pick files by extension never see one, even a leftover
/// from a crash.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .expect("state files have a file name")
        .to_string_lossy();
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

/// Whether `name`, taken from outside (a block ID, a snapshot ID), can name
/// a state file directly inside a directory: not empty, no `/` or NUL, so it
/// can't reach anywhere else, and not hidden, so it can't be a temporary
/// file.
pub fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\0'])
}

/// Whether `name` is one of [`temp_path`]'s: `.<name>.<pid>.tmp`.
pub fn is_temp_name(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|name| name.strip_suffix(".tmp"))
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(file, pid)| {
            !file.is_empty() && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())
        })
}

/// How long a temporary file goes unchanged before it counts as left
/// behind by a write that crashed: far longer than any write takes.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// Removes the temporary files in `dir` (the names `is_temp` picks) that
/// haven't changed for over an hour: what writes that crashed before their
/// rename left behind. Fresher ones may belong to writes still going on.
pub fn remove_stale_temp_files(dir: &Path, is_temp: impl Fn(&str) -> bool) -> Result<()> {
    let stale_before = SystemTime::now() - STALE_AFTER;
    for entry in read_dir_or_empty(dir)? {
        if entry.file_name().to_str().is_some_and(&is_temp) {
            remove_if_unchanged_since(&entry.path(), stale_before)?;
        }
    }
    Ok(())
}

/// Removes the file (or symlink) at `path` if it hasn't changed since
/// `before`. A directory stays, and one that has gone already is fine: a
/// write still going on may have renamed it since its directory was listed.
fn remove_if_unchanged_since(path: &Path, before: SystemTime) -> Result<()> {
    let reading = || format!("reading {}", path.display());
    // Not following symlinks: a temporary symlink is stale by its own age.
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(reading),
    };
    let modified = metadata.modified().with_context(reading)?;
    if !metadata.is_dir() && modified < before {
        remove_if_exists(path)?;
    }
    Ok(())
}

/// The entries of `dir`; none if it doesn't exist.
pub fn read_dir_or_empty(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let listing = || format!("listing {}", dir.display());
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .collect::<std::io::Result<_>>()
            .with_context(listing),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(listing),
    }
}

/// Removes the file (or symlink) at `path`; `false` if there was none.
pub fn remove_if_exists(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Removes a file or a whole directory; one that's already gone is fine.
pub fn remove_entry(path: &Path) -> Result<()> {
    let removed = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) => Err(e),
    };
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

fn write_new_file(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    // Left over from a crashed run whose process had the same pid.
    let _ = fs::remove_file(path);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Creates `dir` and any missing parents as `0700`. Existing directories are
/// left as they are.
pub fn create_private_dirs(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))
}

fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Reads a JSON state file; `T`'s default if it doesn't exist.
pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match read_file(path)? {
        Some(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("reading {}", path.display()))
        }
        None => Ok(T::default()),
    }
}

/// Writes a JSON state file (see [`write_file`]).
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let json = serde_json::to_vec_pretty(value).expect("state files serialize to JSON");
    write_file(path, &json)
}

/// Reads a state file; `None` if it doesn't exist.
pub fn read_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary file can go between listing its directory and looking at
    /// it: a recorder renames its own over a record at every command.
    #[test]
    fn a_temporary_file_that_has_already_gone_is_nothing_to_remove() {
        let gone = Path::new("/nonexistent-rexurrect-dir/.record.123.tmp");
        let removed = remove_if_unchanged_since(gone, SystemTime::now());
        assert!(removed.is_ok(), "{}", removed.unwrap_err());
    }
}
