//! Turning a live session (its `session.view`) into a saved session.

use std::fmt;
use std::path::PathBuf;

use crate::account;
use crate::defaults::DefaultCommands;
use crate::error::{Error, Result};
use crate::foreground;
use crate::label;
use crate::lineage::Lineage;
use crate::recorder::{Recorded, RecorderFiles};
use crate::rex::api::{
    BlockView, LayerView, LayoutView, SHELL_FLAVOR, SessionView, TerminalProcess, WindowView,
};
use crate::rex::{BlockRef, Rex, RexError};
use crate::scrollback::{
    NoScrollbackPrograms, SavedScrollback, ScrollbackSource, ScrollbackStore, SnapshotScrollback,
};
use crate::snapshot::{SavedBlock, SavedLayout, SavedSession, SavedSplit, SavedWindow};
use crate::startup::Starting;
use crate::state::StateDir;
use crate::timestamp::Timestamp;
use crate::workdir;

/// What a save captures blocks with: what it compares them against (the
/// local server's host, the home directory), what it knows about them on
/// this machine (the recorder files, the sessions and blocks in their
/// startup window, the default last commands), the programs whose blocks
/// get no scrollback, and where it saves scrollback.
pub struct CaptureContext {
    /// The local server's host. A block with a different `host_id` is a
    /// remote block.
    pub host_id: String,
    /// Where a block with no working directory to save comes back.
    pub home: PathBuf,
    /// Each local shell block's last command, as its recorder recorded it.
    pub recorder: RecorderFiles,
    /// The sessions and blocks restore created that are still starting.
    pub starting: Starting,
    /// Each block's default last command. A program the capture sees in a
    /// block's foreground becomes the block's default
    /// ([`CapturedSession::programs_seen`]); save records that.
    pub defaults: DefaultCommands,
    /// Where the snapshot this save writes keeps its blocks' scrollback.
    pub scrollback: SnapshotScrollback,
    /// The programs whose blocks get no scrollback.
    pub no_scrollback: NoScrollbackPrograms,
}

impl CaptureContext {
    /// `host_id`: the local server's host. `taken_at`: the time of the
    /// snapshot this save writes. `starting`: what's starting then.
    pub fn read(
        state: &StateDir,
        host_id: &str,
        taken_at: Timestamp,
        starting: Starting,
        no_scrollback: NoScrollbackPrograms,
    ) -> Result<CaptureContext> {
        Ok(CaptureContext {
            host_id: host_id.to_string(),
            home: account::home()?,
            recorder: RecorderFiles::new(state),
            starting,
            defaults: DefaultCommands::load(state)?,
            scrollback: ScrollbackStore::new(state).for_snapshot(taken_at),
            no_scrollback,
        })
    }

    /// A shell block on the local server: the only kind of block whose
    /// working directory save can read. A block without a `host_id` counts
    /// as local.
    fn is_local_shell(&self, block: &BlockView) -> bool {
        block.flavor == SHELL_FLAVOR
            && block
                .host_id
                .as_deref()
                .is_none_or(|host_id| host_id == self.host_id)
    }

    /// A plain shell block (default label) in the home directory: what a
    /// block that can't be saved as it is comes back as, so the layout around
    /// it stays intact.
    fn plain_shell(&self) -> SavedBlock {
        SavedBlock {
            label: None,
            working_directory: self.home.clone(),
            last_command: None,
            scrollback: None,
        }
    }
}

/// A live session as save captured it: the saved session, still without its
/// blocks' scrollback, and where that scrollback is to come from. Save only
/// saves the scrollback of a session it keeps
/// ([`CapturedSession::with_scrollback`]), so a session it leaves out (an
/// untouched one, say) leaves nothing behind.
pub struct CapturedSession {
    pub saved: SavedSession,
    /// One entry per block of `saved`, window by window in layout order: the
    /// block whose scrollback to save, or `None` for a block that gets none.
    scrollback: Vec<Option<ScrollbackSource>>,
    /// The programs the capture saw in its blocks' foregrounds, as (block
    /// ID, command line): each is its block's default last command from now
    /// on.
    pub programs_seen: Vec<(String, String)>,
}

