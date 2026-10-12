//! Telling user-set labels from default labels.
//!
//! Rex generates a default label for every window and block nobody labeled.
//! Only user-set labels are saved: a default one is left out so Rex generates
//! a fresh one on restore, instead of a stale `Window 1a2b3c4d` that looks
//! deliberate.

use crate::rex::api::{BlockView, WindowView};

/// The window's label if the user set it; `None` for its default label,
/// `Window ` plus the last 8 hex characters of its ID.
pub fn user_set_window_label(window: &WindowView) -> Option<&str> {
    let default = window
        .label
        .strip_prefix("Window ")
        .is_some_and(|suffix| Some(suffix) == id_suffix(&window.window_id));
    (!default).then_some(window.label.as_str())
}

/// The block's label if the user set it; `None` for either default label:
/// `shell`, which a new shell block gets, or `Block ` plus the last 8 hex
/// characters of its ID, which Rex gives a block whose label was cleared.
pub fn user_set_block_label(block: &BlockView) -> Option<&str> {
    let default = block.label == "shell"
        || block
            .label
            .strip_prefix("Block ")
            .is_some_and(|suffix| Some(suffix) == id_suffix(&block.block_id));
    (!default).then_some(block.label.as_str())
}

/// The last 8 characters of a Rex ID (`window:<uuid>`, `block:<uuid>`): hex,
/// from the UUID's last group. `None` for an ID that can't end that way.
fn id_suffix(id: &str) -> Option<&str> {
    id.get(id.len().checked_sub(8)?..)
}
