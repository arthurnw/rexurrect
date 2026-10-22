//! `rexurrect restore`: recreate a snapshot's sessions, the latest by default.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::ExitCode;

use crate::account;
use crate::create;
use crate::defaults::DefaultCommands;
use crate::error::{Context, Error, Result};
use crate::instance::RestoreLock;
use crate::lineage::{Lineage, LineageMap};
use crate::log;
use crate::rex::Rex;
use crate::rex::api::{CreatedSession, ServerStatus};
use crate::scrollback::Replay;
use crate::snapshot::{SavedSession, Snapshot};
use crate::startup::StartingSessions;
use crate::state::StateDir;
use crate::store::SnapshotStore;
use crate::timestamp::Timestamp;

#[derive(Debug, Default)]
pub struct RestoreOptions {
    /// Restore from this snapshot (an identifier `list` shows) instead of the latest.
    pub snapshot: Option<String>,
    /// Restore only the saved sessions with this label (every one of them).
    pub session: Option<String>,
    /// Print what would be created and skipped, and create nothing.
    pub dry_run: bool,
}

impl RestoreOptions {
    /// Whether this restore counts as restoring the server instance. A dry
    /// run creates nothing and a single-session restore leaves the other
    /// sessions out, so a save after either still restores first. Restoring
    /// a whole older snapshot counts.
    ///
    /// This deliberately departs from #15, which counts any restore: after
    /// `--session X` on a fresh server instance, the next autosave would
    /// otherwise replace the latest snapshot with one missing every other
    /// session, the very loss restoring first exists to prevent (story 48).
    fn restores_whole_snapshot(&self) -> bool {
        !self.dry_run && self.session.is_none()
    }
}

/// What happened to one saved session.
#[derive(Debug)]
pub enum Outcome {
    Created {
        label: String,
        session_id: String,
    },
    /// A dry run would create it.
    WouldCreate {
        label: String,
    },
    /// A session of its lineage is already live.
    Skipped {
        label: String,
        live_session_id: String,
    },
    Failed {
        label: String,
        error: String,
        /// The session restore created for it all the same, if it got that
        /// far. Its lineage is recorded, so no restore creates it again.
        session_id: Option<String>,
    },
}

#[derive(Debug, Default)]
pub struct RestoreReport {
    /// The identifier of the snapshot it restored from; `None` when there
    /// was no snapshot yet.
    pub snapshot: Option<String>,
    pub dry_run: bool,
    pub outcomes: Vec<Outcome>,
}

impl RestoreReport {
    /// Prints the report to `out`, logs each session that failed as a
    /// failure of `command`, and returns how the run should exit: non-zero
    /// if any session failed. Every command that restores ends this way.
    pub fn finish(&self, state: &StateDir, command: &str, out: &mut impl Write) -> ExitCode {
        let _ = self.print(out);
        self.log_failures(state, command);
        if self.failed() > 0 {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    }

    fn failed(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Failed { .. }))
    }

    /// Whether restore tried to create at least one session and every try
    /// failed before creating anything. Such a restore restored nothing, so
    /// it doesn't count as restoring the server instance, and a save after
    /// it mustn't replace the latest snapshot.
    pub fn every_attempt_failed(&self) -> bool {
        let attempted =
            self.count(|o| matches!(o, Outcome::Created { .. } | Outcome::Failed { .. }));
        attempted > 0 && !self.created_a_session()
    }

    /// Whether restore created at least one session, counting one that
    /// failed after it was created. Only then does auto-restore look for the
    /// app session.
    pub fn created_a_session(&self) -> bool {
        self.outcomes.iter().any(|o| {
            matches!(
                o,
                Outcome::Created { .. }
                    | Outcome::Failed {
                        session_id: Some(_),
                        ..
                    }
            )
        })
    }

    /// Appends each session that failed to the log, as a failure of `command`.
    fn log_failures(&self, state: &StateDir, command: &str) {
        for outcome in &self.outcomes {
            if let Outcome::Failed { label, error, .. } = outcome {
                log::log_failure(state, command, &format!("session {label:?}: {error}"));
            }
        }
    }

    fn count(&self, pred: impl Fn(&Outcome) -> bool) -> usize {
        self.outcomes.iter().filter(|o| pred(o)).count()
    }

    fn print(&self, out: &mut impl Write) -> std::io::Result<()> {
        if self.snapshot.is_none() {
            return writeln!(out, "nothing to restore: no snapshot yet");
        }
        for outcome in &self.outcomes {
            match outcome {
                Outcome::Created { label, session_id } => {
                    writeln!(out, "created  {label}  {session_id}")?
                }
                Outcome::WouldCreate { label } => writeln!(out, "would create  {label}")?,
                Outcome::Skipped {
                    label,
                    live_session_id,
                } => writeln!(out, "skipped  {label}  (live as {live_session_id})")?,
                Outcome::Failed { label, error, .. } => writeln!(out, "failed   {label}: {error}")?,
            }
        }
        if self.dry_run {
            return writeln!(
                out,
                "dry run: {} would be created, {} skipped; nothing was changed",
                self.count(|o| matches!(o, Outcome::WouldCreate { .. })),
                self.count(|o| matches!(o, Outcome::Skipped { .. })),
            );
        }
        writeln!(
            out,
            "restore: {} created, {} skipped, {} failed",
            self.count(|o| matches!(o, Outcome::Created { .. })),
            self.count(|o| matches!(o, Outcome::Skipped { .. })),
            self.failed(),
        )
    }
}

