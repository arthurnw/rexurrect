//! The zsh configuration fixture blocks run with.
//!
//! Block shells are `login` -> `-zsh` children of the throwaway server and
//! inherit its environment, so the harness points `ZDOTDIR` at a directory it
//! writes here. The user's own zsh config never runs in a fixture block: it is
//! slow, can stop at interactive prompts (oh-my-zsh's update question), and
//! would record test commands into the user's history.

use std::path::{Path, PathBuf};

/// The zsh recorder plugin shipped at the repo root.
pub fn recorder_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("rexurrect.plugin.zsh")
}

/// `path` single-quoted for a zsh command line.
pub fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', r"'\''"))
}

/// Loaded by every fixture block before any extra lines:
/// - no history file, so nothing reaches the user's history;
/// - a plain prompt (`<dir> %`), so captures are predictable;
/// - an OSC 7 report before each prompt, so the terminal `pwd` method
///   answers about 0.2 s after a block starts. Rex doesn't inject shell
///   integration itself: without this, `pwd` stays `{}` forever.
const BASE_ZSHRC: &str = r#"# rexurrect test harness: minimal zsh for fixture blocks.
unset HISTFILE
SAVEHIST=0
PROMPT='%~ %# '
RPROMPT=''
_rxt_osc7() { printf '\e]7;file://%s%s\e\\' "$HOST" "${${PWD//\%/%25}// /%20}"; }
precmd_functions+=(_rxt_osc7)
"#;

/// The harness `ZDOTDIR` contents. Extend it with [`ZshConfig::line`], e.g. to
/// source the recorder plugin in every fixture block.
#[derive(Clone, Debug, Default)]
pub struct ZshConfig {
    extra_lines: Vec<String>,
}

impl ZshConfig {
    /// Appends a line to `.zshrc`, after the base config.
    pub fn line(mut self, line: impl Into<String>) -> Self {
        self.extra_lines.push(line.into());
        self
    }

    /// Writes `.zshrc` (and empty `.zshenv`, `.zprofile`, `.zlogin`) into `zdotdir`.
    pub fn write(&self, zdotdir: &Path) {
        std::fs::create_dir_all(zdotdir).unwrap();
        let mut zshrc = BASE_ZSHRC.to_string();
        for line in &self.extra_lines {
            zshrc.push_str(line);
            zshrc.push('\n');
        }
        std::fs::write(zdotdir.join(".zshrc"), zshrc).unwrap();
        for empty in [".zshenv", ".zprofile", ".zlogin"] {
            std::fs::write(zdotdir.join(empty), "").unwrap();
        }
    }
}
