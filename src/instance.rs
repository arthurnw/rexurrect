//! Server instances and the restore lock.
//!
//! A server instance is one run of the Rex server, told apart by the
//! `instance_id` in `server.status`. rexurrect remembers the instances it
//! restored into, so it can tell a freshly started server (an unrestored
//! server instance) from one the user has been working in. It remembers
//! several, not just the last: another server can restore too (one started
//! by hand that runs the user's init.lua, auto-restore included), and that
//! mustn't make the server the user works in look unrestored.
//!
//! The restore lock lets one restore run at a time. Without it, two restores
//! (auto-restore and the restore a save starts first, say) could both create
//! the same session before either records its lineage (ADR-0001). A save
//! takes it too, so it never captures a half-restored server. The restored
//! instances are only read and written through the lock, so "is this
//! instance restored? if not, restore it and record it" can't interleave
//! with another process doing the same.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use crate::error::{Context, Result};
use crate::state::{self, StateDir};

const LOCK: &str = "restore.lock";
/// The restored instances' IDs, one per line, oldest first.
const RESTORED: &str = "restored-instances";
/// How many restored instances to remember. Only the instances of servers
/// still running matter, and few run at once.
const REMEMBERED: usize = 32;

/// The restore lock, held until dropped (or until the process exits, however
/// it exits: it's an advisory `flock` on a file in the state directory).
#[derive(Debug)]
pub struct RestoreLock {
    _file: File,
    restored: PathBuf,
}

impl RestoreLock {
    /// Takes the restore lock, waiting for as long as another rexurrect
    /// process holds it. Save and restore both start here, so this is also
    /// where the state directory is created, or made, private.
    pub fn acquire(state: &StateDir) -> Result<RestoreLock> {
        state.make_private()?;
        let path = state.join(LOCK);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("opening the restore lock {}", path.display()))?;
        file.lock()
            .with_context(|| format!("taking the restore lock {}", path.display()))?;
        Ok(RestoreLock {
            _file: file,
            restored: state.join(RESTORED),
        })
    }

    /// Whether a restore into `instance_id` has finished.
    pub fn is_restored(&self, instance_id: &str) -> Result<bool> {
        Ok(self
            .restored_instances()?
            .iter()
            .any(|id| id == instance_id))
    }

    /// Records that a restore into `instance_id` finished, forgetting the
    /// oldest instance once there are more than it remembers.
    pub fn record_restored(&self, instance_id: &str) -> Result<()> {
        let mut ids = self.restored_instances()?;
        ids.retain(|id| id != instance_id);
        ids.push(instance_id.to_string());
        let kept = &ids[ids.len().saturating_sub(REMEMBERED)..];
        let contents: String = kept.iter().map(|id| format!("{id}\n")).collect();
        state::write_file(&self.restored, contents.as_bytes())
    }

    fn restored_instances(&self) -> Result<Vec<String>> {
        let bytes = state::read_file(&self.restored)?.unwrap_or_default();
        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect())
    }
}