pub fn run(options: &RestoreOptions) -> Result<ExitCode> {
    let state = StateDir::from_env()?;
    let lock = RestoreLock::acquire(&state)?;
    let rex = Rex::from_env()?;
    let server = rex.server_status()?;
    let report = restore(&rex, &state, &lock, &server, options)?;
    Ok(report.finish(&state, "restore", &mut std::io::stdout().lock()))
}

/// Restores into the server's current instance, the one `server` (its
/// `server.status`, read under `lock`) names, unless rexurrect already has:
/// `None` then. `save` runs this first; auto-restore, once the server is
/// ready, needs nothing else.
pub fn restore_unrestored_instance(
    rex: &Rex,
    state: &StateDir,
    lock: &RestoreLock,
    server: &ServerStatus,
) -> Result<Option<RestoreReport>> {
    if lock.is_restored(&server.instance_id)? {
        return Ok(None);
    }
    restore(rex, state, lock, server, &RestoreOptions::default()).map(Some)
}

/// Restores a snapshot and, once that has finished, records the server
/// instance it restored into (`server`'s), unless `options` restore less
/// than a whole snapshot or every session it tried to create failed.
/// Sessions that failed don't stop it from finishing.
pub fn restore(
    rex: &Rex,
    state: &StateDir,
    lock: &RestoreLock,
    server: &ServerStatus,
    options: &RestoreOptions,
) -> Result<RestoreReport> {
    let report = restore_sessions(rex, state, options)?;
    if options.restores_whole_snapshot() && !report.every_attempt_failed() {
        lock.record_restored(&server.instance_id)?;
    }
    Ok(report)
}

/// Creates each saved session whose lineage has no live session (ADR-0001)
/// and records the new session's lineage, what it was created with for
/// saves in its startup window (see `startup`), and its blocks' default
/// last commands for saves after that (see `defaults`); a dry run only
/// reports what it would create. A session that fails, to create or to have
/// its blocks told apart, doesn't stop the others; errors that stop the
/// whole restore (no server, unreadable or unwritable state) are returned.
fn restore_sessions(
    rex: &Rex,
    state: &StateDir,
    options: &RestoreOptions,
) -> Result<RestoreReport> {
    let Some((snapshot_id, snapshot)) = chosen_snapshot(state, options)? else {
        return Ok(RestoreReport::default());
    };
    let selected = selected_sessions(&snapshot, options)?;
    let home = account::home()?;
    let live_sessions = rex.list_sessions()?;
    let mut lineages = LineageMap::load(state)?;
    let mut live: HashMap<Lineage, String> = live_sessions
        .iter()
        .map(|s| (lineages.lineage_of(&s.session_id), s.session_id.clone()))
        .collect();
    let is_live = |id: &str| live_sessions.iter().any(|s| s.session_id == id);
    lineages.retain_live(is_live);
    let mut starting = StartingSessions::load(state)?;
    starting.retain(Timestamp::now(), is_live);
    let mut defaults = DefaultCommands::load(state)?;
    let replay = Replay::new(state);
    let mut resuming = HashSet::new();

    let mut report = RestoreReport {
        snapshot: Some(snapshot_id.clone()),
        dry_run: options.dry_run,
        ..RestoreReport::default()
    };
    for saved in selected {
        let label = saved.label.clone();
        if let Some(live_session_id) = live.get(&saved.lineage) {
            report.outcomes.push(Outcome::Skipped {
                label,
                live_session_id: live_session_id.clone(),
            });
            continue;
        }
        if options.dry_run {
            report.outcomes.push(Outcome::WouldCreate { label });
            continue;
        }
        let as_created = create::as_created(saved, &home, &replay, &mut resuming);
        let created = match rex.create_session(&create::create_request(&as_created, &replay)) {
            Ok(created) => created,
            Err(e) => {
                report.outcomes.push(Outcome::Failed {
                    label,
                    error: e.to_string(),
                    session_id: None,
                });
                continue;
            }
        };
        // The session exists from here on, whatever else fails: record its
        // lineage first, so no restore creates it again.
        lineages.record(&created.session_id, saved.lineage.clone());
        lineages.save().with_context(|| {
            format!(
                "recording the lineage of restored session {}",
                created.session_id
            )
        })?;
        live.insert(saved.lineage.clone(), created.session_id.clone());
        // Restore no longer writes recorder files (#6 had it seed each new
        // block's with what it typed there, which kept the foreground
        // fallback from ever applying to a restored block): what it typed
        // is the block's default, which a program seen there replaces.
        let block_ids = new_block_ids(&as_created, &created);
        let recording = || {
            format!(
                "recording what restored session {} was created with",
                created.session_id
            )
        };
        if let Ok(block_ids) = &block_ids {
            let mut changed = false;
            for (block_id, block) in block_ids.iter().zip(as_created.blocks()) {
                if let Some(command) = &block.last_command {
                    changed |= defaults.set(block_id, command);
                }
            }
            if changed {
                defaults.save().with_context(recording)?;
            }
        }
        starting.record(
            &created.session_id,
            &snapshot_id,
            as_created,
            block_ids.as_ref().ok().cloned(),
        );
        starting.save().with_context(recording)?;
        report.outcomes.push(match block_ids {
            Ok(_) => Outcome::Created {
                label,
                session_id: created.session_id,
            },
            Err(e) => Outcome::Failed {
                label,
                error: format!(
                    "created as {}, but its new blocks couldn't be told apart, so their last \
                     commands won't carry forward: {e}",
                    created.session_id
                ),
                session_id: Some(created.session_id),
            },
        });
    }
    Ok(report)
}

