//! When Rex's output doesn't have the shape rex-resurrect expects, or fails
//! validation, `save` fails loudly, names what didn't match and writes
//! nothing: a changed beta API never produces an empty or broken snapshot.
//!
//! A real server can't produce bad output, so a stub `rex` answers one method
//! with the real server's output, changed, and hands every other call to it.

mod common;

use common::*;
use serde_json::{Value, json};

/// A server with one saved fixture session (a 70/30 split), so "latest"
/// exists before the bad save. Returns the layout it was created with.
fn server_with_a_saved_split(env: &TestEnv) -> (Server, FixtureSession, Layout) {
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let layout = hsplit(0.7, shell(&a), shell(&b));
    let fixture = server.create_session("zz-good", &[window(layout.clone())]);
    server.wait_for_pwds(&fixture);
    env.save(&server);
    (server, fixture, layout)
}

/// Runs `save` with `session.view` answered by `view` instead of the server.
fn save_with_view(env: &TestEnv, server: &Server, view: &Value) -> RunOutput {
    let stub = RexStub::new(env).answer("session.view", view);
    env.resurrect(server)
        .path_prepend(stub.dir())
        .arg("save")
        .run()
}

/// Runs `save` with every block's terminal `process` method answered by
/// `process` instead of the server.
fn save_with_process(env: &TestEnv, server: &Server, process: &Value) -> RunOutput {
    let stub = RexStub::new(env).answer_when("block.method", r#""method":"process""#, process);
    env.resurrect(server)
        .path_prepend(stub.dir())
        .arg("save")
        .run()
}

/// A real block's terminal `process` answer with its `field` renamed to `to`.
fn process_with_field_moved(
    server: &Server,
    fixture: &FixtureSession,
    field: &str,
    to: &str,
) -> Value {
    let block = fixture.block_ids()[0];
    let mut process = server.block_method(&fixture.id, block, "process", json!({}));
    let value = process.as_object_mut().unwrap().remove(field).unwrap();
    process[to] = value;
    process
}

/// The latest snapshot is still the good one: after killing the fixture,
/// restore brings it back exactly as it was created.
fn assert_latest_is_unchanged(
    env: &TestEnv,
    server: &Server,
    fixture: &FixtureSession,
    layout: &Layout,
) {
    server.kill_session(&fixture.id);
    env.restore(server);
    let restored = server.session_ids_labelled("zz-good");
    assert_eq!(restored.len(), 1, "the good snapshot is still the latest");
    assert_eq!(
        server.window_shapes(&restored[0]),
        [layout.expected_shape()]
    );
}

fn tiled_layer(view: &mut Value, window: usize) -> &mut Value {
    view["windows"][window]["layers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|layer| layer["kind"] == "tiled")
        .unwrap()
}

#[test]
fn a_split_ratio_outside_0_to_1_fails_save_and_writes_nothing() {
    let env = TestEnv::new();
    let (server, fixture, layout) = server_with_a_saved_split(&env);
    let mut view = server.view(&fixture.id);
    tiled_layer(&mut view, 0)["layout"]["split"]["ratio"] = json!(1.5);

    let out = save_with_view(&env, &server, &view).assert_failure();
    assert!(out.stderr.contains("ratio"), "{out}");
    assert!(out.stderr.contains("1.5"), "{out}");

    assert_latest_is_unchanged(&env, &server, &fixture, &layout);
}

#[test]
fn a_session_list_in_another_shape_fails_save_instead_of_looking_like_an_empty_server() {
    let env = TestEnv::new();
    let (server, fixture, layout) = server_with_a_saved_split(&env);
    let mut list = server.call("session.list", &json!({}));
    list = json!({ "items": list["sessions"].take() });
    let stub = RexStub::new(&env).answer("session.list", &list);

    let out = env
        .resurrect(&server)
        .path_prepend(stub.dir())
        .arg("save")
        .run()
        .assert_failure();
    assert!(out.stderr.contains("sessions"), "{out}");

    assert_latest_is_unchanged(&env, &server, &fixture, &layout);
}

/// Rex omits `windows` for a session that has none, so a renamed `windows`
/// must not pass for a session with no windows (which save leaves out).
#[test]
fn a_session_view_whose_windows_moved_fails_save() {
    let env = TestEnv::new();
    let (server, fixture, layout) = server_with_a_saved_split(&env);
    let mut view = server.view(&fixture.id);
    let windows = view.as_object_mut().unwrap().remove("windows").unwrap();
    view["tabs"] = windows;

    let out = save_with_view(&env, &server, &view).assert_failure();
    assert!(out.stderr.contains("windows"), "{out}");

    assert_latest_is_unchanged(&env, &server, &fixture, &layout);
}

/// A tiled layer's `layout` is null when all its blocks floated away, so a
/// renamed `layout` must not pass for a window with nothing tiled.
#[test]
fn a_tiled_layer_whose_layout_moved_fails_save() {
    let env = TestEnv::new();
    let (server, fixture, layout) = server_with_a_saved_split(&env);
    let mut view = server.view(&fixture.id);
    let tiled = tiled_layer(&mut view, 0);
    let tree = tiled.as_object_mut().unwrap().remove("layout").unwrap();
    tiled["tree"] = tree;

    let out = save_with_view(&env, &server, &view).assert_failure();
    assert!(out.stderr.contains("layout"), "{out}");

    assert_latest_is_unchanged(&env, &server, &fixture, &layout);
}

/// Rex always reports a block's foreground process, so a renamed
/// `foreground` must not pass for a block with nothing in its foreground:
/// that would quietly lose its fallback last command, its working directory
/// before its shell reports one, and the no-scrollback check.
#[test]
fn a_process_answer_whose_foreground_moved_fails_save() {
    let env = TestEnv::new();
    let (server, fixture, layout) = server_with_a_saved_split(&env);
    let process = process_with_field_moved(&server, &fixture, "foreground", "fg");

    let out = save_with_process(&env, &server, &process).assert_failure();
    assert!(out.stderr.contains("foreground"), "{out}");

    assert_latest_is_unchanged(&env, &server, &fixture, &layout);
}

/// Rex always reports the user's home directory, so a renamed `home` must
/// not pass for a block with no directory to fall back on.
#[test]
fn a_process_answer_whose_home_moved_fails_save() {
    let env = TestEnv::new();
    let (server, fixture, layout) = server_with_a_saved_split(&env);
    let process = process_with_field_moved(&server, &fixture, "home", "home_dir");

    let out = save_with_process(&env, &server, &process).assert_failure();
    assert!(out.stderr.contains("home"), "{out}");

    assert_latest_is_unchanged(&env, &server, &fixture, &layout);
}
