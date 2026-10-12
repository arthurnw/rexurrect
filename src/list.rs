//! `rex-resurrect list`: the snapshots `restore` can use, newest first.

use std::io::Write;
use std::process::ExitCode;

use crate::error::Result;
use crate::state::StateDir;
use crate::store::SnapshotStore;

pub fn run() -> Result<ExitCode> {
    let store = SnapshotStore::new(&StateDir::from_env()?);
    let ids = store.list()?;
    let mut out = std::io::stdout().lock();
    if ids.is_empty() {
        let _ = writeln!(out, "no snapshots yet");
    }
    // One line per snapshot: `<id>  <time>  <session labels>`. A snapshot
    // that can't be read is still listed, so the others stay usable.
    for id in ids {
        let _ = match store.read(&id) {
            Ok(Some(snapshot)) => {
                let labels: Vec<&str> =
                    snapshot.sessions.iter().map(|s| s.label.as_str()).collect();
                let labels = if labels.is_empty() {
                    "(no sessions)".to_string()
                } else {
                    labels.join(", ")
                };
                writeln!(out, "{id}  {}  {labels}", snapshot.taken_at.display())
            }
            Ok(None) => continue,
            Err(e) => writeln!(out, "{id}  (unreadable: {e})"),
        };
    }
    Ok(ExitCode::SUCCESS)
}
