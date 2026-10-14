//! [`TestEnv`]: one test's isolated world.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::rexurrect::RexurrectCmd;
use super::server::{Server, ServerBuilder};
use super::tempdir::TempDir;
use super::zsh::{self, ZshConfig};

/// Variables copied from the test process into every child environment.
/// Everything else is dropped: in particular `REX_BLOCK`, `REX_SESSION`,
/// `REX_SERVER`, `REX_CONFIG`, `ZDOTDIR`, `HISTFILE` and the user's `XDG_*`
/// dirs, which point at the real Rex server and the user's real state.
const INHERITED_VARS: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "TZ",
];

/// Variable the binary reads for its state directory override.
pub const STATE_DIR_VAR: &str = "REXURRECT_STATE_DIR";

/// One test's isolated world: a short temp directory holding rexurrect's
/// state directory, the harness `ZDOTDIR`, scratch directories for fixture
/// blocks, and one directory per throwaway server started from it.
///
/// Servers started from the same `TestEnv` share its state directory, so a
/// test can save on one server and restore on another, like a real restart.
/// Everything is removed when the last server and the `TestEnv` are dropped.
///
/// Nothing here touches process-global state (env vars, cwd), so tests can
/// run concurrently.
pub struct TestEnv {
    root: Arc<TempDir>,
    servers_started: AtomicU32,
    own_home: bool,
}

#[derive(Default)]
pub struct TestEnvBuilder {
    zsh: ZshConfig,
    own_home: bool,
}

impl TestEnvBuilder {
    /// Appends a line to the fixture blocks' `.zshrc` (see [`ZshConfig`]).
    pub fn zshrc_line(mut self, line: impl Into<String>) -> Self {
        self.zsh = self.zsh.line(line);
        self
    }

    /// Gives every throwaway server (and so every block shell) and every
    /// `rexurrect` run [`TestEnv::home`] as `HOME` instead of the user's own.
    /// A block created without a `cwd` starts there.
    pub fn own_home(mut self) -> Self {
        self.own_home = true;
        self
    }

    pub fn build(self) -> TestEnv {
        let root = Arc::new(TempDir::new());
        let env = TestEnv {
            root,
            servers_started: AtomicU32::new(0),
            own_home: self.own_home,
        };
        self.zsh.write(&env.zdotdir());
        for dir in [
            env.home(),
            env.state_dir(),
            env.xdg_dir("runtime"),
            env.xdg_dir("state"),
            env.xdg_dir("config"),
            env.xdg_dir("data"),
            env.xdg_dir("cache"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        env
    }
}

impl TestEnv {
    pub fn new() -> TestEnv {
        TestEnv::builder().build()
    }

    pub fn builder() -> TestEnvBuilder {
        TestEnvBuilder::default()
    }

    /// A `TestEnv` whose fixture blocks load the zsh recorder plugin with a
    /// plain `source`.
    pub fn with_recorder() -> TestEnv {
        TestEnv::builder()
            .zshrc_line(format!("source {}", zsh::quote(&zsh::recorder_plugin())))
            .build()
    }

    /// The temp directory everything lives in (`/tmp/rxt.<pid>-<n>`).
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// rexurrect's state directory for this test (passed through
    /// [`STATE_DIR_VAR`] to the binary and to every block shell).
    pub fn state_dir(&self) -> PathBuf {
        self.root().join("state")
    }

    /// Writes a block's recorder file the way any recorder would (the format
    /// recorders share with rexurrect: `recorder/<block ID>` in the state
    /// directory, holding the command and a newline). For playing a recorder
    /// that records what the zsh one wouldn't.
    pub fn write_recorder_file(&self, block_id: &str, command: &str) {
        let dir = self.state_dir().join("recorder");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(block_id), format!("{command}\n")).unwrap();
    }

    /// Ends the startup window of every session restore has created so far,
    /// as if two minutes had passed: from now on, saves read their blocks as
    /// they are, not as restore created them.
    ///
    /// The window is too long to wait out, so this is the one place tests
    /// reach inside rexurrect's own state: it moves each session's
    /// `restored_at` in `starting-sessions.json` far into the past.
    pub fn end_startup_windows(&self) {
        let path = self.state_dir().join("starting-sessions.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}; has restore run?", path.display()));
        let mut sessions: serde_json::Value = serde_json::from_str(&text).unwrap();
        for session in sessions.as_object_mut().unwrap().values_mut() {
            session["restored_at"] = "2000-01-01T00:00:00.000000Z".into();
        }
        std::fs::write(&path, sessions.to_string()).unwrap();
    }

    /// The entries in rexurrect's log (`rexurrect.log` in the state
    /// directory), oldest first; none if there's no log yet.
    pub fn log_entries(&self) -> Vec<String> {
        match std::fs::read_to_string(self.state_dir().join("rexurrect.log")) {
            Ok(log) => log.lines().map(str::to_string).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("reading the log: {e}"),
        }
    }

    /// The home directory of a `TestEnv` built with
    /// [`TestEnvBuilder::own_home`]. The path stays logical (`/tmp/...`, not
    /// `/private/tmp/...`).
    pub fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    /// The harness `ZDOTDIR` (see [`ZshConfig`]).
    pub fn zdotdir(&self) -> PathBuf {
        self.root().join("zdot")
    }

