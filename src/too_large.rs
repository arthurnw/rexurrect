//! The blocks the last save saved without scrollback because Rex refused to
//! hand it over as too large (see `scrollback`).
//!
//! A block's scrollback stays too large save after save until it's cleared,
//! and autosave saves every 15 minutes. So save logs a block when it starts
//! falling back, not at every save after that: only a block that isn't here
//! yet. A block whose scrollback is saved again, or that's gone, drops out,
//! and is logged again if it falls back again.
//!
//! Only save touches it, under the restore lock, and only once it has
//! written its snapshot: a save that writes none saved no block. It only
//! decides what's logged, so an unreadable file counts as empty (at worst a
//! block is logged again) rather than failing the save.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::error::Result;
use crate::state::{self, StateDir};

const FILE: &str = "too-large-scrollback.json";

/// Block IDs.
#[derive(Debug)]
pub struct TooLargeBlocks {
    path: PathBuf,
    block_ids: BTreeSet<String>,
}

impl TooLargeBlocks {
    pub fn load(state: &StateDir) -> TooLargeBlocks {
        let path = state.join(FILE);
        TooLargeBlocks {
            block_ids: state::read_json(&path).unwrap_or_default(),
            path,
        }
    }

    pub fn contains(&self, block_id: &str) -> bool {
        self.block_ids.contains(block_id)
    }

    /// Replaces the blocks with `block_ids`. Writes nothing if that changes
    /// nothing, so a state directory where no block ever fell back never
    /// gets the file.
    pub fn replace(self, block_ids: BTreeSet<String>) -> Result<()> {
        if block_ids == self.block_ids {
            return Ok(());
        }
        state::write_json(&self.path, &block_ids)
    }
}
