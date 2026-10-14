//! The startup window: for two minutes after restore creates a session, its
//! blocks' shells may still be starting. With a slow zsh config (here a
//! `sleep` in `.zshrc`), a block hasn't reported a working directory yet,
//! its startup runs in its foreground, and the scrollback replay may still
//! be printing. A save in that window, whichever process restored, saves
//! such a block as restore created it.
//!
//! How a test tells: a block's scrollback shows the `Last login:` banner of
//! each server its shell started on. One saved as restore created it, and
//! then restored, shows two: its first shell's and its latest one's. One
//! read while its shell started on a server in between shows three.

mod common;

use common::*;

/// Saves zz-proj, two blocks side by side, on a server that then stops. The
/// first block's last command is `make test`; the second has none, and
/// `zz-earlier` in its scrollback. No recorder: the first block's recorder
/// file is written by hand.
fn saved_on_an_earlier_server(env: &TestEnv) -> Layout {
    let [proj, other] = ["proj", "other"].map(|name| env.mkdir(name));
    let layout = hsplit(0.7, shell(&proj), shell(&other));
    let before = env.start_server();
    let fixture = before.create_session("zz-proj", &[window(layout.clone())]);
    before.wait_for_pwds(&fixture);
    let [typed, idle] = [0, 1].map(|i| fixture.block_ids()[i]);
    before.run(&fixture.id, idle, "echo zz-earlier");
    before.wait_for_capture(&fixture.id, idle, "the echo", |c| {
        c.contains("\nzz-earlier")
    });
    env.write_recorder_file(typed, "make test");
    env.save(&before);
    layout
}

/// Waits until the shell of zz-proj's second block on `server` is running
/// its slow startup.
fn wait_for_slow_startup(server: &Server) {
    let restored = server.the_session_labelled("zz-proj");
    server.wait_for_foreground(&restored, &server.block_ids(&restored)[1], "sleep");
}

/// Restores the latest snapshot into a new server, and checks zz-proj
/// comes back as [`saved_on_an_earlier_server`] saved it.
fn assert_restored_as_first_saved(env: &TestEnv, layout: &Layout) {
    let server = env.start_server();
    env.restore(&server);
    let restored = server.the_session_labelled("zz-proj");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], "make test");
    server.assert_empty_prompt(&restored, &blocks[1]);
    let capture = server.capture(&restored, &blocks[1]);
    assert!(capture.contains("\nzz-earlier"), "{capture}");
    assert_eq!(
        capture.matches("Last login:").count(),
        2,
        "only the first shell's banner and this one's:\n{capture}"
    );
    assert_eq!(server.window_shapes(&restored), [layout.expected_shape()]);
}

#[test]
fn a_save_right_after_a_restore_in_another_process_saves_the_restored_blocks_as_restore_created_them()
 {
    let env = TestEnv::builder().zshrc_line("sleep 3").build();
    let layout = saved_on_an_earlier_server(&env);
    {
        let server = env.start_server();
        env.restore(&server);
        wait_for_slow_startup(&server);
        env.save(&server);
    }
    assert_restored_as_first_saved(&env, &layout);
}

/// Restore can't tell which new block is which when `session.create`
/// answers with block IDs that don't match the saved blocks. A save in the
/// window saves such a session as it was in the snapshot it came from.
#[test]
fn a_save_in_the_startup_window_saves_a_session_whose_blocks_restore_couldnt_match_as_it_was_saved()
{
    let env = TestEnv::builder().zshrc_line("sleep 3").build();
    let layout = saved_on_an_earlier_server(&env);
    {
        let server = env.start_server();
        // session.create creates zz-proj as asked, but answers with no block IDs.
        let stub = RexStub::new(&env).rewrite_when(
            "session.create",
            "zz-proj",
            r#"s/"block_ids":\[[^]]*\]/"block_ids":[]/g"#,
        );
        env.rexurrect(&server)
            .arg("restore")
            .path_prepend(stub.dir())
            .run()
            .assert_failure();
        wait_for_slow_startup(&server);
        env.save(&server);
    }
    assert_restored_as_first_saved(&env, &layout);
}

/// Keeping one snapshot, the first save after a restore removes the
/// snapshot restore used. Its scrollback stays for the startup window: the
/// replay may not have printed it yet, and later saves in the window save
/// the restored blocks' scrollback from it.
#[test]
fn saves_keeping_one_snapshot_keep_the_scrollback_a_restore_in_its_startup_window_uses() {
    let env = TestEnv::builder().zshrc_line("sleep 3").build();
    let layout = saved_on_an_earlier_server(&env);
    {
        let server = env.start_server();
        env.restore(&server);
        wait_for_slow_startup(&server);
        for _ in 0..2 {
            env.rexurrect(&server)
                .arg("save")
                .env("REXURRECT_KEEP", "1")
                .run()
                .assert_success();
        }
    }
    assert_restored_as_first_saved(&env, &layout);
}
