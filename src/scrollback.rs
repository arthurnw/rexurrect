//! Scrollback: a shell block's earlier output, saved with a snapshot and
//! printed back above the new prompt when restore recreates the block.
//!
//! Saving (prototype #13):
//! - Rex's terminal `format` method renders the block's scrollback, not just
//!   its screen. It's saved as VT (text plus SGR styles) with soft wraps
//!   joined, so it reflows at whatever width the restored block has.
//! - A block on the alternate screen is skipped: it's showing a full-screen
//!   program (`nvim`, `less`, `top`), and its capture would be a frozen
//!   picture of that program rather than earlier output. Rex reports the alternate
//!   screen itself, which is more reliable than guessing from the foreground
//!   process: a block running `tail -f` keeps its scrollback, and one left
//!   on the alternate screen by a killed program loses it.
//!
//! On disk, in the state directory:
//! - `scrollback/<snapshot ID>/<block ID>.vt`: one directory per snapshot,
//!   named like the snapshot (see `store`), holding one file per block;
//! - each saved block records its file's path relative to the state
//!   directory;
//! - every file goes through `state::write_file`, like snapshots;
//! - retention: a snapshot's directory is removed once the snapshot is gone
//!   (see [`ScrollbackStore::remove_orphans`]), so scrollback is kept as
//!   long as the snapshot it belongs to, or, if that's longer, as long as a
//!   restore from that snapshot is in its startup window (see `startup`):
//!   its replay may not have printed it yet, and saves in the window save
//!   the restored blocks' scrollback from it.
//!
//! Restoring: see [`Replay`].

use std::path::{Component, Path, PathBuf};

use crate::account::Account;
use crate::error::{Error, Result};
use crate::rex::api::{FormatArgs, TerminalOptions, TerminalProcess};
use crate::rex::{BlockRef, Rex};
use crate::snapshot::SavedBlock;
use crate::state::{self, StateDir};
use crate::timestamp::Timestamp;

const DIR: &str = "scrollback";

/// The variable listing the programs whose blocks get no scrollback.
pub const NO_SCROLLBACK_VAR: &str = "REX_RESURRECT_NO_SCROLLBACK";

/// Programs that draw their interface on the main screen, not the alternate
/// one, and whose blocks therefore get no scrollback: a replay would print a
/// frozen copy of the interface, as for a full-screen program. Claude Code
/// (`claude`) is one.
#[derive(Debug)]
pub struct NoScrollbackPrograms {
    names: Vec<String>,
}

impl NoScrollbackPrograms {
    /// `$REX_RESURRECT_NO_SCROLLBACK`: program names separated by spaces.
    /// `claude` if it's unset; none if it's set but empty.
    pub fn from_env() -> Result<NoScrollbackPrograms> {
        let Some(value) = std::env::var_os(NO_SCROLLBACK_VAR) else {
            return Ok(NoScrollbackPrograms {
                names: vec!["claude".to_string()],
            });
        };
        let value = value.to_str().ok_or_else(|| {
            Error::new(format!("{NO_SCROLLBACK_VAR} isn't valid UTF-8: {value:?}"))
        })?;
        Ok(NoScrollbackPrograms {
            names: value.split_whitespace().map(str::to_string).collect(),
        })
    }

    /// Whether the block whose terminal `process` method answered `process`
    /// is running one of these programs, by any name the program goes by
    /// (the name it was run by, or its executable's).
    pub fn running_in(&self, process: &TerminalProcess) -> bool {
        process.foreground_program().is_some_and(|program| {
            program
                .names()
                .any(|name| self.names.iter().any(|listed| listed == name))
        })
    }
}

/// What the terminal `format` method's `extra_modes` prefix holds while the
/// block is on the alternate screen (xterm's three ways to switch to it).
const ALTERNATE_SCREEN: [&str; 3] = ["\x1b[?1049h", "\x1b[?1047h", "\x1b[?47h"];

/// Every snapshot's scrollback.
#[derive(Debug, Clone)]
pub struct ScrollbackStore {
    state: StateDir,
}

impl ScrollbackStore {
    pub fn new(state: &StateDir) -> ScrollbackStore {
        ScrollbackStore {
            state: state.clone(),
        }
    }