    /// Creates (if needed) and returns a scratch directory for fixture blocks.
    /// The path stays logical (`/tmp/...`, not `/private/tmp/...`).
    pub fn mkdir(&self, name: &str) -> PathBuf {
        let dir = self.root().join("w").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Configures a throwaway server; call `.start()` on the result.
    pub fn server(&self) -> ServerBuilder<'_> {
        let n = self.servers_started.fetch_add(1, Ordering::Relaxed);
        ServerBuilder::new(self, self.root().join(format!("s{n}")))
    }

    /// Starts a throwaway server with an empty init.lua.
    pub fn start_server(&self) -> Server {
        self.server().start()
    }

    /// A `rexurrect` invocation aimed at `server`.
    pub fn rexurrect(&self, server: &Server) -> RexurrectCmd {
        self.rexurrect_at(server.endpoint())
    }

    /// A `rexurrect` invocation aimed at an arbitrary endpoint (e.g. one
    /// where no server listens).
    pub fn rexurrect_at(&self, endpoint: &str) -> RexurrectCmd {
        let mut env = self.child_env();
        env.insert("REX_SERVER".into(), endpoint.into());
        RexurrectCmd::new(env)
    }

    /// The pid of a Rex server answering at `endpoint`, if any.
    pub fn server_pid_at(&self, endpoint: &str) -> Option<u64> {
        let out = Command::new(super::server::rex_bin())
            .args(["-S", endpoint, "--autostart=false", "--timeout", "2s"])
            .args(["api", "call", "server.status"])
            .env_clear()
            .envs(self.child_env())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        if !out.status.success() {
            return None;
        }
        let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        status["pid"].as_u64()
    }

    /// Runs an interactive zsh outside Rex, on the harness config and
    /// environment (so no `REX_BLOCK`) plus `extra_env`, feeding it `input`
    /// on stdin until it exits at the end of it.
    pub fn zsh_outside_rex(&self, extra_env: &[(&str, &str)], input: &str) {
        use std::io::Write;
        let mut child = Command::new("zsh")
            .arg("-i")
            .env_clear()
            .envs(self.child_env())
            .env("TERM", "dumb")
            .envs(extra_env.iter().copied())
            .current_dir(self.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("running zsh");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait().unwrap();
    }

    /// Runs `rexurrect save` against `server` and asserts it succeeded.
    pub fn save(&self, server: &Server) -> super::rexurrect::RunOutput {
        self.rexurrect(server).arg("save").run().assert_success()
    }

    /// Runs `rexurrect restore` against `server` and asserts it succeeded.
    pub fn restore(&self, server: &Server) -> super::rexurrect::RunOutput {
        self.rexurrect(server).arg("restore").run().assert_success()
    }

    /// What `rexurrect list` prints: one line per snapshot.
    pub fn listed(&self, server: &Server) -> String {
        self.rexurrect(server)
            .arg("list")
            .run()
            .assert_success()
            .stdout
    }

    /// Saves one session per label, each a shell in its own directory, on a
    /// server that then stops. The next server started from this `TestEnv`
    /// is a restart.
    pub fn saved_on_an_earlier_server(&self, labels: &[&str]) {
        let server = self.start_server();
        for label in labels {
            let fixture =
                server.create_session(label, &[super::window(super::shell(self.mkdir(label)))]);
            server.wait_for_pwds(&fixture);
        }
        self.save(&server);
    }

    pub(crate) fn root_handle(&self) -> Arc<TempDir> {
        Arc::clone(&self.root)
    }

    fn xdg_dir(&self, which: &str) -> PathBuf {
        self.root().join("xdg").join(which)
    }

    /// The scrubbed environment shared by throwaway servers (and so every
    /// block shell) and `rexurrect` subprocesses. `REX_SERVER` is added per
    /// server.
    ///
    /// `XDG_RUNTIME_DIR` matters for safety: `rex` falls back to
    /// `$XDG_RUNTIME_DIR/rex/server.sock` (or, without it, the real server's
    /// socket) when the endpoint in `REX_SERVER` doesn't exist. Pointing it at
    /// an empty temp dir makes any such fallback fail instead of reaching the
    /// user's real server.
    pub(crate) fn child_env(&self) -> BTreeMap<OsString, OsString> {
        let mut env = BTreeMap::new();
        for var in INHERITED_VARS {
            if let Some(value) = std::env::var_os(var) {
                env.insert(OsString::from(var), value);
            }
        }
        let mut set = |k: &str, v: &OsStr| {
            env.insert(OsString::from(k), v.to_os_string());
        };
        if self.own_home {
            set("HOME", self.home().as_os_str());
        }
        set(STATE_DIR_VAR, self.state_dir().as_os_str());
        set("ZDOTDIR", self.zdotdir().as_os_str());
        set("XDG_RUNTIME_DIR", self.xdg_dir("runtime").as_os_str());
        set("XDG_STATE_HOME", self.xdg_dir("state").as_os_str());
        set("XDG_CONFIG_HOME", self.xdg_dir("config").as_os_str());
        set("XDG_DATA_HOME", self.xdg_dir("data").as_os_str());
        set("XDG_CACHE_HOME", self.xdg_dir("cache").as_os_str());
        env
    }
}

/// Prepends `dirs` to the `PATH` in `env`.
pub(crate) fn prepend_path(env: &mut BTreeMap<OsString, OsString>, dirs: &[PathBuf]) {
    if dirs.is_empty() {
        return;
    }
    let existing = env.get(OsStr::new("PATH")).cloned().unwrap_or_default();
    let mut paths: Vec<PathBuf> = dirs.to_vec();
    paths.extend(std::env::split_paths(&existing));
    let joined = std::env::join_paths(paths).expect("PATH entries must not contain ':'");
    env.insert("PATH".into(), joined);
}
