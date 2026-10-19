//! `rexurrect save`: snapshot every session on the server.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::capture::{self, SessionWithScrollback};
use crate::create;
use crate::error::{Context, Error, Result};
use crate::instance::RestoreLock;
use crate::lineage::LineageMap;
use crate::log;
use crate::restore;
use crate::retention::Retention;
use crate::rex::Rex;
use crate::rex::api::ServerStatus;
use crate::scrollback::{NoScrollbackPrograms, ScrollbackStore};
use crate::snapshot::{SavedLayout, SavedSession, Snapshot};
use crate::startup::{Starting, StartingSessions};
use crate::state::StateDir;
use crate::store::SnapshotStore;
use crate::timestamp::Timestamp;
use crate::too_large::TooLargeBlocks;
use crate::upkeep::{self, Live};

/// Save's settings, from the environment.
pub struct Settings {
    pub retention: Retention,
    pub no_scrollback: NoScrollbackPrograms,
}

impl Settings {
    pub fn from_env() -> Result<Settings> {
        Ok(Settings {
            retention: Retention::from_env()?,
            no_scrollback: NoScrollbackPrograms::from_env()?,
        })
    }
}

/// What a save did.
pub enum Saved {
    /// A new snapshot, now the latest.
    Snapshot { path: PathBuf, sessions: usize },
    /// The server is empty: nothing was written and the latest snapshot is
    /// unchanged.
    EmptyServer,
}

/// Restores first if rexurrect hasn't restored into this server instance yet,
/// so that a save on a freshly started server can't replace the latest
/// snapshot with a near-empty one. The blocks that restore created are
/// saved as it created them, like those of any restore in its startup
/// window: their shells are still starting. Still saves if some sessions
/// failed to restore, but then exits non-zero.
///
/// If every session it tried failed, it restored nothing, so it saves
/// nothing either: the latest snapshot stays as it is. It counts the server
/// instance as restored all the same, unlike `restore` and auto-restore:
/// otherwise every later save would restore first, fail again, and never
/// save anything on this server. Its error names the snapshot, to retry by
/// hand.
///
/// Holds the restore lock throughout: a save that finds a restore running
/// waits for it rather than saving a half-restored server.
pub fn run() -> Result<ExitCode> {
    // Configuration first: a bad setting fails before anything is restored.
    let settings = Settings::from_env()?;
    let rex = Rex::from_env()?;
    let state = StateDir::from_env()?;
    let lock = RestoreLock::acquire(&state)?;
    // Read once, under the lock: restoring first and saving agree on the
    // server instance, and on which host is local.
    let server = rex.server_status()?;
    let mut status = ExitCode::SUCCESS;
    if let Some(report) = restore::restore_unrestored_instance(&rex, &state, &lock, &server)? {
        println!("restored first: this server instance hadn't been restored into yet");
        let mut out = std::io::stdout().lock();
        status = report.finish(&state, "save (restoring first)", &mut out);
        if report.every_attempt_failed() {
            lock.record_restored(&server.instance_id)?;
            let snapshot = report.snapshot.as_deref().unwrap_or_default();
            return Err(Error::new(format!(
                "every session in snapshot {snapshot} failed to restore into this server \
                 instance, so nothing was saved and the latest snapshot is unchanged; the next \
                 save won't restore first, and saves what's live. To retry the restore, run \
                 `rexurrect restore --snapshot {snapshot}`"
            )));
        }
    }
    match save(&rex, &state, &server, settings)? {
        Saved::Snapshot { path, sessions } => {
            println!("saved {sessions} session(s) to {}", path.display());
        }
        Saved::EmptyServer => {
            println!("nothing to save: no sessions worth saving; the latest snapshot is unchanged");
        }
    }
    Ok(status)
}

/// Saves every session worth saving on the server `server` (its
/// `server.status`) describes.
pub fn save(
    rex: &Rex,
    state: &StateDir,
    server: &ServerStatus,
    settings: Settings,
) -> Result<Saved> {
    // Taken first: each kept session's scrollback is saved under the
    // snapshot's name as the walk reaches it, and startup windows are judged
    // as of then.
    let taken_at = Timestamp::now();
    let starting = StartingSessions::load(state)?.at(taken_at);
    let replaying: Vec<String> = starting.snapshots().map(str::to_string).collect();
    let saved = save_snapshot(rex, state, server, settings, taken_at, starting);
    // Scrollback is kept as long as its snapshot: old snapshots' goes with
    // them, and a save that wrote no snapshot (an empty server, a failure)
    // leaves none behind. A restore in its startup window keeps its
    // snapshot's longer: the replay may not have printed it yet, and saves
    // in the window save the restored blocks' scrollback from it.
    let removed = SnapshotStore::new(state)
        .list()
        .and_then(|mut kept| {
            kept.extend(replaying);
            ScrollbackStore::new(state).remove_orphans(&kept)
        })
        .with_context(|| "removing old scrollback");
    let saved = saved?;
    removed?;
    Ok(saved)
}

