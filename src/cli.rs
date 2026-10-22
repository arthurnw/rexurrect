//! Command-line parsing.
//!
//! Each subcommand with options parses them into the options struct its module
//! defines. To add a subcommand: a `Command` variant, a line in `parse`, a
//! `parse_<name>` function and a line in `USAGE`.

use std::ffi::OsString;
use std::fmt;

use crate::agent::Agent;
use crate::auto_restore::AutoRestoreOptions;
use crate::autosave::{AutosaveCommand, InstallOptions};
use crate::restore::RestoreOptions;
use crate::scheduler::Interval;

pub enum Command {
    Save,
    Restore(RestoreOptions),
    List,
    Autosave(AutosaveCommand),
    AutoRestore(AutoRestoreOptions),
    AgentHook(Agent),
    Help,
    Version,
}

pub const USAGE: &str = "\
Save Rex sessions and recreate them after the Rex server stops.

Usage: rexurrect <command> [options]

Commands:
  save      Save every session on the Rex server to a new snapshot
  restore   Recreate the latest snapshot's sessions that aren't live
  list      List the snapshots, newest first, with their session labels
  autosave install
            Run `save` every 15 minutes in the background (a launchd agent)
  autosave uninstall
            Stop autosave, and say how to remove the rest of rexurrect
  auto-restore
            For Rex's init.lua: once the server running it is ready, restore
            into it in the background, once per server instance
  agent-hook <agent>
            For an agent's hooks (pi, claude, codex): record the conversation
            it has open in this block, so restore resumes it there
  help      Show this help

Restore options:
  --snapshot <id>    Restore from this snapshot (an ID from `list`) instead
                     of the latest
  --session <label>  Restore only the saved sessions with this label
  -n, --dry-run      Print what would be created and skipped; create nothing

Autosave install options:
  --interval <minutes>  Save this often instead (default: 15)

Auto-restore options:
  --print-hook        Print the line that turns auto-restore on, to append to
                      Rex's init.lua; it pins this state directory
  --server-pid <pid>  Wait for the Rex server with this pid and restore into
                      it, in the foreground (what the background run does)

Options:
  -h, --help     Show this help
  -V, --version  Show the version

Environment:
  REX_SERVER               The Rex server to talk to (default: rex's own default)
  REXURRECT_STATE_DIR      Where snapshots and state live (default:
                           $XDG_STATE_HOME/rexurrect, else
                           ~/.local/state/rexurrect)
  REXURRECT_KEEP           How many snapshots save keeps, newest first
                           (default: 10)
  REXURRECT_NO_SCROLLBACK  Programs, separated by spaces, whose blocks save
                           keeps no scrollback for (default: claude)
  REXURRECT_READY_TIMEOUT  How long auto-restore waits for the Rex server to
                           be ready, in seconds (default: 60)
";

#[derive(Debug)]
pub struct UsageError(String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut args = Args::new(args)?;
    if args.wants_help() {
        return Ok(Command::Help);
    }
    let Some(command) = args.next() else {
        return Err(UsageError("missing command".into()));
    };
    match command.as_str() {
        "save" => no_options(&mut args, "save").map(|()| Command::Save),
        "restore" => parse_restore(&mut args).map(Command::Restore),
        "list" => no_options(&mut args, "list").map(|()| Command::List),
        "autosave" => parse_autosave(&mut args).map(Command::Autosave),
        "auto-restore" => parse_auto_restore(&mut args).map(Command::AutoRestore),
        "agent-hook" => parse_agent_hook(&mut args).map(Command::AgentHook),
        "help" => Ok(Command::Help),
        "-V" | "--version" | "version" => Ok(Command::Version),
        other => Err(UsageError(format!("unknown command {other:?}"))),
    }
}

/// For a command that takes no options or arguments.
fn no_options(args: &mut Args, command: &str) -> Result<(), UsageError> {
    match args.next() {
        Some(arg) => Err(args.unexpected(&arg, command)),
        None => Ok(()),
    }
}

fn parse_restore(args: &mut Args) -> Result<RestoreOptions, UsageError> {
    let mut options = RestoreOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--snapshot" => options.snapshot = Some(args.value(&arg)?),
            "--session" => options.session = Some(args.value(&arg)?),
            "-n" | "--dry-run" => options.dry_run = true,
            _ => return Err(args.unexpected(&arg, "restore")),
        }
    }
    Ok(options)
}

