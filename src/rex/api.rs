//! The Rex API shapes rexurrect reads and sends, as verified against
//! Rex 0.0.1791570264-g7b5bf3.
//!
//! Parsing is strict about what rexurrect relies on (a changed shape is an
//! error, not an empty snapshot) and lenient about the rest: unknown keys are
//! ignored, and keys Rex omits instead of sending `false`/`null` (`windows`
//! on an empty session, `layout` on a tiled layer whose blocks all floated
//! away) default.
//!
//! The model covers only the fields rexurrect reads.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The terminal shell block flavor, today the only one Rex has.
pub const SHELL_FLAVOR: &str = "com.superlogical.terminal.shell";

/// Split direction. `horizontal`: side by side, `before` on the left.
/// `vertical`: stacked, `before` on top. `ratio` is `before`'s share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Horizontal,
    Vertical,
}

// ---- server.status ---------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ServerStatus {
    /// The server's process.
    pub pid: u32,
    /// New for every run of the server (a server instance).
    pub instance_id: String,
    /// The local server's host; equals the `host_id` of every local block.
    pub host_id: String,
}

// ---- session.list ----------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SessionList {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
}

// ---- session.view ----------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SessionView {
    pub session_id: String,
    pub label: String,
    /// Absent for a session with no windows.
    #[serde(default)]
    pub active_window_id: Option<String>,
    /// In window order. Absent for a session with no windows (Rex
    /// keeps a session live after its last shell exits).
    #[serde(default)]
    pub windows: Vec<WindowView>,
}

