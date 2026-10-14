//! Snapshot files on disk: private to the user, and only the newest kept.
//!
//! The privacy test looks at the modes of the state directory's files, never
//! inside them. Retention is observed through `list`.

mod common;

use std::os::unix::fs::PermissionsExt;

use common::files::accessible_to_others;
use common::*;

const KEEP_VAR: &str = "REXURRECT_KEEP";

/// The snapshots `rexurrect list` shows, newest first, each as its session
/// labels (`zz-1, zz-2`).
fn listed_labels(env: &TestEnv, server: &Server) -> Vec<String> {
    let out = env.rexurrect(server).arg("list").run().assert_success();
    out.stdout
        .lines()
        .filter(|line| *line != "no snapshots yet")
        .map(|line| {
            // `<id>  <time>  <labels>`; the time has single spaces only.
            line.splitn(3, "  ")
                .nth(2)
                .unwrap_or_else(|| panic!("list line {line:?} has too few columns"))
                .to_string()
        })
        .collect()
}

#[test]
fn the_state_directory_and_everything_in_it_are_private_to_the_user() {
    let env = TestEnv::new();
    // As an earlier version, or the user, might have left it.
    std::fs::set_permissions(env.state_dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let server = env.start_server();
    let fixture = server.create_session("zz-private", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server); // Writes the lineage map too.
    env.save(&server);

    assert_eq!(accessible_to_others(&env.state_dir()), Vec::<String>::new());
}

/// Restore runs first after a restart (auto-restore), before any save.
#[test]
fn a_restore_on_its_own_also_leaves_the_state_directory_private() {
    let env = TestEnv::new();
    std::fs::set_permissions(env.state_dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let server = env.start_server();

    env.restore(&server);

    assert_eq!(accessible_to_others(&env.state_dir()), Vec::<String>::new());
}

#[test]
fn only_the_newest_snapshots_are_kept_and_latest_is_the_newest() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let mut fixtures = Vec::new();
    // Each save holds one more session than the one before.
    for label in ["zz-1", "zz-2", "zz-3"] {
        let fixture = server.create_session(label, &[window(shell(&a))]);
        server.wait_for_pwds(&fixture);
        env.rexurrect(&server)
            .arg("save")
            .env(KEEP_VAR, "2")
            .run()
            .assert_success();
        fixtures.push(fixture);
    }

    assert_eq!(
        listed_labels(&env, &server),
        ["zz-1, zz-2, zz-3", "zz-1, zz-2"]
    );

    for fixture in &fixtures {
        server.kill_session(&fixture.id);
    }
    env.restore(&server);
    assert_eq!(server.sorted_labels(), ["zz-1", "zz-2", "zz-3"]);
}

#[test]
fn ten_snapshots_are_kept_by_default() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-often", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);

    for _ in 0..12 {
        env.save(&server);
    }

    assert_eq!(listed_labels(&env, &server).len(), 10);
}

#[test]
fn a_keep_count_that_is_not_a_positive_number_fails_save_and_writes_nothing() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-kept", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);

    for bad in ["0", "ten", "-1"] {
        let out = env
            .rexurrect(&server)
            .arg("save")
            .env(KEEP_VAR, bad)
            .run()
            .assert_failure();
        assert!(out.stderr.contains(KEEP_VAR), "{out}");
    }

    assert_eq!(listed_labels(&env, &server), Vec::<String>::new());
}
