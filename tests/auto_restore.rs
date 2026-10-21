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
use std::time::Duration;

use common::server::wait_for;
use common::*;
use serde_json::json;

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

/// A `TestEnv` whose servers and rexurrect runs have a home directory of
/// their own, so a session created without a `cwd` is in home and can be
/// untouched.
fn env_with_own_home() -> TestEnv {
    TestEnv::builder().own_home().build()
}

/// A server whose auto-restore, once turned on, looks for the app session
/// for `seconds` after restoring.
fn server_watching_for(env: &TestEnv, seconds: u64) -> Server {
    env.server()
        .env(APP_SESSION_WATCH_VAR, seconds.to_string())
        .start()
}

/// An untouched session: one shell block in home, created the way the Rex
/// app creates its own. It stands in for the app session.
fn untouched_session(server: &Server, label: &str) -> FixtureSession {
    let session = server.create_session(label, &[window(block(json!({})))]);
    server.wait_for_pwds(&session);
    session
}

/// Adds the hook to a running server's init.lua and reloads its config: its
/// auto-restore restores into it if nothing has yet, after whatever the test
/// created in it first.
fn turn_on_auto_restore(env: &TestEnv, server: &Server) {
    std::fs::write(server.dir().join("init.lua"), hook(env)).unwrap();
    reload_config(server);
}

/// Waits until a session labelled `label` is live on `server`.
fn wait_until_live(server: &Server, label: &str) {
    wait_for(&format!("{label} live"), || {
        (!server.session_ids_labelled(label).is_empty()).then_some(())
    });
}

/// Waits until auto-restore has restored `label` into `server` and finished
/// restoring: it may still be looking for the app session.
fn wait_until_restored(env: &TestEnv, server: &Server, label: &str) {
    wait_until_live(server, label);
    // A restore waits for the restore lock, which auto-restore holds until
    // it has finished restoring. A dry run changes nothing.
    env.rexurrect(server)
        .args(["restore", "--dry-run"])
        .run()
        .assert_success();
}

/// What an auto-restore entry says about a closed app session.
fn closed(label: &str, session: &FixtureSession) -> String {
    format!("closed app session {label:?} ({})", session.id)
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
fn rex_config_check_runs_the_hook_but_nothing_is_restored_or_closed() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = env.start_server();
    untouched_session(&server, "zz-app");
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
    assert_eq!(server.sorted_labels(), ["zz-app"]);
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

/// `rex.exec` returns `nil` when its program never ran, as when rexurrect
/// moved after the hook was printed. The hook then passes nothing on, and
/// the rest of init.lua still runs: Rex warns about the program itself.
#[test]
fn a_hook_whose_rexurrect_is_gone_lets_the_rest_of_init_lua_run() {
    let env = TestEnv::new();
    let server = env.start_server();
    let printed = hook(&env);
    let gone = env.root().join("gone/rexurrect");
    let line = printed.replace(common::rexurrect::bin(), &gone.to_string_lossy());
    assert_ne!(line, printed, "the hook names rexurrect by its path");
    let after = env.root().join("after-the-hook");
    let config = env.root().join("hook.lua");
    std::fs::write(
        &config,
        format!(
            "{line}rex.exec({{ \"/usr/bin/touch\", [[{}]] }})\n",
            after.display()
        ),
    )
    .unwrap();

    let out = server
        .rex()
        .args(["config", "check"])
        .arg(&config)
        .output()
        .unwrap();

    assert!(out.status.success(), "{out:?}");
    assert!(after.exists(), "init.lua stopped at the hook: {out:?}");
}

#[test]
fn after_auto_restore_creates_sessions_an_untouched_session_restore_did_not_create_is_closed() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 2);
    let app = untouched_session(&server, "zz-app");

    turn_on_auto_restore(&env, &server);
    let entries = wait_for_auto_restores(&env, 1);

    assert_eq!(server.sorted_labels(), ["zz-proj"]);
    assert_eq!(entries.len(), 1, "{entries:#?}");
    assert!(entries[0].contains("1 created"), "{}", entries[0]);
    assert!(
        entries[0].contains(&closed("zz-app", &app)),
        "{}",
        entries[0]
    );
}

