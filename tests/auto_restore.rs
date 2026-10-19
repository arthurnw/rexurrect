//! Auto-restore: a line in Rex's init.lua (the hook) runs
//! `rexurrect auto-restore`, which returns at once and, in the background,
//! waits for the server that ran init.lua to be ready, then restores the
//! latest snapshot into it, once per server instance. Results and failures go
//! to the log.
//!
//! Each throwaway server is its own server instance, and servers started from
//! one `TestEnv` share its state directory, so starting a server with the
//! hook after saving on another is a restart with auto-restore on.

mod common;

use std::path::Path;
use std::process::Command;

use common::server::wait_for;
use common::*;

/// The init.lua line that turns auto-restore on, as the README has users
/// add it: what `rexurrect auto-restore --print-hook` prints.
fn hook(env: &TestEnv) -> String {
    env.rexurrect_at("unix:///tmp/rxt-none/s.sock")
        .args(["auto-restore", "--print-hook"])
        .run()
        .assert_success()
        .stdout
}

/// The log's auto-restore entries, once there are at least `n` (each
/// auto-restore run writes its outcome last, so this waits for `n` runs to
/// finish).
fn wait_for_auto_restores(env: &TestEnv, n: usize) -> Vec<String> {
    wait_for(&format!("{n} auto-restore entries in the log"), || {
        let entries: Vec<String> = env
            .log_entries()
            .into_iter()
            .filter(|e| e.contains(" auto-restore"))
            .collect();
        (entries.len() >= n).then_some(entries)
    })
}

/// Rex runs commands from init.lua only through `rex.exec`, which takes an
/// argv list: `os.execute` stops init.lua.
#[test]
fn the_printed_hook_is_one_line_that_runs_rexurrect_without_a_shell() {
    let env = TestEnv::new();

    let printed = hook(&env);

    assert_eq!(
        printed,
        format!(
            "do local r = rex.exec({{ [[{bin}]], \"auto-restore\" }}, \
             {{ env = {{ REXURRECT_STATE_DIR = [[{state}]] }} }}) \
             if r then io.stderr:write(r.stderr) end end\n",
            bin = common::rexurrect::bin(),
            state = env.state_dir().display(),
        )
    );
}

#[test]
fn a_server_whose_init_lua_has_the_hook_restores_the_latest_snapshot_when_it_starts() {
    let env = TestEnv::new();
    let [proj, other] = ["proj", "other"].map(|name| env.mkdir(name));
    let layout = || hsplit(0.7, shell(&proj), shell(&other));
    {
        let before = env.start_server();
        let fixture = before.create_session("zz-proj", &[window(layout())]);
        before.wait_for_pwds(&fixture);
        env.save(&before);
    } // The server stops, as in a restart.

    let after = env.server().init_lua(hook(&env)).start();
    let entries = wait_for_auto_restores(&env, 1);

    let restored = after.the_session_labelled("zz-proj");
    assert_eq!(after.window_shapes(&restored), [layout().expected_shape()]);
    assert_eq!(entries.len(), 1, "{entries:#?}");
    assert!(entries[0].contains(after.endpoint()), "{}", entries[0]);
    assert!(entries[0].contains("1 created"), "{}", entries[0]);
}

#[test]
fn the_hook_restores_into_the_server_running_it_even_when_rex_server_names_another() {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    // A server started from inside another server's block inherits that
    // block's REX_SERVER, and so does its init.lua.
    let other = env.start_server();
    let server = env
        .server()
        .init_lua(hook(&env))
        .env("REX_SERVER", other.endpoint())
        .start();
    let entries = wait_for_auto_restores(&env, 1);

    assert_eq!(server.sorted_labels(), ["zz-proj"]);
    assert_eq!(other.sessions(), [], "restored into the other server");
    assert!(entries[0].contains(server.endpoint()), "{}", entries[0]);
}

#[test]
fn a_config_reload_does_not_restore_again_even_after_a_restored_session_was_killed() {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = env.server().init_lua(hook(&env)).start();
    wait_for_auto_restores(&env, 1);
    server.kill_session(&server.the_session_labelled("zz-proj"));

    reload_config(&server);
    let entries = wait_for_auto_restores(&env, 2);

    assert!(entries[1].contains("already restored"), "{entries:#?}");
    assert_eq!(server.sessions(), []);
}

