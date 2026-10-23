//! The program in a shell block's foreground: the last command of a block
//! nothing has recorded anything for (no recorder file), because no
//! recorder is installed or its shell started before one was.

use crate::error::Result;
use crate::process;
use crate::rex::api::TerminalProcess;

/// rexurrect's own executable name. A save run from a block is that block's
/// foreground program while it saves, and never what the block was doing.
const REXURRECT: &str = "rexurrect";

/// The command line of the program in the foreground of the block whose
/// terminal `process` method answered `process`, as restore would type it
/// (see [`zsh_line`]), or `None` while the block's own shell, or
/// rexurrect, is in the foreground, or if the program's arguments can't be
/// read or typed. The arguments come from the kernel: the terminal `process`
/// method reports none.
pub fn foreground_command(process: &TerminalProcess) -> Result<Option<String>> {
    match process.foreground_program() {
        Some(program) if !program.names().any(|name| name == REXURRECT) => {
            Ok(process::arguments(program.pid)?.and_then(|arguments| zsh_line(&arguments)))
        }
        _ => Ok(None),
    }
}

/// `arguments` as a line zsh runs as the same program with the same
/// arguments: joined by spaces, each quoted only if it needs it, so
/// `vim notes.txt` reads as it always did. `None` if any argument holds a
/// control character, the rule [`crate::create::typed_command`] applies:
/// typed, a line break would act as Enter.
fn zsh_line(arguments: &[String]) -> Option<String> {
    if arguments.is_empty()
        || arguments
            .iter()
            .any(|argument| argument.contains(char::is_control))
    {
        return None;
    }
    let quoted: Vec<String> = arguments.iter().map(|argument| quoted(argument)).collect();
    Some(quoted.join(" "))
}

/// `argument` as one zsh word: as it is if every character is plain, else in
/// single quotes, inside which zsh treats everything literally but `'`
/// itself (closed, escaped, reopened).
fn quoted(argument: &str) -> String {
    if is_plain(argument) {
        argument.to_string()
    } else {
        format!("'{}'", argument.replace('\'', r"'\''"))
    }
}

/// Whether zsh reads `argument` unquoted as itself: no whitespace, globbing,
/// expansion or redirection characters anywhere, nor `=` first (`=vim`
/// expands to vim's path).
fn is_plain(argument: &str) -> bool {
    !argument.is_empty()
        && !argument.starts_with('=')
        && argument
            .chars()
            .all(|c| c.is_alphanumeric() || "-_./:,+@%=".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(arguments: &[&str]) -> Option<String> {
        zsh_line(&arguments.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn ordinary_arguments_are_joined_unquoted() {
        assert_eq!(
            line(&["vim", "notes.txt"]).as_deref(),
            Some("vim notes.txt")
        );
        assert_eq!(
            line(&["npm", "run", "dev", "--port=3000", "user@host:~x"]).as_deref(),
            Some("npm run dev --port=3000 'user@host:~x'")
        );
    }

    #[test]
    fn arguments_zsh_would_read_otherwise_are_single_quoted() {
        assert_eq!(
            line(&["vim", "my file.txt"]).as_deref(),
            Some("vim 'my file.txt'")
        );
        assert_eq!(
            line(&["echo", "it's", "*", "$HOME", "", "=vim"]).as_deref(),
            Some(r"echo 'it'\''s' '*' '$HOME' '' '=vim'")
        );
    }

    #[test]
    fn arguments_holding_a_control_character_are_refused() {
        assert_eq!(line(&["sh", "-c", "tail -f /dev/null\ntrue"]), None);
        assert_eq!(line(&["printf", "a\tb"]), None);
        assert_eq!(line(&[]), None);
    }
}
