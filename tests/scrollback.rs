//! Each shell block's scrollback comes back after a restore, printed above
//! the new prompt, with the block's last command still typed (not run) at
//! that prompt.

mod common;

use common::files::{accessible_to_others, disk_usage};
use common::server::wait_for_file;
use common::*;
use serde_json::json;

/// The lines of a capture before its last (the prompt line).
fn above_prompt(capture: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = capture.trim_end().lines().collect();
    lines.pop();
    lines
}

#[test]
fn a_shell_blocks_earlier_output_is_printed_above_the_new_prompt_with_its_last_command_typed() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let a = env.mkdir("a");
    let fixture = server.create_session("zz-scrollback", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];

    for i in 1..=3 {
        server.run(&fixture.id, block, &format!("echo zz-earlier-{i}"));
    }
    // A command with a visible side effect, so the test can tell whether it
    // ran again after the restore.
    let marker = a.join("zz-marker");
    let touch = format!("touch {}", marker.display());
    server.run(&fixture.id, block, &touch);
    wait_for_file(&marker);
    server.wait_for_empty_prompt(&fixture.id, block);

    env.save(&server);
    server.kill_session(&fixture.id);
    std::fs::remove_file(&marker).unwrap();
    env.restore(&server);

    let restored = server.the_session_labelled("zz-scrollback");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_typed(&restored, &block, &touch);
    let capture = server.capture(&restored, &block);
    let above = above_prompt(&capture);
    for i in 1..=3 {
        assert!(
            above.contains(&format!("zz-earlier-{i}").as_str()),
            "earlier output zz-earlier-{i} is above the prompt:\n{capture}"
        );
    }
    // Once where it ran before the restart, once typed at the new prompt.
    // Rex echoes typed input as the block starts; that echo is erased before
    // the scrollback is printed, so it isn't glued to the first old line.
    assert_eq!(
        capture.matches(&touch).count(),
        2,
        "the last command shows where it ran and where it's typed:\n{capture}"
    );
    assert!(!marker.exists(), "the typed command must not have run");

    // The block's shell started the way Rex starts every block's shell: as a
    // login shell under login(1).
    let process = server.block_method(&restored, &block, "process", json!({}));
    assert_eq!(process["foreground"]["argv0"], "-zsh", "{process}");
    assert_eq!(process["child"]["name"], "login", "{process}");
}

#[test]
fn a_block_showing_a_full_screen_program_gets_no_scrollback_replay() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let paged = a.join("paged.txt");
    std::fs::write(&paged, "zz-paged-content\n".repeat(5)).unwrap();
    let fixture = server.create_session(
        "zz-full-screen",
        &[window(hsplit(0.5, shell(&a), shell(&b)))],
    );
    server.wait_for_pwds(&fixture);
    let [pager, switched] = [0, 1].map(|i| fixture.windows[0].block_ids[i].as_str());

    // A full-screen program (on the alternate screen).
    server.run(&fixture.id, pager, "echo zz-before-less");
    let less = format!("less {}", paged.display());
    server.run(&fixture.id, pager, &less);
    server.wait_for_foreground(&fixture.id, pager, "less");
    server.wait_for_capture(&fixture.id, pager, "the paged file", |c| {
        c.contains("zz-paged-content")
    });
    // The shell itself in the foreground, but left on the alternate screen
    // (as by a full-screen program that was killed): still nothing to replay.
    server.run(&fixture.id, switched, "echo zz-before-switch");
    let switch = r"printf '\033[?1049h'";
    server.run(&fixture.id, switched, switch);
    server.wait_for_capture(
        &fixture.id,
        switched,
        "a prompt on the alternate screen",
        |c| !c.contains("zz-before-switch") && c.trim_end().ends_with(" %"),
    );

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-full-screen");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], &less);
    let capture = server.capture(&restored, &blocks[0]);
    for absent in ["zz-paged-content", "zz-before-less"] {
        assert!(
            !capture.contains(absent),
            "no replay of {absent}:\n{capture}"
        );
    }
    server.wait_for_typed(&restored, &blocks[1], switch);
    let capture = server.capture(&restored, &blocks[1]);
    assert!(
        !capture.contains("zz-before-switch"),
        "no replay:\n{capture}"
    );
}

