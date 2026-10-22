//! Autosave: `rexurrect autosave install` puts a launchd user agent in place
//! that runs `save` on a timer, and `autosave uninstall` removes it.
//!
//! launchd itself isn't exercised. Install and uninstall run with their
//! LaunchAgents directory pointed at a temp dir and a stand-in `launchctl`
//! first on `PATH` ([`FakeLaunchd`]) that keeps track of which agents are
//! loaded. The installed agent's job is then run the way launchd would run
//! it ([`Agent::run_as_launchd`]): its program, arguments and environment
//! only, so no `REX_SERVER` from the test leaks in.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use serde_json::Value;

const AGENTS_DIR_VAR: &str = "REXURRECT_LAUNCH_AGENTS_DIR";
const LABEL: &str = "io.github.cemcatik.rexurrect.autosave";

/// A stand-in for the user's launchd domains: a LaunchAgents directory and a
/// `launchctl` that models `print`, `bootstrap` and `bootout` on the GUI
/// domain (`gui/<uid>`) and the user domain (`user/<uid>`) the way the real
/// one answers (bootstrapping a loaded agent fails, booting out an agent that
/// isn't loaded fails, the GUI domain is missing without a GUI login, and a
/// domain refuses an agent whose `LimitLoadToSessionType` leaves out its
/// session type). It keeps a copy of each loaded agent's plist as it was when
/// bootstrapped, as launchd does.
struct FakeLaunchd {
    dir: PathBuf,
}

impl FakeLaunchd {
    fn new(env: &TestEnv) -> FakeLaunchd {
        let dir = env.mkdir("launchd");
        for sub in ["bin", "loaded/gui", "loaded/user", "LaunchAgents"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let script = dir.join("bin/launchctl");
        std::fs::write(&script, launchctl_script(&dir)).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        FakeLaunchd { dir }
    }

    /// `cmd` with its LaunchAgents directory and `launchctl` pointed here.
    fn aim(&self, cmd: RexurrectCmd) -> RexurrectCmd {
        cmd.env(AGENTS_DIR_VAR, self.agents_dir())
            .path_prepend(self.dir.join("bin"))
    }

    fn agents_dir(&self) -> PathBuf {
        self.dir.join("LaunchAgents")
    }

    /// Makes every `bootstrap` from now on fail, the way launchd refuses a
    /// plist it can't load.
    fn refuse_to_load(&self) {
        std::fs::write(self.dir.join("refuse-bootstrap"), "").unwrap();
    }

    /// Takes the GUI domain away, as on a Mac nobody is logged in to at the
    /// console: only the user domain is left.
    fn without_gui_login(&self) {
        std::fs::write(self.dir.join("no-gui"), "").unwrap();
    }

    /// Brings the GUI domain back, as when the user logs in at the console.
    fn with_gui_login(&self) {
        std::fs::remove_file(self.dir.join("no-gui")).unwrap();
    }

    /// The plists in the LaunchAgents directory.
    fn installed(&self) -> Vec<Agent> {
        plists_in(&self.agents_dir())
    }

    /// The agents loaded into either fake domain, as they were bootstrapped.
    fn loaded(&self) -> Vec<Agent> {
        let mut agents = self.loaded_in("gui");
        agents.extend(self.loaded_in("user"));
        agents
    }

    /// The agents loaded into one fake domain: `"gui"` or `"user"`.
    fn loaded_in(&self, domain: &str) -> Vec<Agent> {
        plists_in(&self.dir.join("loaded").join(domain))
    }

    /// Loads `agent`'s plist straight into a fake domain, as an earlier
    /// install would have left it.
    fn preload(&self, domain: &str, agent: &Path) {
        let label = Agent::read(agent).label().to_string();
        std::fs::copy(
            agent,
            self.dir
                .join("loaded")
                .join(domain)
                .join(format!("{label}.plist")),
        )
        .unwrap();
    }

    /// The one agent installed and loaded, checking the two agree.
    fn the_agent(&self) -> Agent {
        let installed = self.installed();
        let loaded = self.loaded();
        assert_eq!(installed.len(), 1, "installed agents: {installed:?}");
        assert_eq!(loaded.len(), 1, "loaded agents: {loaded:?}");
        assert_eq!(
            installed[0].plist, loaded[0].plist,
            "the loaded agent is the installed one"
        );
        loaded.into_iter().next().unwrap()
    }
}

fn launchctl_script(dir: &Path) -> String {
    format!(
        r#"#!/bin/sh
# Stand-in launchctl (rexurrect test harness).
state='{dir}'
printf '%s\n' "$*" >> "$state/calls"
# The domain kind (gui or user) of a target like gui/501 or user/501/label.
kind() {{ printf '%s' "${{1%%/*}}"; }}
# Fails if the domain $1 doesn't exist: the GUI domain without a GUI login.
exists() {{
  if [ "$1" = gui ] && [ -f "$state/no-gui" ]; then
    echo "Could not print domain: 125: Domain does not support specified action" >&2
    exit 125
  fi
}}
case "$1" in
  print)
    domain=$(kind "$2")
    exists "$domain"
    case "$2" in */*/*) ;; *) echo "$2 = {{ ... }}"; exit 0 ;; esac
    label=${{2##*/}}
    if [ -f "$state/loaded/$domain/$label.plist" ]; then echo "$2 = {{ ... }}"; exit 0; fi
    echo "Could not find service \"$label\" in domain for user $domain" >&2
    exit 113 ;;
  bootstrap)
    domain=$(kind "$2")
    if [ -f "$state/refuse-bootstrap" ]; then
      echo "Bootstrap failed: 5: Input/output error" >&2
      exit 5
    fi
    if [ "$domain" = gui ] && [ -f "$state/no-gui" ]; then
      echo "Bootstrap failed: 125: Domain does not support specified action" >&2
      exit 125
    fi
    # launchd's default session type is Aqua; the user domain's is Background.
    session=Aqua; [ "$domain" = user ] && session=Background
    types=$(/usr/bin/plutil -extract LimitLoadToSessionType json -o - "$3" 2>/dev/null || echo '["Aqua"]')
    case "$types" in
      *\"$session\"*) ;;
      *) echo "Bootstrap failed: 5: Input/output error" >&2; exit 5 ;;
    esac
    label=$(/usr/bin/plutil -extract Label raw -o - "$3") || exit 5
    if [ -f "$state/loaded/$domain/$label.plist" ]; then
      echo "Bootstrap failed: 5: Input/output error" >&2
      exit 5
    fi
    cp "$3" "$state/loaded/$domain/$label.plist" ;;
  bootout)
    domain=$(kind "$2")
    exists "$domain"
    label=${{2##*/}}
    if [ ! -f "$state/loaded/$domain/$label.plist" ]; then
      echo "Boot-out failed: 3: No such process" >&2
      exit 3
    fi
    rm "$state/loaded/$domain/$label.plist" ;;
  *)
    echo "stand-in launchctl: unexpected call: $*" >&2
    exit 64 ;;
esac
"#,
        dir = dir.display()
    )
}

