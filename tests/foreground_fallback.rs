//! A block with no recorder file (no recorder installed, or its shell
//! started before it was) falls back to the program in its foreground for
//! its last command, and keeps the last program a save saw there once that
//! program exits. A block restore created is no different: what restore
//! typed at its prompt is only its last command until a save sees a program
//! in it. None of these blocks load the zsh recorder.

mod common;

use std::path::Path;

use common::server::{wait_for, wait_for_file};
use common::*;
use serde_json::json;

/// A shell block Rex starts without `login`: its shell is the block's first
/// process rather than `login`'s child.
fn non_login_shell(dir: &Path) -> Layout {
    block(json!({ "cwd": dir.to_str().unwrap(), "login": false }))
}

/// Saves, kills the session `session_id`, restores it, and waits until its
/// one block shows `typed` at the prompt (nothing, for `""`). Then ends the
/// startup window, so what happens in the block from now on is the user's
/// doing. Returns the restored session and its block.
fn restored_and_started(
    env: &TestEnv,
    server: &Server,
    session_id: &str,
    label: &str,
    typed: &str,
) -> (String, String) {
    env.save(server);
    server.kill_session(session_id);
    env.restore(server);
    let restored = server.the_session_labelled(label);
    let block = server.block_ids(&restored).remove(0);
    if typed.is_empty() {
        server.wait_for_empty_prompt(&restored, &block);
    } else {
        server.wait_for_typed(&restored, &block, typed);
    }
    env.end_startup_windows();
    (restored, block)
}

/// Saves the server, restores the latest snapshot into a new server, and
/// waits until the one block of the session labelled `label` shows `typed`
/// at its prompt.
fn assert_restored_elsewhere_with_typed(env: &TestEnv, server: &Server, label: &str, typed: &str) {
    env.save(server);
    let elsewhere = env.start_server();
    env.restore(&elsewhere);
    let restored = elsewhere.the_session_labelled(label);
    elsewhere.wait_for_typed(&restored, &elsewhere.block_ids(&restored)[0], typed);
}

/// #6 had a restored block never fall back: it always had a record of its
/// last command. Without a recorder that kept the program a user starts in
/// it from ever counting. Now what restore typed is only a default.
#[test]
fn a_program_started_in_a_restored_block_comes_back_typed_and_carries_forward_after_it_exits() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-seen", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let (restored, block) = restored_and_started(&env, &server, &fixture.id, "zz-seen", "");

    let tail = "tail -f /dev/null";
    server.run(&restored, &block, tail);
    server.wait_for_foreground(&restored, &block, "tail");
    assert_restored_elsewhere_with_typed(&env, &server, "zz-seen", tail);

    // Ctrl-C: the program exits, and the block sits at its prompt.
    server.write(&restored, &block, "\u{3}");
    server.wait_for_foreground(&restored, &block, "zsh");
    server.wait_for_empty_prompt(&restored, &block);
    assert_restored_elsewhere_with_typed(&env, &server, "zz-seen", tail);
}

#[test]
fn a_command_restore_typed_carries_forward_while_the_block_sits_at_its_prompt() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-typed", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    // A recorder in the block's first shell, played by hand.
    env.write_recorder_file(fixture.block_ids()[0], "make test");
    restored_and_started(&env, &server, &fixture.id, "zz-typed", "make test");

    assert_restored_elsewhere_with_typed(&env, &server, "zz-typed", "make test");
}

/// The last program seen carries forward in a block restore never touched,
/// too: rexurrect treats every block alike, whether or not it has been
/// through a restart.
#[test]
fn a_program_seen_in_a_block_without_a_recorder_file_carries_forward_after_it_exits() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-seen", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    let tail = "tail -f /dev/null";
    server.run(&fixture.id, block, tail);
    server.wait_for_foreground(&fixture.id, block, "tail");
    env.save(&server);

    server.write(&fixture.id, block, "\u{3}");
    server.wait_for_foreground(&fixture.id, block, "zsh");
    server.wait_for_empty_prompt(&fixture.id, block);
    assert_restored_elsewhere_with_typed(&env, &server, "zz-seen", tail);
}