/// On builds before 1282 the app created its session up to 28 seconds after
/// the server was ready, which can be after a fast restore has finished.
#[test]
fn an_untouched_session_created_after_auto_restore_finished_but_while_it_looks_is_closed() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 10);
    turn_on_auto_restore(&env, &server);
    wait_until_restored(&env, &server, "zz-proj");

    let app = untouched_session(&server, "zz-app");
    let entries = wait_for_auto_restores(&env, 1);

    assert_eq!(server.sorted_labels(), ["zz-proj"]);
    assert!(
        entries[0].contains(&closed("zz-app", &app)),
        "{}",
        entries[0]
    );
}

#[test]
fn an_untouched_session_created_once_auto_restore_stopped_looking_is_left_alone() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 1);
    turn_on_auto_restore(&env, &server);
    let entries = wait_for_auto_restores(&env, 1);

    untouched_session(&server, "zz-late");
    std::thread::sleep(Duration::from_secs(2));

    assert_eq!(server.sorted_labels(), ["zz-late", "zz-proj"]);
    // With nothing closed, the entry reads as it did before #36.
    assert_eq!(
        entries[0],
        format!(
            "{} auto-restore into {} (server instance {}): created  zz-proj  {}; \
             restore: 1 created, 0 skipped, 0 failed",
            timestamp_of(&entries[0]),
            server.endpoint(),
            server.status()["instance_id"].as_str().unwrap(),
            server.the_session_labelled("zz-proj"),
        )
    );
}

/// Auto-restore releases the restore lock once it has restored, before it
/// looks for the app session.
#[test]
fn a_save_while_auto_restore_looks_for_the_app_session_does_not_wait_for_it() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 10);
    turn_on_auto_restore(&env, &server);
    wait_until_live(&server, "zz-proj");

    let saved = env.save(&server);

    assert!(saved.stdout.contains("saved 1 session(s)"), "{saved}");
    assert!(!saved.stdout.contains("restored first"), "{saved}");
    assert_eq!(
        env.log_entries(),
        Vec::<String>::new(),
        "auto-restore stopped looking before the save finished"
    );
    wait_for_auto_restores(&env, 1);
}

/// The app creates one session, and the earliest is likelier its: a session
/// the user opened looks the same. Labelled against creation order.
#[test]
fn of_two_untouched_sessions_restore_did_not_create_only_the_earliest_created_is_closed() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 2);
    let earlier = untouched_session(&server, "zz-the-app");
    untouched_session(&server, "zz-mine");

    turn_on_auto_restore(&env, &server);
    let entries = wait_for_auto_restores(&env, 1);

    assert_eq!(server.sorted_labels(), ["zz-mine", "zz-proj"]);
    assert!(
        entries[0].contains(&closed("zz-the-app", &earlier)),
        "{}",
        entries[0]
    );
}

/// Each session here differs from an untouched one in one way only.
#[test]
fn a_session_restore_did_not_create_that_isnt_untouched_is_never_closed() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 2);
    let in_home = || window(block(json!({})));
    let sessions = [
        server.create_session("zz-two-windows", &[in_home(), in_home()]),
        server.create_session("zz-elsewhere", &[window(shell(env.mkdir("a")))]),
        server.create_session(
            "zz-block-label",
            &[window(block(json!({})).labeled("server"))],
        ),
        server.create_session("zz-recorded", &[in_home()]),
    ];
    for session in &sessions {
        server.wait_for_pwds(session);
    }
    env.write_recorder_file(sessions[3].block_ids()[0], "make test");

    turn_on_auto_restore(&env, &server);
    let entries = wait_for_auto_restores(&env, 1);

    assert_eq!(
        server.sorted_labels(),
        [
            "zz-block-label",
            "zz-elsewhere",
            "zz-proj",
            "zz-recorded",
            "zz-two-windows"
        ]
    );
    assert!(!entries[0].contains("closed"), "{}", entries[0]);
}