fn parse_autosave(args: &mut Args) -> Result<AutosaveCommand, UsageError> {
    let Some(action) = args.next() else {
        return Err(UsageError("autosave needs `install` or `uninstall`".into()));
    };
    match action.as_str() {
        "install" => {
            let mut options = InstallOptions::default();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--interval" => {
                        let minutes = args.value(&arg)?;
                        options.interval = Interval::parse_minutes(&minutes).ok_or_else(|| {
                            UsageError(format!(
                                "--interval takes a whole number of minutes, at least 1, not {minutes:?}"
                            ))
                        })?;
                    }
                    _ => return Err(args.unexpected(&arg, "autosave install")),
                }
            }
            Ok(AutosaveCommand::Install(options))
        }
        "uninstall" => match args.next() {
            Some(arg) => Err(args.unexpected(&arg, "autosave uninstall")),
            None => Ok(AutosaveCommand::Uninstall),
        },
        _ => Err(args.unexpected(&action, "autosave")),
    }
}

fn parse_auto_restore(args: &mut Args) -> Result<AutoRestoreOptions, UsageError> {
    let mut options = AutoRestoreOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server-pid" => {
                let pid = args.value(&arg)?;
                options.server_pid = Some(pid.parse().map_err(|_| {
                    UsageError(format!("--server-pid takes a process ID, not {pid:?}"))
                })?);
            }
            "--print-hook" => options.print_hook = true,
            _ => return Err(args.unexpected(&arg, "auto-restore")),
        }
    }
    if options.print_hook && options.server_pid.is_some() {
        return Err(UsageError(
            "--print-hook and --server-pid can't be combined".into(),
        ));
    }
    Ok(options)
}

fn parse_agent_hook(args: &mut Args) -> Result<Agent, UsageError> {
    let Some(name) = args.next() else {
        return Err(UsageError(
            "agent-hook needs an agent: pi, claude or codex".into(),
        ));
    };
    let agent = Agent::from_name(&name).ok_or_else(|| {
        UsageError(format!(
            "agent-hook: unknown agent {name:?} (pi, claude or codex)"
        ))
    })?;
    no_options(args, "agent-hook")?;
    Ok(agent)
}

/// The remaining arguments, as UTF-8 strings.
struct Args {
    rest: std::vec::IntoIter<String>,
}

impl Args {
    fn new(args: impl IntoIterator<Item = OsString>) -> Result<Args, UsageError> {
        let args = args
            .into_iter()
            .map(|a| {
                a.into_string()
                    .map_err(|a| UsageError(format!("argument {a:?} isn't valid UTF-8")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Args {
            rest: args.into_iter(),
        })
    }

    fn wants_help(&self) -> bool {
        self.rest
            .as_slice()
            .iter()
            .any(|a| a == "-h" || a == "--help")
    }

    fn next(&mut self) -> Option<String> {
        self.rest.next()
    }

    /// The value following `option`.
    fn value(&mut self, option: &str) -> Result<String, UsageError> {
        self.next()
            .ok_or_else(|| UsageError(format!("{option} needs a value")))
    }

    fn unexpected(&self, arg: &str, command: &str) -> UsageError {
        if arg.starts_with('-') {
            UsageError(format!("unknown option {arg:?} for {command}"))
        } else {
            UsageError(format!("unexpected argument {arg:?} for {command}"))
        }
    }
}