#[test]
fn a_block_restored_with_its_scrollback_saves_and_restores_again_like_any_other() {
    // No recorder. The way the replay starts the shell never becomes a last
    // command (the startup window covers it), but a program the user starts
    // in the block once it has started does, as in any block.
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session("zz-again", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    server.wait_for_pwds(&fixture);
    for block in fixture.block_ids() {
        server.run(&fixture.id, block, "echo zz-first-run");
        server.wait_for_capture(&fixture.id, block, "the echo", |c| {
            c.contains("\nzz-first-run")
        });
    }

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-again");
    let blocks = server.block_ids(&restored);
    for block in &blocks {
        server.wait_for_empty_prompt(&restored, block);
    }
    env.end_startup_windows();
    // One restored block starts a program; the other stays idle.
    let tail = "tail -f /dev/null";
    server.run(&restored, &blocks[1], tail);
    server.wait_for_foreground(&restored, &blocks[1], "tail");
    // Rex reports the program as it does in any block: run by the block's
    // login shell, under login(1).
    let process = server.block_method(&restored, &blocks[1], "process", json!({}));
    let ancestors = process["ancestors"].as_array().expect("ancestors");
    let names: Vec<&str> = ancestors
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert_eq!(names, ["zsh", "login"], "{process}");
    assert_eq!(ancestors[0]["argv0"], "-zsh", "{process}");

    env.save(&server);
    server.kill_session(&restored);
    env.restore(&server);

    let again = server.the_session_labelled("zz-again");
    let blocks = server.block_ids(&again);
    server.wait_for_typed(&again, &blocks[1], tail);
    server.assert_empty_prompt(&again, &blocks[0]);
    let idle = server.capture(&again, &blocks[0]);
    assert!(
        above_prompt(&idle).contains(&"zz-first-run"),
        "its scrollback carries over from before the first restore:\n{idle}"
    );
}

#[test]
fn replayed_output_wraps_at_the_width_of_the_restored_block() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-wrap", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    // Saved from a block 60 columns wide.
    server.resize(&fixture.id, block, 60, 24);
    let long = format!("zz-long-{}-end", "abcdefghij".repeat(15));
    server.run(&fixture.id, block, &format!("echo {long}"));
    server.wait_for_capture(&fixture.id, block, "the long line", |c| {
        c.lines().any(|line| line == long)
    });

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-wrap");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_empty_prompt(&restored, &block);
    let rows_at = |columns: usize| {
        let rows = server.capture_rows(&restored, &block);
        assert!(
            rows.lines().any(|row| row == &long[..columns]),
            "the long line wraps at {columns} columns:\n{rows}"
        );
        rows
    };
    // Restored 80 columns wide: wrapped at 80, not where it wrapped when saved.
    let rows = rows_at(80);
    assert!(
        !rows.lines().any(|row| row == &long[..60]),
        "no break left at the saved width:\n{rows}"
    );
    let capture = server.capture(&restored, &block);
    assert!(
        capture.lines().any(|line| line == long),
        "still one line:\n{capture}"
    );
    // And it reflows when the block is resized later.
    server.resize(&restored, &block, 50, 24);
    rows_at(50);
}

#[test]
fn scrollback_is_private_and_kept_only_as_long_as_its_snapshot() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-kept", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    // Enough scrollback to dwarf everything else rex-resurrect keeps.
    server.run(&fixture.id, block, "seq 1 20000");
    server.wait_for_capture(&fixture.id, block, "the whole sequence", |c| {
        c.contains("\n20000\n")
    });
    let save = || {
        env.resurrect(&server)
            .arg("save")
            .env("REX_RESURRECT_KEEP", "2")
            .run()
            .assert_success();
    };

    save();
    let one_snapshot = disk_usage(&env.state_dir());
    for _ in 0..4 {
        save();
    }
    let kept = disk_usage(&env.state_dir());

    assert!(
        kept < one_snapshot * 5 / 2,
        "two snapshots' worth kept ({kept} bytes), not five ({one_snapshot} bytes each)"
    );
    assert_eq!(accessible_to_others(&env.state_dir()), Vec::<String>::new());
    server.kill_session(&fixture.id);
    env.restore(&server);
    let restored = server.the_session_labelled("zz-kept");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_empty_prompt(&restored, &block);
    let capture = server.capture(&restored, &block);
    assert!(
        capture.contains("\n20000\n"),
        "the latest snapshot's scrollback is still there"
    );
}

#[test]
fn an_untouched_sessions_scrollback_is_not_kept() {
    // No recorder, and rex-resurrect's home is the untouched session's
    // directory: after `seq` it has no last command and is still at home.
    let env = TestEnv::new();
    let server = env.start_server();
    let home = env.mkdir("home");
    let untouched = server.create_session("zz-untouched", &[window(shell(&home))]);
    let touched = server.create_session("zz-touched", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&untouched);
    server.wait_for_pwds(&touched);
    let block = untouched.block_ids()[0];
    server.run(&untouched.id, block, "seq 1 20000");
    server.wait_for_capture(&untouched.id, block, "the whole sequence", |c| {
        c.contains("\n20000\n")
    });

    let out = env
        .resurrect(&server)
        .env("HOME", &home)
        .arg("save")
        .run()
        .assert_success();

    assert!(out.stdout.contains("saved 1 session(s)"), "{out}");
    let kept = disk_usage(&env.state_dir());
    assert!(
        kept < 20_000,
        "the untouched session's 20,000 lines aren't kept ({kept} bytes)"
    );
}

