//! A block whose scrollback is too large for Rex to hand over is saved
//! without it, like a block on the alternate screen, and the save carries
//! on. Rex refuses any control response over its limit with `too_large`. A
//! throwaway server's blocks would need a megabyte of output to get there,
//! so a stub `rex` fails a block's terminal `format` calls the way Rex does.
//! Any other failure to read scrollback still fails the save.

mod common;

use common::server::above_prompt;
use common::*;

/// What `rex` prints when Rex refuses a response over its limit (exit 1).
const TOO_LARGE: &str = r#"Error: stream response "control-2" failed: control response exceeds 1048576 bytes (status=413 code=too_large)"#;

/// What a terminal `format` call's JSON arguments contain: the method's name.
const FORMAT: &str = r#""method":"format""#;

/// Prints `printed` in `block`, then runs `last`, its last command, to the
/// end.
fn print_then_finish(server: &Server, session: &str, block: &str, printed: &str, last: &str) {
    server.run(session, block, &format!("echo {printed}"));
    server.wait_for_capture(session, block, printed, |c| {
        c.contains(&format!("\n{printed}"))
    });
    server.run(session, block, last);
    server.wait_for_empty_prompt(session, block);
}

#[test]
fn a_block_whose_scrollback_is_too_large_for_rex_is_saved_without_it() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let [a, b, c] = ["a", "b", "c"].map(|name| env.mkdir(name));
    let big = server.create_session("zz-big", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    let other = server.create_session("zz-other", &[window(shell(&c))]);
    server.wait_for_pwds(&big);
    server.wait_for_pwds(&other);
    let [oversized, sibling] = [0, 1].map(|i| big.block_ids()[i]);
    let elsewhere = other.block_ids()[0];
    print_then_finish(
        &server,
        &big.id,
        oversized,
        "zz-oversized-output",
        "echo zz-oversized-last",
    );
    print_then_finish(
        &server,
        &big.id,
        sibling,
        "zz-sibling-output",
        "echo zz-sibling-last",
    );
    print_then_finish(
        &server,
        &other.id,
        elsewhere,
        "zz-elsewhere-output",
        "echo zz-elsewhere-last",
    );

    let stub = RexStub::new(&env).fail_when_all("block.method", &[oversized, FORMAT], 1, TOO_LARGE);
    env.rexurrect(&server)
        .path_prepend(stub.dir())
        .arg("save")
        .run()
        .assert_success();

    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "one entry for the fallback: {entries:#?}");
    for named in ["\"zz-big\"", &big.id, oversized, "too_large"] {
        assert!(entries[0].contains(named), "names {named}: {}", entries[0]);
    }

    server.kill_session(&big.id);
    server.kill_session(&other.id);
    env.restore(&server);

    // The oversized block comes back like any block without scrollback: in
    // its working directory, its last command typed, nothing replayed.
    let restored = server.the_session_labelled("zz-big");
    let blocks = server.block_ids(&restored);
    server.wait_for_typed(&restored, &blocks[0], "echo zz-oversized-last");
    server.wait_for_pwd_eq(&restored, &blocks[0], &a);
    let capture = server.capture(&restored, &blocks[0]);
    assert!(
        !capture.contains("zz-oversized-output"),
        "no replay:\n{capture}"
    );
    // Every other block keeps its scrollback, in its session and in others.
    let restored_other = server.the_session_labelled("zz-other");
    let other_block = server.block_ids(&restored_other).remove(0);
    for (session, block, printed, last) in [
        (
            &restored,
            &blocks[1],
            "zz-sibling-output",
            "echo zz-sibling-last",
        ),
        (
            &restored_other,
            &other_block,
            "zz-elsewhere-output",
            "echo zz-elsewhere-last",
        ),
    ] {
        server.wait_for_typed(session, block, last);
        let capture = server.capture(session, block);
        assert!(
            above_prompt(&capture).contains(&printed),
            "{printed} is replayed above the prompt:\n{capture}"
        );
    }
}

