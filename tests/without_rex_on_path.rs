//! Without `rex` on `PATH` (autosave's agent never has it), save and
//! restore run the `rex` a running Rex server runs, still naming the server
//! to call with `-S` (ADR-0003).
//!
//! Every throwaway server runs the same `rex`, so which server's executable
//! gets picked can't be told apart here; that choice has unit tests of its
//! own.

mod common;

use common::*;

#[test]
fn save_and_restore_reach_a_running_server_without_rex_on_path() {
    let env = TestEnv::new();
    let server = env.start_server();
    let fixture = server.create_session("zz-off-path", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let run = |command: &str| {
        env.rexurrect(&server)
            .env("PATH", SYSTEM_PATH)
            .arg(command)
            .run()
            .assert_success()
    };

    run("save");
    server.kill_session(&fixture.id);
    run("restore");

    assert_eq!(server.sorted_labels(), ["zz-off-path"]);
}

#[test]
fn a_save_with_no_server_at_the_endpoint_logs_the_failure_naming_it_and_starts_no_server() {
    let env = TestEnv::new();
    let endpoint = {
        let server = env.start_server();
        server.endpoint().to_string()
    }; // The server stops here.
    // A temp HOME too: if autostart were ever on, rex's default server would
    // start in there rather than anywhere real.
    let home = env.mkdir("home");

    let out = env
        .rexurrect_at(&endpoint)
        .env("PATH", SYSTEM_PATH)
        .env("HOME", &home)
        .arg("save")
        .run();

    env.assert_no_server_started_at(&endpoint, "the save");
    out.assert_failure();
    let entries = env.log_entries();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    assert!(entries[0].contains("save failed"), "{entries:#?}");
    assert!(entries[0].contains(&endpoint), "{entries:#?}");
}
