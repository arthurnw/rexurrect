//! rex-resurrect: save Rex sessions and recreate them after the Rex server
//! stops. Talks to Rex only through the `rex` CLI (see `rex`).

mod account;
mod auto_restore;
mod autosave;
mod capture;
mod cli;
mod create;
mod defaults;
mod error;
mod foreground;
mod instance;
mod label;
mod lineage;
mod list;
mod log;
mod process;
mod recorder;
mod restore;
mod retention;
mod rex;
mod save;
mod scheduler;
mod scrollback;
mod settings;
mod snapshot;
mod startup;
mod state;
mod store;
mod timestamp;
mod too_large;
mod upkeep;
mod workdir;

use std::process::ExitCode;

use cli::Command;

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("rex-resurrect: {e}\nRun `rex-resurrect --help` for usage.");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Save => logging_failure("save", save::run()),
        Command::Restore(options) => logging_failure("restore", restore::run(&options)),
        Command::List => list::run(),
        Command::Autosave(command) => autosave::run(&command),
        Command::AutoRestore(options) => {
            logging_failure("auto-restore", auto_restore::run(&options))
        }
        Command::Help => {
            print!("{}", cli::USAGE);
            Ok(ExitCode::SUCCESS)
        }
        Command::Version => {
            println!("rex-resurrect {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
    };
    result.unwrap_or_else(|e| {
        eprintln!("rex-resurrect: error: {e}");
        ExitCode::FAILURE
    })
}

/// Appends `result`'s error, if any, to the log in the state directory.
fn logging_failure(command: &str, result: error::Result<ExitCode>) -> error::Result<ExitCode> {
    if let Err(e) = &result
        && let Ok(state) = state::StateDir::from_env()
    {
        log::log_failure(&state, command, e);
    }
    result
}