/// A launchd agent plist, parsed by `plutil`.
#[derive(Debug)]
struct Agent {
    plist: Value,
}

fn plists_in(dir: &Path) -> Vec<Agent> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    paths.iter().map(|p| Agent::read(p)).collect()
}

impl Agent {
    fn read(path: &Path) -> Agent {
        let out = Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{} isn't a valid plist: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        Agent {
            plist: serde_json::from_slice(&out.stdout).unwrap(),
        }
    }

    fn label(&self) -> &str {
        self.plist["Label"].as_str().unwrap()
    }

    fn program_arguments(&self) -> Vec<&str> {
        self.plist["ProgramArguments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect()
    }

    fn start_interval(&self) -> u64 {
        self.plist["StartInterval"].as_u64().unwrap()
    }

    /// Where launchd sends the job's stdout and stderr.
    fn output_paths(&self) -> [Option<&str>; 2] {
        ["StandardOutPath", "StandardErrorPath"].map(|key| self.plist[key].as_str())
    }

    fn environment(&self) -> Vec<(&str, &str)> {
        self.plist["EnvironmentVariables"]
            .as_object()
            .map(|vars| {
                vars.iter()
                    .map(|(k, v)| (k.as_str(), v.as_str().unwrap()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Runs the agent's job the way launchd runs it: its program and
    /// arguments, in `/`, with launchd's own small environment (no
    /// `REX_SERVER`, no state directory override, a bare `PATH`) plus the
    /// agent's `EnvironmentVariables`.
    ///
    /// Safety: `HOME` is a temp dir, so if the agent didn't name a server or
    /// a state directory, the job would fall back to defaults under that
    /// temp dir, never the real ones. `XDG_RUNTIME_DIR` (which launchd
    /// doesn't set) points at the harness's empty temp dir for the same
    /// reason: `rex` falls back to `$XDG_RUNTIME_DIR/rex/server.sock`.
    fn run_as_launchd(&self, env: &TestEnv) -> RunOutput {
        let args = self.program_arguments();
        let harness = env.child_env();
        let out = Command::new(args[0])
            .args(&args[1..])
            .env_clear()
            .env("HOME", env.mkdir("launchd-home"))
            .env("PATH", SYSTEM_PATH)
            .env(
                "XDG_RUNTIME_DIR",
                &harness[std::ffi::OsStr::new("XDG_RUNTIME_DIR")],
            )
            .envs(
                ["USER", "LOGNAME", "SHELL", "TMPDIR"]
                    .iter()
                    .filter_map(|var| {
                        harness
                            .get(std::ffi::OsStr::new(var))
                            .map(|value| (var, value))
                    }),
            )
            .envs(self.environment())
            .current_dir("/")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        RunOutput {
            command: format!("the agent's job {args:?}"),
            status: out.status,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

/// An endpoint for install tests that never run the agent's job.
const NO_SERVER: &str = "unix:///tmp/rxt-none/s.sock";

#[test]
fn install_loads_an_agent_that_saves_every_15_minutes_by_default() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);

    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install"])
        .run()
        .assert_success();

    let agent = launchd.the_agent();
    assert_eq!(agent.label(), LABEL);
    assert_eq!(
        agent.program_arguments(),
        [common::rexurrect::bin(), "save"]
    );
    assert_eq!(agent.start_interval(), 15 * 60);
}

#[test]
fn the_interval_can_be_changed_at_install() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);

    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install", "--interval", "5"])
        .run()
        .assert_success();

    assert_eq!(launchd.the_agent().start_interval(), 5 * 60);
}

#[test]
fn installing_twice_leaves_exactly_one_agent_running_with_the_latest_settings() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let install = |args: &[&str]| {
        launchd
            .aim(env.rexurrect_at(NO_SERVER))
            .args(["autosave", "install"])
            .args(args)
            .run()
            .assert_success()
    };

    install(&["--interval", "5"]);
    install(&[]);

    // the_agent checks there's one installed and one loaded, and that
    // they're the same: the second install reloaded the agent.
    assert_eq!(launchd.the_agent().start_interval(), 15 * 60);
}

#[test]
fn an_autosave_saves_the_server_and_state_install_ran_with_though_launchd_gives_neither() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let server = env.start_server();
    let fixture = server.create_session("zz-autosaved", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    launchd
        .aim(env.rexurrect(&server))
        .args(["autosave", "install"])
        .run()
        .assert_success();

    launchd.the_agent().run_as_launchd(&env).assert_success();

    // The snapshot went to the state directory install ran with.
    server.kill_session(&fixture.id);
    env.restore(&server);
    assert_eq!(server.session_ids_labelled("zz-autosaved").len(), 1);
}

#[test]
fn installed_outside_rex_autosave_saves_the_rex_apps_server() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let server = env.start_server();
    let fixture = server.create_session("zz-app", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    // The throwaway server plays the Rex app's: it answers at the app's
    // socket path under this HOME.
    let home = env.mkdir("home");
    let app_socket = home.join("Library/Application Support/rex/server.sock");
    std::fs::create_dir_all(app_socket.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(server.dir().join("s.sock"), &app_socket).unwrap();

    // No REX_SERVER, as outside Rex. The harness sets XDG_RUNTIME_DIR, which
    // moves rex's own default endpoint away from the app's server.
    let install = launchd
        .aim(env.rexurrect(&server))
        .env_remove("REX_SERVER")
        .env("HOME", &home)
        .args(["autosave", "install"])
        .run()
        .assert_success();
    launchd.the_agent().run_as_launchd(&env).assert_success();

    server.kill_session(&fixture.id);
    env.restore(&server);
    assert_eq!(server.session_ids_labelled("zz-app").len(), 1);
    // Install says which server autosave saves, so the user can check.
    let endpoint = format!(
        "unix://{}",
        app_socket.to_str().unwrap().replace(' ', "%20")
    );
    assert!(install.stdout.contains(&endpoint), "{install}");
}

#[test]
fn a_successful_autosave_is_silent() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let server = env.start_server();
    launchd
        .aim(env.rexurrect(&server))
        .args(["autosave", "install"])
        .run()
        .assert_success();
    let agent = launchd.the_agent();

    agent.run_as_launchd(&env).assert_success();

    // launchd's default for output isn't documented, so the agent says.
    assert_eq!(agent.output_paths(), [Some("/dev/null"); 2]);
    assert_eq!(env.log_entries(), Vec::<String>::new());
}

#[test]
fn an_autosave_while_rex_is_not_running_logs_the_failure_and_starts_no_server() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let endpoint = {
        let server = env.start_server();
        launchd
            .aim(env.rexurrect(&server))
            .args(["autosave", "install"])
            .run()
            .assert_success();
        server.endpoint().to_string()
    }; // Rex quits.

    let job = launchd.the_agent().run_as_launchd(&env);

    env.assert_no_server_started_at(&endpoint, "the autosave");
    job.assert_failure();
    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    assert!(entries[0].contains("save failed"), "{entries:#?}");
    assert!(entries[0].contains("isn't running"), "{entries:#?}");
}

/// Asserts `out` tells the user how to remove the zsh recorder and the
/// auto-restore line in Rex's init.lua.
fn assert_explains_backing_out(out: &RunOutput) {
    for what in [
        "rexurrect.plugin.zsh",
        ".zshrc",
        "init.lua",
        "https://github.com/cemcatik/rexurrect#auto-restore",
    ] {
        assert!(out.stdout.contains(what), "no mention of {what}: {out}");
    }
}

#[test]
fn uninstall_unloads_and_removes_the_agent_and_says_how_to_back_out_of_the_rest() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install"])
        .run()
        .assert_success();

    let out = launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "uninstall"])
        .run()
        .assert_success();

    assert!(launchd.loaded().is_empty(), "the agent was unloaded");
    assert!(launchd.installed().is_empty(), "its plist was removed");
    assert_explains_backing_out(&out);
}