/// Untouched isn't one-way: a shell that goes back home is untouched again.
#[test]
fn a_session_that_turns_untouched_while_auto_restore_looks_is_closed() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 10);
    let away = server.create_session("zz-away", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&away);
    turn_on_auto_restore(&env, &server);
    wait_until_restored(&env, &server, "zz-proj");
    // Looked at, about once a second, while it's away from home.
    std::thread::sleep(Duration::from_secs(2));

    // By path: `login` gives the shell the user's real home as `HOME`.
    let block = away.block_ids()[0];
    server.run(&away.id, block, &format!("cd {}", env.home().display()));
    server.wait_for_pwd_eq(&away.id, block, &env.home());
    let entries = wait_for_auto_restores(&env, 1);

    assert_eq!(server.sorted_labels(), ["zz-proj"]);
    assert!(
        entries[0].contains(&closed("zz-away", &away)),
        "{}",
        entries[0]
    );
}

/// A saved session whose directory is gone comes back in home, as untouched
/// as the app session: only the lineage map tells them apart.
#[test]
fn a_restored_session_is_never_closed_even_when_it_looks_untouched() {
    let env = env_with_own_home();
    let gone = env.mkdir("gone");
    {
        let before = env.start_server();
        let session = before.create_session("zz-gone", &[window(shell(&gone))]);
        before.wait_for_pwds(&session);
        env.save(&before);
    }
    std::fs::remove_dir(&gone).unwrap();
    let server = server_watching_for(&env, 2);

    turn_on_auto_restore(&env, &server);
    let entries = wait_for_auto_restores(&env, 1);

    assert_eq!(server.sorted_labels(), ["zz-gone"]);
    assert!(!entries[0].contains("closed"), "{}", entries[0]);
}

#[test]
fn with_no_snapshot_auto_restore_closes_nothing() {
    let env = env_with_own_home();
    let server = server_watching_for(&env, 2);
    untouched_session(&server, "zz-app");

    turn_on_auto_restore(&env, &server);
    let entries = wait_for_auto_restores(&env, 1);

    assert!(entries[0].contains("no snapshot yet"), "{entries:#?}");
    assert_eq!(server.sorted_labels(), ["zz-app"]);
}

#[test]
fn an_auto_restore_in_which_every_session_failed_before_being_created_closes_nothing() {
    let env = env_with_own_home();
    let a = env.mkdir("a");
    {
        let before = env.start_server();
        let wide = before.create_session("zz-wide", &[window(shell(&a)), window(shell(&a))]);
        before.wait_for_pwds(&wide);
        env.save(&before);
    }
    // Refuses a second window in any session, so zz-wide can't come back.
    let server = env
        .server()
        .arg("--max-windows")
        .arg("1")
        .env(APP_SESSION_WATCH_VAR, "2")
        .start();
    untouched_session(&server, "zz-app");

    turn_on_auto_restore(&env, &server);
    // The failed session's entry, then the outcome.
    let entries = wait_for_auto_restores(&env, 2);

    assert!(entries[1].contains("1 failed"), "{entries:#?}");
    assert_eq!(server.sorted_labels(), ["zz-app"]);
}

#[test]
fn a_config_reload_after_auto_restore_closes_nothing() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = env.server().init_lua(hook(&env)).start();
    wait_for_auto_restores(&env, 1);
    untouched_session(&server, "zz-app");

    reload_config(&server);
    let entries = wait_for_auto_restores(&env, 2);

    assert!(entries[1].contains("already restored"), "{entries:#?}");
    assert_eq!(server.sorted_labels(), ["zz-app", "zz-proj"]);
}

/// A by-hand restore usually runs in a shell in the app session, which has
/// to stay; and either can run long after the server started, when an
/// untouched session is likelier the user's own.
#[test]
fn a_restore_or_a_save_by_hand_closes_nothing_even_on_an_unrestored_instance() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);

    for command in ["restore", "save"] {
        let server = env.start_server();
        untouched_session(&server, "zz-app");

        let out = env.rexurrect(&server).arg(command).run().assert_success();

        assert!(out.stdout.contains("1 created"), "{command}: {out}");
        assert_eq!(server.sorted_labels(), ["zz-app", "zz-proj"], "{command}");
    }
}