/// The snapshot to restore from, with its identifier: the one asked for,
/// else the latest (`None` before the first save).
fn chosen_snapshot(
    state: &StateDir,
    options: &RestoreOptions,
) -> Result<Option<(String, Snapshot)>> {
    let store = SnapshotStore::new(state);
    let Some(id) = &options.snapshot else {
        return store.read_latest();
    };
    match store.read(id)? {
        Some(snapshot) => Ok(Some((id.clone(), snapshot))),
        None => Err(Error::new(format!(
            "no snapshot {id:?}: `rexurrect list` shows the snapshots"
        ))),
    }
}

/// The saved sessions to restore: every one, or every one with the label
/// asked for. Asking for a label the snapshot doesn't hold is an error.
fn selected_sessions<'a>(
    snapshot: &'a Snapshot,
    options: &RestoreOptions,
) -> Result<Vec<&'a SavedSession>> {
    let Some(label) = &options.session else {
        return Ok(snapshot.sessions.iter().collect());
    };
    let selected: Vec<_> = snapshot
        .sessions
        .iter()
        .filter(|s| s.label == *label)
        .collect();
    if selected.is_empty() {
        let which = match &options.snapshot {
            Some(id) => format!("snapshot {id}"),
            None => "the latest snapshot".to_string(),
        };
        let labels: Vec<&str> = snapshot.sessions.iter().map(|s| s.label.as_str()).collect();
        return Err(Error::new(format!(
            "no saved session labelled {label:?} in {which}, which holds: {}",
            labels.join(", ")
        )));
    }
    Ok(selected)
}

/// The IDs of the blocks `session.create` created for `as_created`, in
/// [`SavedSession::blocks`] order.
///
/// `session.create` returns each window's block IDs in layout order
/// (depth-first, `before` then `after`), the order `SavedLayout::blocks`
/// walks the saved layout in.
fn new_block_ids(as_created: &SavedSession, created: &CreatedSession) -> Result<Vec<String>> {
    if created.initial_windows.len() != as_created.windows.len() {
        return Err(Error::new(format!(
            "session.create returned {} windows for {} saved ones",
            created.initial_windows.len(),
            as_created.windows.len()
        )));
    }
    let mut ids = Vec::new();
    for (i, (window, created_window)) in as_created
        .windows
        .iter()
        .zip(&created.initial_windows)
        .enumerate()
    {
        let blocks = window.layout.blocks().len();
        if blocks != created_window.block_ids.len() {
            return Err(Error::new(format!(
                "session.create returned {} block IDs for window {i}, which has {blocks} saved blocks",
                created_window.block_ids.len(),
            )));
        }
        ids.extend(created_window.block_ids.iter().cloned());
    }
    Ok(ids)
}
