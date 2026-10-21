//! The app session (ADR-0004): a session the Rex app creates on its own
//! when it finds the server with no sessions. On Rex 1282 it does so even
//! while auto-restore is creating sessions in the same second, so every
//! restart would leave one untouched session beside the restored ones.
//!
//! Rex doesn't say who created a session, so rexurrect recognises the app
//! session only by what it isn't: restore didn't create it (it isn't in the
//! lineage map), and nothing in it is worth restoring (it's untouched, as
//! save judges it). Once auto-restore has created sessions, it looks for one
//! for a while, and closes the first it finds: the earliest-created, if
//! several turn up at once. At most one per server instance: the app
//! creates one, and a session the user has just opened looks the same.
//!
//! Looking writes nothing to the state directory. Each session is captured
//! the way save captures it, but neither its scrollback nor the programs
//! seen in its blocks are kept.

use std::fmt;
use std::time::{Duration, Instant};

use crate::capture::{self, CaptureContext};
use crate::error::Result;
use crate::lineage::LineageMap;
use crate::process;
use crate::rex::api::{ServerStatus, SessionSummary};
use crate::rex::{Rex, RexError};
use crate::save;
use crate::scrollback::NoScrollbackPrograms;
use crate::settings;
use crate::startup::StartingSessions;
use crate::state::StateDir;
use crate::timestamp::Timestamp;

/// How long auto-restore looks for the app session after restoring, in
/// seconds. Only tests set it.
const WATCH_VAR: &str = "REXURRECT_APP_SESSION_WATCH";
const DEFAULT_WATCH: Duration = Duration::from_secs(30);
/// The pause between looks.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// `REXURRECT_APP_SESSION_WATCH`, or the default of 30 seconds.
pub fn watch_length() -> Result<Duration> {
    settings::seconds(WATCH_VAR, DEFAULT_WATCH)
}

/// A live session taken for the app session.
#[derive(Debug)]
pub struct AppSession {
    pub label: String,
    pub session_id: String,
}

impl fmt::Display for AppSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "app session {:?} ({})", self.label, self.session_id)
    }
}

/// How looking for the app session ended.
#[derive(Debug)]
pub enum Watched {
    Closed(AppSession),
    /// None turned up in time.
    NoneFound,
    /// The server stopped first.
    ServerStopped,
    /// Rex refused to close it. It isn't tried again, and no other session
    /// is closed instead.
    CloseFailed(AppSession, RexError),
}

/// Looks for the app session on the server `server` (its `server.status`)
/// describes, about once a second for `watch`, and closes the first it
/// finds. A look that fails is skipped, and the next one tries again.
pub fn close_app_session(
    rex: &Rex,
    state: &StateDir,
    server: &ServerStatus,
    watch: Duration,
) -> Watched {
    let deadline = Instant::now() + watch;
    loop {
        if !process::is_running(server.pid) {
            return Watched::ServerStopped;
        }
        match rex.list_sessions() {
            Err(RexError::NotRunning { .. }) => return Watched::ServerStopped,
            Err(_) => {}
            Ok(live) => {
                if let Ok(Some(found)) = find_app_session(rex, state, server, &live) {
                    return match rex.destroy_session(&found.session_id) {
                        Ok(()) => Watched::Closed(found),
                        Err(e) => Watched::CloseFailed(found, e),
                    };
                }
            }
        }
        if Instant::now() >= deadline {
            return Watched::NoneFound;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The earliest-created session in `live` (`session.list` lists them in
/// creation order) that restore didn't create and that's untouched right
/// now. Fails if a session before it can't be captured: that one may be the
/// app session.
fn find_app_session(
    rex: &Rex,
    state: &StateDir,
    server: &ServerStatus,
    live: &[SessionSummary],
) -> Result<Option<AppSession>> {
    // Read at every look: a restore run by hand meanwhile records the
    // sessions it creates here.
    let lineages = LineageMap::load(state)?;
    // No snapshot is written: `taken_at` only names where its scrollback
    // would go, and none is saved.
    let taken_at = Timestamp::now();
    let context = CaptureContext::read(
        state,
        &server.host_id,
        taken_at,
        StartingSessions::load(state)?.at(taken_at),
        NoScrollbackPrograms::from_env()?,
    )?;
    for summary in live {
        if lineages.is_restored_session(&summary.session_id) {
            continue;
        }
        let view = rex.view_session(&summary.session_id)?;
        let lineage = lineages.lineage_of(&view.session_id);
        let captured = capture::saved_session(rex, &context, &view, lineage)?;
        if save::is_untouched(&captured.saved, &context.home) {
            return Ok(Some(AppSession {
                label: view.label,
                session_id: view.session_id,
            }));
        }
    }
    Ok(None)
}
