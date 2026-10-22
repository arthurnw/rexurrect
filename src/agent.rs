//! Agent conversations: the conversation a coding agent (pi, Claude Code,
//! Codex) has open in a shell block, so restore can resume it there.
//!
//! Each agent reports its conversation through a hook that runs
//! `rexurrect agent-hook <agent>` (see `agent_hook`), as herdr's agent
//! integrations do. The hook writes the block's agent file, and save reads
//! it. A save keeps a block's conversation only while that agent is still
//! running in the block's foreground: once it exits, the block is a shell
//! again, and its last command is what comes back.
//!
//! Agent files, one per block:
//! - `agents/<block ID>` in the state directory, named with the block ID
//!   exactly as Rex gives it (the value of `REX_BLOCK` in the block's shell);
//! - its contents: JSON, `{"agent": "pi", "conversation": "<ref>"}`, where
//!   `<ref>` is pi's session file, or Claude Code's or Codex's session ID;
//! - written only by the hook, atomically (a temporary file whose name
//!   starts with `.`, then a rename), so save never races a hook. Save
//!   removes the files of blocks that aren't live, as it does recorder files;
//! - the directory is private (0700): a conversation names what the user
//!   worked on.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::error::{Context, Result};
use crate::rex::api::TerminalProcess;
use crate::state::{self, StateDir};

const DIR: &str = "agents";

/// The longest conversation reference accepted: a session file's path, at
/// most. Anything longer isn't one.
const MAX_CONVERSATION_LEN: usize = 4096;

/// A coding agent whose conversations restore can resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Pi,
    Claude,
    Codex,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Pi, Agent::Claude, Agent::Codex];

    /// The agent by its command name (`pi`, `claude`, `codex`).
    pub fn from_name(name: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|agent| agent.name() == name)
    }

    /// Its command name: what the user runs, and a name its process goes by.
    pub fn name(self) -> &'static str {
        match self {
            Agent::Pi => "pi",
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// The command line that resumes `conversation`. Only the conversation
    /// comes back: the flags the agent was first started with don't.
    fn resume_argv(self, conversation: &str) -> [&str; 3] {
        match self {
            Agent::Pi => ["pi", "--session", conversation],
            Agent::Claude => ["claude", "--resume", conversation],
            Agent::Codex => ["codex", "resume", conversation],
        }
    }
}

/// The conversation an agent has open in a block.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentConversation {
    pub agent: Agent,
    /// pi's session file (an absolute path), or Claude Code's or Codex's
    /// session ID.
    pub conversation: String,
}

impl AgentConversation {
    /// `conversation` for `agent`, if it can be one (see
    /// [`AgentConversation::is_valid`]).
    pub fn new(agent: Agent, conversation: &str) -> Option<AgentConversation> {
        let found = AgentConversation {
            agent,
            conversation: conversation.to_string(),
        };
        found.is_valid().then_some(found)
    }

    /// Whether it can name a conversation: not empty, not too long, no
    /// control characters, not an option (`-…`), and for pi an absolute path.
    /// Checked again on restore: the snapshot isn't trusted to hold only
    /// what a hook wrote.
    fn is_valid(&self) -> bool {
        let c = &self.conversation;
        !c.is_empty()
            && c.len() <= MAX_CONVERSATION_LEN
            && !c.starts_with('-')
            && !c.chars().any(char::is_control)
            && (self.agent != Agent::Pi || Path::new(c).is_absolute())
    }

    /// Whether restore can resume it now: it's valid, and pi's session file
    /// still exists (pi would start a new conversation under a missing one).
    pub fn can_resume(&self) -> bool {
        self.is_valid() && (self.agent != Agent::Pi || Path::new(&self.conversation).is_file())
    }

    /// The command that resumes it, shell-quoted, without the Enter that
    /// runs it.
    pub fn resume_command(&self) -> String {
        self.agent
            .resume_argv(&self.conversation)
            .map(shell_quote)
            .join(" ")
    }

    /// Whether its agent is the program in the foreground of the block whose
    /// terminal `process` method answered `process`, by any name the program
    /// goes by (pi runs as `node`, under the name `pi`).
    pub fn is_running_in(&self, process: &TerminalProcess) -> bool {
        process
            .foreground_program()
            .is_some_and(|program| program.names().any(|name| name == self.agent.name()))
    }
}

