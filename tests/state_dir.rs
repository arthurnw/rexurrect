//! Where rex-resurrect keeps its state: `$REX_RESURRECT_STATE_DIR`, else
//! `$XDG_STATE_HOME/rex-resurrect`, else `~/.local/state/rex-resurrect`.

mod common;

use std::path::Path;

use common::files::has_entries;
use common::*;

/// A server holding one fixture session whose block has reported its `pwd`.
fn server_with_fixture(env: &TestEnv, label: &str) -> (Server, FixtureSession) {
    let server = env.start_server();
    let fixture = server.create_session(label, &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    (server, fixture)
}

#[test]
fn the_override_wins_over_xdg_state_home() {
    let env = TestEnv::new();
    let (server, _) = server_with_fixture(&env, "zz-override");
    let xdg = env.mkdir("xdg-state");

    env.resurrect(&server)
        .arg("save")
        .env("XDG_STATE_HOME", &xdg)
        .run()
        .assert_success();

    assert!(
        has_entries(&env.state_dir()),
        "state written to the override dir"
    );
    assert!(!has_entries(&xdg), "nothing written under XDG_STATE_HOME");
}

#[test]
fn without_the_override_state_lives_in_xdg_state_home() {
    let env = TestEnv::new();
    let (server, fixture) = server_with_fixture(&env, "zz-xdg");
    let [xdg, other_xdg] = ["xdg-state", "other-xdg-state"].map(|name| env.mkdir(name));
    let run = |command: &str, xdg: &Path| {
        env.resurrect(&server)
            .arg(command)
            .env_remove(STATE_DIR_VAR)
            .env("XDG_STATE_HOME", xdg)
            .run()
            .assert_success()
    };

    run("save", &xdg);
    assert!(has_entries(&xdg.join("rex-resurrect")));
    assert!(
        !has_entries(&env.state_dir()),
        "the override dir is untouched"
    );
    server.kill_session(&fixture.id);

    run("restore", &other_xdg);
    assert!(
        server.sessions().is_empty(),
        "another XDG_STATE_HOME has no snapshot"
    );
    run("restore", &xdg);
    assert_eq!(server.session_ids_labelled("zz-xdg").len(), 1);
}

#[test]
fn without_xdg_state_home_state_lives_under_home() {
    let env = TestEnv::new();
    let (server, _) = server_with_fixture(&env, "zz-home");
    let home = env.mkdir("home");

    env.resurrect(&server)
        .arg("save")
        .env_remove(STATE_DIR_VAR)
        .env_remove("XDG_STATE_HOME")
        .env("HOME", &home)
        .run()
        .assert_success();

    assert!(has_entries(&home.join(".local/state/rex-resurrect")));
}