/// Also measures how long the save takes (shown with `--nocapture`), to
/// check that autosaves stay fast with a long scrollback.
#[test]
fn a_block_with_50000_lines_of_scrollback_comes_back_in_full() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let fixture = server.create_session("zz-50k", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    let seq = "seq 1 50000";
    server.run(&fixture.id, block, seq);
    server.wait_for_capture(&fixture.id, block, "the whole sequence", |c| {
        c.contains("\n50000\n")
    });

    let started = std::time::Instant::now();
    env.save(&server);
    eprintln!("save with a 50,000-line block took {:?}", started.elapsed());
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-50k");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_typed(&restored, &block, seq);
    let above = above_prompt(&server.capture(&restored, &block)).join("\n");
    assert!(
        above.contains("\n1\n2\n3\n") && above.contains("\n49999\n50000\n"),
        "the whole sequence is above the prompt"
    );
}

#[test]
fn a_block_running_a_program_on_the_main_screen_keeps_its_scrollback() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let fixture = server.create_session("zz-main-screen", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    server.run(&fixture.id, block, "echo zz-before-tail");
    let tail = "tail -f /dev/null";
    server.run(&fixture.id, block, tail);
    server.wait_for_foreground(&fixture.id, block, "tail");

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-main-screen");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_typed(&restored, &block, tail);
    let capture = server.capture(&restored, &block);
    assert!(
        above_prompt(&capture).contains(&"zz-before-tail"),
        "the output before the program is above the prompt:\n{capture}"
    );
}

/// A directory holding stand-ins for programs, to put first on the server's
/// `PATH`: each a symlink to `sleep` under the program's name. Like Claude
/// Code's own `claude`, a symlink to an executable named for its version:
/// Rex reports the executable's name (`sleep`) and the name it was run by
/// (`argv0`, the stand-in's).
fn stand_ins(env: &TestEnv, names: &[&str]) -> std::path::PathBuf {
    let dir = env.mkdir("stand-ins");
    for name in names {
        std::os::unix::fs::symlink("/bin/sleep", dir.join(name)).unwrap();
    }
    dir
}

/// Prints `printed` in `block`, then starts `command` (a stand-in and a
/// number of seconds) and waits for it to be in the foreground.
fn print_then_run(server: &Server, session: &str, block: &str, printed: &str, command: &str) {
    server.run(session, block, &format!("echo {printed}"));
    server.wait_for_capture(session, block, printed, |c| {
        c.contains(&format!("\n{printed}"))
    });
    server.run(session, block, command);
    server.wait_for_foreground(session, block, "sleep");
}

/// Claude Code draws its interface on the main screen, not the alternate
/// one, so a replay would print a frozen copy of it. A block running it gets
/// no scrollback; its command still comes back typed.
#[test]
fn a_block_running_claude_gets_no_scrollback_replay() {
    let env = TestEnv::with_recorder();
    let server = env
        .server()
        .path_prepend(stand_ins(&env, &["claude"]))
        .start();
    let fixture = server.create_session("zz-claude", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    print_then_run(
        &server,
        &fixture.id,
        block,
        "zz-before-claude",
        "claude 300",
    );

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-claude");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_typed(&restored, &block, "claude 300");
    let capture = server.capture(&restored, &block);
    assert!(
        !capture.contains("zz-before-claude"),
        "no replay:\n{capture}"
    );
}

#[test]
fn the_programs_whose_blocks_get_no_scrollback_replay_can_be_set() {
    let env = TestEnv::with_recorder();
    let server = env
        .server()
        .path_prepend(stand_ins(&env, &["claude", "dev-server"]))
        .start();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session("zz-set", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    server.wait_for_pwds(&fixture);
    let [listed, unlisted] = [0, 1].map(|i| fixture.block_ids()[i]);
    print_then_run(&server, &fixture.id, listed, "zz-listed", "dev-server 300");
    print_then_run(&server, &fixture.id, unlisted, "zz-unlisted", "claude 300");

    // The list replaces the default (`claude`).
    env.resurrect(&server)
        .env("REX_RESURRECT_NO_SCROLLBACK", "vim dev-server")
        .arg("save")
        .run()
        .assert_success();
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-set");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], "dev-server 300");
    server.wait_for_typed(&restored, &blocks[1], "claude 300");
    let [listed, unlisted] = [0, 1].map(|i| server.capture(&restored, &blocks[i]));
    assert!(!listed.contains("zz-listed"), "no replay:\n{listed}");
    assert!(
        above_prompt(&unlisted).contains(&"zz-unlisted"),
        "replayed:\n{unlisted}"
    );
}
