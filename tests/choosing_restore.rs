//! Choosing what to restore: `list` shows the snapshots, and `restore` can
//! take an older snapshot, a single session label and a dry run.

mod common;

use common::*;

/// One line of `rexurrect list`.
#[derive(Debug)]
struct Listed {
    id: String,
    time: String,
    labels: Vec<String>,
}

/// Runs `rexurrect list` and parses its lines: `<id>  <time>  <labels>`.
fn list(env: &TestEnv, server: &Server) -> Vec<Listed> {
    let out = env.rexurrect(server).arg("list").run().assert_success();
    out.stdout
        .lines()
        .map(|line| {
            let mut columns = line.split("  ");
            let mut next = || {
                columns
                    .next()
                    .unwrap_or_else(|| panic!("list line {line:?} has too few columns"))
                    .to_string()
            };
            let (id, time, labels) = (next(), next(), next());
            Listed {
                id,
                time,
                labels: labels.split(", ").map(str::to_string).collect(),
            }
        })
        .collect()
}

/// `2026-10-11 07:04:49 UTC`
fn looks_like_utc_time(time: &str) -> bool {
    let shape = "dddd-dd-dd dd:dd:dd UTC";
    time.len() == shape.len()
        && time.chars().zip(shape.chars()).all(|(c, s)| match s {
            'd' => c.is_ascii_digit(),
            _ => c == s,
        })
}

/// Creates a session holding one shell block in `dir` and waits for it to
/// report its working directory, so a save sees it.
fn session_in(server: &Server, label: &str, dir: &std::path::Path) -> FixtureSession {
    let session = server.create_session(label, &[window(shell(dir))]);
    server.wait_for_pwds(&session);
    session
}

#[test]
fn list_shows_each_snapshot_newest_first_with_its_time_and_session_labels() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    session_in(&server, "zz-first", &a);
    env.save(&server);
    session_in(&server, "zz-second", &a);
    env.save(&server);

    let snapshots = list(&env, &server);

    assert_eq!(snapshots.len(), 2, "{snapshots:?}");
    assert_eq!(snapshots[0].labels, ["zz-first", "zz-second"]);
    assert_eq!(snapshots[1].labels, ["zz-first"]);
    assert_ne!(snapshots[0].id, snapshots[1].id);
    for snapshot in &snapshots {
        assert!(looks_like_utc_time(&snapshot.time), "{snapshot:?}");
    }
    assert!(snapshots[0].time >= snapshots[1].time, "{snapshots:?}");
}

#[test]
fn list_before_the_first_save_says_there_are_no_snapshots_without_needing_rex() {
    let env = TestEnv::new();
    let nowhere = format!("unix://{}/none.sock", env.root().display());

    let out = env
        .rexurrect_at(&nowhere)
        .arg("list")
        .run()
        .assert_success();

    assert_eq!(out.stdout.trim(), "no snapshots yet", "{out}");
}

#[test]
fn restore_can_use_an_older_snapshot_chosen_from_list() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let gone = session_in(&server, "zz-gone", &a);
    let kept = session_in(&server, "zz-kept", &a);
    env.save(&server);
    server.kill_session(&gone.id);
    env.save(&server); // The latest snapshot holds only zz-kept.
    server.kill_session(&kept.id);

    let older = &list(&env, &server)[1];
    assert_eq!(older.labels, ["zz-gone", "zz-kept"], "{older:?}");
    let out = env
        .rexurrect(&server)
        .args(["restore", "--snapshot", &older.id])
        .run()
        .assert_success();

    assert_eq!(out.lines_starting("created").len(), 2, "{out}");
    assert_eq!(server.sorted_labels(), ["zz-gone", "zz-kept"]);
}

#[test]
fn restore_from_a_snapshot_that_does_not_exist_fails_and_creates_nothing() {
    let env = TestEnv::new();
    let server = env.start_server();
    let proj = session_in(&server, "zz-proj", &env.mkdir("a"));
    env.save(&server);
    server.kill_session(&proj.id);

    let out = env
        .rexurrect(&server)
        .args(["restore", "--snapshot", "20000101T000000.000000Z"])
        .run()
        .assert_failure();

    assert!(out.stderr.contains("20000101T000000.000000Z"), "{out}");
    assert!(server.sessions().is_empty(), "{:?}", server.sessions());
}

