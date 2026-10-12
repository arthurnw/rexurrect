//! A save from an empty server writes nothing and leaves the latest snapshot
//! alone, so an autosave between a restart and a restore can't destroy it.

mod common;

use common::server::wait_for;
use common::*;

/// Types `exit` in the session's only shell and waits until Rex reports the
/// session with no windows. Rex keeps such a session live.
fn exit_last_shell(server: &Server, session: &FixtureSession) {
    // Rex keeps a block open when its process exits within 250 ms of
    // starting (the terminal option `exit.quick_exit_threshold_ms`), and the
    // harness shell reports its `pwd` sooner than that.
    server.wait_for_pwds(session);
    std::thread::sleep(std::time::Duration::from_millis(500));
    server.write(&session.id, session.block_ids()[0], "exit\n");
    wait_for("the session to have no windows", || {
        server
            .view(&session.id)
            .get("windows")
            .is_none()
            .then_some(())
    });
}

#[test]
fn save_on_a_fresh_server_with_no_sessions_keeps_the_latest_snapshot() {
    let env = TestEnv::new();
    let a = env.mkdir("a");
    let layout = || hsplit(0.6, shell(&a), shell(&a));
    {
        let before = env.start_server();
        let fixture = before.create_session("zz-kept", &[window(layout())]);
        before.wait_for_pwds(&fixture);
        env.save(&before);
    } // The server stops, as in a restart.

    let after = env.start_server();
    assert!(
        after.sessions().is_empty(),
        "a fresh server has no sessions"
    );
    env.save(&after);

    env.restore(&after);
    let restored = after.session_ids_labelled("zz-kept");
    assert_eq!(
        restored.len(),
        1,
        "the earlier snapshot is still the latest"
    );
    assert_eq!(
        after.window_shapes(&restored[0]),
        [layout().expected_shape()]
    );
}

/// The same guard on a server instance that has already been restored into,
/// where a save never restores first.
#[test]
fn save_after_every_session_was_killed_keeps_the_latest_snapshot() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let fixture = server.create_session("zz-kept", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    env.save(&server);
    server.kill_session(&fixture.id);

    env.save(&server);

    env.restore(&server);
    let restored = server.session_ids_labelled("zz-kept");
    assert_eq!(
        restored.len(),
        1,
        "the earlier snapshot is still the latest"
    );
    assert_eq!(
        server.window_shapes(&restored[0]),
        [shell(&a).expected_shape()]
    );
}

/// A session with no windows has nothing to restore: it's left out instead
/// of failing the save, and a server holding only such sessions is empty.
#[test]
fn a_session_whose_last_shell_exited_is_left_out_of_the_snapshot() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let kept = server.create_session("zz-kept", &[window(shell(&a))]);
    let emptied = server.create_session("zz-emptied", &[window(shell(&a))]);
    server.wait_for_pwds(&kept);
    exit_last_shell(&server, &emptied);

    env.save(&server);
    server.kill_session(&kept.id);
    env.save(&server); // Only zz-emptied is live: an empty server.
    server.kill_session(&emptied.id);

    env.restore(&server);
    assert_eq!(server.sorted_labels(), ["zz-kept"]);
}
