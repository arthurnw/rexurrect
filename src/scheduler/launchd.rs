//! The launchd scheduler: a user agent in `~/Library/LaunchAgents`, loaded
//! into the user's GUI domain with `launchctl`.
//!
//! For testing:
//! - `launchctl` is found on `PATH`, and `$REX_RESURRECT_LAUNCH_AGENTS_DIR`
//!   replaces `~/Library/LaunchAgents`, so tests can install and uninstall
//!   with a stand-in `launchctl` and never touch the real launchd;
//! - `$REX_RESURRECT_AUTOSAVE_LABEL` replaces the agent's label, so a
//!   scratch agent tried out on the real launchd stays apart from the
//!   user's own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{Job, Scheduler};
use crate::account;
use crate::error::{Context, Error, Result};
use crate::settings;
use crate::state;

/// The autosave agent's label: its name in launchd and its plist's file name.
const LABEL: &str = "io.github.cemcatik.rex-resurrect.autosave";
const AGENTS_DIR_VAR: &str = "REX_RESURRECT_LAUNCH_AGENTS_DIR";
const LABEL_VAR: &str = "REX_RESURRECT_AUTOSAVE_LABEL";

#[derive(Debug)]
pub struct Launchd {
    label: String,
    agents_dir: PathBuf,
    /// The user's GUI domain, `gui/<uid>`, where agents in
    /// `~/Library/LaunchAgents` are loaded at login.
    domain: String,
}

impl Launchd {
    pub fn from_env() -> Result<Launchd> {
        let agents_dir = match settings::env_non_empty(AGENTS_DIR_VAR) {
            Some(dir) => PathBuf::from(dir),
            None => account::home()?.join("Library/LaunchAgents"),
        };
        let label = match settings::env_non_empty(LABEL_VAR) {
            Some(label) => label
                .into_string()
                .map_err(|label| Error::new(format!("{LABEL_VAR} isn't valid UTF-8: {label:?}")))?,
            None => LABEL.to_string(),
        };
        if label.contains('/') {
            return Err(Error::new(format!(
                "{LABEL_VAR} can't contain `/`: {label:?}"
            )));
        }
        Ok(Launchd {
            label,
            agents_dir,
            domain: format!("gui/{}", current_uid()),
        })
    }

    fn plist_path(&self) -> PathBuf {
        self.agents_dir.join(format!("{}.plist", self.label))
    }

    /// The agent as a launchctl service target: `gui/<uid>/<label>`.
    fn service(&self) -> String {
        format!("{}/{}", self.domain, self.label)
    }

    fn is_loaded(&self) -> Result<bool> {
        Ok(launchctl(&["print", &self.service()])?.is_ok())
    }
}

impl Scheduler for Launchd {
    fn describe(&self) -> String {
        format!(
            "launchd agent {} ({})",
            self.label,
            self.plist_path().display()
        )
    }

    /// A plist left in the LaunchAgents directory is loaded at every login,
    /// so one that launchd refused to load now is removed again.
    fn install(&self, job: &Job) -> Result<()> {
        let path = self.plist_path();
        write_plist(&path, &plist(&self.label, job))?;
        if self.is_loaded()? {
            launchctl_ok(&["bootout", &self.service()])?;
        }
        let loaded = launchctl_ok(&["bootstrap", &self.domain, path_str(&path)?]);
        if let Err(e) = loaded {
            return Err(match state::remove_if_exists(&path) {
                Ok(_) => Error::new(format!("{e}; removed {} again", path.display())),
                Err(removing) => Error::new(format!("{e}; and then {removing}")),
            });
        }
        Ok(())
    }

    fn uninstall(&self) -> Result<bool> {
        let loaded = self.is_loaded()?;
        if loaded {
            launchctl_ok(&["bootout", &self.service()])?;
        }
        let removed = state::remove_if_exists(&self.plist_path())?;
        Ok(loaded || removed)
    }
}

/// The agent's plist: runs the job every interval, in the background, with
/// its output discarded.
fn plist(label: &str, job: &Job) -> String {
    let mut arguments = String::new();
    for arg in std::iter::once(&job.program).chain(&job.args) {
        arguments.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(arg)));
    }
    let mut environment = String::new();
    for (name, value) in &job.env {
        environment.push_str(&format!(
            "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
            xml_escape(name),
            xml_escape(value)
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
{arguments}	</array>
	<key>EnvironmentVariables</key>
	<dict>
{environment}	</dict>
	<key>StartInterval</key>
	<integer>{interval}</integer>
	<key>ProcessType</key>
	<string>Background</string>
	<key>StandardOutPath</key>
	<string>/dev/null</string>
	<key>StandardErrorPath</key>
	<string>/dev/null</string>
</dict>
</plist>
"#,
        label = xml_escape(label),
        interval = job.interval.seconds(),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Writes the plist atomically (a temporary file, then a rename), so launchd
/// never reads half of one. Agent plists are `0644`, like any other.
fn write_plist(path: &Path, contents: &str) -> Result<()> {
    let dir = path.parent().expect("the plist is in a directory");
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    state::write_atomically(path, contents.as_bytes(), 0o644)
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::new(format!("{} isn't valid UTF-8", path.display())))
}

/// Runs `launchctl` (from `PATH`): `Ok(Err(message))` when it ran and failed.
fn launchctl(args: &[&str]) -> Result<std::result::Result<(), String>> {
    let out = Command::new("launchctl")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running `launchctl {}`", args.join(" ")))?;
    if out.status.success() {
        return Ok(Ok(()));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Ok(Err(format!(
        "`launchctl {}` failed ({}): {}",
        args.join(" "),
        out.status,
        stderr.trim()
    )))
}

/// Runs `launchctl`, failing if it does.
fn launchctl_ok(args: &[&str]) -> Result<()> {
    launchctl(args)?.map_err(Error::new)
}

fn current_uid() -> u32 {
    // SAFETY: getuid can't fail and has no preconditions.
    unsafe { libc::getuid() }
}
