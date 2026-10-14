//! A save on an unrestored server instance restores first, so a freshly
//! started server never replaces the latest snapshot with a near-empty one.
//! Restores take turns, and a save waits for a running restore.
//!
//! Each throwaway server is its own server instance, and servers started from
//! one `TestEnv` share its state directory, so stopping one server and
//! starting another is a restart.

mod common;

use common::stub::slow_rex;
use common::*;

#[test]
fn save_on_a_fresh_server_instance_restores_the_latest_snapshot_first() {
    let env = TestEnv::new();
    let [proj, other] = ["proj", "other"].map(|name| env.mkdir(name));
    let layout = || hsplit(0.7, shell(&proj), shell(&other));
    {
        let before = env.start_server();
        let fixture = before.create_session("zz-proj", &[window(layout())]);
        before.wait_for_pwds(&fixture);
        env.save(&before);
    } // The server stops, as in a restart.

    // A session someone created on the new server before anything restored.
    let after = env.start_server();
    let early = after.create_session("zz-early", &[window(shell(&other))]);
    after.wait_for_pwds(&early);
    env.save(&after);

    assert_eq!(after.sorted_labels(), ["zz-early", "zz-proj"]);
    let restored = after.session_ids_labelled("zz-proj");
    assert_eq!(
        after.window_shapes(&restored[0]),
        [layout().expected_shape()]
    );

    // The snapshot that save wrote holds the restored session as well as
    // the one it found.
    drop(after);
    let third = env.start_server();
    env.restore(&third);
    assert_eq!(third.sorted_labels(), ["zz-early", "zz-proj"]);
}

/// A save that restores first reads the server straight after creating the
/// restored blocks, while their shells are still starting (a slow zsh
/// config, here a `sleep` in `.zshrc`): no working directory reported yet,
/// and whatever the startup runs in the foreground. Nothing can have run in
/// them yet, so it saves them as restore created them.
#[test]
fn a_save_that_restores_first_saves_the_blocks_it_restored_as_restore_created_them() {
    let env = TestEnv::builder().zshrc_line("sleep 3").build();
    let [proj, other] = ["proj", "other"].map(|name| env.mkdir(name));
    let layout = || hsplit(0.7, shell(&proj), shell(&other));
    {
        let before = env.start_server();
        let fixture = before.create_session("zz-proj", &[window(layout())]);
        before.wait_for_pwds(&fixture);
        let [typed, idle] = [0, 1].map(|i| fixture.block_ids()[i]);
        before.run(&fixture.id, idle, "echo zz-earlier");
        before.wait_for_capture(&fixture.id, idle, "the echo", |c| {
            c.contains("\nzz-earlier")
        });
        env.write_recorder_file(typed, "make test");
        env.save(&before);
    } // The server stops, as in a restart.
    {
        let after = env.start_server();
        env.save(&after);
    }

    let third = env.start_server();
    env.restore(&third);
    let restored = third.the_session_labelled("zz-proj");
    let blocks = third.block_ids(&restored);
    third.wait_for_typed(&restored, &blocks[0], "make test");
    third.assert_empty_prompt(&restored, &blocks[1]);
    let capture = third.capture(&restored, &blocks[1]);
    assert!(
        capture.contains("\nzz-earlier"),
        "the scrollback restore printed is saved again:\n{capture}"
    );
    assert_eq!(third.window_shapes(&restored), [layout().expected_shape()]);
}

/// Restoring first and saving read the server instance once, so they can't
/// disagree on which instance they restored into and saved.
#[test]
fn a_save_that_restores_first_reads_the_server_status_once() {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = env.start_server();
    let calls = env.root().join("server-status-calls");
    let stub = RexStub::new(&env).after(
        "server.status",
        &format!("echo >> {}", common::stub::sh_quote(&calls)),
    );

    let out = env
        .rexurrect(&server)
        .arg("save")
        .path_prepend(stub.dir())
        .run()
        .assert_success();
    assert!(out.stdout.contains("restored first"), "{out}");
    let count = std::fs::read_to_string(&calls).unwrap().lines().count();
    assert_eq!(count, 1, "{out}");
}

#[test]
fn with_no_snapshot_yet_save_saves_and_counts_the_instance_as_restored() {
    let env = TestEnv::new();
    let server = env.start_server();
    let first = server.create_session("zz-first", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&first);
    // First use: nothing to restore. (The round-trip test above shows that
    // this save writes a snapshot holding zz-first.)
    env.save(&server);

    server.kill_session(&first.id);
    env.save(&server);
    assert!(
        server.sessions().is_empty(),
        "the second save restored zz-first: {:?}",
        server.sessions()
    );
}

