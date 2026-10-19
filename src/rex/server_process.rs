//! Rex server processes: finding the one that started this process, and the
//! endpoints it may be listening on; and finding one whose `rex` to run when
//! there's none on `PATH`.
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
    /// it's the server's own build, and it's there even when `rex`
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
        if is_rex(&executable) {
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

/// The `rex` to call `endpoint` through when there's none on `PATH`: the
/// one a Rex server of this user's runs (see [`choose`]). `None` if there's
/// no such server.
pub fn rex_for(endpoint: &str) -> Result<Option<PathBuf>> {
    let mut servers = ServerProcess::running()?;
    // A bare `rex`, from a server whose executable `ps` didn't give in full,
    // is the very one that isn't on `PATH`.
    servers.retain(|server| server.program.is_absolute());
    Ok(choose(&servers, endpoint).map(|server| server.program.clone()))
}

/// The server whose `rex` to run to call `endpoint`, from `servers`: the one
/// whose command line says it listens there, else any.
///
/// The call names `endpoint` with `-S` either way, so any working `rex`
/// reaches the right server; preferring the endpoint's own keeps the CLI the
/// same build as the server, which only matters while two builds run at
/// once. There's no `server.status` check of which server answers there
/// (ADR-0003).
fn choose<'a>(servers: &'a [ServerProcess], endpoint: &str) -> Option<&'a ServerProcess> {
    servers
        .iter()
        .find(|server| server.listen.as_deref() == Some(endpoint))
        .or(servers.first())
}

fn is_rex(executable: &Path) -> bool {
    executable.file_name().is_some_and(|name| name == "rex")
}

impl ServerProcess {
    /// Every Rex server this user runs.
    fn running() -> Result<Vec<ServerProcess>> {
        let mut servers = Vec::new();
        for (pid, executable) in process::own_executables()? {
            if !is_rex(&executable) {
                continue;
            }
            // `None`: it has exited since the listing.
            if let Some(command) = process::command_line(pid)? {
                servers.extend(ServerProcess::from_command_line(pid, &executable, &command));
            }
        }
        Ok(servers)
    }

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
        endpoints.push(super::default_endpoint(home));
        let mut unique = Vec::new();
        for endpoint in endpoints {
            if !unique.contains(&endpoint) {
                unique.push(endpoint);
            }
        }
        unique
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server as `ps` shows it: the Rex app's build `build`, listening at
    /// `endpoint`.
    fn server(pid: u32, build: &str, endpoint: &str) -> ServerProcess {
        let executable = format!(
            "/Users/you/Library/Application Support/com.superlogical.rex/RexServer/Versions/{build}/Rex Server.app/Contents/MacOS/rex"
        );
        let command = format!("{executable} server run --listen {endpoint} --max-sessions 64");
        ServerProcess::from_command_line(pid, Path::new(&executable), &command).unwrap()
    }

    fn build_of(server: Option<&ServerProcess>) -> Option<String> {
        let program = server?.program.to_str()?;
        let (_, rest) = program.split_once("/Versions/")?;
        Some(rest.split('/').next()?.to_string())
    }

    const APP: &str = "unix:///Users/you/Library/Application%20Support/rex/server.sock";
    const OTHER: &str = "unix:///tmp/other/s.sock";

    #[test]
    fn the_server_listening_at_the_endpoint_is_chosen_over_any_other() {
        let servers = [server(10, "1202", OTHER), server(20, "1203", APP)];
        assert_eq!(build_of(choose(&servers, APP)), Some("1203".into()));
        assert_eq!(build_of(choose(&servers, OTHER)), Some("1202".into()));
    }

    #[test]
    fn with_none_listening_at_the_endpoint_any_running_server_is_chosen() {
        let servers = [server(10, "1203", OTHER)];
        assert_eq!(
            build_of(choose(&servers, "unix:///tmp/gone/s.sock")),
            Some("1203".into())
        );
    }

    #[test]
    fn with_no_server_running_none_is_chosen() {
        assert_eq!(build_of(choose(&[], APP)), None);
    }
}
