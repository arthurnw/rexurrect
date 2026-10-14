//! Running a job on a timer: the one interface autosave installs through.
//!
//! launchd (macOS) is the only scheduler today ([`launchd::Launchd`]). A
//! systemd user timer for Linux would be a second [`Scheduler`], picked in
//! [`for_this_platform`].

mod launchd;

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;

use crate::error::Result;
use crate::settings;

/// What a scheduler runs: a program with its arguments, every [`Interval`].
#[derive(Debug)]
pub struct Job {
    /// An absolute path: schedulers don't search `PATH` for it.
    pub program: String,
    pub args: Vec<String>,
    /// Variables set for the job, on top of the scheduler's own.
    pub env: BTreeMap<String, String>,
    pub interval: Interval,
}

/// Installs one job, the autosave, and removes it again.
pub trait Scheduler {
    /// What the job is installed as, for messages: "launchd agent …".
    fn describe(&self) -> String;

    /// Installs `job` and starts its timer, replacing the installed job if
    /// there is one, so installing twice leaves exactly one.
    fn install(&self, job: &Job) -> Result<()>;

    /// Stops the job and removes it. `false` if there was nothing to remove.
    fn uninstall(&self) -> Result<bool>;
}

/// The scheduler for the platform rexurrect runs on.
pub fn for_this_platform() -> Result<Box<dyn Scheduler>> {
    Ok(Box::new(launchd::Launchd::from_env()?))
}

/// How often a job runs: a whole number of minutes, at least 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    minutes: NonZeroU32,
}

impl Interval {
    /// Every 15 minutes, like tmux-continuum.
    pub const DEFAULT: Interval = Interval {
        minutes: NonZeroU32::new(15).unwrap(),
    };

    /// Parses a whole number of minutes, at least 1 (`"15"`).
    pub fn parse_minutes(minutes: &str) -> Option<Interval> {
        settings::whole_number_at_least_1(minutes)
            .and_then(|minutes| NonZeroU32::try_from(minutes).ok())
            .map(|minutes| Interval { minutes })
    }

    pub fn seconds(&self) -> u64 {
        u64::from(self.minutes.get()) * 60
    }
}

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.minutes.get() {
            1 => f.write_str("minute"),
            n => write!(f, "{n} minutes"),
        }
    }
}