    /// Where the save that writes the snapshot taken at `taken_at` puts its
    /// blocks' scrollback: a directory named like that snapshot.
    pub fn for_snapshot(&self, taken_at: Timestamp) -> SnapshotScrollback {
        let relative = Path::new(DIR).join(taken_at.compact());
        SnapshotScrollback {
            store: self.clone(),
            dir: self.state.join(&relative),
            relative,
        }
    }

    /// Removes the scrollback of every snapshot not in `snapshot_ids` (the
    /// snapshots to keep it for), and anything a save left behind without
    /// writing its snapshot.
    pub fn remove_orphans(&self, snapshot_ids: &[String]) -> Result<()> {
        for entry in state::read_dir_or_empty(&self.state.join(DIR))? {
            let name = entry.file_name();
            if !snapshot_ids.iter().any(|id| name == id.as_str()) {
                state::remove_entry(&entry.path())?;
            }
        }
        Ok(())
    }

    /// Removes the temporary files that crashed scrollback writes left
    /// behind in the snapshots' directories (see
    /// [`state::remove_stale_temp_files`]).
    pub fn remove_stale_temp_files(&self) -> Result<()> {
        for entry in state::read_dir_or_empty(&self.state.join(DIR))? {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                state::remove_stale_temp_files(&entry.path(), state::is_temp_name)?;
            }
        }
        Ok(())
    }

    /// The file holding a saved block's scrollback, where the block records
    /// it (`relative`, relative to the state directory), if the file is
    /// still there. A recorded path that would reach outside the state
    /// directory counts as none.
    fn file(&self, relative: &Path) -> Option<PathBuf> {
        let inside = relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
        let path = self.state.join(relative);
        (inside && path.is_file()).then_some(path)
    }
}

/// One save's scrollback directory.
#[derive(Debug)]
pub struct SnapshotScrollback {
    /// Where restored blocks' scrollback comes from.
    store: ScrollbackStore,
    dir: PathBuf,
    /// `dir` relative to the state directory.
    relative: PathBuf,
}

/// Where a block's scrollback comes from.
#[derive(Debug)]
pub enum ScrollbackSource {
    /// The block's terminal, read through Rex.
    Terminal(BlockRef),
    /// The scrollback restore printed in a block it created, in the startup
    /// window: nothing else can have reached the block's terminal yet, and
    /// the printing may not have finished. `scrollback` is where the saved
    /// block it was created from records it.
    Restored {
        /// The name to save it under: the block's ID, or, for a block
        /// restore couldn't tell apart from its siblings, the ID of the
        /// block it was saved from.
        block_id: String,
        scrollback: PathBuf,
    },
}

impl SnapshotScrollback {
    /// Saves a shell block's scrollback and returns the path its saved block
    /// records. `None`, and nothing saved, for a block on the alternate
    /// screen, a block with nothing in it, a restored block whose file has
    /// gone, or a block ID that can't name a file.
    pub fn save(&self, rex: &Rex, source: &ScrollbackSource) -> Result<Option<PathBuf>> {
        let (block_id, content) = match source {
            ScrollbackSource::Terminal(block) => {
                (&block.block_id, terminal_scrollback(rex, block)?)
            }
            ScrollbackSource::Restored {
                block_id,
                scrollback,
            } => {
                let content = match self.store.file(scrollback) {
                    Some(file) => state::read_file(&file)?
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                    None => None,
                };
                (block_id, content)
            }
        };
        let Some(content) = content.filter(|_| state::is_plain_file_name(block_id)) else {
            return Ok(None);
        };
        let name = format!("{block_id}.vt");
        state::write_file(&self.dir.join(&name), content.as_bytes())?;
        Ok(Some(self.relative.join(name)))
    }
}

/// A shell block's scrollback as VT, ready to replay; `None` for a block on
/// the alternate screen or with nothing in it.
fn terminal_scrollback(rex: &Rex, block: &BlockRef) -> Result<Option<String>> {
    let (session_id, block_id) = (&block.session_id, &block.block_id);
    let modes = rex.terminal_format(
        session_id,
        block_id,
        &FormatArgs {
            format: "vt",
            extra_modes: true,
            ..FormatArgs::default()
        },
    )?;
    if ALTERNATE_SCREEN
        .iter()
        .any(|mode| modes.content.contains(mode))
    {
        return Ok(None);
    }
    let mut content = rex
        .terminal_format(
            session_id,
            block_id,
            &FormatArgs {
                format: "vt",
                unwrap: true,
                ..FormatArgs::default()
            },
        )?
        .content;
    if content.trim().is_empty() {
        return Ok(None);
    }
    // The last line is usually the old prompt: end it, so what's printed
    // after the replay starts on a line of its own.
    if !content.ends_with('\n') {
        content.push_str("\r\n");
    }
    Ok(Some(content))
}

