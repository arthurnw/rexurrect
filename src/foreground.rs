//! The program in a shell block's foreground: the last command of a block
//! nothing has recorded anything for (no recorder file), because no
//! recorder is installed or its shell started before one was.

use crate::error::Result;
use crate::process;
use crate::rex::api::TerminalProcess;

/// rexurrect's own executable name. A save run from a block is that block's
/// foreground program while it saves, and never what the block was doing.
const REXURRECT: &str = "rexurrect";

/// The full command line of the program in the foreground of the block whose
/// terminal `process` method answered `process`, or `None` while the block's
/// own shell, or rexurrect, is in the foreground. The command line comes from
/// `ps`: the terminal `process` method reports no arguments.
pub fn foreground_command(process: &TerminalProcess) -> Result<Option<String>> {
    match process.foreground_program() {
        Some(program) if !program.names().any(|name| name == REXURRECT) => {
            process::command_line(program.pid)
        }
        _ => Ok(None),
    }
}
