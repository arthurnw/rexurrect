//! One session failing to restore doesn't stop the others.

mod common;

use common::*;

#[test]
fn a_session_that_fails_to_restore_does_not_stop_the_others_and_restore_exits_non_zero() {
    let env = TestEnv::new();
    let a = env.mkdir("a");
    {
        let before = env.start_server();
        let first = before.create_session("zz-narrow-1", &[window(shell(&a))]);
        let wide = before.create_session("zz-wide", &[window(shell(&a)), window(shell(&a))]);
        let last = before.create_session("zz-narrow-2", &[window(shell(&a))]);
        for session in [&first, &wide, &last] {
            before.wait_for_pwds(session);
        }
        env.save(&before);
    } // The server stops, as in a restart.

    // The new server refuses a second window in any session, so zz-wide
    // can't be recreated.
    let after = env.server().arg("--max-windows").arg("1").start();
    let out = env.rexurrect(&after).arg("restore").run().assert_failure();

    let created = out.lines_starting("created");
    let failed = out.lines_starting("failed");
    assert_eq!(created.len(), 2, "{out}");
    assert_eq!(failed.len(), 1, "{out}");
    assert!(failed[0].contains("zz-wide"), "{out}");
    let mut labels: Vec<String> = after.sessions().into_iter().map(|s| s.label).collect();
    labels.sort();
    assert_eq!(labels, ["zz-narrow-1", "zz-narrow-2"]);
}

/// Restore matches the block IDs `session.create` answers with to the saved
/// blocks, to carry their last commands forward. When they don't match up,
/// the session exists but can't be carried forward: it fails on its own, and
/// the next restore still recognises it, so it's never created twice.
#[test]
fn a_session_whose_new_blocks_cant_be_matched_to_its_saved_ones_fails_alone_and_isnt_duplicated() {
    let env = TestEnv::new();
    let a = env.mkdir("a");
    {
        let before = env.start_server();
        let odd = before.create_session("zz-odd", &[window(hsplit(0.5, shell(&a), shell(&a)))]);
        let fine = before.create_session("zz-fine", &[window(shell(&a))]);
        before.wait_for_pwds(&odd);
        before.wait_for_pwds(&fine);
        env.save(&before);
    } // The server stops, as in a restart.

    let after = env.start_server();
    // session.create creates zz-odd as asked, but answers with no block IDs.
    let stub = RexStub::new(&env).rewrite_when(
        "session.create",
        "zz-odd",
        r#"s/"block_ids":\[[^]]*\]/"block_ids":[]/g"#,
    );
    let out = env
        .rexurrect(&after)
        .arg("restore")
        .path_prepend(stub.dir())
        .run()
        .assert_failure();

    let failed = out.lines_starting("failed");
    assert_eq!(failed.len(), 1, "{out}");
    assert!(failed[0].contains("zz-odd"), "{out}");
    assert_eq!(out.lines_starting("created").len(), 1, "{out}");
    let again = env.restore(&after);
    assert_eq!(again.lines_starting("skipped").len(), 2, "{again}");
    assert_eq!(after.session_ids_labelled("zz-odd").len(), 1);
}