/// How restore prints a block's saved scrollback above its new prompt.
///
/// A block with a scrollback file doesn't start the user's shell directly:
/// Rex runs [`REPLAY_SCRIPT`] instead (`shell: "none"` and `login: false`
/// make Rex run the command as given, with no shell of its own around it,
/// so no startup file runs twice). The script prints the file, then starts
/// the shell exactly as Rex would. `initial_input` is unchanged: the last
/// command is typed, never run.
pub struct Replay {
    store: ScrollbackStore,
    account: Option<Account>,
}

/// Run by `/bin/bash` with the scrollback file as `$1`, the user as `$2`,
/// [`LOGIN_SHELL_EXEC`] as `$3` and the login shell as `$4`. They're all
/// arguments, so nothing is ever interpolated into shell code.
///
/// 1. Erases the screen and scrollback. Rex has already written
///    `initial_input` into the new terminal, which echoed it; erasing keeps
///    that echo from being glued to the first replayed line. The typed input
///    itself is still waiting in the terminal for the shell to read.
/// 2. Prints the file (one that has gone since restore checked is skipped,
///    and the shell still starts), then resets styles.
/// 3. Execs Rex's own launch chain for a block's shell, copied from `ps` of
///    a default block: login(1), whose `Last login:` banner separates the old
///    output from the new (`-l` keeps the working directory, `-p` the
///    environment), then the shell as a login shell (`-zsh`). The shell reads
///    its startup files once, like any block's, and its first prompt picks
///    up the typed last command.
///
/// If Rex changes its own chain, restored blocks won't follow; the
/// scrollback round-trip test checks the shell still starts as `-zsh` under
/// `login`.
const REPLAY_SCRIPT: &str = r#"printf "\033[H\033[2J\033[3J"; cat -- "$1" 2>/dev/null; printf "\033[0m"; exec /usr/bin/login -flp "$2" /bin/bash --noprofile --norc -c "$3" rex-login "${4##*/}" "$4""#;

/// The last link of Rex's launch chain: execs the shell with `-<name>` as
/// its argv0, which makes it a login shell.
const LOGIN_SHELL_EXEC: &str = r#"name=$1; shift; exec -a "-$name" -- "$@""#;

impl Replay {
    pub fn new(state: &StateDir) -> Replay {
        Replay {
            store: ScrollbackStore::new(state),
            account: Account::current(),
        }
    }

    /// Whether [`Replay::apply`] prints `block`'s saved scrollback.
    pub fn prints(&self, block: &SavedBlock) -> bool {
        self.arguments(block).is_some()
    }

    /// Makes `options` start the block by printing `block`'s saved
    /// scrollback. Leaves them alone if there's none to print.
    pub fn apply(&self, block: &SavedBlock, options: &mut TerminalOptions) {
        let Some(ReplayArguments { file, user, shell }) = self.arguments(block) else {
            return;
        };
        let argv = [
            "/bin/bash",
            "--noprofile",
            "--norc",
            "-c",
            REPLAY_SCRIPT,
            "rex-resurrect",
            &file,
            user,
            LOGIN_SHELL_EXEC,
            shell,
        ];
        options.command = Some(argv.map(str::to_string).to_vec());
        options.shell = Some("none".to_string());
        options.login = Some(false);
    }

    /// What [`REPLAY_SCRIPT`] prints `block`'s scrollback with, as the text
    /// its arguments must be. `None` if the block has no scrollback file
    /// (any more), there's no account to start the shell as, or either path
    /// isn't valid UTF-8.
    fn arguments(&self, block: &SavedBlock) -> Option<ReplayArguments<'_>> {
        let account = self.account.as_ref()?;
        let file = self.store.file(block.scrollback.as_deref()?)?;
        Some(ReplayArguments {
            file: file.into_os_string().into_string().ok()?,
            user: &account.user,
            shell: account.shell.to_str()?,
        })
    }
}

/// [`REPLAY_SCRIPT`]'s arguments: the scrollback file, the user, and the
/// login shell.
struct ReplayArguments<'a> {
    file: String,
    user: &'a str,
    shell: &'a str,
}