#[test]
fn uninstall_without_an_agent_changes_nothing_and_still_says_how_to_back_out() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);

    let out = launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "uninstall"])
        .run()
        .assert_success();

    assert!(out.stdout.contains("wasn't installed"), "{out}");
    assert_explains_backing_out(&out);
}

#[test]
fn an_interval_that_is_not_a_whole_number_of_minutes_installs_nothing() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);

    for bad in ["0", "-5", "1.5", "15m", ""] {
        let out = launchd
            .aim(env.rexurrect_at(NO_SERVER))
            .args(["autosave", "install", "--interval", bad])
            .run()
            .assert_failure();
        assert!(out.stderr.contains("--interval"), "{out}");
    }
    assert!(launchd.installed().is_empty());
    assert!(launchd.loaded().is_empty());
}

#[test]
fn a_test_label_keeps_a_scratch_agent_apart_from_the_real_one() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let scratch = "io.github.cemcatik.rexurrect.autosave.test-1234";
    let autosave = |action: &str| {
        launchd
            .aim(env.rexurrect_at(NO_SERVER))
            .env("REXURRECT_AUTOSAVE_LABEL", scratch)
            .args(["autosave", action])
            .run()
            .assert_success()
    };

    autosave("install");
    assert_eq!(launchd.the_agent().label(), scratch);
    assert!(
        launchd
            .agents_dir()
            .join(format!("{scratch}.plist"))
            .is_file()
    );

    autosave("uninstall");
    assert!(launchd.installed().is_empty());
    assert!(launchd.loaded().is_empty());
}