/// Logged like a session that failed to restore, and not tried again: nor
/// is another session closed in its place. The run exits non-zero.
#[test]
fn a_close_that_fails_is_logged_once_and_no_other_session_is_closed() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let stub = RexStub::new(&env).fail_when(
        "session.destroy",
        "session_id",
        1,
        "Error: refused (status=500 code=internal)",
    );
    let server = env.server().shown_as(stub.dir().join("rex")).start();
    let app = untouched_session(&server, "zz-app");
    untouched_session(&server, "zz-mine");

    // What the hook starts in the background, run in the foreground to see
    // how it exits.
    env.rexurrect(&server)
        .args(["auto-restore", "--server-pid", &server.pid().to_string()])
        .run()
        .assert_failure();
    // The failure's entry, then the outcome.
    let entries = env.log_entries();

    assert_eq!(entries.len(), 2, "{entries:#?}");
    assert_eq!(
        entries[0],
        format!(
            "{} auto-restore failed: closing app session \"zz-app\" ({}): rex api call \
             session.destroy failed (exit 1): refused (status=500 code=internal)",
            timestamp_of(&entries[0]),
            app.id
        )
    );
    assert!(entries[1].contains("1 created"), "{entries:#?}");
    assert!(!entries[1].contains("closed"), "{entries:#?}");
    assert_eq!(server.sorted_labels(), ["zz-app", "zz-mine", "zz-proj"]);
}

#[test]
fn if_the_server_stops_while_auto_restore_looks_for_the_app_session_it_says_so() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 30);
    turn_on_auto_restore(&env, &server);
    wait_until_restored(&env, &server, "zz-proj");

    drop(server);
    let entries = wait_for_auto_restores(&env, 1);

    assert!(entries[0].contains("1 created"), "{}", entries[0]);
    assert!(
        entries[0].contains("stopped looking for the app session because the server stopped"),
        "{}",
        entries[0]
    );
}

/// Each look captures sessions the way a save does, but keeps nothing a
/// save would: a session's scrollback, the program in a block's foreground
/// as its default last command.
#[test]
fn looking_for_the_app_session_writes_nothing_to_the_state_directory_but_the_log() {
    let env = env_with_own_home();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = server_watching_for(&env, 4);
    let busy = server.create_session("zz-busy", &[window(shell(env.mkdir("busy")))]);
    server.wait_for_pwds(&busy);
    turn_on_auto_restore(&env, &server);
    wait_until_restored(&env, &server, "zz-proj");
    let before = state_without_log(&env);

    let block = busy.block_ids()[0];
    server.run(&busy.id, block, "tail -n 0 -f /dev/null");
    server.wait_for_foreground(&busy.id, block, "tail");
    let entries = wait_for_auto_restores(&env, 1);

    assert!(!entries[0].contains("closed"), "{}", entries[0]);
    assert_eq!(state_without_log(&env), before);
}

/// Every file in the state directory but the log, with its contents.
fn state_without_log(env: &TestEnv) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = common::files::contents(&env.state_dir());
    files.remove(&env.state_dir().join("rexurrect.log"));
    files
}

/// The timestamp a log entry starts with.
fn timestamp_of(entry: &str) -> &str {
    entry.split_once(' ').unwrap().0
}

/// Checked before auto-restore carries on in the background, like
/// `REXURRECT_READY_TIMEOUT`. There's no 0, so it can't keep the app
/// session.
#[test]
fn an_app_session_watch_other_than_a_whole_number_of_seconds_from_1_is_refused_in_the_servers_log()
{
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);

    for watch in ["0", "soon", "1.5"] {
        let server = env
            .server()
            .init_lua(hook(&env))
            .env(APP_SESSION_WATCH_VAR, watch)
            .start();

        let log = server.log();
        assert!(
            log.contains(APP_SESSION_WATCH_VAR) && log.contains(&format!("{watch:?}")),
            "{watch}: {log}"
        );
        assert_eq!(server.sessions(), [], "{watch}");
    }
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