impl CapturedSession {
    /// The saved session, with its blocks' scrollback saved for the snapshot
    /// `store` belongs to.
    pub fn with_scrollback(
        mut self,
        rex: &Rex,
        store: &SnapshotScrollback,
    ) -> Result<SessionWithScrollback> {
        let mut too_large = Vec::new();
        for (block, source) in self.saved.blocks_mut().zip(self.scrollback) {
            let Some(source) = source else { continue };
            block.scrollback = match store.save(rex, &source)? {
                SavedScrollback::File(path) => Some(path),
                SavedScrollback::Nothing => None,
                SavedScrollback::TooLarge(error) => {
                    too_large.push(TooLargeScrollback {
                        block_id: source.block_id().to_string(),
                        error,
                    });
                    None
                }
            };
        }
        Ok(SessionWithScrollback {
            saved: self.saved,
            too_large,
        })
    }
}

/// A saved session with its blocks' scrollback saved
/// ([`CapturedSession::with_scrollback`]).
pub struct SessionWithScrollback {
    pub saved: SavedSession,
    /// The blocks saved without scrollback because Rex refused to hand it
    /// over as too large.
    pub too_large: Vec<TooLargeScrollback>,
}

/// A block saved without scrollback because Rex refused to hand it over as
/// too large (see [`SavedScrollback::TooLarge`]).
pub struct TooLargeScrollback {
    block_id: String,
    /// Rex's refusal.
    error: RexError,
}

impl TooLargeScrollback {
    pub fn block_id(&self) -> &str {
        &self.block_id
    }
}

impl fmt::Display for TooLargeScrollback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "saved block {} without scrollback, which Rex refused to hand over as too large: {}",
            self.block_id, self.error
        )
    }
}

pub fn saved_session(
    rex: &Rex,
    context: &CaptureContext,
    view: &SessionView,
    lineage: Lineage,
) -> Result<CapturedSession> {
    if let Some(created) = context.starting.unmatched_session(&view.session_id) {
        return Ok(as_created(view, lineage, created));
    }
    let mut capture = Capture {
        rex,
        context,
        session: view,
        scrollback: Vec::new(),
        programs_seen: Vec::new(),
    };
    let windows = view
        .windows
        .iter()
        .map(|window| capture.saved_window(window))
        .collect::<Result<_>>()?;
    Ok(CapturedSession {
        saved: SavedSession {
            lineage,
            label: view.label.clone(),
            windows,
        },
        scrollback: capture.scrollback,
        programs_seen: capture.programs_seen,
    })
}

/// A session in its startup window whose blocks restore couldn't tell apart,
/// so it can't tell which live block is which either: saved as restore
/// created it, scrollback included, under its live label.
fn as_created(view: &SessionView, lineage: Lineage, created: &SavedSession) -> CapturedSession {
    let mut saved = SavedSession {
        lineage,
        label: view.label.clone(),
        windows: created.windows.clone(),
    };
    let scrollback = saved
        .blocks_mut()
        .map(|block| {
            let scrollback = block.scrollback.take()?;
            let block_id = scrollback.file_stem()?.to_str()?.to_string();
            Some(ScrollbackSource::Restored {
                block_id,
                scrollback,
            })
        })
        .collect();
    CapturedSession {
        saved,
        scrollback,
        programs_seen: Vec::new(),
    }
}

/// One session's capture, under way.
struct Capture<'a> {
    rex: &'a Rex,
    context: &'a CaptureContext,
    session: &'a SessionView,
    /// [`CapturedSession::scrollback`] so far.
    scrollback: Vec<Option<ScrollbackSource>>,
    /// [`CapturedSession::programs_seen`] so far.
    programs_seen: Vec<(String, String)>,
}

