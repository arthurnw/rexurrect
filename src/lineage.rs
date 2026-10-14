//! Lineage (ADR-0001): how restore recognises a saved session that's already
//! live, without ever comparing labels.
//!
//! Rex assigns session IDs itself and allows duplicate labels, so rexurrect
//! keeps its own map, in the state directory, from each live session restore
//! created to the lineage of the saved session it came from. A live session
//! missing from the map (one the user or Rex created) starts a new lineage,
//! identified by its own session ID.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Context, Result};
use crate::state::{self, StateDir};

const FILE: &str = "lineage.json";

/// Identifies a lineage: the ID of the session that started it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Lineage(String);

/// Live session ID -> lineage, for the sessions restore created.
#[derive(Debug)]
pub struct LineageMap {
    path: PathBuf,
    restored: BTreeMap<String, Lineage>,
}

impl LineageMap {
    pub fn load(state: &StateDir) -> Result<LineageMap> {
        let path = state.join(FILE);
        let restored = match state::read_file(&path)? {
            Some(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("reading the lineage map {}", path.display()))?,
            None => BTreeMap::new(),
        };
        Ok(LineageMap { path, restored })
    }

    /// The lineage a live session belongs to.
    pub fn lineage_of(&self, session_id: &str) -> Lineage {
        self.restored
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| Lineage(session_id.to_string()))
    }

    /// Records that restore created `session_id` from a saved session of `lineage`.
    pub fn record(&mut self, session_id: &str, lineage: Lineage) {
        self.restored.insert(session_id.to_string(), lineage);
    }

    /// Forgets sessions that aren't live any more, and says whether there
    /// were any. Session IDs are never reused, so their entries can't matter
    /// again.
    pub fn retain_live(&mut self, is_live: impl Fn(&str) -> bool) -> bool {
        let before = self.restored.len();
        self.restored.retain(|session_id, _| is_live(session_id));
        self.restored.len() != before
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.restored).expect("the lineage map serializes");
        state::write_file(&self.path, &json)
    }
}
