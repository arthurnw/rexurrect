//! Recorder files: each block's last command, as recorded by the shell
//! recorder (`rex-resurrect.plugin.zsh` at the repo root). They belong to
//! recorders: rex-resurrect reads them and removes those of blocks that
//! aren't live, but never writes one, so a recorder file always means a
//! recorder wrote it, and save can't race a recorder. What rex-resurrect
//! knows about a block besides (what restore typed there, the last program a
//! save saw in it) lives in its own state (see `defaults`).
//!
//! The format is shared by every recorder, whatever its shell:
//! - one file per block, `recorder/<block ID>` in the state directory, named
//!   with the block ID exactly as Rex gives it (`block:<uuid>`, the value of
//!   `REX_BLOCK` in the block's shell);
//! - its contents: the block's last command, UTF-8 text on one line, ending
//!   in a newline (accepted without one);
//! - an empty line (or an empty file) records that the block has **no last
//!   command**, as firmly as a command: save then neither falls back to the
//!   program in its foreground nor uses its default. The zsh recorder never
//!   writes one (it only records commands), but a recorder may. Restores by
//!   earlier versions of rex-resurrect wrote them too, and recorder files
//!   holding commands, for the blocks they created: these still count as
//!   recorders' records;
//! - written to a temporary file whose name starts with `.` in the same
//!   directory, then renamed over the record. Such a file left unchanged
//!   for over an hour counts as left behind by a recorder that crashed, and
//!   a save removes it;
//! - the directory is private (0700): commands can hold secrets.
//!
//! Recorders only record single-line commands, but restore doesn't rely on
//! that (see `create::typed_command`).

use std::path::PathBuf;
use std::time::SystemTime;

use crate::error::{Context, Result};
use crate::state::{self, StateDir};

const DIR: &str = "recorder";

pub struct RecorderFiles {
    dir: PathBuf,
}

/// What a block's recorder file says.
#[derive(Debug, PartialEq, Eq)]
pub enum Recorded {
    /// There's no recorder file (or it isn't UTF-8 text): nothing has
    /// recorded anything for the block.
    Nothing,
    /// The block has no last command.
    NoLastCommand,
    LastCommand(String),
}

impl RecorderFiles {
    pub fn new(state: &StateDir) -> RecorderFiles {
        RecorderFiles {
            dir: state.join(DIR),
        }
    }

    /// What the block's recorder file says.
    pub fn read(&self, block_id: &str) -> Result<Recorded> {
        let Some(path) = self.file(block_id) else {
            return Ok(Recorded::Nothing);
        };
        let Some(bytes) = state::read_file(&path)? else {
            return Ok(Recorded::Nothing);
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(Recorded::Nothing);
        };
        let command = text.strip_suffix('\n').unwrap_or(&text);
        Ok(if command.is_empty() {
            Recorded::NoLastCommand
        } else {
            Recorded::LastCommand(command.to_string())
        })
    }

    /// Removes the records of blocks `is_live` says aren't live, unless they
    /// changed at or after `kept_since`. Leaves alone temporary files (named
    /// `.…`), which a recorder may be about to rename over a record, and
    /// anything that isn't a file.
    pub fn retain_live(
        &self,
        is_live: impl Fn(&str) -> bool,
        kept_since: SystemTime,
    ) -> Result<()> {
        for entry in state::read_dir_or_empty(&self.dir)? {
            let name = entry.file_name();
            let Some(block_id) = name.to_str() else {
                continue;
            };
            let Some(path) = self.file(block_id) else {
                continue;
            };
            if is_live(block_id) {
                continue;
            }
            let reading = || format!("reading {}", path.display());
            let metadata = entry.metadata().with_context(reading)?;
            if !metadata.is_file() || metadata.modified().with_context(reading)? >= kept_since {
                continue;
            }
            state::remove_if_exists(&path)?;
        }
        Ok(())
    }

    /// Removes the temporary files (named `.…`) that recorders crashed
    /// before renaming over a record (see
    /// [`state::remove_stale_temp_files`]).
    pub fn remove_stale_temp_files(&self) -> Result<()> {
        state::remove_stale_temp_files(&self.dir, |name| name.starts_with('.'))
    }

    /// The block's recorder file. `None` for an ID that isn't a plain file
    /// name, so no ID can reach outside the recorder directory.
    fn file(&self, block_id: &str) -> Option<PathBuf> {
        state::is_plain_file_name(block_id).then(|| self.dir.join(block_id))
    }
}