#[test]
fn restoring_an_older_snapshot_skips_saved_sessions_whose_lineage_is_live() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let original = session_in(&server, "zz-proj", &a);
    env.save(&server);
    let older = list(&env, &server)[0].id.clone();
    server.kill_session(&original.id);
    env.restore(&server);

    // The restored session (of the original's lineage) moves on to `b`.
    let [restored] = server.session_ids_labelled("zz-proj").try_into().unwrap();
    let Shape::Block(block) = &common::fixture::tiled_layouts(&server.view(&restored))[0] else {
        panic!("the restored session holds one block");
    };
    server.write(&restored, block, &format!("cd '{}'\n", b.display()));
    server.wait_for_pwd_eq(&restored, block, &b);
    env.save(&server);

    let out = env
        .rexurrect(&server)
        .args(["restore", "--snapshot", &older])
        .run()
        .assert_success();

    assert!(out.lines_starting("created").is_empty(), "{out}");
    assert_eq!(out.lines_starting("skipped").len(), 1, "{out}");
    assert_eq!(server.session_ids_labelled("zz-proj"), [restored.as_str()]);
    assert_eq!(
        server.window_shapes(&restored),
        [shell(&b).expected_shape()]
    );
}

#[test]
fn a_single_session_restore_creates_only_the_saved_sessions_with_that_label() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [left, right, elsewhere] = ["left", "right", "elsewhere"].map(|name| env.mkdir(name));
    let saved = [
        session_in(&server, "zz-twin", &left),
        session_in(&server, "zz-other", &elsewhere),
        session_in(&server, "zz-twin", &right),
    ];
    env.save(&server);
    for session in &saved {
        server.kill_session(&session.id);
    }

    let out = env
        .rexurrect(&server)
        .args(["restore", "--session", "zz-twin"])
        .run()
        .assert_success();

    assert_eq!(out.lines_starting("created").len(), 2, "{out}");
    assert_eq!(server.sorted_labels(), ["zz-twin", "zz-twin"]);
    let mut shapes: Vec<Vec<Shape>> = server
        .session_ids_labelled("zz-twin")
        .iter()
        .map(|id| server.window_shapes(id))
        .collect();
    shapes.sort_by_key(|shapes| format!("{shapes:?}"));
    assert_eq!(
        shapes,
        [
            vec![shell(&left).expected_shape()],
            vec![shell(&right).expected_shape()]
        ],
    );
}

#[test]
fn a_single_session_restore_of_a_label_not_in_the_snapshot_fails_and_creates_nothing() {
    let env = TestEnv::new();
    let server = env.start_server();
    let proj = session_in(&server, "zz-proj", &env.mkdir("a"));
    env.save(&server);
    server.kill_session(&proj.id);

    let out = env
        .rexurrect(&server)
        .args(["restore", "--session", "zz-missing"])
        .run()
        .assert_failure();

    assert!(
        out.stderr
            .contains("no saved session labelled \"zz-missing\""),
        "{out}"
    );
    assert!(server.sessions().is_empty(), "{:?}", server.sessions());
}

/// Dry-run lines naming a saved session that restore would create.
fn would_create(out: &RunOutput) -> Vec<&str> {
    out.stdout
        .lines()
        .filter(|line| line.starts_with("would create "))
        .collect()
}

#[test]
fn a_dry_run_prints_what_would_be_created_and_skipped_and_creates_nothing() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let gone = session_in(&server, "zz-gone", &a);
    session_in(&server, "zz-kept", &a);
    env.save(&server);
    server.kill_session(&gone.id);
    let before = server.sessions();

    let out = env
        .rexurrect(&server)
        .args(["restore", "--dry-run"])
        .run()
        .assert_success();

    let [created] = would_create(&out)
        .try_into()
        .expect("one would-create line");
    assert!(created.contains("zz-gone"), "{out}");
    let [skipped] = out
        .lines_starting("skipped")
        .try_into()
        .expect("one skipped line");
    assert!(skipped.contains("zz-kept"), "{out}");
    assert!(out.lines_starting("created").is_empty(), "{out}");
    assert_eq!(server.sessions(), before, "a dry run creates no sessions");

    // Nothing the dry run did stops a real restore.
    let real = env.restore(&server);
    assert_eq!(real.lines_starting("created").len(), 1, "{real}");
}

#[test]
fn the_options_combine_into_a_dry_run_of_one_session_from_an_older_snapshot() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let one = session_in(&server, "zz-one", &a);
    session_in(&server, "zz-two", &a);
    env.save(&server);
    let older = list(&env, &server)[0].id.clone();
    server.kill_session(&one.id);
    env.save(&server); // The latest snapshot holds only zz-two.
    let before = server.sessions();
    let restore = |args: &[&str]| env.rexurrect(&server).arg("restore").args(args).run();

    restore(&["--session", "zz-one", "--dry-run"]).assert_failure();
    let dry = restore(&["--snapshot", &older, "--session", "zz-one", "--dry-run"]).assert_success();

    let [created] = would_create(&dry)
        .try_into()
        .expect("one would-create line");
    assert!(created.contains("zz-one"), "{dry}");
    assert!(
        dry.lines_starting("skipped").is_empty(),
        "zz-two isn't selected: {dry}"
    );
    assert_eq!(server.sessions(), before, "a dry run creates no sessions");

    restore(&["--snapshot", &older, "--session", "zz-one"]).assert_success();
    assert_eq!(server.sorted_labels(), ["zz-one", "zz-two"]);
}
