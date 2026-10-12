//! Rex server processes: finding the one that started this process, and the
//! endpoints it may be listening on.
//!
//! Rex runs init.lua in the server's own process, so whatever init.lua
//! starts is a descendant of the server. But it tells init.lua nothing about
//! the server's endpoint, and the `REX_SERVER` there is whatever the server
//! was started with: a server started from inside a Rex block inherits that
//! block's, which names a different server. So the endpoint has to come from
//! the server process itself, and be checked against it.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::process;
use crate::settings;

use super::SERVER_VAR;

/// How far up the process tree to look for the server.
const MAX_ANCESTORS: usize = 32;

/// A running `rex server run`.
#[derive(Debug, Clone)]
pub struct ServerProcess {
    pub pid: u32,
    /// The `rex` CLI the server runs as. Calls to the server go through it:
    /// it's the same version as the server, and it's there even when `rex`
    /// isn't on `PATH` (the Rex app starts its server with a bare `PATH`).
    pub program: PathBuf,
    /// The endpoint on its command line (`--listen`), if any.
    listen: Option<String>,
}

/// What started this process, judged by its nearest `rex` ancestor.
#[derive(Debug)]
pub enum Starter {
    /// A Rex server, running its init.lua.
    Server(ServerProcess),
    /// Some other `rex` command: `rex config check` runs init.lua too.
    OtherRex { command: String },
    /// No `rex` process at all.
    NotRex,
}

/// Walks up from this process's parent to the nearest process whose
/// executable is called `rex`.
pub fn find_starter() -> Result<Starter> {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..MAX_ANCESTORS {
        if pid <= 1 {
            break;
        }
        let Some((parent, executable)) = process::parent_and_executable(pid)? else {
            break;
        };
        if executable.file_name().is_some_and(|name| name == "rex") {
            let command = process::command_line(pid)?.unwrap_or_default();
            let starter = match ServerProcess::from_command_line(pid, &executable, &command) {
                Some(server) => Starter::Server(server),
                None => Starter::OtherRex { command },
            };
            return Ok(starter);
        }
        pid = parent;
    }
    Ok(Starter::NotRex)
}

impl ServerProcess {
    /// The Rex server with this pid; `None` if there's no such process or it
    /// isn't a `rex server run`.
    pub fn read(pid: u32) -> Result<Option<ServerProcess>> {
        let Some((_, executable)) = process::parent_and_executable(pid)? else {
            return Ok(None);
        };
        let Some(command) = process::command_line(pid)? else {
            return Ok(None);
        };
        Ok(ServerProcess::from_command_line(pid, &executable, &command))
    }

    /// A server, if `command` (arguments joined by spaces, as `ps` shows
    /// them) runs `rex server run`. Its executable's path may contain spaces
    /// (`/Applications/Rex Beta.app/...`); its arguments are taken not to.
    fn from_command_line(pid: u32, executable: &Path, command: &str) -> Option<ServerProcess> {
        let words: Vec<&str> = command.split_whitespace().collect();
        let run = words.windows(2).position(|w| w == ["server", "run"])?;
        let mut options = words[run + 2..].iter();
        let mut listen = None;
        while let Some(&option) = options.next() {
            if option == "--listen" {
                listen = options.next().map(|e| e.to_string());
            } else if let Some(endpoint) = option.strip_prefix("--listen=") {
                listen = Some(endpoint.to_string());
            }
        }
        let program = if executable.is_absolute() {
            executable.to_path_buf()
        } else {
            PathBuf::from("rex")
        };
        Some(ServerProcess {
            pid,
            program,
            listen,
        })
    }

    /// Where the server may be listening, most likely first: the endpoint on
    /// its command line, then, for a server started without one, the
    /// `REX_SERVER` and rex default it may have used instead (it shares this
    /// process's environment). Only one that reports this server's pid is
    /// the server.
    pub fn candidate_endpoints(&self, home: &Path) -> Vec<String> {
        let var = settings::env_non_empty;
        let mut endpoints: Vec<String> = Vec::new();
        endpoints.extend(self.listen.clone());
        endpoints.extend(var(SERVER_VAR).and_then(|v| v.into_string().ok()));
        let xdg_runtime_dir = var("XDG_RUNTIME_DIR").map(PathBuf::from);
        endpoints.push(super::default_endpoint(xdg_runtime_dir.as_deref(), home));
        let mut unique = Vec::new();
        for endpoint in endpoints {
            if !unique.contains(&endpoint) {
                unique.push(endpoint);
            }
        }
        unique
    }
}
