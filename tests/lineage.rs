//! Restore recognises saved sessions by lineage, never by label (ADR-0001).

mod common;

use common::*;

#[test]
fn restoring_twice_creates_no_duplicates() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let one = server.create_session("zz-one", &[window(shell(&a))]);
    let two = server.create_session("zz-two", &[window(shell(&a))]);
    server.wait_for_pwds(&one);
    server.wait_for_pwds(&two);
    env.save(&server);
    server.kill_session(&one.id);

    let first = env.restore(&server);
    assert_eq!(first.lines_starting("created").len(), 1, "{first}");
    assert!(
        first.lines_starting("created")[0].contains("zz-one"),
        "{first}"
    );
    assert_eq!(first.lines_starting("skipped").len(), 1, "{first}");
    assert!(
        first.lines_starting("skipped")[0].contains("zz-two"),
        "{first}"
    );

    let second = env.restore(&server);
    assert!(second.lines_starting("created").is_empty(), "{second}");
    assert_eq!(second.lines_starting("skipped").len(), 2, "{second}");
    assert_eq!(server.sorted_labels(), ["zz-one", "zz-two"]);
}

#[test]
fn a_restored_session_keeps_its_lineage_through_later_saves() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let original = server.create_session("zz-proj", &[window(shell(&a))]);
    server.wait_for_pwds(&original);
    env.save(&server);
    server.kill_session(&original.id);
    env.restore(&server);

    // The new snapshot holds the restored session, which carries the
    // original's lineage, so restore still finds it live.
    env.save(&server);
    let again = env.restore(&server);
    assert!(again.lines_starting("created").is_empty(), "{again}");
    assert_eq!(server.session_ids_labelled("zz-proj").len(), 1);
}

#[test]
fn sessions_sharing_a_label_are_both_restored_and_a_renamed_one_is_still_recognised() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [left, right] = ["left", "right"].map(|name| env.mkdir(name));
    let first = server.create_session("zz-twin", &[window(shell(&left))]);
    let second = server.create_session("zz-twin", &[window(shell(&right))]);
    server.wait_for_pwds(&first);
    server.wait_for_pwds(&second);
    env.save(&server);
    server.kill_session(&first.id);
    server.kill_session(&second.id);

    env.restore(&server);
    let twins = server.session_ids_labelled("zz-twin");
    assert_eq!(twins.len(), 2, "both saved zz-twin sessions restored");
    let mut dirs: Vec<Vec<Shape>> = twins.iter().map(|id| server.window_shapes(id)).collect();
    dirs.sort_by_key(|shapes| format!("{shapes:?}"));
    assert_eq!(
        dirs,
        [
            vec![shell(&left).expected_shape()],
            vec![shell(&right).expected_shape()]
        ],
    );

    server.rename_session(&twins[0], "zz-renamed");
    let again = env.restore(&server);
    assert!(again.lines_starting("created").is_empty(), "{again}");
    assert_eq!(server.sorted_labels(), ["zz-renamed", "zz-twin"]);
}

#[test]
fn a_session_created_by_hand_under_a_saved_label_does_not_count_as_live() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [saved_dir, by_hand_dir] = ["saved", "by-hand"].map(|name| env.mkdir(name));
    let original = server.create_session("zz-proj", &[window(shell(&saved_dir))]);
    server.wait_for_pwds(&original);
    env.save(&server);
    server.kill_session(&original.id);
    let by_hand = server.create_session("zz-proj", &[window(shell(&by_hand_dir))]);

    let out = env.restore(&server);
    assert_eq!(out.lines_starting("created").len(), 1, "{out}");
    let restored: Vec<String> = server
        .session_ids_labelled("zz-proj")
        .into_iter()
        .filter(|id| *id != by_hand.id)
        .collect();
    assert_eq!(
        restored.len(),
        1,
        "the saved zz-proj restored next to the hand-made one"
    );
    assert_eq!(
        server.window_shapes(&restored[0]),
        [shell(&saved_dir).expected_shape()]
    );
}
