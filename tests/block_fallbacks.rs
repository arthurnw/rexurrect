//! A block that can't come back exactly as saved still comes back as a shell
//! block, so one bad block never breaks a session's layout.
//!
//! "The home directory" is rexurrect's own `HOME`. These tests give rexurrect
//! a home of its own, different from the throwaway server's, so they can tell
//! rexurrect choosing the home directory apart from Rex falling back to its
//! own home for a `cwd` that doesn't exist.

mod common;

use std::path::Path;

use common::*;
use serde_json::{Value, json};

/// `rexurrect <command>` against `server`, with `home` as its home directory.
fn run_with_home(env: &TestEnv, server: &Server, home: &Path, command: &str) -> RunOutput {
    env.rexurrect(server)
        .env("HOME", home)
        .arg(command)
        .run()
        .assert_success()
}

/// A stand-in `rex` that reports one block of a live session as something a
/// throwaway server can't create (a block that isn't a shell block, a block
/// on another host). Put it first on rexurrect's `PATH`.
///
/// - `session.view` of that session returns the real view, with `edit`
///   applied to the block's entry in its layer's `blocks`;
/// - terminal methods (`block.method`) on the block fail the way they do for a
///   block that has none (exit 3, `method not found`);
/// - every other call goes to the real `rex`, unchanged.
fn stub_rex_reporting(
    env: &TestEnv,
    server: &Server,
    session_id: &str,
    block_id: &str,
    edit: impl Fn(&mut Value),
) -> RexStub {
    let mut view = server.view(session_id);
    let entry = view["windows"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .flat_map(|window| window["layers"].as_array_mut().unwrap())
        .flat_map(|layer| layer["blocks"].as_array_mut().unwrap())
        .find(|block| block["block_id"] == block_id)
        .unwrap_or_else(|| panic!("{block_id} isn't in session {session_id}"));
    edit(entry);
    RexStub::new(env)
        .answer_when("session.view", session_id, &view)
        .fail_when(
            "block.method",
            block_id,
            3,
            r#"Error: stream response "control-1" failed: method not found (status=404 code=method_not_found)"#,
        )
}

/// Moves a tiled block into a new floating layer of its window.
fn float(server: &Server, session_id: &str, block_id: &str) {
    server.call(
        "session.float_block",
        &json!({
            "session_id": session_id,
            "block_id": block_id,
            "bounds": { "x": 0.1, "y": 0.1, "w": 0.5, "h": 0.5 },
        }),
    );
}

/// How many floating layers each window of a live session has.
fn floating_layers(server: &Server, session_id: &str) -> Vec<usize> {
    let view = server.view(session_id);
    view["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|window| {
            window["layers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|layer| layer["kind"] == "floating")
                .count()
        })
        .collect()
}

#[test]
fn a_window_with_a_floating_layer_saves_and_restores_only_its_tiled_layout() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, b, floated] = ["a", "b", "floated"].map(|name| env.mkdir(name));
    let fixture = server.create_session(
        "zz-floating",
        &[window(hsplit(
            0.4,
            shell(&a),
            vsplit(0.5, shell(&floated), shell(&b)),
        ))],
    );
    server.wait_for_pwds(&fixture);
    float(&server, &fixture.id, &fixture.windows[0].block_ids[1]);

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.session_ids_labelled("zz-floating");
    assert_eq!(restored.len(), 1, "exactly one session restored");
    assert_eq!(
        server.window_shapes(&restored[0]),
        vec![hsplit(0.4, shell(&a), shell(&b)).expected_shape()],
        "the tiled layout as it was with the block floated away",
    );
    assert_eq!(floating_layers(&server, &restored[0]), [0]);
}

