//! The snapshot model: what `save` writes and `restore` reads.
//!
//! Only rexurrect reads this format (tests never look inside), so it can
//! change freely. Optional fields are skipped when empty so that fields added
//! later don't break older snapshots.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::lineage::Lineage;
use crate::rex::api::Direction;
use crate::timestamp::Timestamp;

/// Bumped on incompatible changes to this format.
pub const FORMAT: u32 = 1;

/// Every saved session at one point in time.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub format: u32,
    /// Stored as RFC 3339, UTC.
    pub taken_at: Timestamp,
    pub sessions: Vec<SavedSession>,
}

impl Snapshot {
    pub fn new(taken_at: Timestamp, sessions: Vec<SavedSession>) -> Snapshot {
        Snapshot {
            format: FORMAT,
            taken_at,
            sessions,
        }
    }

    /// Checks what restore relies on: every session has at least one window
    /// and every split ratio is in (0, 1). That every layout node is exactly
    /// one of a block or a split holds by construction ([`SavedLayout`]).
    pub fn validate(&self) -> Result<()> {
        for session in &self.sessions {
            if session.windows.is_empty() {
                return Err(Error::new(format!(
                    "session {:?} has no windows",
                    session.label
                )));
            }
            for (i, window) in session.windows.iter().enumerate() {
                validate_layout(&window.layout).map_err(|e| {
                    Error::new(format!(
                        "session {:?}, window {}: {e}",
                        session.label,
                        i + 1
                    ))
                })?;
            }
        }
        Ok(())
    }
}

fn validate_layout(layout: &SavedLayout) -> Result<(), String> {
    match layout {
        SavedLayout::Block(_) => Ok(()),
        SavedLayout::Split(split) => {
            // Written so that NaN fails too.
            if !(split.ratio > 0.0 && split.ratio < 1.0) {
                return Err(format!("split ratio {} isn't between 0 and 1", split.ratio));
            }
            validate_layout(&split.before)?;
            validate_layout(&split.after)
        }
    }
}

/// A session as recorded in a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    /// Which lineage this session belongs to (ADR-0001). Restore skips it
    /// while a session of the same lineage is live.
    pub lineage: Lineage,
    /// Always saved: Rex's generated session labels can't be told apart
    /// from user-set ones.
    pub label: String,
    /// In window order.
    pub windows: Vec<SavedWindow>,
}

impl SavedSession {
    /// Every block, window by window, in [`SavedLayout::blocks`] order.
    pub fn blocks(&self) -> impl Iterator<Item = &SavedBlock> {
        self.windows
            .iter()
            .flat_map(|window| window.layout.blocks())
    }

    /// [`SavedSession::blocks`], to change.
    pub fn blocks_mut(&mut self) -> impl Iterator<Item = &mut SavedBlock> {
        self.windows
            .iter_mut()
            .flat_map(|window| window.layout.blocks_mut())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedWindow {
    /// Only a user-set label; omitted so Rex generates a fresh default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The window's tiled layout.
    pub layout: SavedLayout,
}

/// A layout node: exactly one of a block or a split, by construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedLayout {
    Block(SavedBlock),
    Split(SavedSplit),
}

impl SavedLayout {
    /// The layout's blocks in depth-first, `before`-then-`after` order.
    pub fn blocks(&self) -> Vec<&SavedBlock> {
        match self {
            SavedLayout::Block(block) => vec![block],
            SavedLayout::Split(split) => {
                let mut blocks = split.before.blocks();
                blocks.extend(split.after.blocks());
                blocks
            }
        }
    }

    /// [`SavedLayout::blocks`], to change.
    pub fn blocks_mut(&mut self) -> Vec<&mut SavedBlock> {
        match self {
            SavedLayout::Block(block) => vec![block],
            SavedLayout::Split(split) => {
                let mut blocks = split.before.blocks_mut();
                blocks.extend(split.after.blocks_mut());
                blocks
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSplit {
    pub direction: Direction,
    /// `before`'s share, in (0, 1).
    pub ratio: f64,
    pub before: Box<SavedLayout>,
    pub after: Box<SavedLayout>,
}

/// A shell block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedBlock {
    /// Only a user-set label; omitted so Rex uses its default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub working_directory: PathBuf,
    /// The block's recorder file, as found: it may not be typeable (see
    /// `create::typed_command`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command: Option<String>,
    /// The block's saved scrollback: a file's path relative to the state
    /// directory (see `scrollback`). Absent for a block that was showing a
    /// full-screen program, or had nothing to save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollback: Option<PathBuf>,
}
