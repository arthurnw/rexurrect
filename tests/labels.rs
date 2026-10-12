//! Default labels are regenerated on restore, never copied.

mod common;

use common::*;

#[test]
fn a_block_whose_label_was_cleared_restores_with_the_default_label() {
    let env = TestEnv::new();
    let server = env.start_server();
    let a = env.mkdir("a");
    let fixture = server.create_session(
        "zz-cleared",
        &[window(hsplit(0.5, shell(&a), shell(&a).labeled("scratch")))],
    );
    server.wait_for_pwds(&fixture);
    let cleared = &fixture.windows[0].block_ids[1];
    // Rex's default for a cleared label differs from a new block's (`shell`).
    server.set_block_label(&fixture.id, cleared, "");
    let before = labels(&server.view(&fixture.id));
    assert_eq!(
        before[0].block_labels[1],
        format!("Block {}", &cleared[cleared.len() - 8..]),
    );

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.session_ids_labelled("zz-cleared");
    assert_eq!(restored.len(), 1, "exactly one session restored");
    assert_eq!(
        labels(&server.view(&restored[0]))[0].block_labels,
        ["shell", "shell"],
        "the stale `Block <hex>` label isn't copied"
    );
}
