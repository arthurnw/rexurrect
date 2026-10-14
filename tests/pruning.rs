//! After a save writes a snapshot, recorder files for blocks that aren't live
//! any more, and lineage entries for sessions that aren't, are removed, so
//! the state directory doesn't grow forever.
//!
//! Block and session IDs are never reused, so a removed file or entry can't
//! change what a later restore does: removal is observed in the state
//! directory, through the recorder-file contract (`recorder/<block ID>`).
//! What must survive is observed through Rex.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::stub::sh_quote;
use common::*;

fn recorder_file(env: &TestEnv, name: &str) -> PathBuf {
    env.state_dir().join("recorder").join(name)
}

/// The first block of a new single-shell session, once its shell is up.
fn session_with_one_block(env: &TestEnv, server: &Server, label: &str) -> (String, String) {
    let fixture = server.create_session(label, &[window(shell(env.mkdir(label)))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0].to_string();
    (fixture.id, block)
}

#[test]
fn a_save_removes_recorder_files_for_blocks_that_are_not_live_and_keeps_the_others() {
    let env = TestEnv::new();
    let server = env.start_server();
    let (live, live_block) = session_with_one_block(&env, &server, "zz-live");
    let (gone, gone_block) = session_with_one_block(&env, &server, "zz-gone");
    env.write_recorder_file(&live_block, "echo zz-live");
    env.write_recorder_file(&gone_block, "echo zz-gone");
    // A recorder's temporary file, renamed over the record a moment later.
    let in_flight = recorder_file(&env, &format!(".{gone_block}.4242"));
    std::fs::write(&in_flight, "echo zz-in-flight\n").unwrap();
    server.kill_session(&gone);

    env.save(&server);
    assert!(
        !recorder_file(&env, &gone_block).exists(),
        "the recorder file of a block that isn't live is removed"
    );
    assert!(in_flight.exists(), "temporary files are left alone");

    // The live block's file is still there for the next save to read.
    env.save(&server);
    server.kill_session(&live);
    env.restore(&server);
    let restored = server.the_session_labelled("zz-live");
    server.wait_for_typed(&restored, &server.block_ids(&restored)[0], "echo zz-live");
}

#[test]
fn a_save_that_fails_to_write_its_snapshot_removes_no_recorder_files() {
    let env = TestEnv::new();
    let server = env.start_server();
    let (_live, live_block) = session_with_one_block(&env, &server, "zz-live");
    let (gone, gone_block) = session_with_one_block(&env, &server, "zz-gone");
    env.write_recorder_file(&live_block, "echo zz-live");
    env.write_recorder_file(&gone_block, "echo zz-gone");
    server.kill_session(&gone);
    // A file where the snapshots directory goes: writing the snapshot fails.
    std::fs::write(env.state_dir().join("snapshots"), "").unwrap();

    env.rexurrect(&server).arg("save").run().assert_failure();
    assert!(recorder_file(&env, &gone_block).exists());
}

/// An empty server writes no snapshot, so it removes nothing either: a save
/// that finds no sessions can't tell a server that lost them from one that
/// never had them.
#[test]
fn a_save_from_an_empty_server_removes_nothing() {
    let env = TestEnv::new();
    let server = env.start_server();
    let (original, original_block) = session_with_one_block(&env, &server, "zz-proj");
    env.write_recorder_file(&original_block, "echo zz-proj");
    env.save(&server);
    server.kill_session(&original);
    env.restore(&server);
    let restored = server.the_session_labelled("zz-proj");
    let restored_block = server.block_ids(&restored).remove(0);
    env.write_recorder_file(&restored_block, "echo zz-restored");
    server.kill_session(&restored);

    env.save(&server);
    assert!(lineage_map_mentions(&env, &restored));
    assert!(recorder_file(&env, &restored_block).exists());
}

/// A stand-in `rex` that passes every call to the real one and, each time
/// `session.list` has answered, writes `recorder` as a recorder would for a
/// block that appeared just then. Put it first on rexurrect's `PATH`.
fn stub_rex_recording_after_session_list(env: &TestEnv, recorder: &Path) -> RexStub {
    RexStub::new(env).after(
        "session.list",
        &format!("echo 'echo zz-new' > {}", sh_quote(recorder)),
    )
}

#[test]
fn a_recorder_file_written_while_a_save_runs_is_kept() {
    let env = TestEnv::new();
    let server = env.start_server();
    session_with_one_block(&env, &server, "zz-live");
    // Restores into this server instance, so the next save doesn't.
    env.save(&server);
    // A block that appeared after the save read the server, and recorded a
    // command before the save finished.
    let new_block = recorder_file(&env, "block:zz-new");
    std::fs::create_dir_all(new_block.parent().unwrap()).unwrap();
    let stub = stub_rex_recording_after_session_list(&env, &new_block);

    let out = env
        .rexurrect(&server)
        .path_prepend(stub.dir())
        .arg("save")
        .run()
        .assert_success();
    assert!(
        new_block.exists(),
        "the new block's record was removed: {out}"
    );
}

/// Whether rexurrect's lineage map mentions `session_id` at all.
fn lineage_map_mentions(env: &TestEnv, session_id: &str) -> bool {
    std::fs::read_to_string(env.state_dir().join("lineage.json"))
        .unwrap_or_default()
        .contains(session_id)
}

#[test]
fn a_save_removes_lineage_entries_for_sessions_that_are_not_live_and_keeps_the_others() {
    let env = TestEnv::new();
    let server = env.start_server();
    let (kept, _) = session_with_one_block(&env, &server, "zz-kept");
    let (dropped, _) = session_with_one_block(&env, &server, "zz-dropped");
    env.save(&server);
    server.kill_session(&kept);
    server.kill_session(&dropped);
    env.restore(&server);
    let restored_dropped = server.the_session_labelled("zz-dropped");
    assert!(lineage_map_mentions(&env, &restored_dropped));
    server.kill_session(&restored_dropped);

    env.save(&server);
    assert!(
        !lineage_map_mentions(&env, &restored_dropped),
        "the lineage entry of a session that isn't live is removed"
    );
    // The live restored session is still recognised as its saved session.
    let again = env.restore(&server);
    assert!(again.lines_starting("created").is_empty(), "{again}");
    assert_eq!(again.lines_starting("skipped").len(), 1, "{again}");
}

/// A write that crashed between writing its temporary file and renaming it
/// over the real one leaves the temporary file behind. A save removes those
/// once they're clearly stale (over an hour old); a fresh one may belong to
/// a write still going on.
#[test]
fn a_save_removes_temporary_files_left_by_crashed_writes_once_they_are_stale() {
    let env = TestEnv::new();
    let server = env.start_server();
    session_with_one_block(&env, &server, "zz-live");
    env.save(&server);
    // A recorder's temporary files (named `.…` by the recorder-file
    // contract; the zsh recorder's are `.<block ID>.<pid>`), and the ones
    // rexurrect writes its own files through (`.<name>.<pid>.tmp`, next to
    // the file).
    let temporary = |name: &str| {
        [
            recorder_file(&env, &format!(".block:zz-{name}.4242")),
            env.state_dir()
                .join(format!("snapshots/.zz-{name}.json.4242.tmp")),
            env.state_dir().join(format!(".zz-{name}.json.4242.tmp")),
        ]
    };
    let (stale, fresh) = (temporary("stale"), temporary("fresh"));
    for path in stale.iter().chain(&fresh) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "half written").unwrap();
    }
    let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    for path in &stale {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(two_hours_ago).unwrap();
    }

    env.save(&server);

    for path in &stale {
        assert!(!path.exists(), "{} is left behind", path.display());
    }
    for path in &fresh {
        assert!(path.exists(), "{} was removed", path.display());
    }
}

#[test]
fn a_save_keeps_the_recorder_file_of_a_floating_block() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session("zz-float", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    server.wait_for_pwds(&fixture);
    let floating = fixture.block_ids()[1].to_string();
    env.write_recorder_file(&floating, "echo zz-floating");
    // Floating blocks aren't saved, but they're live.
    server.call(
        "session.float_block",
        &serde_json::json!({
            "session_id": fixture.id,
            "block_id": floating,
            "bounds": { "x": 0.1, "y": 0.1, "w": 0.5, "h": 0.5 },
        }),
    );

    env.save(&server);
    assert!(recorder_file(&env, &floating).exists());
}
