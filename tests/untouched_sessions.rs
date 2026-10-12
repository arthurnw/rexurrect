//! Untouched sessions aren't saved, so sessions that Rex or the user create
//! and abandon don't pile up across restarts.
//!
//! An untouched session: one window holding one shell block, default window
//! and block labels, its working directory still home, and no last command.
//! Its label doesn't count.
//!
//! Each test gives its servers and rex-resurrect a home directory of their
//! own ([`TestEnvBuilder::own_home`]), so "home" is a directory the test
//! knows and the user's real home is never involved.

mod common;

use common::*;
use serde_json::json;

fn env_with_own_home() -> TestEnv {
    TestEnv::builder().own_home().build()
}

/// A shell block created without a `cwd`, the way the Rex app creates one:
/// it starts in the server's home directory.
fn shell_in_home() -> Layout {
    block(json!({}))
}

fn kill_every_session(server: &Server) {
    for session in server.sessions() {
        server.kill_session(&session.id);
    }
}

/// Saves, kills every live session, restores, and returns the labels of the
/// sessions that came back: the saved ones.
fn labels_saved(env: &TestEnv, server: &Server) -> Vec<String> {
    env.save(server);
    kill_every_session(server);
    env.restore(server);
    server.sorted_labels()
}

#[test]
fn a_session_with_one_shell_in_home_default_labels_and_no_last_command_isnt_saved() {
    let env = env_with_own_home();
    let server = env.start_server();
    let a = env.mkdir("a");
    let untouched = server.create_session("zz-untouched", &[window(shell_in_home())]);
    let touched = server.create_session("zz-touched", &[window(shell(&a))]);
    server.wait_for_pwds(&untouched);
    server.wait_for_pwds(&touched);

    assert_eq!(labels_saved(&env, &server), ["zz-touched"]);
}

/// A second block or window, a user-set window or block label, a working
/// directory other than home (even one below it), or a last command, whether
/// recorded or the program in the foreground of a block with no record: each
/// session here differs from an untouched one in one of these ways only. (A
/// last command restore carried forward is covered further down.)
#[test]
fn any_one_change_from_untouched_gets_a_session_saved() {
    let env = env_with_own_home();
    let server = env.start_server();
    let projects = env.home().join("projects");
    std::fs::create_dir(&projects).unwrap();
    let in_home = || window(shell_in_home());
    let sessions = [
        server.create_session("zz-untouched", &[in_home()]),
        server.create_session(
            "zz-two-blocks",
            &[window(hsplit(0.5, shell_in_home(), shell_in_home()))],
        ),
        server.create_session("zz-two-windows", &[in_home(), in_home()]),
        server.create_session("zz-window-label", &[in_home().labeled("notes")]),
        server.create_session(
            "zz-block-label",
            &[window(shell_in_home().labeled("server"))],
        ),
        server.create_session("zz-below-home", &[window(shell(&projects))]),
        server.create_session("zz-recorded", &[in_home()]),
        server.create_session("zz-foreground", &[in_home()]),
    ];
    for session in &sessions {
        server.wait_for_pwds(session);
    }
    env.write_recorder_file(sessions[6].block_ids()[0], "make test");
    let (foreground, block) = (&sessions[7].id, sessions[7].block_ids()[0]);
    server.run(foreground, block, "tail -n 0 -f /dev/null");
    server.wait_for_foreground(foreground, block, "tail");

    assert_eq!(
        labels_saved(&env, &server),
        [
            "zz-below-home",
            "zz-block-label",
            "zz-foreground",
            "zz-recorded",
            "zz-two-blocks",
            "zz-two-windows",
            "zz-window-label",
        ]
    );
}

/// Rex's generated session labels (`amber-ridge`) can't be told apart from
/// user-set ones, so a session's label never makes it touched.
#[test]
fn a_session_that_was_only_renamed_isnt_saved() {
    let env = env_with_own_home();
    let server = env.start_server();
    let a = env.mkdir("a");
    let renamed = server.create_session("zz-created", &[window(shell_in_home())]);
    let touched = server.create_session("zz-touched", &[window(shell(&a))]);
    server.rename_session(&renamed.id, "zz-renamed");
    server.wait_for_pwds(&renamed);
    server.wait_for_pwds(&touched);

    assert_eq!(labels_saved(&env, &server), ["zz-touched"]);
}