/// The first `format` call, which reads the modes, gets through; the second,
/// which reads the scrollback itself, is refused.
#[test]
fn a_block_whose_scrollback_is_refused_on_the_second_read_is_saved_without_it() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let fixture = server.create_session("zz-second", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    print_then_finish(
        &server,
        &fixture.id,
        block,
        "zz-second-output",
        "echo zz-second-last",
    );

    let unwrapped = r#""unwrap":true"#;
    let stub =
        RexStub::new(&env).fail_when_all("block.method", &[block, FORMAT, unwrapped], 1, TOO_LARGE);
    env.rexurrect(&server)
        .path_prepend(stub.dir())
        .arg("save")
        .run()
        .assert_success();

    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "one entry for the fallback: {entries:#?}");
    assert!(entries[0].contains(block), "{}", entries[0]);
    server.kill_session(&fixture.id);
    env.restore(&server);
    let restored = server.the_session_labelled("zz-second");
    let block = server.block_ids(&restored).remove(0);
    server.wait_for_typed(&restored, &block, "echo zz-second-last");
    let capture = server.capture(&restored, &block);
    assert!(
        !capture.contains("zz-second-output"),
        "no replay:\n{capture}"
    );
}

/// Only `too_large` falls back: any other failure to read scrollback (a
/// changed Rex API, a broken `format`) fails the save, and is logged, as
/// every failed save is.
#[test]
fn any_other_failure_to_read_a_blocks_scrollback_still_fails_the_save() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-broken", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];

    let failures = [
        r#"Error: stream response "control-2" failed: invalid format (status=400 code=bad_request)"#,
        r#"Error: stream response "control-2" failed: formatting failed (status=500 code=internal)"#,
    ];
    for failure in failures {
        let stub = RexStub::new(&env).fail_when_all("block.method", &[block, FORMAT], 1, failure);
        let run = env
            .rexurrect(&server)
            .path_prepend(stub.dir())
            .arg("save")
            .run()
            .assert_failure();
        assert!(run.stderr.contains("zz-broken"), "{}", run.stderr);
    }

    let entries = env.log_entries();
    assert_eq!(entries.len(), 2, "{entries:#?}");
    for (entry, code) in entries.iter().zip(["code=bad_request", "code=internal"]) {
        assert!(entry.contains("save failed: "), "{entry}");
        assert!(entry.contains(code), "{entry}");
    }
    assert_eq!(env.listed(&server).trim(), "no snapshots yet");
}

/// A save that fails after all saved no block, so the log holds only its
/// failure, not the fallback of a block in a snapshot never written.
#[test]
fn a_save_that_fails_after_a_block_fell_back_logs_only_its_failure() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-unwritten", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    // A file where the snapshots directory goes: writing the snapshot fails.
    std::fs::write(env.state_dir().join("snapshots"), "").unwrap();

    let stub = RexStub::new(&env).fail_when_all("block.method", &[block, FORMAT], 1, TOO_LARGE);
    env.rexurrect(&server)
        .path_prepend(stub.dir())
        .arg("save")
        .run()
        .assert_failure();

    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "only the failure: {entries:#?}");
    assert!(entries[0].contains("save failed: "), "{}", entries[0]);
}

/// A block's scrollback stays too large save after save until it's cleared,
/// and autosave saves every 15 minutes, so the log says so once: when the
/// block starts falling back. Once its scrollback is saved again, it's
/// logged again if it grows too large again.
#[test]
fn a_block_whose_scrollback_stays_too_large_is_logged_once() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-stays", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    let stub = RexStub::new(&env).fail_when_all("block.method", &[block, FORMAT], 1, TOO_LARGE);
    let save_too_large = || {
        env.rexurrect(&server)
            .path_prepend(stub.dir())
            .arg("save")
            .run()
            .assert_success()
    };

    save_too_large();
    save_too_large();
    save_too_large();
    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "logged when it starts: {entries:#?}");

    // Its scrollback is saved again (it was cleared), then grows too large
    // again.
    env.save(&server);
    save_too_large();
    let entries = env.log_entries();
    assert_eq!(
        entries.len(),
        2,
        "logged when it starts again: {entries:#?}"
    );
    assert!(
        entries[1].contains("save fell back: ") && entries[1].contains(block),
        "{}",
        entries[1]
    );
}
