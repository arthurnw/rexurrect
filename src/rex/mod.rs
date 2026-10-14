//! The only way rex-resurrect talks to Rex: the `rex` CLI's `api call`.
//!
//! Every call is `rex [-S <endpoint>] --autostart=false api call METHOD -`
//! with the arguments as JSON on stdin and the result as JSON on stdout.
//!
//! - `--autostart=false` always: otherwise a save that runs while Rex is quit
//!   would start an empty server.
//! - `-S` explicitly whenever there's an endpoint: `REX_SERVER`, or one given
//!   ([`Rex::at`]). If that server isn't running, a bare `rex` silently falls
//!   back to its default endpoint, which may be a different (real) server.

pub mod api;
pub mod server_process;

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::settings;

use api::{
    CreateSession, CreatedSession, FormatArgs, ServerStatus, SessionList, SessionSummary,
    SessionView, TerminalFormat, TerminalProcess, TerminalPwd,
};

/// The variable naming the Rex server to talk to. Rex sets it in every
/// block's shell.
pub const SERVER_VAR: &str = "REX_SERVER";

/// The endpoint of the Rex app's own server: rex's default endpoint when
/// neither `REX_SERVER` nor `XDG_RUNTIME_DIR` is set, which is how the app
/// starts its server. (With `XDG_RUNTIME_DIR` set, rex's default is
/// `$XDG_RUNTIME_DIR/rex/server.sock` instead, which isn't the app's.)
/// Percent-encoded, as Rex writes `REX_SERVER`.
pub fn app_server_endpoint(home: &Path) -> String {
    unix_endpoint(&home.join("Library/Application Support/rex/server.sock"))
}

/// rex's default endpoint, the one `rex` dials without `-S` or `REX_SERVER`:
/// `$XDG_RUNTIME_DIR/rex/server.sock` when `XDG_RUNTIME_DIR` is set,
/// otherwise the Rex app's server.
pub fn default_endpoint(xdg_runtime_dir: Option<&Path>, home: &Path) -> String {
    match xdg_runtime_dir {
        Some(dir) => unix_endpoint(&dir.join("rex/server.sock")),
        None => app_server_endpoint(home),
    }
}

/// The endpoint of a Unix socket, percent-encoded as Rex writes it.
fn unix_endpoint(socket: &Path) -> String {
    let mut endpoint = String::from("unix://");
    for &byte in socket.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            endpoint.push(char::from(byte));
        } else {
            endpoint.push_str(&format!("%{byte:02X}"));
        }
    }
    endpoint
}

/// A block on the server: the two IDs every block method needs.
#[derive(Debug, Clone)]
pub struct BlockRef {
    pub session_id: String,
    pub block_id: String,
}

/// A handle on one Rex server, reached through the `rex` CLI on `PATH`.
#[derive(Debug, Clone)]
pub struct Rex {
    program: OsString,
    server: Option<OsString>,
    timeout: Option<Duration>,
}

/// Why a `rex` call failed.
#[derive(Debug)]
pub enum RexError {
    /// `rex` couldn't be run at all (not on `PATH`, ...).
    Spawn {
        method: String,
        error: std::io::Error,
    },
    /// No server at the endpoint (`rex` exit status 4). Autostart is off, so
    /// this is the normal answer while the Rex server is down.
    NotRunning { method: String, message: String },
    /// `rex` ran and reported an error (bad request, not found, timeout, ...).
    Failed {
        method: String,
        status: Option<i32>,
        message: String,
    },
    /// `rex` succeeded but its output isn't the shape rex-resurrect expects.
    UnexpectedOutput { method: String, detail: String },
}