/// Clearing a label gives back a default one: `Window <8 hex>` for a window,
/// and `Block <8 hex>` (not `shell`) for a block.
#[test]
fn a_session_whose_window_and_block_labels_were_cleared_isnt_saved() {
    let env = env_with_own_home();
    let server = env.start_server();
    let a = env.mkdir("a");
    let cleared = server.create_session(
        "zz-cleared",
        &[window(shell_in_home().labeled("server")).labeled("notes")],
    );
    let touched = server.create_session("zz-touched", &[window(shell(&a))]);
    server.call(
        "session.set_window_label",
        &json!({ "session_id": cleared.id, "window_id": cleared.windows[0].id, "label": "" }),
    );
    server.set_block_label(&cleared.id, cleared.block_ids()[0], "");
    server.wait_for_pwds(&cleared);
    server.wait_for_pwds(&touched);

    assert_eq!(labels_saved(&env, &server), ["zz-touched"]);
}

/// Without an OSC 7 emitter (zsh without oh-my-zsh, say) a shell never
/// reports its working directory, and save takes its foreground process's
/// instead: a physical path (`/private/tmp/...`), while `HOME` here is
/// logical (`/tmp/...`). It's home all the same.
#[test]
fn a_shell_in_home_that_never_reports_its_working_directory_is_untouched_too() {
    let env = TestEnv::builder()
        .own_home()
        .zshrc_line("precmd_functions=(${precmd_functions:#_rxt_osc7})")
        .build();
    let server = env.start_server();
    let a = env.mkdir("a");
    let untouched = server.create_session("zz-untouched", &[window(shell_in_home())]);
    let touched = server.create_session("zz-touched", &[window(shell(&a))]);
    for session in [&untouched, &touched] {
        server.wait_for_empty_prompt(&session.id, session.block_ids()[0]);
        assert_eq!(server.pwd(&session.id, session.block_ids()[0]), None);
    }

    assert_eq!(labels_saved(&env, &server), ["zz-touched"]);
}

/// A server whose only sessions are untouched is an empty server: like one
/// with no sessions at all, it never replaces the latest snapshot.
#[test]
fn save_on_a_server_holding_only_untouched_sessions_writes_nothing() {
    let env = env_with_own_home();
    let server = env.start_server();
    let a = env.mkdir("a");
    let kept = server.create_session("zz-kept", &[window(shell(&a))]);
    server.wait_for_pwds(&kept);
    env.save(&server);
    server.kill_session(&kept.id);
    let untouched = server.create_session("zz-untouched", &[window(shell_in_home())]);
    let renamed = server.create_session("zz-created", &[window(shell_in_home())]);
    server.rename_session(&renamed.id, "zz-renamed");
    server.wait_for_pwds(&untouched);
    server.wait_for_pwds(&renamed);
    let snapshots = env.listed(&server);

    env.save(&server);

    assert_eq!(env.listed(&server), snapshots, "no new snapshot");
    kill_every_session(&server);
    env.restore(&server);
    assert_eq!(
        server.sorted_labels(),
        ["zz-kept"],
        "the earlier snapshot is still the latest"
    );
}

/// Restore carries a block's last command forward, so a restored session
/// whose only content is that command doesn't turn untouched just because
/// nothing has run in it since.
#[test]
fn a_session_whose_only_content_is_a_last_command_stays_saved_through_a_restore() {
    let env = env_with_own_home();
    {
        let first = env.start_server();
        let session = first.create_session("zz-last-command", &[window(shell_in_home())]);
        first.wait_for_pwds(&session);
        env.write_recorder_file(session.block_ids()[0], "make test");
        env.save(&first);
    } // The server stops, as in a restart.

    {
        let second = env.start_server();
        env.restore(&second);
        let restored = second.the_session_labelled("zz-last-command");
        let block = &second.block_ids(&restored)[0];
        second.wait_for_typed(&restored, block, "make test");
        second.wait_for_pwd(&restored, block);
        // Only in the second snapshot: shows that the second save wrote one.
        let a = env.mkdir("a");
        let marker = second.create_session("zz-marker", &[window(shell(&a))]);
        second.wait_for_pwds(&marker);
        env.save(&second);
    }

    let third = env.start_server();
    env.restore(&third);
    assert_eq!(third.sorted_labels(), ["zz-last-command", "zz-marker"]);
    let restored = third.the_session_labelled("zz-last-command");
    third.wait_for_typed(&restored, &third.block_ids(&restored)[0], "make test");
}
