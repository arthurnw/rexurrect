//! Turning a saved session into a `session.create` request.

use std::collections::HashSet;
use std::path::Path;

use crate::agent::AgentConversation;
use crate::rex::api::{
    CreateBlock, CreateLayout, CreateSession, CreateSplit, CreateWindow, SHELL_FLAVOR,
    TerminalOptions,
};
use crate::scrollback::Replay;
use crate::snapshot::{SavedBlock, SavedLayout, SavedSession, SavedWindow};

/// `saved` as restore creates it. Every block comes back as a plain shell
/// block in its saved directory, or in `home` if that directory no longer
/// exists. A block with an agent conversation restore can resume
/// ([`resumes`]) runs the agent's resume command at its prompt, with no
/// scrollback: the agent draws its interface again. Any other block gets its
/// last command typed at its prompt if it can be typed ([`typed_command`]),
/// and its scrollback printed above that prompt if `replay` can print it.
/// Rex would quietly start the shell in its own home anyway, but rexurrect
/// doesn't rely on that.
///
/// `resuming` holds the conversations this restore has resumed so far, in
/// any session: each is resumed in one block only.
pub fn as_created(
    saved: &SavedSession,
    home: &Path,
    replay: &Replay,
    resuming: &mut HashSet<AgentConversation>,
) -> SavedSession {
    let mut created = saved.clone();
    for block in created.blocks_mut() {
        block.agent = block
            .agent
            .take()
            .filter(|agent| resumes(agent, &block.working_directory, resuming));
        if block.agent.is_some() || !replay.prints(block) {
            block.scrollback = None;
        }
        block.last_command = typed_command(block).map(str::to_string);
        if !block.working_directory.is_dir() {
            block.working_directory = home.to_path_buf();
        }
    }
    created
}

/// Whether restore resumes `agent` in a block saved in `working_directory`:
/// it can be resumed now, the directory still exists (Claude Code finds a
/// conversation by the directory it ran in), and no other block in this
/// restore resumes it (two agents writing one conversation would garble it).
/// Such a block comes back as a shell with its last command typed instead.
fn resumes(
    agent: &AgentConversation,
    working_directory: &Path,
    resuming: &mut HashSet<AgentConversation>,
) -> bool {
    agent.can_resume() && working_directory.is_dir() && resuming.insert(agent.clone())
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
    let initial_input = match &block.agent {
        // A carriage return, as Enter sends: the shell runs the command.
        Some(agent) => Some(format!("{}\r", agent.resume_command())),
        None => block.last_command.clone(),
    };
    let mut options = TerminalOptions {
        cwd: Some(block.working_directory.clone()),
        initial_input,
        ..TerminalOptions::default()
    };
    replay.apply(block, &mut options);
    CreateBlock {
        flavor: SHELL_FLAVOR.to_string(),
        label: block.label.clone(),
        options,
    }
}

/// The command restore types at a block's prompt, without running it, when
/// it resumes no agent conversation there: its last command, only if
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