/// `value` quoted for a POSIX shell command line: as it is if it's plain,
/// else single-quoted.
fn shell_quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'%' | b'+' | b'='
                )
        });
    if plain {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// Every block's agent file.
pub struct AgentFiles {
    dir: PathBuf,
}

impl AgentFiles {
    pub fn new(state: &StateDir) -> AgentFiles {
        AgentFiles {
            dir: state.join(DIR),
        }
    }

    /// The conversation the block's agent file names, if there's one that
    /// reads as a valid conversation.
    pub fn read(&self, block_id: &str) -> Result<Option<AgentConversation>> {
        let Some(path) = self.file(block_id) else {
            return Ok(None);
        };
        let Some(bytes) = state::read_file(&path)? else {
            return Ok(None);
        };
        Ok(serde_json::from_slice::<AgentConversation>(&bytes)
            .ok()
            .filter(AgentConversation::is_valid))
    }

    /// Records `conversation` as the block's, or, with `None`, that the
    /// block has none. A block ID that isn't a plain file name is ignored.
    pub fn write(&self, block_id: &str, conversation: Option<&AgentConversation>) -> Result<()> {
        let Some(path) = self.file(block_id) else {
            return Ok(());
        };
        match conversation {
            Some(conversation) => {
                state::create_private_dirs(&self.dir)?;
                let json = serde_json::to_vec(conversation).expect("a conversation serializes");
                state::write_atomically(&path, &json, 0o600)
            }
            None => state::remove_if_exists(&path).map(|_| ()),
        }
    }

    /// Removes the files of blocks `is_live` says aren't live, unless they
    /// changed at or after `kept_since` (see `RecorderFiles::retain_live`).
    pub fn retain_live(
        &self,
        is_live: impl Fn(&str) -> bool,
        kept_since: SystemTime,
    ) -> Result<()> {
        for entry in state::read_dir_or_empty(&self.dir)? {
            let name = entry.file_name();
            let Some(block_id) = name.to_str() else {
                continue;
            };
            let Some(path) = self.file(block_id) else {
                continue;
            };
            if is_live(block_id) {
                continue;
            }
            let reading = || format!("reading {}", path.display());
            let metadata = entry.metadata().with_context(reading)?;
            if !metadata.is_file() || metadata.modified().with_context(reading)? >= kept_since {
                continue;
            }
            state::remove_if_exists(&path)?;
        }
        Ok(())
    }

    /// Removes the temporary files hooks crashed before renaming (see
    /// [`state::remove_stale_temp_files`]).
    pub fn remove_stale_temp_files(&self) -> Result<()> {
        state::remove_stale_temp_files(&self.dir, |name| name.starts_with('.'))
    }

    /// The block's agent file. `None` for an ID that isn't a plain file
    /// name, so no ID can reach outside the agents directory.
    fn file(&self, block_id: &str) -> Option<PathBuf> {
        state::is_plain_file_name(block_id).then(|| self.dir.join(block_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(agent: Agent, value: &str) -> Option<AgentConversation> {
        AgentConversation::new(agent, value)
    }

    #[test]
    fn each_agent_resumes_with_its_own_command() {
        for (agent, value, command) in [
            (
                Agent::Pi,
                "/s/2026_abc.jsonl",
                "pi --session /s/2026_abc.jsonl",
            ),
            (
                Agent::Claude,
                "1c33ba69-2b4f",
                "claude --resume 1c33ba69-2b4f",
            ),
            (Agent::Codex, "019a-77", "codex resume 019a-77"),
        ] {
            assert_eq!(
                conversation(agent, value).unwrap().resume_command(),
                command
            );
        }
    }

    #[test]
    fn a_conversation_that_needs_quoting_is_quoted() {
        let c = conversation(Agent::Pi, "/Users/me/my sessions/it's.jsonl").unwrap();
        assert_eq!(
            c.resume_command(),
            r"pi --session '/Users/me/my sessions/it'\''s.jsonl'"
        );
    }

    #[test]
    fn what_cannot_name_a_conversation_is_refused() {
        for (agent, value) in [
            (Agent::Claude, ""),
            (Agent::Claude, "--dangerously-skip-permissions"),
            (Agent::Claude, "abc\rrm -rf ~"),
            (Agent::Codex, "abc\n"),
            (Agent::Pi, "relative/session.jsonl"),
        ] {
            assert!(conversation(agent, value).is_none(), "{agent:?} {value:?}");
        }
        let long = "a".repeat(MAX_CONVERSATION_LEN + 1);
        assert!(conversation(Agent::Claude, &long).is_none());
    }
}