#[test]
fn an_auto_restore_in_which_every_session_failed_does_not_count_as_restoring_the_instance() {
    let env = TestEnv::new();
    let a = env.mkdir("a");
    {
        let before = env.start_server();
        let wide = before.create_session("zz-wide", &[window(shell(&a)), window(shell(&a))]);
        before.wait_for_pwds(&wide);
        env.save(&before);
    }
    // A server that refuses a second window in any session can't recreate
    // zz-wide.
    let server = env
        .server()
        .arg("--max-windows")
        .arg("1")
        .init_lua(hook(&env))
        .start();
    // The failed session's entry, then the outcome.
    let entries = wait_for_auto_restores(&env, 2);
    assert!(entries[1].contains("1 failed"), "{entries:#?}");

    let out = env.rexurrect(&server).arg("save").run().assert_failure();
    assert!(out.stdout.contains("restored first"), "{out}");
}

#[test]
fn a_repeated_trigger_creates_no_duplicates() {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-one", "zz-two"]);
    // Both background runs wait for the server and then restore at once.
    let server = env
        .server()
        .init_lua(format!("{}{}", hook(&env), hook(&env)))
        .start();
    let entries = wait_for_auto_restores(&env, 2);

    assert_eq!(server.sorted_labels(), ["zz-one", "zz-two"]);
    // One run restored; the other waited for it, then found this server
    // instance restored into.
    let mut outcomes: Vec<&str> = entries
        .iter()
        .map(|e| match e {
            e if e.contains("2 created") => "restored",
            e if e.contains("already restored") => "nothing to do",
            _ => "something else",
        })
        .collect();
    outcomes.sort();
    assert_eq!(outcomes, ["nothing to do", "restored"], "{entries:#?}");
}

#[test]
fn a_trigger_while_a_restore_is_running_creates_no_duplicates() {
    let env = TestEnv::new();
    let labels = ["zz-one", "zz-three", "zz-two"];
    env.saved_on_an_earlier_server(&labels);
    let server = env.start_server();
    let slow = common::stub::slow_rex(&env, "session.create", 0.5);
    let restore = env
        .rexurrect(&server)
        .arg("restore")
        .path_prepend(&slow)
        .spawn();
    wait_for("the restore's first session", || {
        (!server.sessions().is_empty()).then_some(())
    });

    // Auto-restore turned on, and triggered, mid-restore.
    std::fs::write(server.dir().join("init.lua"), hook(&env)).unwrap();
    reload_config(&server);
    let entries = wait_for_auto_restores(&env, 1);

    let restored = restore.wait().assert_success();
    assert_eq!(restored.lines_starting("created").len(), 3, "{restored}");
    assert_eq!(server.sorted_labels(), labels);
    assert!(entries[0].contains("already restored"), "{entries:#?}");
}

#[test]
fn if_the_server_never_becomes_ready_the_background_restore_gives_up_and_logs_it() {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let go = env.root().join("go");
    let mut server = env
        .server()
        .init_lua(format!("{}{}", hook(&env), held_until(&go)))
        .env("REXURRECT_READY_TIMEOUT", "1")
        .start_without_waiting();
    let entries = wait_for_auto_restores(&env, 1);

    assert!(
        entries[0].contains("auto-restore failed: ") && entries[0].contains("ready after 1s"),
        "{entries:#?}"
    );
    // The run that gave up restores nothing once the server is ready after all.
    std::fs::write(&go, "").unwrap();
    server.wait_until_ready();
    assert_eq!(server.sessions(), []);
}

#[test]
fn the_background_restore_holds_none_of_the_servers_open_files() {
    let env = TestEnv::new();
    // Stands in for the start lock `rex server start` hands the server.
    let held = env.root().join("held");
    let go = env.root().join("go");
    let mut server = env
        .server()
        .init_lua(format!("{}{}", hook(&env), held_until(&go)))
        .holding_open(&held)
        .start_without_waiting();
    let background = wait_for("the background auto-restore", || {
        background_auto_restore(&server)
    });

    let holders = processes_holding(&held);
    assert!(
        holders.contains(&server.pid()),
        "the server holds it: {holders:?}"
    );
    assert!(
        !holders.contains(&background),
        "the background run ({background}) holds it too: {holders:?}"
    );
    std::fs::write(&go, "").unwrap();
    server.wait_until_ready();
    wait_for_auto_restores(&env, 1);
}

