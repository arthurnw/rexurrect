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
//! Kept in rexurrect's own state, never in recorder files: those are the
//! recorders', so a recorder file always means a recorder wrote it, and
//! rexurrect can't race one. Only restore and save touch it, both under the
//! restore lock. A save that writes a snapshot forgets the blocks that
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

    /// The block's default last command. Never one an earlier version saved
    /// from `ps`, with its control characters escaped: that isn't what ran
    /// (see [`is_escaped_by_ps`]).
    pub fn get(&self, block_id: &str) -> Option<&str> {
        self.commands
            .get(block_id)
            .map(String::as_str)
            .filter(|command| !is_escaped_by_ps(command))
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

/// Whether `command` holds `\` and three octal digits: how `ps`, which
/// earlier versions read the foreground program from, prints a control
/// character (a line break as `\012`). Such a default last command isn't what
/// ran, so it's treated as absent. A program seen with that text really in its
/// arguments is rarely dropped with it: the cost is an empty prompt.
/// Recorded commands aren't checked: `printf '\033[0m'` is typed by hand.
fn is_escaped_by_ps(command: &str) -> bool {
    command.as_bytes().windows(4).any(|window| {
        window[0] == b'\\'
            && window[1..]
                .iter()
                .all(|digit| (b'0'..=b'7').contains(digit))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_escapes_are_recognised() {
        assert!(is_escaped_by_ps(r"sh -c tail -f /dev/null\012true"));
        assert!(is_escaped_by_ps(r"printf a\011b"));
        assert!(!is_escaped_by_ps(r"printf '\n'"));
        assert!(!is_escaped_by_ps(r"grep \089"));
        assert!(!is_escaped_by_ps("vim notes.txt"));
    }
}
