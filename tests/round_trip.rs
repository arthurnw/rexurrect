//! Save, kill, restore: the session comes back with the same windows, splits,
//! ratios, working directories and user-set labels.

mod common;

use std::path::Path;

use common::*;
use serde_json::json;

#[test]
fn restore_recreates_windows_splits_ratios_working_directories_and_labels() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, spaced, c, d, e, f, moved] =
        ["a", "with space", "c", "d", "e", "f", "moved"].map(|name| env.mkdir(name));

    // Window 1: a 70/30 side-by-side split whose left half is stacked 35/65.
    let editor = || {
        hsplit(
            0.7,
            vsplit(0.35, shell(&a).labeled("notes"), shell(&spaced)),
            shell(&c),
        )
    };
    // Window 2: a different tree, so window order shows in the comparison.
    let tools = |last: &Path| vsplit(0.25, shell(&d), hsplit(0.6, shell(&e), shell(last)));
    let fixture = server.create_session(
        "zz-round-trip",
        &[window(editor()).labeled("editor"), window(tools(&f))],
    );
    server.wait_for_pwds(&fixture);

    // The last block moves after it started: save must read where it is now.
    let moved_block = &fixture.windows[1].block_ids[2];
    server.write(
        &fixture.id,
        moved_block,
        &format!("cd '{}'\n", moved.display()),
    );
    server.wait_for_pwd_eq(&fixture.id, moved_block, &moved);

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.session_ids_labelled("zz-round-trip");
    assert_eq!(restored.len(), 1, "exactly one session restored");
    // Paths compare exactly (logical `/tmp/...`), so they must come from the
    // shell's reported directory, not the process's physical one.
    assert_eq!(
        server.window_shapes(&restored[0]),
        vec![editor().expected_shape(), tools(&moved).expected_shape()],
    );

    let labels = labels(&server.view(&restored[0]));
    assert_eq!(
        labels[0].label, "editor",
        "the labeled window keeps its label"
    );
    assert_eq!(
        labels[1].label,
        default_window_label(&labels[1].window_id),
        "the unlabeled window gets a fresh default label"
    );
    assert_ne!(
        labels[1].label,
        default_window_label(&fixture.windows[1].id),
        "not the old default label"
    );
    assert_eq!(
        labels
            .iter()
            .map(|w| w.block_labels.clone())
            .collect::<Vec<_>>(),
        vec![["notes", "shell", "shell"], ["shell", "shell", "shell"]],
        "the labeled block keeps its label, the others get Rex's default"
    );
}

#[test]
fn a_block_that_never_reported_a_directory_restores_in_its_foreground_process_directory() {
    let env = TestEnv::new();
    let server = env.start_server();
    let [a, quiet] = ["a", "quiet"].map(|name| env.mkdir(name));
    // `sleep` instead of a shell: nothing in that block ever reports a `pwd`.
    let sleeper = block(json!({ "cwd": quiet.to_str().unwrap(), "command": ["sleep", "600"] }));
    let layout = || hsplit(0.4, shell(&a), sleeper.clone());
    let fixture = server.create_session("zz-quiet", &[window(layout())]);
    let [talking, silent] = [0, 1].map(|i| fixture.windows[0].block_ids[i].as_str());
    server.wait_for_pwd(&fixture.id, talking);
    assert_eq!(
        server.pwd(&fixture.id, silent),
        None,
        "the sleep block reports no pwd"
    );

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.session_ids_labelled("zz-quiet");
    assert_eq!(restored.len(), 1, "exactly one session restored");
    // The process reports a physical path (`/private/tmp/...`): compare canonically.
    let shapes: Vec<Shape> = server
        .window_shapes(&restored[0])
        .into_iter()
        .map(Shape::canonicalized)
        .collect();
    assert_eq!(shapes, vec![layout().expected_shape().canonicalized()]);
}
