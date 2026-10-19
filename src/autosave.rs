//! `rexurrect autosave install|uninstall`: run `save` on a timer.
//!
//! The scheduler (launchd today) is behind [`crate::scheduler::Scheduler`].

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::process::ExitCode;

use crate::account;
use crate::error::{Context, Error, Result};
use crate::log::Log;
use crate::retention::{KEEP_VAR, Retention};
use crate::rex;
use crate::scheduler::{self, Interval, Job};
use crate::scrollback::NO_SCROLLBACK_VAR;
use crate::settings;
use crate::state::{STATE_DIR_VAR, StateDir};

#[derive(Debug)]
pub enum AutosaveCommand {
    Install(InstallOptions),
    Uninstall,
}

#[derive(Debug)]
pub struct InstallOptions {
    pub interval: Interval,
}

impl Default for InstallOptions {
    fn default() -> Self {
        InstallOptions {
            interval: Interval::DEFAULT,
        }
    }
}

pub fn run(command: &AutosaveCommand) -> Result<ExitCode> {
    match command {
        AutosaveCommand::Install(options) => install(options),
        AutosaveCommand::Uninstall => uninstall(),
    }
}

fn install(options: &InstallOptions) -> Result<ExitCode> {
    let program = std::env::current_exe().with_context(|| "finding rexurrect's own path")?;
    let state = StateDir::from_env()?;
    let job = Job {
        program: utf8(program.as_os_str())?,
        args: vec!["save".to_string()],
        env: job_env(&state)?,
        interval: options.interval,
    };
    let scheduler = scheduler::for_this_platform()?;
    scheduler.install(&job)?;
    println!(
        "autosave installed: `rexurrect save` runs every {} as the {}\n\
         - server: {}\n\
         - state:  {}\n\
         - log:    {} (failures, and blocks whose scrollback was too large to save)\n\
         Install again after changing which server or state directory to use.",
        job.interval,
        scheduler.describe(),
        job.env[rex::SERVER_VAR],
        state.path().display(),
        Log::new(&state).path().display(),
    );
    Ok(ExitCode::SUCCESS)
}

/// The autosave job's environment. A scheduler starts its jobs with
/// almost none (launchd: no `REX_SERVER`, a bare `PATH`, nothing from the
/// user's shell), so the job gets what `save` depends on, as install finds
/// it:
/// - `REX_SERVER`: the server install was aimed at, else the Rex app's
///   server. Save passes it to `rex` with `-S`, so the job can't end up on
///   another server: not rex's default, which `XDG_RUNTIME_DIR` changes,
///   and not a fallback when the server isn't running.
/// - the state directory, as resolved here, so autosave shares snapshots,
///   lineage and recorder files with manual runs and the zsh recorder;
/// - `REXURRECT_KEEP`, if set, so autosave doesn't prune the snapshots the
///   user wants kept (checked here, so a bad value fails now rather than
///   every autosave);
/// - `REXURRECT_NO_SCROLLBACK`, if set (even empty), so autosave leaves out
///   the scrollback of the same programs as manual saves;
/// - `PATH`: the system directories launchd uses by default, and no `rex`.
///   Rex moves its CLI on updates, so save runs the `rex` a running Rex
///   server runs instead (ADR-0003).
fn job_env(state: &StateDir) -> Result<BTreeMap<String, String>> {
    let server = match settings::env_non_empty(rex::SERVER_VAR) {
        Some(server) => utf8(&server)?,
        None => rex::app_server_endpoint(&account::home()?),
    };
    let mut env = BTreeMap::new();
    env.insert(rex::SERVER_VAR.to_string(), server);
    env.insert(STATE_DIR_VAR.to_string(), utf8(state.path().as_os_str())?);
    Retention::from_env()?;
    if let Some(keep) = settings::env_non_empty(KEEP_VAR) {
        env.insert(KEEP_VAR.to_string(), utf8(&keep)?);
    }
    if let Some(programs) = std::env::var_os(NO_SCROLLBACK_VAR) {
        env.insert(NO_SCROLLBACK_VAR.to_string(), utf8(&programs)?);
    }
    env.insert("PATH".to_string(), SYSTEM_PATH.to_string());
    Ok(env)
}

const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

fn utf8(value: &OsStr) -> Result<String> {
    value
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::new(format!("{value:?} isn't valid UTF-8")))
}

/// Where the README explains auto-restore, including the line to remove.
const AUTO_RESTORE_DOCS: &str = "https://github.com/cemcatik/rexurrect#auto-restore";

fn uninstall() -> Result<ExitCode> {
    let scheduler = scheduler::for_this_platform()?;
    if scheduler.uninstall()? {
        println!("autosave uninstalled: removed the {}", scheduler.describe());
    } else {
        println!(
            "autosave wasn't installed: there was no {}",
            scheduler.describe()
        );
    }
    println!(
        "\
To back out of rexurrect completely:
- zsh recorder: remove the line that sources rexurrect.plugin.zsh from ~/.zshrc
  (or `rexurrect` from oh-my-zsh's plugins=(...)), then open new shells.
- auto-restore: remove the rexurrect line from Rex's init.lua; see
  {AUTO_RESTORE_DOCS}"
    );
    if let Ok(state) = StateDir::from_env() {
        println!(
            "- snapshots: they stay in {}; delete it if you don't need them.",
            state.path().display()
        );
    }
    Ok(ExitCode::SUCCESS)
}