#[test]
fn a_window_whose_blocks_all_float_restores_as_one_shell_in_the_home_directory_in_its_place() {
    let env = TestEnv::new();
    let server = env.start_server();
    let home = env.mkdir("home");
    let [a, floated, c] = ["a", "floated", "c"].map(|name| env.mkdir(name));
    let fixture = server.create_session(
        "zz-all-floating",
        &[
            window(shell(&a)),
            window(shell(&floated)).labeled("floaty"),
            window(shell(&c)),
        ],
    );
    server.wait_for_pwds(&fixture);
    float(&server, &fixture.id, &fixture.windows[1].block_ids[0]);

    run_with_home(&env, &server, &home, "save");
    server.kill_session(&fixture.id);
    run_with_home(&env, &server, &home, "restore");

    let restored = server.session_ids_labelled("zz-all-floating");
    assert_eq!(restored.len(), 1, "exactly one session restored");
    assert_eq!(
        server.window_shapes(&restored[0]),
        vec![
            shell(&a).expected_shape(),
            shell(&home).expected_shape(),
            shell(&c).expected_shape(),
        ],
        "the emptied window keeps its place in the window order",
    );
    let emptied = &labels(&server.view(&restored[0]))[1];
    assert_eq!(
        emptied.label, "floaty",
        "the emptied window keeps its label"
    );
    assert_eq!(emptied.block_labels, ["shell"]);
    assert_eq!(floating_layers(&server, &restored[0]), [0, 0, 0]);
}

/// Saves a session whose middle block the stand-in `rex` reports through
/// `edit`, restores it, and checks that only that block came back as a plain
/// shell (default label) in the home directory, in its place in the split
/// tree.
fn assert_saved_as_a_plain_shell_in_home(label: &str, edit: impl Fn(&mut Value)) {
    let env = TestEnv::new();
    let server = env.start_server();
    let home = env.mkdir("home");
    let [a, odd, c] = ["a", "odd", "c"].map(|name| env.mkdir(name));
    let layout = |middle: &Path| {
        hsplit(
            0.3,
            shell(&a).labeled("left"),
            vsplit(0.7, shell(middle).labeled("odd"), shell(&c)),
        )
    };
    let fixture = server.create_session(label, &[window(layout(&odd))]);
    server.wait_for_pwds(&fixture);
    let stub = stub_rex_reporting(
        &env,
        &server,
        &fixture.id,
        &fixture.windows[0].block_ids[1],
        edit,
    );

    env.rexurrect(&server)
        .env("HOME", &home)
        .path_prepend(stub.dir())
        .arg("save")
        .run()
        .assert_success();
    server.kill_session(&fixture.id);
    run_with_home(&env, &server, &home, "restore");

    let restored = server.session_ids_labelled(label);
    assert_eq!(restored.len(), 1, "exactly one session restored");
    assert_eq!(
        server.window_shapes(&restored[0]),
        vec![layout(&home).expected_shape()],
    );
    assert_eq!(
        labels(&server.view(&restored[0]))[0].block_labels,
        ["left", "shell", "shell"],
    );
}

#[test]
fn a_block_that_isnt_a_shell_block_is_saved_as_a_plain_shell_in_the_home_directory() {
    assert_saved_as_a_plain_shell_in_home("zz-not-a-shell", |block| {
        block["creator_name"] = json!("com.example.notes");
        block["flavor"] = json!("com.example.notes.page");
        block["label"] = json!("notes");
    });
}

#[test]
fn a_block_on_another_host_is_saved_as_a_plain_shell_in_the_home_directory() {
    // Still a shell block, but its `host_id` isn't the local server's.
    assert_saved_as_a_plain_shell_in_home("zz-remote", |block| {
        block["host_id"] = json!("9f4c2a1e-0000-4000-8000-00000000beef");
    });
}

#[test]
fn a_block_whose_directory_was_deleted_restores_in_the_home_directory() {
    let env = TestEnv::new();
    let server = env.start_server();
    let home = env.mkdir("home");
    let [a, doomed, c, d] = ["a", "doomed", "c", "d"].map(|name| env.mkdir(name));

    let editor = |middle: &Path| hsplit(0.3, shell(&a), vsplit(0.6, shell(middle), shell(&c)));
    let fixture = server.create_session(
        "zz-missing-dir",
        &[window(editor(&doomed)), window(shell(&d))],
    );
    server.wait_for_pwds(&fixture);

    run_with_home(&env, &server, &home, "save");
    server.kill_session(&fixture.id);
    std::fs::remove_dir(&doomed).unwrap();
    run_with_home(&env, &server, &home, "restore");

    let restored = server.session_ids_labelled("zz-missing-dir");
    assert_eq!(restored.len(), 1, "exactly one session restored");
    assert_eq!(
        server.window_shapes(&restored[0]),
        vec![editor(&home).expected_shape(), shell(&d).expected_shape()],
        "only the block whose directory is gone moves to the home directory",
    );
}
