//! rexurrect's log: `rexurrect.log` in the state directory.
//!
//! `save` and `restore` append their failures here, one timestamped line
//! each, so a failed autosave, which nobody watches, can still be diagnosed.
//! Successful runs write nothing, with two exceptions:
//! - auto-restore, which nobody watches either, logs every outcome;
//! - save logs each block it starts saving without scrollback because Rex
//!   refused to hand it over as too large: the save succeeds, but that
//!   block's scrollback is lost. Once per block, not at every save while it
//!   stays too large (see `too_large`).

use std::fmt::Display;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::error::{Context, Result};
use crate::state::{self, StateDir};
use crate::timestamp::Timestamp;

const LOG: &str = "rexurrect.log";
/// The size past which the log keeps only its newest half. While Rex is
/// quit, every autosave appends a line, so it would otherwise grow forever.
const MAX_SIZE: u64 = 1 << 20;

#[derive(Debug)]
pub struct Log {
    state: StateDir,
    path: PathBuf,
}

impl Log {
    pub fn new(state: &StateDir) -> Log {
        Log {
            state: state.clone(),
            path: state.join(LOG),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `<UTC timestamp> <message>` as one line: line breaks in
    /// `message` become `; `. Creates the log (`0600`) and the state
    /// directory (private) if needed. A log that has grown past a megabyte
    /// keeps only its newest half.
    pub fn append(&self, message: &str) -> Result<()> {
        self.state.make_private()?;
        let message: Vec<&str> = message
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let entry = format!("{} {}\n", Timestamp::now().rfc3339(), message.join("; "));
        let size = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)
            .and_then(|mut log| {
                log.write_all(entry.as_bytes())?;
                Ok(log.metadata()?.len())
            })
            .with_context(|| format!("appending to {}", self.path.display()))?;
        if size > MAX_SIZE {
            self.keep_newest_half()?;
        }
        Ok(())
    }

    /// Rewrites the log with only its newest half, starting at a whole
    /// line. Atomically, and private, like any other state file. An entry
    /// another run appends while this rewrites is lost; that takes two runs
    /// at once on a log just past its limit.
    fn keep_newest_half(&self) -> Result<()> {
        let Some(log) = state::read_file(&self.path)? else {
            return Ok(());
        };
        let half = log.len() - log.len() / 2;
        let start = log[half..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(log.len(), |newline| half + newline + 1);
        state::write_file(&self.path, &log[start..])
    }
}

/// Appends "`<command>` failed: `<failure>`" to the log in `state`. Best
/// effort: if the log can't be written, says so on stderr and carries on,
/// so a broken log never hides the failure itself.
pub fn log_failure(state: &StateDir, command: &str, failure: &dyn Display) {
    if let Err(e) = Log::new(state).append(&format!("{command} failed: {failure}")) {
        eprintln!("rexurrect: couldn't log a failure: {e}");
    }
}

/// Appends "`<command>` fell back: `<fallback>`" to the log in `state`:
/// something `command` couldn't do, which it worked around without failing.
/// Best effort, like [`log_failure`].
pub fn log_fallback(state: &StateDir, command: &str, fallback: &dyn Display) {
    if let Err(e) = Log::new(state).append(&format!("{command} fell back: {fallback}")) {
        eprintln!("rexurrect: couldn't log a fallback: {e}");
    }
}