#[test]
fn a_block_without_a_recorder_file_running_a_program_restores_with_its_command_line_typed_not_run()
{
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session(
        "zz-fallback",
        &[window(hsplit(0.5, shell(&a), non_login_shell(&b)))],
    );
    server.wait_for_pwds(&fixture);
    let [login, non_login] = [0, 1].map(|i| fixture.block_ids()[i]);

    // A long-lived program with a visible side effect as it starts: `tee`
    // creates its file, then waits for input.
    let marker = a.join("zz-marker");
    let tee = format!("tee {}", marker.display());
    server.run(&fixture.id, login, &tee);
    wait_for_file(&marker);
    server.wait_for_foreground(&fixture.id, login, "tee");
    let tail = "tail -n 0 -f /dev/null";
    server.run(&fixture.id, non_login, tail);
    server.wait_for_foreground(&fixture.id, non_login, "tail");

    env.save(&server);
    server.kill_session(&fixture.id);
    std::fs::remove_file(&marker).unwrap();
    env.restore(&server);

    let restored = server.the_session_labelled("zz-fallback");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], &tee);
    server.wait_for_typed(&restored, &blocks[1], tail);
    for block in &blocks {
        assert_eq!(
            server.foreground(&restored, block).as_deref(),
            Some("zsh"),
            "the restored shell is idle"
        );
    }
    assert!(!marker.exists(), "the typed command must not have run");
}

/// #42: a command block has no shell running, so its command is its first
/// process. It isn't a shell, so it's the block's foreground program all
/// the same.
#[test]
fn a_command_block_restores_with_its_command_line_typed_not_run() {
    let env = TestEnv::new();
    let server = env.start_server();
    let tail = ["tail", "-n", "0", "-f", "/dev/null"];
    let fixture = server.create_session(
        "zz-command",
        &[window(block(json!({
            "cwd": env.mkdir("a").to_str().unwrap(),
            "command": tail,
        })))],
    );
    server.wait_for_foreground(&fixture.id, fixture.block_ids()[0], "tail");

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-command");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_typed(&restored, &block, &tail.join(" "));
    assert_eq!(
        server.foreground(&restored, &block).as_deref(),
        Some("zsh"),
        "the restored shell is idle"
    );
}

/// #42 as reported: Claude Code is installed as a `claude` symlink to an
/// executable named for its version, so Rex names the block's first process
/// `2.1.309`. Only the name it was run by says what it is.
#[test]
fn a_command_block_started_through_a_symlink_named_for_its_version_restores_with_its_command_line_typed()
 {
    let env = TestEnv::new();
    let server = env.start_server();
    let version = env.mkdir("versions").join("2.1.309");
    build_sleeper(&env, &version);
    let claude = env.mkdir("bin").join("claude");
    std::os::unix::fs::symlink(&version, &claude).unwrap();
    let command = [claude.to_str().unwrap(), "600"];
    let fixture = server.create_session(
        "zz-claude",
        &[window(block(json!({
            "cwd": env.mkdir("a").to_str().unwrap(),
            "command": command,
        })))],
    );
    server.wait_for_foreground(&fixture.id, fixture.block_ids()[0], "2.1.309");

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-claude");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_typed(&restored, &block, &command.join(" "));
}

#[test]
fn a_shell_nested_in_a_block_without_a_recorder_file_counts_as_its_foreground_program() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-nested", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];

    // A nested shell sitting at its own prompt, with a program run inside it
    // before: what's in the foreground at save time is the nested shell.
    server.run(&fixture.id, block, "zsh");
    server.wait_for_capture(&fixture.id, block, "the nested shell's prompt", |c| {
        c.contains("% zsh\n") && c.trim_end().ends_with(" %")
    });
    server.run(&fixture.id, block, "echo zz-inside");
    server.wait_for_capture(&fixture.id, block, "the echo", |c| {
        c.contains("\nzz-inside")
    });

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-nested");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], "zsh");
}

#[test]
fn a_block_with_a_recorder_file_uses_the_recorded_command_whatever_is_in_its_foreground() {
    // No zsh recorder: the recorder file is written by hand, playing one.
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture =
        server.create_session("zz-recorded", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    server.wait_for_pwds(&fixture);
    let [running, idle] = [0, 1].map(|i| fixture.block_ids()[i]);
    let tail = "tail -n 0 -f /dev/null";
    server.run(&fixture.id, running, tail);
    server.wait_for_foreground(&fixture.id, running, "tail");
    let recorded = ["echo zz-recorded-running", "echo zz-recorded-idle"];
    env.write_recorder_file(running, recorded[0]);
    env.write_recorder_file(idle, recorded[1]);

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-recorded");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], recorded[0]);
    server.wait_for_typed(&restored, &blocks[1], recorded[1]);
}

/// A save run from a block is that block's foreground program while it
/// saves. That's never what the block was doing.
#[test]
fn a_save_run_from_a_block_without_a_recorder_file_is_not_its_last_command() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-self", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    let save = format!("{} save", common::rexurrect::bin());
    server.run(&fixture.id, block, &save);
    server.wait_for_capture(&fixture.id, block, "the save's report", |c| {
        c.contains("\nsaved 1 session(s)")
    });

    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-self");
    server.assert_empty_prompt(&restored, &server.block_ids(&restored)[0]);
}