impl SessionView {
    /// Cross-checks the keys Rex omits instead of sending them empty, so a
    /// renamed key can't pass for an empty value (and the session for one
    /// with nothing to save):
    /// - `windows` is absent only when there's no `active_window_id` either;
    /// - a tiled layer's `layout` is null only when its `blocks` are empty
    ///   (every block floated away).
    pub fn check_consistency(&self) -> Result<(), String> {
        if self.windows.is_empty() && self.active_window_id.is_some() {
            return Err("it names an active window but has no `windows`".into());
        }
        for window in &self.windows {
            if let Some(tiled) = window.tiled_layer()
                && tiled.layout.is_none()
                && !tiled.blocks.is_empty()
            {
                return Err(format!(
                    "window {}: its tiled layer has blocks but no `layout`",
                    window.window_id
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct WindowView {
    pub window_id: String,
    pub label: String,
    pub layers: Vec<LayerView>,
}

impl WindowView {
    /// The window's tiled layer: its layout. Floating layers aren't part of it.
    pub fn tiled_layer(&self) -> Option<&LayerView> {
        self.layers
            .iter()
            .find(|layer| layer.kind == LayerKind::Tiled)
    }
}

#[derive(Debug, Deserialize)]
pub struct LayerView {
    pub kind: LayerKind,
    /// `null` on a tiled layer whose blocks have all been floated.
    #[serde(default)]
    pub layout: Option<LayoutView>,
    /// The layer's blocks, joined to layout leaves by `block_id`.
    #[serde(default)]
    pub blocks: Vec<BlockView>,
}

impl LayerView {
    pub fn block(&self, block_id: &str) -> Option<&BlockView> {
        self.blocks.iter().find(|block| block.block_id == block_id)
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerKind {
    Tiled,
    Floating,
    #[serde(other)]
    Other,
}

/// A layout node: exactly one of a block leaf or a split. View-only fields
/// (`rect`, `split_id`, `focused`) are ignored.
#[derive(Debug)]
pub enum LayoutView {
    Block { block_id: String },
    Split(SplitView),
}

#[derive(Debug, Deserialize)]
pub struct SplitView {
    pub direction: Direction,
    pub ratio: f64,
    pub before: Box<LayoutView>,
    pub after: Box<LayoutView>,
}

impl<'de> Deserialize<'de> for LayoutView {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Node {
            #[serde(default)]
            block_id: Option<String>,
            #[serde(default)]
            split: Option<SplitView>,
        }
        match Node::deserialize(deserializer)? {
            Node {
                block_id: Some(block_id),
                split: None,
            } => Ok(LayoutView::Block { block_id }),
            Node {
                block_id: None,
                split: Some(split),
            } => Ok(LayoutView::Split(split)),
            Node {
                block_id: None,
                split: None,
            } => Err(serde::de::Error::custom(
                "layout node has neither `block_id` nor `split`",
            )),
            Node { .. } => Err(serde::de::Error::custom(
                "layout node has both `block_id` and `split`",
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct BlockView {
    pub block_id: String,
    pub label: String,
    pub flavor: String,
    #[serde(default)]
    pub host_id: Option<String>,
}

// ---- shell blocks' terminal methods (through block.method) -----------------

/// The terminal `pwd` method. `{}` until the shell has reported a directory
/// (through OSC 7), and forever for shells that never do.
#[derive(Debug, Deserialize)]
pub struct TerminalPwd {
    /// The shell's report verbatim: `file://<host>/<path>`, percent-encoded
    /// (or not), any or empty host.
    #[serde(default)]
    pub pwd: Option<String>,
}

/// The terminal `process` method. Rex always reports `foreground` and
/// `home`, so they're required: without them a save would quietly lose last
/// commands and working directories.
#[derive(Debug, Deserialize)]
pub struct TerminalProcess {
    /// The process in the terminal's foreground: the block's shell while it's
    /// idle, otherwise the program it's running.
    pub foreground: ProcessInfo,
    /// The foreground process's ancestors, nearest first, ending with `login`.
    /// Rex leaves it out when the foreground process is the block's first
    /// (a command it started without `login`).
    #[serde(default)]
    pub ancestors: Vec<ProcessInfo>,
    /// The user's home directory.
    pub home: PathBuf,
}

impl TerminalProcess {
    /// The process in the terminal's foreground, unless it's the block's
    /// own shell: a program the shell runs, or a shell nested in it.
    ///
    /// The block's shell is its outermost process other than `login`, the
    /// wrapper Rex starts login shells through. `ancestors` reach up to the
    /// block's first process (`login`, or the shell itself when Rex starts it
    /// directly), so a foreground process with an ancestor other than `login`
    /// isn't the block's shell.
    pub fn foreground_program(&self) -> Option<&ProcessInfo> {
        self.ancestors
            .iter()
            .any(|ancestor| ancestor.name != LOGIN)
            .then_some(&self.foreground)
    }
}

/// The process Rex starts a block's login shell through.
const LOGIN: &str = "login";

#[derive(Debug, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    /// The name of the executable it runs, which isn't always the name it
    /// was started by: Claude Code's `claude` is a symlink to an executable
    /// named for its version.
    pub name: String,
    /// The path it was started by (`~/.local/bin/claude`), before symlinks.
    #[serde(default)]
    pub invoked_path: Option<PathBuf>,
    /// Absent for root processes.
    #[serde(default)]
    pub argv0: Option<String>,
    /// Physical path (`/private/tmp/...`). Absent for root processes.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

impl ProcessInfo {
    /// Every name the process goes by: its executable's, and the file names
    /// of the path and `argv0` it was started by (the command as typed).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        fn file_name(path: &Path) -> Option<&str> {
            path.file_name().and_then(|name| name.to_str())
        }
        let invoked = self.invoked_path.as_deref().and_then(file_name);
        let argv0 = self.argv0.as_deref().map(Path::new).and_then(file_name);
        std::iter::once(self.name.as_str())
            .chain(invoked)
            .chain(argv0)
    }
}

/// Arguments of the terminal `format` method, which renders the block's
/// scrollback and screen (not just the screen, whatever its description
/// says). Only `format: "vt"` is used: text plus SGR styles, no cursor
/// movement, modes or OSC sequences, so it's safe to print into a new block.
#[derive(Debug, Default, Serialize)]
pub struct FormatArgs {
    pub format: &'static str,
    /// Joins soft-wrapped rows into one line, so the text reflows at any
    /// width instead of breaking where the block's width was.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unwrap: bool,
    /// Prefixes the terminal's modes (`ESC[?1049h` while on the alternate
    /// screen). Only has an effect with `format: "vt"`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub extra_modes: bool,
}

/// The terminal `format` method. CRLF line endings, no final newline.
#[derive(Debug, Deserialize)]
pub struct TerminalFormat {
    pub content: String,
}

// ---- session.create --------------------------------------------------------

/// `session.create` takes only `label` and `initial_windows`, and silently
/// ignores unknown keys (a typo creates an empty session), so this type is
/// the single source of the request's key names.
#[derive(Debug, Serialize)]
pub struct CreateSession {
    pub label: String,
    pub initial_windows: Vec<CreateWindow>,
}

#[derive(Debug, Serialize)]
pub struct CreateWindow {
    /// Omitted: Rex generates a default label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_label: Option<String>,
    pub layout: CreateLayout,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CreateLayout {
    Block(CreateBlock),
    Split(CreateSplit),
}

#[derive(Debug, Serialize)]
pub struct CreateSplit {
    pub direction: Direction,
    /// Must be in (0, 1); Rex turns a missing or zero ratio into 0.5.
    pub ratio: f64,
    pub before: Box<CreateLayout>,
    pub after: Box<CreateLayout>,
}

#[derive(Debug, Serialize)]
pub struct CreateBlock {
    pub flavor: String,
    /// Omitted: Rex uses the default label (`shell`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub options: TerminalOptions,
}

#[derive(Debug, Default, Serialize)]
pub struct TerminalOptions {
    /// A missing directory isn't an error: the shell starts in `$HOME`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Written to the terminal as if typed. A newline acts as Enter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<String>,
    /// What the block runs instead of the user's shell. Rex wraps it in a
    /// login shell unless `shell` is `"none"` and `login` is `false`; with
    /// both, Rex runs this argv as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreatedSession {
    pub session_id: String,
    /// In request order. Absent when no windows were requested.
    #[serde(default)]
    pub initial_windows: Vec<CreatedWindow>,
}

#[derive(Debug, Deserialize)]
pub struct CreatedWindow {
    /// In depth-first, before-then-after order of the window's layout.
    pub block_ids: Vec<String>,
}