impl fmt::Display for RexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RexError::Spawn { method, error } => {
                write!(f, "couldn't run `rex api call {method}`: {error}")
            }
            RexError::NotRunning { method, message } => {
                write!(
                    f,
                    "rex api call {method}: the Rex server isn't running: {message}"
                )
            }
            RexError::Failed {
                method,
                status,
                message,
            } => {
                let status = status.map_or("signal".to_string(), |s| s.to_string());
                write!(f, "rex api call {method} failed (exit {status}): {message}")
            }
            RexError::UnexpectedOutput { method, detail } => {
                write!(
                    f,
                    "rex api call {method} returned unexpected output: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for RexError {}

impl RexError {
    /// Whether Rex refused to answer because the answer is larger than its
    /// limit on one response. `rex` reports it on stderr (exit 1), ending in
    /// the response's status and Rex's error code:
    ///
    /// ```text
    /// stream response "control-2" failed: control response exceeds 1048576 bytes (status=413 code=too_large)
    /// ```
    ///
    /// Judged by the code, never by a size: Rex's documented limit
    /// (65,536 bytes) isn't the one it enforces (1 MiB).
    pub fn is_too_large(&self) -> bool {
        match self {
            RexError::Failed { message, .. } => error_code(message) == Some("too_large"),
            _ => false,
        }
    }
}

/// Rex's error code, from the `(status=<status> code=<code>)` that `rex`
/// ends an error from Rex with. `None` if the message doesn't end that way.
fn error_code(message: &str) -> Option<&str> {
    let (_, suffix) = message.strip_suffix(')')?.rsplit_once('(')?;
    let (status, code) = suffix.split_once(' ')?;
    status.strip_prefix("status=")?;
    code.strip_prefix("code=")
}

impl Rex {
    /// The server in `REX_SERVER`, or `rex`'s own default when it's unset.
    pub fn from_env() -> Rex {
        Rex {
            program: "rex".into(),
            server: settings::env_non_empty(SERVER_VAR),
            timeout: None,
        }
    }

    /// The server at `endpoint`, reached through the `rex` CLI at `program`.
    pub fn at(program: impl Into<OsString>, endpoint: impl Into<OsString>) -> Rex {
        Rex {
            program: program.into(),
            server: Some(endpoint.into()),
            timeout: None,
        }
    }

    /// This server, with each call given up after `timeout` (rex's own
    /// default is 10 seconds). A server that hasn't finished starting
    /// accepts connections but answers nothing, so a call to it waits that
    /// long and fails.
    pub fn with_timeout(&self, timeout: Duration) -> Rex {
        Rex {
            timeout: Some(timeout),
            ..self.clone()
        }
    }

    /// Calls a raw API method and parses its result.
    pub fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        args: &impl Serialize,
    ) -> Result<T, RexError> {
        let input = serde_json::to_vec(args).expect("API arguments serialize to JSON");
        let mut cmd = Command::new(&self.program);
        if let Some(server) = &self.server {
            cmd.arg("-S").arg(server);
        }
        if let Some(timeout) = self.timeout {
            cmd.arg(format!("--timeout={}ms", timeout.as_millis().max(1)));
        }
        let spawned = cmd
            .args(["--autostart=false", "api", "call", method, "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let spawn_error = |error| RexError::Spawn {
            method: method.to_string(),
            error,
        };
        let mut child = spawned.map_err(spawn_error)?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let written = stdin.write_all(&input);
        drop(stdin);
        let out = child.wait_with_output().map_err(spawn_error)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let message = stderr.trim().trim_start_matches("Error: ").to_string();
            return Err(match out.status.code() {
                Some(4) => RexError::NotRunning {
                    method: method.to_string(),
                    message,
                },
                status => RexError::Failed {
                    method: method.to_string(),
                    status,
                    message,
                },
            });
        }
        written.map_err(spawn_error)?;
        serde_json::from_slice(&out.stdout).map_err(|e| RexError::UnexpectedOutput {
            method: method.to_string(),
            detail: e.to_string(),
        })
    }

    pub fn server_status(&self) -> Result<ServerStatus, RexError> {
        self.call("server.status", &json!({}))
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, RexError> {
        let list: SessionList = self.call("session.list", &json!({}))?;
        Ok(list.sessions)
    }

    pub fn view_session(&self, session_id: &str) -> Result<SessionView, RexError> {
        let method = "session.view";
        let view: SessionView = self.call(method, &json!({ "session_id": session_id }))?;
        view.check_consistency()
            .map_err(|detail| RexError::UnexpectedOutput {
                method: method.to_string(),
                detail,
            })?;
        Ok(view)
    }

    pub fn create_session(&self, request: &CreateSession) -> Result<CreatedSession, RexError> {
        self.call("session.create", request)
    }

    /// The terminal `pwd` method: the directory the block's shell last reported.
    pub fn terminal_pwd(&self, session_id: &str, block_id: &str) -> Result<TerminalPwd, RexError> {
        self.block_method(session_id, block_id, "pwd", json!({}))
    }

    /// The terminal `process` method: the block's foreground process and its ancestors.
    pub fn terminal_process(
        &self,
        session_id: &str,
        block_id: &str,
    ) -> Result<TerminalProcess, RexError> {
        self.block_method(session_id, block_id, "process", json!({}))
    }

    /// The terminal `format` method: the block's scrollback and screen.
    pub fn terminal_format(
        &self,
        session_id: &str,
        block_id: &str,
        args: &FormatArgs,
    ) -> Result<TerminalFormat, RexError> {
        let args = serde_json::to_value(args).expect("format arguments serialize to JSON");
        self.block_method(session_id, block_id, "format", args)
    }

    /// Calls a block method by its short name (`pwd`, `process`, `write`)
    /// through the `block.method` wrapper, the only form that accepts `args`.
    fn block_method<T: DeserializeOwned>(
        &self,
        session_id: &str,
        block_id: &str,
        method: &str,
        args: Value,
    ) -> Result<T, RexError> {
        self.call(
            "block.method",
            &json!({ "session_id": session_id, "block_id": block_id, "method": method, "args": args }),
        )
    }
}