#[test]
fn a_block_without_a_recorder_file_sitting_at_a_prompt_restores_with_an_empty_prompt() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session(
        "zz-idle",
        &[window(hsplit(0.5, shell(&a), non_login_shell(&b)))],
    );
    server.wait_for_pwds(&fixture);

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-idle");
    for block in server.block_ids(&restored) {
        server.assert_empty_prompt(&restored, &block);
    }
}

/// `block_id`'s default last command in `default-commands.json`, if any.
fn default_command(env: &TestEnv, block_id: &str) -> Option<String> {
    let text = std::fs::read_to_string(env.state_dir().join("default-commands.json")).ok()?;
    let defaults: serde_json::Value = serde_json::from_str(&text).unwrap();
    defaults[block_id].as_str().map(str::to_string)
}

/// Every last command in every snapshot.
fn snapshot_commands(env: &TestEnv) -> Vec<String> {
    fn collect(value: &serde_json::Value, found: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                found.extend(
                    map.get("last_command")
                        .and_then(|c| c.as_str())
                        .map(str::to_string),
                );
                map.values().for_each(|v| collect(v, found));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| collect(v, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(env.state_dir().join("snapshots")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let text = std::fs::read_to_string(&path).unwrap();
            collect(&serde_json::from_str(&text).unwrap(), &mut found);
        }
    }
    found
}

/// #41: `ps` printed a line break in an argument as `\012`, and restore
/// typed that as one line. A program whose arguments can't be typed is
/// refused, so the block keeps its earlier default: here, none.
#[test]
fn a_program_whose_arguments_hold_a_line_break_is_not_its_blocks_last_command() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-multiline", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    server.run(&fixture.id, block, "sh -c $'tail -n 0 -f /dev/null\\ntrue'");
    // `sh` is bash on macOS, dash elsewhere: whichever it is, it's running.
    wait_for("sh in the foreground", || {
        server
            .foreground(&fixture.id, block)
            .filter(|name| name != "zsh")
    });

    env.save(&server);
    assert_eq!(default_command(&env, block), None);
    assert_eq!(snapshot_commands(&env), Vec::<String>::new());
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-multiline");
    server.assert_empty_prompt(&restored, &server.block_ids(&restored)[0]);
}

/// A refused program leaves the block's default last command as it was: the
/// last program a save saw there.
#[test]
fn a_program_whose_arguments_hold_a_line_break_leaves_the_blocks_default_alone() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-kept", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    let tail = "tail -n 0 -f /dev/null";
    server.run(&fixture.id, block, tail);
    server.wait_for_foreground(&fixture.id, block, "tail");
    env.save(&server);

    server.write(&fixture.id, block, "\u{3}");
    server.wait_for_foreground(&fixture.id, block, "zsh");
    server.wait_for_empty_prompt(&fixture.id, block);
    server.run(&fixture.id, block, "sh -c $'tail -n 0 -f /dev/null\\ntrue'");
    wait_for("sh in the foreground", || {
        server
            .foreground(&fixture.id, block)
            .filter(|name| name != "zsh")
    });
    assert_restored_elsewhere_with_typed(&env, &server, "zz-kept", tail);
    assert_eq!(default_command(&env, block).as_deref(), Some(tail));
}

/// Earlier versions stored `ps`'s escaped text as a block's default. It's
/// dropped rather than typed, and doesn't carry into new snapshots.
#[test]
fn a_default_last_command_escaped_by_ps_is_neither_typed_nor_carried_forward() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-escaped", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    std::fs::write(
        env.state_dir().join("default-commands.json"),
        json!({ block: "sh -c tail -f /dev/null\\012true" }).to_string(),
    )
    .unwrap();

    env.save(&server);
    assert_eq!(snapshot_commands(&env), Vec::<String>::new());
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-escaped");
    server.assert_empty_prompt(&restored, &server.block_ids(&restored)[0]);
}

/// Arguments are quoted where zsh needs it, so the typed command runs the
/// same program with the same arguments: saved again while it runs, it
/// comes back the same.
#[test]
fn a_program_with_an_argument_holding_a_space_comes_back_quoted() {
    let env = TestEnv::new();
    let server = env.start_server();
    let dir = env.mkdir("a");
    let file = dir.join("a b");
    std::fs::write(&file, "").unwrap();
    let fixture = server.create_session("zz-quoted", &[window(shell(&dir))]);
    server.wait_for_pwds(&fixture);
    let tail = format!("tail -n 0 -f '{}'", file.display());
    server.run(&fixture.id, fixture.block_ids()[0], &tail);
    server.wait_for_foreground(&fixture.id, fixture.block_ids()[0], "tail");

    let (restored, block) = restored_and_started(&env, &server, &fixture.id, "zz-quoted", &tail);
    server.write(&restored, &block, "\n");
    server.wait_for_foreground(&restored, &block, "tail");
    assert_restored_elsewhere_with_typed(&env, &server, "zz-quoted", &tail);
}