/// A plist left in LaunchAgents is loaded at the next login, so an agent
/// launchd refused to load now mustn't stay there.
#[test]
fn an_agent_launchd_fails_to_load_is_not_left_installed() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    launchd.refuse_to_load();

    let out = launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install"])
        .run()
        .assert_failure();

    assert!(out.stderr.contains("Bootstrap failed"), "{out}");
    assert!(launchd.installed().is_empty(), "its plist was removed");
    assert!(launchd.loaded().is_empty());
}

/// Rex moves its CLI on updates, so the agent doesn't pin one: with no `rex`
/// on its `PATH`, save runs the one a running Rex server runs (ADR-0003).
#[test]
fn the_agent_gets_only_the_system_path_whether_or_not_rex_is_on_path_at_install() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let with_rex = env.rexurrect_at(NO_SERVER);
    let without_rex = env.rexurrect_at(NO_SERVER).env("PATH", "/usr/bin:/bin");

    for install in [with_rex, without_rex] {
        launchd
            .aim(install)
            .args(["autosave", "install"])
            .run()
            .assert_success();

        let agent = launchd.the_agent();
        assert!(
            agent.environment().contains(&("PATH", SYSTEM_PATH)),
            "{agent:?}"
        );
    }
}

#[test]
fn install_hands_the_programs_whose_blocks_get_no_scrollback_on_to_autosave() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);

    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .env("REXURRECT_NO_SCROLLBACK", "claude aider")
        .args(["autosave", "install"])
        .run()
        .assert_success();

    let agent = launchd.the_agent();
    assert!(
        agent
            .environment()
            .contains(&("REXURRECT_NO_SCROLLBACK", "claude aider")),
        "{agent:?}"
    );
}

