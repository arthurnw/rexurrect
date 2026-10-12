//! Each block's last command comes back typed at its prompt, never run.

mod common;

use common::server::wait_for_file;
use common::*;

#[test]
fn the_last_command_is_typed_at_the_prompt_after_restore_but_not_run() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session("zz-typed", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    server.wait_for_pwds(&fixture);
    let [finished, running] = [0, 1].map(|i| fixture.windows[0].block_ids[i].as_str());

    // A command with a visible side effect, so the test can tell whether it ran.
    let marker = a.join("zz-marker");
    let touch = format!("touch {}", marker.display());
    server.run(&fixture.id, finished, &touch);
    wait_for_file(&marker);
    // A program still running at save time is recorded as it starts.
    let tail = "tail -f /dev/null";
    server.run(&fixture.id, running, tail);
    server.wait_for_foreground(&fixture.id, running, "tail");

    env.save(&server);
    server.kill_session(&fixture.id);
    std::fs::remove_file(&marker).unwrap();
    env.restore(&server);

    let restored = server.the_session_labelled("zz-typed");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], &touch);
    server.wait_for_typed(&restored, &blocks[1], tail);
    assert!(!marker.exists(), "the typed command must not have run");
}

#[test]
fn a_block_where_no_command_was_ever_recorded_restores_with_an_empty_prompt() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session("zz-empty", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    server.wait_for_pwds(&fixture);
    let command = "echo zz-recorded";
    server.run(&fixture.id, fixture.block_ids()[0], command);
    server.wait_for_capture(&fixture.id, fixture.block_ids()[0], "the echo", |c| {
        c.contains("\nzz-recorded")
    });

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-empty");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], command);
    server.assert_empty_prompt(&restored, &blocks[1]);
}

/// A save that reads a block while its shell is still starting (a slow zsh
/// config, here a `sleep` in `.zshrc`) doesn't take the startup's foreground
/// program for its last command: in its startup window, a block restore
/// created is saved as restore created it, here with no last command.
#[test]
fn a_block_restored_with_no_last_command_gets_none_from_a_save_while_its_shell_starts() {
    let env = TestEnv::builder().zshrc_line("sleep 3").build();
    let server = env.start_server();
    let fixture = server.create_session("zz-slow", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);
    let restored = server.the_session_labelled("zz-slow");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_foreground(&restored, &block, "sleep");

    env.save(&server);
    server.kill_session(&restored);
    env.restore(&server);

    let again = server.the_session_labelled("zz-slow");
    let block = server.block_ids(&again).remove(0);
    // The replayed scrollback ends in the old prompt: wait for the shell's
    // own first prompt, which reports its working directory.
    server.wait_for_pwd(&again, &block);
    server.assert_empty_prompt(&again, &block);
}

/// What the recorder recorded beats everything else save knows about a
/// block: the program in its foreground (here `tail`, run through an alias,
/// which the recorder records as typed) and what restore typed there.
#[test]
fn the_recorders_record_beats_the_program_in_a_restored_blocks_foreground() {
    let plugin = common::zsh::quote(&common::zsh::recorder_plugin());
    let env = TestEnv::builder()
        .zshrc_line(format!("source {plugin}"))
        .zshrc_line("alias zz-follow='tail -f /dev/null'")
        .build();
    let server = env.start_server();
    let fixture = server.create_session("zz-alias", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let command = "echo zz-before";
    server.run(&fixture.id, fixture.block_ids()[0], command);
    server.wait_for_capture(&fixture.id, fixture.block_ids()[0], "the echo", |c| {
        c.contains("\nzz-before")
    });
    let (session, block) = save_kill_restore(&env, &server, &fixture.id, "zz-alias");
    server.wait_for_typed(&session, &block, command);
    env.end_startup_windows();

    // Clear the typed command (Ctrl-U) and follow.
    server.write(&session, &block, "\u{15}zz-follow\n");
    server.wait_for_foreground(&session, &block, "tail");
    let (session, block) = save_kill_restore(&env, &server, &session, "zz-alias");
    server.wait_for_typed(&session, &block, "zz-follow");
}

#[test]
fn restore_never_types_a_multi_line_command_even_if_the_snapshot_holds_one() {
    // No zsh recorder: a recorder file is written by hand instead, playing a
    // recorder that doesn't skip multi-line commands.
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session("zz-multi", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    server.wait_for_pwds(&fixture);
    let markers = ["m1", "m2", "m3", "m4"].map(|name| a.join(name));
    let touch = |i: usize| format!("touch {}", markers[i].display());
    // Typed into a terminal, a line feed or a carriage return acts as Enter.
    let commands = [
        format!("{}\n{}", touch(0), touch(1)),
        format!("{}\r{}", touch(2), touch(3)),
    ];
    for (block, command) in fixture.block_ids().into_iter().zip(&commands) {
        env.write_recorder_file(block, command);
    }

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-multi");
    for block in server.block_ids(&restored) {
        server.assert_empty_prompt(&restored, &block);
    }
    for marker in &markers {
        assert!(
            !marker.exists(),
            "{} must not have been created",
            marker.display()
        );
    }
}

/// Saves, kills `session` and restores; returns the restored session (the one
/// labelled `label`) and its first block.
fn save_kill_restore(
    env: &TestEnv,
    server: &Server,
    session: &str,
    label: &str,
) -> (String, String) {
    env.save(server);
    server.kill_session(session);
    env.restore(server);
    let restored = server.the_session_labelled(label);
    let block = server.block_ids(&restored).remove(0);
    (restored, block)
}

#[test]
fn the_last_command_carries_forward_through_later_saves_and_restores_until_a_new_one_runs() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let fixture = server.create_session("zz-carry", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let command = "echo zz-carried";
    server.run(&fixture.id, fixture.block_ids()[0], command);
    server.wait_for_capture(&fixture.id, fixture.block_ids()[0], "the echo", |c| {
        c.contains("\nzz-carried")
    });

    // Nothing runs in the restored blocks: the command is only typed.
    let (session, block) = save_kill_restore(&env, &server, &fixture.id, "zz-carry");
    server.wait_for_typed(&session, &block, command);
    let (session, block) = save_kill_restore(&env, &server, &session, "zz-carry");
    server.wait_for_typed(&session, &block, command);

    // Clear the typed command (Ctrl-U) and run a new one: it replaces the old.
    let new_command = "echo zz-new";
    server.write(&session, &block, &format!("\u{15}{new_command}\n"));
    server.wait_for_capture(&session, &block, "the new echo", |c| c.contains("\nzz-new"));
    let (session, block) = save_kill_restore(&env, &server, &session, "zz-carry");
    server.wait_for_typed(&session, &block, new_command);
}