/// Another server can restore too: one started by hand without its own
/// `--config` runs the user's init.lua, auto-restore included. That mustn't
/// make a server already restored into look unrestored, or its next save
/// would bring back sessions killed since.
#[test]
fn a_restore_into_another_server_leaves_this_one_counted_as_restored() {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let server = env.start_server();
    env.restore(&server);
    server.kill_session(&server.the_session_labelled("zz-proj"));
    let other = env.start_server();
    env.restore(&other);

    env.save(&server);
    assert_eq!(server.sessions(), [], "zz-proj came back");
}

/// Restores the latest snapshot into a fresh server instance with `command`
/// (`restore`, or a `save` that restores first), then kills every session
/// restored and saves again.
fn kill_restored_sessions_and_save_again(command: &str) -> Server {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-proj"]);
    let after = env.start_server();
    env.rexurrect(&after).arg(command).run().assert_success();
    let restored = after.session_ids_labelled("zz-proj");
    assert_eq!(restored.len(), 1, "`{command}` restored zz-proj");

    after.kill_session(&restored[0]);
    env.save(&after);
    after
}

#[test]
fn after_a_restore_save_does_not_restore_again_even_if_the_restored_sessions_were_killed() {
    let server = kill_restored_sessions_and_save_again("restore");
    assert!(server.sessions().is_empty(), "{:?}", server.sessions());
}

#[test]
fn after_a_save_that_restored_first_save_does_not_restore_again() {
    let server = kill_restored_sessions_and_save_again("save");
    assert!(server.sessions().is_empty(), "{:?}", server.sessions());
}

/// Saves zz-one and zz-two on an earlier server, then on a fresh one runs
/// `restore <args>` followed by `save`.
fn restore_with_then_save(args: &[&str]) -> Server {
    let env = TestEnv::new();
    env.saved_on_an_earlier_server(&["zz-one", "zz-two"]);
    let server = env.start_server();
    env.rexurrect(&server)
        .arg("restore")
        .args(args)
        .run()
        .assert_success();
    env.save(&server);
    server
}

#[test]
fn a_dry_run_restore_does_not_count_as_restoring_the_instance() {
    let server = restore_with_then_save(&["--dry-run"]);
    assert_eq!(server.sorted_labels(), ["zz-one", "zz-two"]);
}

#[test]
fn a_single_session_restore_does_not_count_as_restoring_the_instance() {
    let server = restore_with_then_save(&["--session", "zz-one"]);
    assert_eq!(server.sorted_labels(), ["zz-one", "zz-two"]);
}

#[test]
fn restoring_a_whole_older_snapshot_counts_as_restoring_the_instance() {
    let env = TestEnv::new();
    {
        let before = env.start_server();
        for label in ["zz-old", "zz-new"] {
            let fixture = before.create_session(label, &[window(shell(env.mkdir(label)))]);
            before.wait_for_pwds(&fixture);
            env.save(&before);
        }
    }
    let server = env.start_server();
    let listed = env.rexurrect(&server).arg("list").run().assert_success();
    // Newest first: the last line is the snapshot holding only zz-old.
    let oldest = listed.stdout.lines().last().unwrap();
    let id = oldest.split("  ").next().unwrap();

    env.rexurrect(&server)
        .args(["restore", "--snapshot", id])
        .run()
        .assert_success();
    env.save(&server);
    assert_eq!(server.sorted_labels(), ["zz-old"]);
}

/// Saves zz-wide, a session with two windows, on a server that then stops,
/// and starts one that refuses a second window in any session: no restore
/// can recreate zz-wide there. A session of its own, zz-early, gives a save
/// there something to save.
fn a_server_that_cant_restore_anything(env: &TestEnv) -> Server {
    let a = env.mkdir("a");
    {
        let before = env.start_server();
        let wide = before.create_session("zz-wide", &[window(shell(&a)), window(shell(&a))]);
        before.wait_for_pwds(&wide);
        env.save(&before);
    }
    let server = env.server().arg("--max-windows").arg("1").start();
    let early = server.create_session("zz-early", &[window(shell(&a))]);
    server.wait_for_pwds(&early);
    server
}

