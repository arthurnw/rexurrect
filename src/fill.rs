//! Filling in the sessions Rex restored itself (ADR-0006).
//!
//! Rex's server saves its sessions and brings them back when it starts
//! (Rex 0.1.0-preview.23 on Mac, Linux server preview.11): windows, splits,
//! labels, and the same session and block IDs, each block a new shell in its
//! saved directory. Restore then finds every saved session live under its
//! own ID and skips it, so no agent would be resumed and no last command
//! typed.
//!
//! So right after a server instance starts, restore fills in each such
//! block that has a saved counterpart: it runs the agent's resume command,
//! or types the last command, exactly as it would in a block it created.
//! Scrollback isn't printed: the replay works by starting the block's shell
//! itself, and Rex has already started it.

use std::collections::HashSet;

use crate::rex::Rex;
use crate::rex::api::{LayoutView, SessionView};
use crate::snapshot::SavedSession;

/// For each of `saved`'s blocks, in [`SavedSession::blocks`] order, the live
/// block in `view` it came back as, if any.
///
/// A saved block that recorded its ID matches the live block with that ID.
/// In a snapshot saved before blocks recorded their IDs, blocks match by
/// position instead: window by window, in layout order, but only in a
/// window whose live layout has as many blocks as the saved one.
pub fn match_blocks(saved: &SavedSession, view: &SessionView) -> Vec<Option<String>> {
    let live: Vec<Vec<String>> = view
        .windows
        .iter()
        .map(|window| {
            let mut ids = Vec::new();
            if let Some(layout) = window.tiled_layer().and_then(|layer| layer.layout.as_ref()) {
                leaves(layout, &mut ids);
            }
            ids
        })
        .collect();
    if saved.blocks().any(|block| block.block_id.is_some()) {
        let all: HashSet<&String> = live.iter().flatten().collect();
        return saved
            .blocks()
            .map(|block| block.block_id.clone().filter(|id| all.contains(id)))
            .collect();
    }
    let mut matched = Vec::new();
    for (i, window) in saved.windows.iter().enumerate() {
        let count = window.layout.blocks().len();
        match live.get(i) {
            Some(ids) if ids.len() == count => matched.extend(ids.iter().cloned().map(Some)),
            _ => matched.extend(std::iter::repeat_n(None, count)),
        }
    }
    matched
}

fn leaves(layout: &LayoutView, ids: &mut Vec<String>) {
    match layout {
        LayoutView::Block { block_id } => ids.push(block_id.clone()),
        LayoutView::Split(split) => {
            leaves(&split.before, ids);
            leaves(&split.after, ids);
        }
    }
}

/// Fills in the live blocks `block_ids` names (from [`match_blocks`]) as
/// `as_created` (`create::as_created`) has them: an agent's resume command
/// is run, else a last command is typed. Returns how many blocks it wrote
/// to, or the first error, with how many it had written to by then.
pub fn fill(
    rex: &Rex,
    session_id: &str,
    as_created: &SavedSession,
    block_ids: &[Option<String>],
) -> Result<usize, (usize, String)> {
    let mut filled = 0;
    for (block, block_id) in as_created.blocks().zip(block_ids) {
        let Some(block_id) = block_id else { continue };
        let data = match (&block.agent, &block.last_command) {
            // A carriage return, as Enter sends: the shell runs the command.
            (Some(agent), _) => format!("{}\r", agent.resume_command()),
            (None, Some(command)) => command.clone(),
            (None, None) => continue,
        };
        rex.terminal_write(session_id, block_id, &data)
            .map_err(|e| (filled, format!("block {block_id}: {e}")))?;
        filled += 1;
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view(windows: serde_json::Value) -> SessionView {
        serde_json::from_value(json!({
            "session_id": "session:s",
            "label": "s",
            "windows": windows,
        }))
        .unwrap()
    }

    fn tiled(layout: serde_json::Value, blocks: &[&str]) -> serde_json::Value {
        json!({
            "window_id": "window:w",
            "label": "w",
            "layers": [{
                "kind": "tiled",
                "layout": layout,
                "blocks": blocks.iter().map(|id| json!({
                    "block_id": id, "label": "shell",
                    "flavor": "com.superlogical.terminal.shell",
                })).collect::<Vec<_>>(),
            }],
        })
    }

    fn split(before: &str, after: &str) -> serde_json::Value {
        json!({ "split": {
            "direction": "horizontal", "ratio": 0.5,
            "before": { "block_id": before }, "after": { "block_id": after },
        }})
    }

    fn saved(windows: serde_json::Value) -> SavedSession {
        serde_json::from_value(json!({ "lineage": "session:s", "label": "s", "windows": windows }))
            .unwrap()
    }

    fn block(id: Option<&str>) -> serde_json::Value {
        let mut block = json!({ "working_directory": "/tmp" });
        if let Some(id) = id {
            block["block_id"] = json!(id);
        }
        json!({ "block": block })
    }

    fn saved_split(a: Option<&str>, b: Option<&str>) -> serde_json::Value {
        json!({ "layout": { "split": {
            "direction": "horizontal", "ratio": 0.5, "before": block(a), "after": block(b),
        }}})
    }

    #[test]
    fn blocks_with_saved_ids_match_the_live_blocks_with_those_ids() {
        let live = view(json!([tiled(
            split("block:b", "block:a"),
            &["block:a", "block:b"]
        )]));
        let saved = saved(json!([saved_split(Some("block:a"), Some("block:gone"))]));
        assert_eq!(match_blocks(&saved, &live), [Some("block:a".into()), None]);
    }

    #[test]
    fn blocks_without_saved_ids_match_by_position_in_windows_of_the_same_size() {
        let live = view(json!([
            tiled(split("block:a", "block:b"), &["block:a", "block:b"]),
            tiled(json!({ "block_id": "block:c" }), &["block:c"]),
        ]));
        let saved = saved(json!([saved_split(None, None), saved_split(None, None)]));
        assert_eq!(
            match_blocks(&saved, &live),
            [Some("block:a".into()), Some("block:b".into()), None, None]
        );
    }
}