/// The server runs init.lua in its own environment, which usually lacks the
/// user's `XDG_STATE_HOME` (the Rex app sets none). The hook
/// `auto-restore --print-hook` prints pins the state directory resolved
/// where it ran, so auto-restore uses the same one as the user's shells.
#[test]
fn the_printed_hook_restores_from_the_state_directory_it_was_printed_with() {
    let env = TestEnv::new();
    // The user's shells: no REXURRECT_STATE_DIR, an XDG_STATE_HOME of their
    // own.
    let xdg_state_home = env.root().join("user-xdg-state");
    let users = |cmd: RexurrectCmd| {
        cmd.env_remove(STATE_DIR_VAR)
            .env("XDG_STATE_HOME", &xdg_state_home)
    };
    {
        let before = env.start_server();
        let fixture = before.create_session("zz-proj", &[window(shell(env.mkdir("proj")))]);
        before.wait_for_pwds(&fixture);
        users(env.rexurrect(&before))
            .arg("save")
            .run()
            .assert_success();
    }
    let printed = users(env.rexurrect_at("unix:///tmp/rxt-none/s.sock"))
        .args(["auto-restore", "--print-hook"])
        .run()
        .assert_success();

    // The server's environment names another state directory.
    let server = env.server().init_lua(&printed.stdout).start();
    let log = xdg_state_home.join("rexurrect/rexurrect.log");
    wait_for("auto-restore's entry in the user's log", || {
        let log = std::fs::read_to_string(&log).unwrap_or_default();
        log.contains(" auto-restore").then_some(())
    });

    assert_eq!(server.sorted_labels(), ["zz-proj"]);
}

#[test]
fn rex_config_check_runs_the_hook_but_nothing_is_restored() {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = env.start_server();
    let config = env.root().join("hook.lua");
    std::fs::write(&config, hook(&env)).unwrap();

    // `rex config check` loads init.lua in its own process, running the hook.
    let out = server
        .rex()
        .args(["config", "check"])
        .arg(&config)
        .output()
        .unwrap();

    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not by a Rex server"), "{stderr}");
    assert_eq!(server.sessions(), []);
    assert_eq!(env.log_entries(), Vec::<String>::new());
}

/// `rex.exec` captures what auto-restore prints, so the hook passes its
/// stderr on: the server's own log is where a user looks when nothing came
/// back.
#[test]
fn an_auto_restore_that_fails_at_once_says_why_in_the_servers_log() {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);

    let server = env
        .server()
        .init_lua(hook(&env))
        .env("REXURRECT_READY_TIMEOUT", "soon")
        .start();

    let log = server.log();
    assert!(log.contains("REXURRECT_READY_TIMEOUT"), "{log}");
    assert_eq!(server.sessions(), []);
}

/// An init.lua line that keeps the server from becoming ready until `go`
/// exists (for at most 30 seconds, so nothing outlives a failed test).
fn held_until(go: &Path) -> String {
    format!(
        "rex.exec({{ \"/bin/sh\", \"-c\", [[for i in $(seq 300); do [ -e '{}' ] && break; sleep 0.1; done]] }})\n",
        go.display()
    )
}

/// The pid of the background auto-restore aimed at `server`, once it runs.
fn background_auto_restore(server: &Server) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-ax", "-ww", "-o", "pid=,args="])
        .output()
        .unwrap();
    let marker = format!("auto-restore --server-pid {}", server.pid());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|line| line.trim_end().ends_with(&marker))
        .and_then(|line| line.split_whitespace().next()?.parse().ok())
}

/// The pids of the processes that have `path` open.
fn processes_holding(path: &Path) -> Vec<u32> {
    let out = Command::new("lsof").arg("-t").arg(path).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|pid| pid.trim().parse().ok())
        .collect()
}

/// `rex config reload`: the server runs its init.lua again.
fn reload_config(server: &Server) {
    let out = server.rex().args(["config", "reload"]).output().unwrap();
    assert!(
        out.status.success(),
        "rex config reload failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
