//! rex-resurrect never starts a Rex server as a side effect.

mod common;

use common::*;

#[test]
fn save_and_restore_fail_without_starting_a_server_when_none_is_running() {
    let env = TestEnv::new();
    // Take a snapshot first so restore gets as far as talking to Rex.
    let endpoint = {
        let server = env.start_server();
        let fixture = server.create_session("zz-gone", &[window(shell(env.mkdir("a")))]);
        server.wait_for_pwds(&fixture);
        env.save(&server);
        server.endpoint().to_string()
    }; // The server stops here.
    assert_eq!(env.server_pid_at(&endpoint), None);
    // A temp HOME too: if autostart were ever on, rex's default server would
    // start in there rather than anywhere real.
    let home = env.mkdir("home");

    for command in ["save", "restore"] {
        let out = env
            .resurrect_at(&endpoint)
            .arg(command)
            .env("HOME", &home)
            .run();
        if let Some(pid) = env.server_pid_at(&endpoint) {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
            panic!("`rex-resurrect {command}` started a Rex server (pid {pid}) at {endpoint}");
        }
        let out = out.assert_failure();
        assert!(out.stderr.contains("autostart is disabled"), "{out}");
    }
}
