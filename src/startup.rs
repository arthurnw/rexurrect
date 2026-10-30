//! The startup window: the first two minutes after restore creates a
//! session, while its blocks' shells may still be starting.
//!
//! A save can't read such a block as it is. Its shell may not have reported
//! a working directory yet, its startup (a slow `.zshrc`) may be running in
//! its foreground, and the scrollback replay may still be printing. Nothing
//! the user did can have reached it yet either, unless its recorder has
//! recorded a command. So a save in the window saves it as restore created
//! it.
//!
//! Restore records each session it creates in the state directory: when,
//! from which snapshot, the session as restore created it, and its blocks'
//! IDs. Every save reads that, whichever process restored: auto-restore,
//! `restore`, or the restore a save runs first. Both only touch it under the
//! restore lock. Restore forgets sessions whose window has closed, or that
//! aren't live, whenever it records new ones, and so does a save that writes
//! a snapshot.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::snapshot::{SavedBlock, SavedSession};
use crate::state::{self, StateDir};
use crate::timestamp::Timestamp;

const FILE: &str = "starting-sessions.json";

/// How long after restore creates a session its blocks count as starting:
/// far longer than any shell takes to start.
pub const WINDOW: Duration = Duration::from_secs(2 * 60);

/// A session restore created, as a save in its startup window needs it.
#[derive(Debug, Serialize, Deserialize)]
struct StartingSession {
    /// When restore created it.
    restored_at: Timestamp,
    /// The identifier of the snapshot it was restored from.
    snapshot: String,
    /// The session as restore created it (`create::as_created`).
    created: SavedSession,
    /// The ID of each of `created`'s blocks, in [`SavedSession::blocks`]
    /// order. Absent when restore couldn't tell which new block is which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    block_ids: Option<Vec<String>>,
}

impl StartingSession {
    /// Whether `now` is in its window. A clock that has gone back to before
    /// restore created it closes the window: its blocks are then read as
    /// they are, as before there was one.
    fn is_starting_at(&self, now: Timestamp) -> bool {
        now.since(self.restored_at)
            .is_some_and(|elapsed| elapsed < WINDOW)
    }
}

/// The sessions restore created whose startup window may still be open, by
/// session ID.
#[derive(Debug)]
pub struct StartingSessions {
    path: PathBuf,
    sessions: BTreeMap<String, StartingSession>,
}

impl StartingSessions {
    pub fn load(state: &StateDir) -> Result<StartingSessions> {
        let path = state.join(FILE);
        Ok(StartingSessions {
            sessions: state::read_json(&path)?,
            path,
        })
    }

    /// Records the session `session_id` that restore just created from the
    /// snapshot `snapshot`: `created`, as restore created it, with its
    /// blocks' IDs if restore could tell them apart.
    pub fn record(
        &mut self,
        session_id: &str,
        snapshot: &str,
        created: SavedSession,
        block_ids: Option<Vec<String>>,
    ) {
        let session = StartingSession {
            restored_at: Timestamp::now(),
            snapshot: snapshot.to_string(),
            created,
            block_ids,
        };
        self.sessions.insert(session_id.to_string(), session);
    }

    /// Forgets the sessions whose window has closed by `now`, or that
    /// `is_live` says aren't live, and says whether there were any.
    pub fn retain(&mut self, now: Timestamp, is_live: impl Fn(&str) -> bool) -> bool {
        let before = self.sessions.len();
        self.sessions
            .retain(|session_id, session| session.is_starting_at(now) && is_live(session_id));
        self.sessions.len() != before
    }

    pub fn save(&self) -> Result<()> {
        state::write_json(&self.path, &self.sessions)
    }

    /// What's starting at `now`.
    pub fn at(self, now: Timestamp) -> Starting {
        let mut starting = Starting::default();
        for (session_id, session) in self.sessions {
            if !session.is_starting_at(now) {
                continue;
            }
            starting.snapshots.insert(session.snapshot);
            match session.block_ids {
                Some(ids) => starting
                    .blocks
                    .extend(ids.into_iter().zip(session.created.blocks().cloned())),
                None => {
                    starting.unmatched.insert(session_id, session.created);
                }
            }
        }
        starting
    }
}

/// The sessions and blocks restore created that are starting at one moment,
/// as restore created them.
#[derive(Debug, Default)]
pub struct Starting {
    /// By block ID.
    blocks: HashMap<String, SavedBlock>,
    /// The sessions whose blocks restore couldn't tell apart, by session ID.
    unmatched: HashMap<String, SavedSession>,
    /// The snapshots the starting sessions were restored from.
    snapshots: BTreeSet<String>,
}

impl Starting {
    /// The block `block_id` as restore created it, if it's starting.
    pub fn block(&self, block_id: &str) -> Option<&SavedBlock> {
        self.blocks.get(block_id)
    }

    /// The session `session_id` as restore created it, if it's starting
    /// and restore couldn't tell its blocks apart.
    pub fn unmatched_session(&self, session_id: &str) -> Option<&SavedSession> {
        self.unmatched.get(session_id)
    }

    /// The identifiers of the snapshots the starting sessions were restored
    /// from. Their scrollback is still in use: the replay may not have
    /// printed it yet, and a save saves the starting blocks' from it.
    pub fn snapshots(&self) -> impl Iterator<Item = &str> {
        self.snapshots.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> Timestamp {
        serde_json::from_value(json!(text)).unwrap()
    }

    #[test]
    fn a_session_is_starting_for_two_minutes_after_restore_created_it() {
        let session: StartingSession = serde_json::from_value(json!({
            "restored_at": "2026-10-12T10:00:00.000000Z",
            "snapshot": "20261012T095000.000000Z",
            "created": { "lineage": "session:zz", "label": "zz", "windows": [] },
        }))
        .unwrap();
        for (now, starting) in [
            ("2026-10-12T09:59:59.999999Z", false),
            ("2026-10-12T10:00:00.000000Z", true),
            ("2026-10-12T10:01:59.999999Z", true),
            ("2026-10-12T10:02:00.000000Z", false),
        ] {
            assert_eq!(session.is_starting_at(at(now)), starting, "at {now}");
        }
    }
}
