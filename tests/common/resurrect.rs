//! Running the built `rex-resurrect` binary.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};

use super::env::prepend_path;

/// The binary under test.
pub fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_rex-resurrect")
}

/// One `rex-resurrect` invocation, with the harness's scrubbed environment:
/// `REX_SERVER` is the target server, the state directory override points at
/// the test's temp dir, and `REX_BLOCK`/`REX_SESSION` are absent. Get one from
/// [`super::TestEnv::resurrect`].
#[must_use]
pub struct ResurrectCmd {
    args: Vec<OsString>,
    env: BTreeMap<OsString, OsString>,
    path_prepend: Vec<PathBuf>,
}

impl ResurrectCmd {
    pub(crate) fn new(env: BTreeMap<OsString, OsString>) -> Self {
        ResurrectCmd {
            args: Vec::new(),
            env,
            path_prepend: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .insert(key.as_ref().to_os_string(), value.as_ref().to_os_string());
        self
    }

    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.env.remove(key.as_ref());
        self
    }

    /// Puts `dir` first on `PATH`, e.g. a directory holding a stub `rex`.
    pub fn path_prepend(mut self, dir: impl Into<PathBuf>) -> Self {
        self.path_prepend.push(dir.into());
        self
    }

    pub fn run(self) -> RunOutput {
        self.spawn().wait()
    }

    /// Starts the run without waiting for it, e.g. to run two at once.
    pub fn spawn(mut self) -> Running {
        prepend_path(&mut self.env, &self.path_prepend);
        let child = Command::new(bin())
            .args(&self.args)
            .env_clear()
            .envs(&self.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("running rex-resurrect");
        Running {
            command: format!("rex-resurrect {:?}", self.args),
            child: Some(child),
        }
    }
}

/// A `rex-resurrect` run that may still be going. [`Running::wait`] for it.
/// Dropped without waiting (its test panicked), it kills the run, so the run
/// can't outlive the test's temp dir and recreate files in it.
#[must_use]
pub struct Running {
    command: String,
    child: Option<Child>,
}

impl Running {
    pub fn wait(mut self) -> RunOutput {
        let child = self.child.take().expect("a run is waited for once");
        let out = child.wait_with_output().expect("waiting for rex-resurrect");
        RunOutput {
            command: std::mem::take(&mut self.command),
            status: out.status,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// What a `rex-resurrect` run printed and how it exited.
#[derive(Debug)]
pub struct RunOutput {
    pub command: String,
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    pub fn assert_success(self) -> Self {
        assert!(self.status.success(), "{} failed: {self}", self.command);
        self
    }

    pub fn assert_failure(self) -> Self {
        assert!(
            !self.status.success(),
            "{} unexpectedly succeeded: {self}",
            self.command
        );
        self
    }

    /// Stdout lines that start with `word` (e.g. `created`, `skipped`, `failed`).
    pub fn lines_starting(&self, word: &str) -> Vec<&str> {
        self.stdout
            .lines()
            .filter(|line| line.split_whitespace().next() == Some(word))
            .collect()
    }
}

impl std::fmt::Display for RunOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\n--- stdout\n{}--- stderr\n{}",
            self.status, self.stdout, self.stderr
        )
    }
}
