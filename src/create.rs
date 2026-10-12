//! Turning a saved session into a `session.create` request.

use std::path::Path;

use crate::rex::api::{
    CreateBlock, CreateLayout, CreateSession, CreateSplit, CreateWindow, SHELL_FLAVOR,
    TerminalOptions,
};
use crate::scrollback::Replay;
use crate::snapshot::{SavedBlock, SavedLayout, SavedSession, SavedWindow};

/// `saved` as restore creates it. Every block comes back as a plain shell
/// block in its saved directory, or in `home` if that directory no longer
/// exists, with its last command typed at its prompt if it can be typed
/// ([`typed_command`]), and its scrollback printed above that prompt if
/// `replay` can print it. Rex would quietly start the shell in its own home
/// anyway, but rex-resurrect doesn't rely on that.
pub fn as_created(saved: &SavedSession, home: &Path, replay: &Replay) -> SavedSession {
    let mut created = saved.clone();
    for block in created.blocks_mut() {
        if !replay.prints(block) {
            block.scrollback = None;
        }
        block.last_command = typed_command(block).map(str::to_string);
        if !block.working_directory.is_dir() {
            block.working_directory = home.to_path_buf();
        }
    }
    created
}

/// The request that creates `created`, a session [`as_created`] made.
/// `replay` prints each block's scrollback above its new prompt.
pub fn create_request(created: &SavedSession, replay: &Replay) -> CreateSession {
    CreateSession {
        label: created.label.clone(),
        initial_windows: created
            .windows
            .iter()
            .map(|window| create_window(window, replay))
            .collect(),
    }
}

fn create_window(window: &SavedWindow, replay: &Replay) -> CreateWindow {
    CreateWindow {
        window_label: window.label.clone(),
        layout: create_layout(&window.layout, replay),
    }
}

fn create_layout(layout: &SavedLayout, replay: &Replay) -> CreateLayout {
    match layout {
        SavedLayout::Block(block) => CreateLayout::Block(create_block(block, replay)),
        SavedLayout::Split(split) => CreateLayout::Split(CreateSplit {
            direction: split.direction,
            ratio: split.ratio,
            before: Box::new(create_layout(&split.before, replay)),
            after: Box::new(create_layout(&split.after, replay)),
        }),
    }
}

fn create_block(block: &SavedBlock, replay: &Replay) -> CreateBlock {
    let mut options = TerminalOptions {
        cwd: Some(block.working_directory.clone()),
        initial_input: block.last_command.clone(),
        ..TerminalOptions::default()
    };
    replay.apply(block, &mut options);
    CreateBlock {
        flavor: SHELL_FLAVOR.to_string(),
        label: block.label.clone(),
        options,
    }
}

/// The command restore types at a block's prompt: its last command, only if
/// it's a single line without control characters. Rex writes `initial_input`
/// to the terminal as if typed, so a line break would act as Enter and run
/// what came before it (other control characters can be editor keys too).
/// Recorders already ignore such commands; this check doesn't trust them, or
/// the snapshot.
pub fn typed_command(block: &SavedBlock) -> Option<&str> {
    block
        .last_command
        .as_deref()
        // Any control character, tab included: typed, a tab triggers completion.
        .filter(|command| !command.is_empty() && !command.chars().any(char::is_control))
}