fn save_snapshot(
    rex: &Rex,
    state: &StateDir,
    server: &ServerStatus,
    settings: Settings,
    taken_at: Timestamp,
    starting: Starting,
) -> Result<Saved> {
    let lineages = LineageMap::load(state)?;
    let mut context = capture::CaptureContext::read(
        state,
        &server.host_id,
        taken_at,
        starting,
        settings.no_scrollback,
    )?;
    let mut live = Live::new();
    let mut sessions = Vec::new();
    let mut programs_seen = Vec::new();
    // (session, block) for each block saved without its too-large scrollback.
    let mut fallbacks = Vec::new();
    for summary in rex.list_sessions()? {
        let view = rex.view_session(&summary.session_id)?;
        live.add(&view);
        let lineage = lineages.lineage_of(&view.session_id);
        let session = format!("session {:?} ({})", view.label, view.session_id);
        let saving = || format!("saving {session}");
        let mut captured =
            capture::saved_session(rex, &context, &view, lineage).with_context(saving)?;
        programs_seen.append(&mut captured.programs_seen);
        if worth_saving(&captured.saved, &context.home) {
            let SessionWithScrollback { saved, too_large } = captured
                .with_scrollback(rex, &context.scrollback)
                .with_context(saving)?;
            fallbacks.extend(too_large.into_iter().map(|block| (session.clone(), block)));
            sessions.push(saved);
        }
    }
    // Remembered whether or not a snapshot is written: they're what the
    // blocks were seen running.
    let mut remembered = false;
    for (block_id, program) in &programs_seen {
        remembered |= context.defaults.set(block_id, program);
    }
    if remembered {
        context
            .defaults
            .save()
            .with_context(|| "remembering the programs seen in blocks")?;
    }
    if sessions.is_empty() {
        return Ok(Saved::EmptyServer);
    }
    let snapshot = Snapshot::new(taken_at, sessions);
    snapshot
        .validate()
        .with_context(|| "the snapshot failed validation, so nothing was written")?;
    let path = SnapshotStore::new(state).write(&snapshot, settings.retention)?;
    // Logged only now: until the snapshot is written, no block is saved,
    // with scrollback or without. Only the blocks that start falling back
    // with this save (see `too_large`).
    let known = TooLargeBlocks::load(state);
    for (session, block) in &fallbacks {
        if !known.contains(block.block_id()) {
            log::log_fallback(state, "save", &format!("{session}: {block}"));
        }
    }
    known
        .replace(
            fallbacks
                .iter()
                .map(|(_, block)| block.block_id().to_string())
                .collect(),
        )
        .with_context(|| {
            format!(
                "saved {}, but then failed to remember the blocks whose scrollback was too large",
                path.display()
            )
        })?;
    upkeep::forget_what_is_not_live(state, &live).with_context(|| {
        format!(
            "saved {}, but then failed to forget blocks and sessions that are gone",
            path.display()
        )
    })?;
    upkeep::remove_stale_temp_files(state).with_context(|| {
        format!(
            "saved {}, but then failed to remove temporary files that crashed writes left behind",
            path.display()
        )
    })?;
    Ok(Saved::Snapshot {
        path,
        sessions: snapshot.sessions.len(),
    })
}

/// Whether a live session has anything to restore. A server where no
/// session does is an empty server, and saving from it writes nothing.
///
/// A session that saved no windows has nothing to restore. Rex keeps a
/// session live, with no windows, after its last shell exits. Neither has an
/// untouched session.
fn worth_saving(session: &SavedSession, home: &Path) -> bool {
    !session.windows.is_empty() && !is_untouched(session, home)
}

/// An untouched session: one window holding one shell block, with default
/// window and block labels, its working directory still `home` and no last
/// command. Restoring it would bring back nothing a new session doesn't
/// have. Its label doesn't count: Rex's generated session labels can't be
/// told apart from user-set ones.
///
/// Judged from the session as saved: a last command counts however save
/// found it, including one restore carried forward, and "no last command"
/// means none that restore would type. A block saved as a plain shell in
/// home (a remote block, say) is judged as that plain shell.
fn is_untouched(session: &SavedSession, home: &Path) -> bool {
    let [window] = session.windows.as_slice() else {
        return false;
    };
    let SavedLayout::Block(block) = &window.layout else {
        return false;
    };
    window.label.is_none()
        && block.label.is_none()
        && create::typed_command(block).is_none()
        && same_directory(&block.working_directory, home)
}

/// Whether two paths name the same directory: equal, or equal once symlinks
/// are resolved (a shell's logical `/tmp/x` and its physical `/private/tmp/x`).
fn same_directory(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}