impl Capture<'_> {
    /// A window's tiled layout. Floating layers aren't saved. A window whose
    /// blocks have all floated away has nothing tiled; it comes back holding
    /// one plain shell, so it keeps its place among the session's windows.
    fn saved_window(&mut self, window: &WindowView) -> Result<SavedWindow> {
        let tiled = window.tiled_layer().ok_or_else(|| {
            Error::new(format!(
                "session.view: window {} has no tiled layer",
                window.window_id
            ))
        })?;
        let layout = match &tiled.layout {
            Some(layout) => self.saved_layout(tiled, layout)?,
            None => SavedLayout::Block(self.plain_shell()),
        };
        Ok(SavedWindow {
            label: label::user_set_window_label(window).map(str::to_owned),
            layout,
        })
    }

    fn saved_layout(&mut self, layer: &LayerView, node: &LayoutView) -> Result<SavedLayout> {
        match node {
            LayoutView::Block { block_id } => {
                let block = layer.block(block_id).ok_or_else(|| {
                    Error::new(format!(
                        "session.view: block {block_id} is in a layout but not in its layer's blocks"
                    ))
                })?;
                Ok(SavedLayout::Block(self.saved_block(block)?))
            }
            LayoutView::Split(split) => Ok(SavedLayout::Split(SavedSplit {
                direction: split.direction,
                ratio: split.ratio,
                before: Box::new(self.saved_layout(layer, &split.before)?),
                after: Box::new(self.saved_layout(layer, &split.after)?),
            })),
        }
    }

    /// A local shell block in its working directory, with its last command
    /// (see [`Capture::last_command`]). Its scrollback is saved later, if
    /// save keeps the session. A block in its startup window that its
    /// recorder has recorded nothing for is saved as restore created it. A
    /// block that isn't a shell block has no working directory, and a remote
    /// block's is on another host, so either is saved as a plain shell in
    /// the home directory.
    fn saved_block(&mut self, block: &BlockView) -> Result<SavedBlock> {
        if !self.context.is_local_shell(block) {
            return Ok(self.plain_shell());
        }
        let label = label::user_set_block_label(block).map(str::to_owned);
        let (rex, session_id, block_id) = (self.rex, &self.session.session_id, &block.block_id);
        let recorded = self.context.recorder.read(block_id)?;
        if recorded == Recorded::Nothing
            && let Some(created) = self.context.starting.block(block_id)
        {
            // Its shell may still be starting: it may not have reported a
            // working directory, its startup may be running in its
            // foreground, and the replay may still be printing. Nothing the
            // user did can have reached it, or its recorder would have
            // recorded it.
            let scrollback = created.scrollback.clone();
            self.scrollback
                .push(scrollback.map(|scrollback| ScrollbackSource::Restored {
                    block_id: block_id.clone(),
                    scrollback,
                }));
            return Ok(SavedBlock {
                label,
                working_directory: created.working_directory.clone(),
                last_command: created.last_command.clone(),
                scrollback: None,
            });
        }
        let process = rex.terminal_process(session_id, block_id)?;
        let saved = SavedBlock {
            label,
            working_directory: workdir::working_directory(rex, session_id, block_id, &process)?,
            last_command: self.last_command(block_id, recorded, &process)?,
            scrollback: None,
        };
        let has_scrollback = !self.context.no_scrollback.running_in(&process);
        self.scrollback.push(has_scrollback.then(|| {
            ScrollbackSource::Terminal(BlockRef {
                session_id: session_id.clone(),
                block_id: block_id.clone(),
            })
        }));
        Ok(saved)
    }

    /// A block's last command, read as it is: what its recorder recorded,
    /// including a record that it has none, beats everything. Without one,
    /// the program in its foreground (not rexurrect itself), which also
    /// becomes the block's default last command, so it carries forward once
    /// the program exits. Otherwise the block's default: the last program
    /// seen there, else what restore typed there. Else none.
    ///
    /// This supersedes #6, under which a block restore created always had a
    /// recorder file, so the fallback never applied to it: without a
    /// recorder, a restored block then kept what restore typed forever.
    fn last_command(
        &mut self,
        block_id: &str,
        recorded: Recorded,
        process: &TerminalProcess,
    ) -> Result<Option<String>> {
        Ok(match recorded {
            Recorded::LastCommand(command) => Some(command),
            Recorded::NoLastCommand => None,
            Recorded::Nothing => match foreground::foreground_command(process)? {
                Some(program) => {
                    self.programs_seen
                        .push((block_id.to_string(), program.clone()));
                    Some(program)
                }
                None => self.context.defaults.get(block_id).map(str::to_string),
            },
        })
    }

    /// [`CaptureContext::plain_shell`]: no scrollback.
    fn plain_shell(&mut self) -> SavedBlock {
        self.scrollback.push(None);
        self.context.plain_shell()
    }
}
