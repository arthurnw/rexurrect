//! Each block's default last command: what a save saves as the last command
//! of a block whose recorder has recorded nothing (it has no recorder file)
//! while its shell sits at a prompt.
//!
//! It's the last program a save saw in the block's foreground, else the
//! command restore typed at the block's prompt when it created the block.
//! So without a recorder, a block keeps the program it was last seen
//! running after that program exits, and a restored block keeps the command
//! restore typed there, through any number of restores, until a save sees
//! a program in it. A block restore types nothing at has no default.
//!
//! Kept in rex-resurrect's own state, never in recorder files: those are
//! the recorders', so a recorder file always means a recorder wrote it, and
//! rex-resurrect can't race one. Only restore and save touch it, both under
//! the restore lock. A save that writes a snapshot forgets the blocks that
//! aren't live, as it does their recorder files.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::Result;
use crate::state::{self, StateDir};

const FILE: &str = "default-commands.json";

/// Block ID -> default last command.
#[derive(Debug)]
pub struct DefaultCommands {
    path: PathBuf,
    commands: BTreeMap<String, String>,
}

impl DefaultCommands {
    pub fn load(state: &StateDir) -> Result<DefaultCommands> {
        let path = state.join(FILE);
        Ok(DefaultCommands {
            commands: state::read_json(&path)?,
            path,
        })
    }

    pub fn get(&self, block_id: &str) -> Option<&str> {
        self.commands.get(block_id).map(String::as_str)
    }

    /// Makes `command` the block's default last command, and says whether
    /// that changed anything.
    pub fn set(&mut self, block_id: &str, command: &str) -> bool {
        if self.get(block_id) == Some(command) {
            return false;
        }
        self.commands
            .insert(block_id.to_string(), command.to_string());
        true
    }

    /// Forgets the blocks `is_live` says aren't live, and says whether there
    /// were any. Block IDs are never reused, so their entries can't matter
    /// again.
    pub fn retain_live(&mut self, is_live: impl Fn(&str) -> bool) -> bool {
        let before = self.commands.len();
        self.commands.retain(|block_id, _| is_live(block_id));
        self.commands.len() != before
    }

    pub fn save(&self) -> Result<()> {
        state::write_json(&self.path, &self.commands)
    }
}