/// A save whose restore first fails for every session restored nothing, so
/// it saves nothing either and exits non-zero, naming the snapshot that
/// failed and how to retry. It counts the server instance as restored all
/// the same: otherwise every later save would restore first, fail again and
/// never save anything on this server. The next save saves what's live.
#[test]
fn a_save_whose_restore_first_fails_for_every_session_saves_nothing_and_the_next_save_saves_whats_live()
 {
    let env = TestEnv::new();
    let server = a_server_that_cant_restore_anything(&env);
    let snapshots = env.listed(&server);
    let id = snapshots.split("  ").next().unwrap().to_string();
    let retry = format!("rexurrect restore --snapshot {id}");

    let out = env.rexurrect(&server).arg("save").run().assert_failure();
    assert!(out.stdout.contains("restored first"), "{out}");
    assert!(out.stderr.contains(&retry), "{out}");
    assert_eq!(env.listed(&server), snapshots, "no new snapshot: {out}");
    let entries = env.log_entries();
    assert!(
        entries
            .iter()
            .any(|e| e.contains("save failed") && e.contains(&retry)),
        "{entries:#?}"
    );

    let next = env.save(&server);
    assert!(!next.stdout.contains("restored first"), "{next}");
    assert_eq!(server.sorted_labels(), ["zz-early"]);

    drop(server);
    let third = env.start_server();
    env.restore(&third);
    assert_eq!(third.sorted_labels(), ["zz-early"]);
    // The snapshot that failed to restore is still there to retry.
    env.rexurrect(&third)
        .args(["restore", "--snapshot", &id])
        .run()
        .assert_success();
    assert_eq!(third.sorted_labels(), ["zz-early", "zz-wide"]);
}

#[test]
fn a_restore_in_which_every_session_failed_does_not_count_as_restoring_the_instance() {
    let env = TestEnv::new();
    let server = a_server_that_cant_restore_anything(&env);
    let snapshots = env.listed(&server);
    env.rexurrect(&server).arg("restore").run().assert_failure();

    let out = env.rexurrect(&server).arg("save").run().assert_failure();
    assert!(out.stdout.contains("restored first"), "{out}");
    assert_eq!(env.listed(&server), snapshots, "no new snapshot: {out}");
}

#[test]
fn two_restores_started_at_once_create_no_duplicates() {
    let env = TestEnv::new();
    let labels = ["zz-one", "zz-three", "zz-two"];
    env.saved_on_an_earlier_server(&labels);
    let server = env.start_server();
    // Every session.create waits a moment, so both restores have listed the
    // live sessions long before either creates one.
    let slow = slow_rex(&env, "session.create", 0.3);
    let restore = || {
        env.rexurrect(&server)
            .arg("restore")
            .path_prepend(&slow)
            .spawn()
    };
    let (first, second) = (restore(), restore());
    let outs = [first.wait(), second.wait()].map(RunOutput::assert_success);

    assert_eq!(server.sorted_labels(), labels);
    // One restore created every session; the other waited for it, then
    // found every saved session's lineage live.
    let counts = |out: &RunOutput| {
        (
            out.lines_starting("created").len(),
            out.lines_starting("skipped").len(),
        )
    };
    let mut counts: Vec<(usize, usize)> = outs.iter().map(counts).collect();
    counts.sort();
    assert_eq!(counts, [(0, 3), (3, 0)], "{}\n{}", outs[0], outs[1]);
}

#[test]
fn a_save_that_starts_during_a_restore_waits_for_it_instead_of_saving_a_partial_server() {
    let env = TestEnv::new();
    let labels = ["zz-one", "zz-three", "zz-two"];
    env.saved_on_an_earlier_server(&labels);
    let server = env.start_server();
    // An instance already restored into, so the save has no reason to
    // restore anything itself.
    env.restore(&server);
    for session in server.sessions() {
        server.kill_session(&session.id);
    }

    let slow = slow_rex(&env, "session.create", 0.5);
    let restore = env
        .rexurrect(&server)
        .arg("restore")
        .path_prepend(&slow)
        .spawn();
    common::server::wait_for("the restore's first session", || {
        (!server.sessions().is_empty()).then_some(())
    });
    env.save(&server);

    assert_eq!(
        server.sorted_labels(),
        labels,
        "save returned before the restore finished"
    );
    let restored = restore.wait().assert_success();
    assert_eq!(restored.lines_starting("created").len(), 3, "{restored}");

    // The snapshot that save wrote holds every session the restore created.
    drop(server);
    let third = env.start_server();
    env.restore(&third);
    assert_eq!(third.sorted_labels(), labels);
}
