//! `save` and `restore` failures are appended to a timestamped log in the
//! state directory, so a failed autosave (which nobody sees) can still be
//! diagnosed. Successful runs write nothing to it (but for auto-restore's
//! outcomes and save's fallbacks: see tests/auto_restore.rs and
//! tests/oversized_scrollback.rs).

mod common;

use std::os::unix::fs::PermissionsExt;

use common::files::accessible_to_others;
use common::*;
use serde_json::json;

/// Splits a log entry into its timestamp and message, checking the
/// timestamp looks like `2026-10-11T07:04:49.856123Z` (UTC).
fn timestamp_and_message(entry: &str) -> (&str, &str) {
    let (timestamp, message) = entry
        .split_once(' ')
        .unwrap_or_else(|| panic!("no message in log entry {entry:?}"));
    let shape_ok = timestamp.len() > 20
        && timestamp.ends_with('Z')
        && timestamp.chars().enumerate().all(|(i, c)| match i {
            4 | 7 => c == '-',
            10 => c == 'T',
            13 | 16 => c == ':',
            19 => c == '.',
            _ => c.is_ascii_digit() || c == 'Z',
        });
    assert!(
        shape_ok,
        "log entry doesn't start with a timestamp: {entry:?}"
    );
    (timestamp, message)
}

#[test]
fn each_failed_save_or_restore_appends_a_timestamped_entry() {
    let env = TestEnv::new();
    let endpoint = {
        let server = env.start_server();
        server.endpoint().to_string()
    }; // The server stops: nothing answers at its endpoint any more.

    env.rexurrect_at(&endpoint)
        .arg("save")
        .run()
        .assert_failure();
    env.rexurrect_at(&endpoint)
        .arg("restore")
        .run()
        .assert_failure();

    let entries = env.log_entries();
    assert_eq!(entries.len(), 2, "{entries:#?}");
    let (first, save) = timestamp_and_message(&entries[0]);
    let (second, restore) = timestamp_and_message(&entries[1]);
    assert!(first <= second, "entries are in time order: {entries:#?}");
    assert!(save.starts_with("save failed: "), "{save}");
    assert!(restore.starts_with("restore failed: "), "{restore}");
    for message in [save, restore] {
        assert!(message.contains("isn't running"), "{message}");
    }
}

#[test]
fn a_save_rejected_for_unexpected_rex_output_is_logged() {
    let env = TestEnv::new();
    let server = env.start_server();
    let stub = RexStub::new(&env).answer("session.list", &json!({ "sessionz": [] }));

    env.rexurrect(&server)
        .arg("save")
        .path_prepend(stub.dir())
        .run()
        .assert_failure();

    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    let (_, message) = timestamp_and_message(&entries[0]);
    assert!(message.contains("session.list"), "{message}");
}

#[test]
fn each_session_that_fails_to_restore_is_logged() {
    let env = TestEnv::new();
    let a = env.mkdir("a");
    {
        let before = env.start_server();
        let narrow = before.create_session("zz-narrow", &[window(shell(&a))]);
        let wide = before.create_session("zz-wide", &[window(shell(&a)), window(shell(&a))]);
        before.wait_for_pwds(&narrow);
        before.wait_for_pwds(&wide);
        env.save(&before);
    } // The server stops, as in a restart.

    // The new server refuses a second window, so zz-wide can't be recreated.
    let after = env.server().arg("--max-windows").arg("1").start();
    env.rexurrect(&after).arg("restore").run().assert_failure();

    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    let (_, message) = timestamp_and_message(&entries[0]);
    assert!(message.starts_with("restore failed: "), "{message}");
    assert!(message.contains("zz-wide"), "{message}");
}

#[test]
fn a_session_that_fails_to_restore_when_save_restores_first_is_logged() {
    let env = TestEnv::new();
    let a = env.mkdir("a");
    {
        let before = env.start_server();
        let narrow = before.create_session("zz-narrow", &[window(shell(&a))]);
        let wide = before.create_session("zz-wide", &[window(shell(&a)), window(shell(&a))]);
        before.wait_for_pwds(&narrow);
        before.wait_for_pwds(&wide);
        env.save(&before);
    } // The server stops, as in a restart.

    // A fresh server instance, so save restores first, and zz-wide fails.
    // (A restore in which every session fails is covered in
    // tests/unrestored_instance.rs.)
    let after = env.server().arg("--max-windows").arg("1").start();
    env.rexurrect(&after).arg("save").run().assert_failure();

    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    let (_, message) = timestamp_and_message(&entries[0]);
    assert!(message.starts_with("save"), "{message}");
    assert!(message.contains("zz-wide"), "{message}");
}

/// While Rex is quit, every autosave fails and logs it, so the log would
/// grow forever. Once it passes about a megabyte, only its newest half is
/// kept, still readable by the user only.
#[test]
fn the_log_keeps_only_its_newest_entries_once_it_passes_a_megabyte() {
    let env = TestEnv::new();
    let endpoint = {
        let server = env.start_server();
        server.endpoint().to_string()
    }; // Rex quits.
    // A log that has been growing for a long time.
    let log = env.state_dir().join("rexurrect.log");
    let filler = "x".repeat(40);
    let old: String = (0..20_000)
        .map(|i| format!("2026-01-01T00:00:00.000000Z save failed: zz-old-{i:05} {filler}\n"))
        .collect();
    assert!(old.len() > 1_100_000);
    std::fs::write(&log, &old).unwrap();
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600)).unwrap();

    env.rexurrect_at(&endpoint)
        .arg("save")
        .run()
        .assert_failure();

    let size = std::fs::metadata(&log).unwrap().len();
    assert!(size < 1_000_000, "the log is still {size} bytes");
    let entries = env.log_entries();
    for entry in &entries {
        timestamp_and_message(entry);
    }
    let newest = entries.last().unwrap();
    assert!(newest.contains("isn't running"), "{newest}");
    assert!(
        entries[0].contains("zz-old-1"),
        "the oldest are gone: {}",
        entries[0]
    );
    assert!(entries.iter().any(|e| e.contains("zz-old-19999")));
    assert_eq!(accessible_to_others(&log), Vec::<String>::new());
}

#[test]
fn successful_saves_and_restores_write_nothing_to_the_log() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-fine", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    assert_eq!(env.log_entries(), Vec::<String>::new());
}