#[test]
fn autosaves_keep_as_many_snapshots_as_install_was_told_to() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let server = env.start_server();
    let fixture = server.create_session("zz-kept", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    launchd
        .aim(env.rexurrect(&server))
        .env("REXURRECT_KEEP", "2")
        .args(["autosave", "install"])
        .run()
        .assert_success();
    let agent = launchd.the_agent();

    for _ in 0..3 {
        agent.run_as_launchd(&env).assert_success();
    }

    let list = env.rexurrect(&server).arg("list").run().assert_success();
    assert_eq!(list.stdout.lines().count(), 2, "{list}");
}

#[test]
fn with_a_gui_login_install_loads_the_agent_into_the_gui_domain() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);

    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install"])
        .run()
        .assert_success();

    assert_eq!(launchd.loaded_in("gui").len(), 1);
    assert!(launchd.loaded_in("user").is_empty());
}

/// A headless Mac reached over SSH has no GUI domain, only the user domain.
#[test]
fn without_a_gui_login_install_loads_the_agent_into_the_user_domain() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    launchd.without_gui_login();

    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install"])
        .run()
        .assert_success();

    assert!(launchd.loaded_in("gui").is_empty());
    assert_eq!(launchd.loaded_in("user").len(), 1);
    // the_agent checks the loaded agent is the installed plist.
    assert_eq!(launchd.the_agent().label(), LABEL);
}

#[test]
fn an_agent_installed_without_a_gui_login_still_saves() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    launchd.without_gui_login();
    let server = env.start_server();
    let fixture = server.create_session("zz-headless", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    launchd
        .aim(env.rexurrect(&server))
        .args(["autosave", "install"])
        .run()
        .assert_success();

    launchd.the_agent().run_as_launchd(&env).assert_success();

    server.kill_session(&fixture.id);
    env.restore(&server);
    assert_eq!(server.session_ids_labelled("zz-headless").len(), 1);
}

/// An agent loaded into the user domain stays there after a GUI login, so
/// installing again then must not leave a second copy running.
#[test]
fn installing_again_after_a_gui_login_moves_the_agent_to_the_gui_domain() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    let install = || {
        launchd
            .aim(env.rexurrect_at(NO_SERVER))
            .args(["autosave", "install"])
            .run()
            .assert_success()
    };
    launchd.without_gui_login();
    install();
    launchd.with_gui_login();

    install();

    assert_eq!(launchd.loaded_in("gui").len(), 1);
    assert!(launchd.loaded_in("user").is_empty());
}

#[test]
fn uninstall_unloads_the_agent_from_whichever_domain_it_is_in() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    launchd.without_gui_login();
    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install"])
        .run()
        .assert_success();
    // The user has since logged in at the console.
    launchd.with_gui_login();

    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "uninstall"])
        .run()
        .assert_success();

    assert!(launchd.loaded().is_empty(), "the agent was unloaded");
    assert!(launchd.installed().is_empty(), "its plist was removed");
}

/// An agent an earlier rexurrect loaded into both domains (by hand, say) is
/// replaced by one.
#[test]
fn install_replaces_copies_loaded_in_both_domains_with_one() {
    let env = TestEnv::new();
    let launchd = FakeLaunchd::new(&env);
    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install"])
        .run()
        .assert_success();
    let plist = launchd.agents_dir().join(format!("{LABEL}.plist"));
    launchd.preload("user", &plist);

    launchd
        .aim(env.rexurrect_at(NO_SERVER))
        .args(["autosave", "install"])
        .run()
        .assert_success();

    assert_eq!(launchd.loaded_in("gui").len(), 1);
    assert!(launchd.loaded_in("user").is_empty());
}
