//! Upkeep after a save: forgetting blocks and sessions that aren't live, and
//! removing temporary files that crashed writes left behind.
//!
//! Recorder files, agent files and default last commands are keyed by block ID, lineage
//! entries and startup-window records by session ID, and Rex never reuses
//! either, so once a block or session is gone its file or entry can't matter
//! again. Save removes them only once it has written a snapshot. A save that
//! fails, or finds an empty server and writes nothing, removes nothing: what
//! it read can't be trusted to say what's gone.

use std::collections::HashSet;
use std::time::SystemTime;

use crate::agent::AgentFiles;
use crate::defaults::DefaultCommands;
use crate::error::Result;
use crate::lineage::LineageMap;
use crate::recorder::RecorderFiles;
use crate::rex::api::SessionView;
use crate::scrollback::ScrollbackStore;
use crate::startup::StartingSessions;
use crate::state::{self, StateDir};
use crate::store::SnapshotStore;
use crate::timestamp::Timestamp;

/// The sessions and blocks live on the server, as save read it.
#[derive(Debug)]
pub struct Live {
    /// When save started reading the server. A block that appeared after
    /// that is live but missing here.
    read_at: SystemTime,
    sessions: HashSet<String>,
    blocks: HashSet<String>,
}

impl Live {
    /// Call just before reading the server's sessions.
    pub fn new() -> Live {
        Live {
            read_at: SystemTime::now(),
            sessions: HashSet::new(),
            blocks: HashSet::new(),
        }
    }

    /// Adds a live session and every block in it, tiled or floating.
    pub fn add(&mut self, view: &SessionView) {
        self.sessions.insert(view.session_id.clone());
        let blocks = view
            .windows
            .iter()
            .flat_map(|w| &w.layers)
            .flat_map(|l| &l.blocks);
        self.blocks.extend(blocks.map(|b| b.block_id.clone()));
    }
}

/// Removes the temporary files that crashed writes left behind: in the state
/// directory, next to snapshots and scrollback, and in the recorder
/// directory. Only stale ones, never one a write still going on may rename.
pub fn remove_stale_temp_files(state: &StateDir) -> Result<()> {
    state::remove_stale_temp_files(state.path(), state::is_temp_name)?;
    SnapshotStore::new(state).remove_stale_temp_files()?;
    ScrollbackStore::new(state).remove_stale_temp_files()?;
    RecorderFiles::new(state).remove_stale_temp_files()?;
    AgentFiles::new(state).remove_stale_temp_files()
}

/// Removes the recorder files and default last commands of blocks, and the
/// lineage entries and startup-window records of sessions, that aren't
/// `live`, and startup-window records whose window has closed. A recorder
/// file changed since save started reading the server stays: it may belong
/// to a block that appeared since. The rest need no such care: only restore
/// and save write them, and they take turns.
pub fn forget_what_is_not_live(state: &StateDir, live: &Live) -> Result<()> {
    let is_live_block = |block_id: &str| live.blocks.contains(block_id);
    let is_live_session = |session_id: &str| live.sessions.contains(session_id);
    RecorderFiles::new(state).retain_live(is_live_block, live.read_at)?;
    AgentFiles::new(state).retain_live(is_live_block, live.read_at)?;
    let mut defaults = DefaultCommands::load(state)?;
    if defaults.retain_live(is_live_block) {
        defaults.save()?;
    }
    let mut lineages = LineageMap::load(state)?;
    if lineages.retain_live(is_live_session) {
        lineages.save()?;
    }
    let mut starting = StartingSessions::load(state)?;
    if starting.retain(Timestamp::now(), is_live_session) {
        starting.save()?;
    }
    Ok(())
}
