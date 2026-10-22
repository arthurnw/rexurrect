//! `rexurrect agent-hook <agent>`: an agent's hook, recording the
//! conversation the agent has open in its block (see `agent`).
//!
//! Each agent runs it whenever it starts a conversation or switches to
//! another, with JSON on stdin:
//! - Claude Code and Codex: their `SessionStart` hook input, as they pass it
//!   (`hook_event_name`, `session_id`, and Claude Code's `agent_id` in a
//!   subagent). Claude Code fires it on startup, `/resume`, `/clear` and
//!   compaction, so the block always names the conversation on screen;
//! - pi: `{"session_file": "<path>"}` from the rexurrect pi extension
//!   (`integrations/pi/rexurrect.ts`), on startup and on every new, resumed
//!   or forked session. Without `session_file` (a session pi doesn't save),
//!   the block has no conversation.
//!
//! It does nothing outside Rex (no `REX_BLOCK`), and prints nothing on
//! stdout: Claude Code adds a `SessionStart` hook's output to the
//! conversation.

use std::io::Read;
use std::process::ExitCode;

use serde_json::Value;

use crate::agent::{Agent, AgentConversation, AgentFiles};
use crate::error::{Context, Result};
use crate::settings;
use crate::state::StateDir;

/// The variable Rex sets in every block's shell to the block's ID.
const BLOCK_VAR: &str = "REX_BLOCK";

/// The variable Codex sets in the processes it starts to its own session
/// ID. A Codex started by another (a subagent, a tool) inherits it.
const CODEX_THREAD_VAR: &str = "CODEX_THREAD_ID";

/// The most hook input read: far more than any agent sends.
const MAX_INPUT: u64 = 1 << 20;

pub fn run(agent: Agent) -> Result<ExitCode> {
    let Some(block_id) = settings::env_non_empty(BLOCK_VAR) else {
        return Ok(ExitCode::SUCCESS);
    };
    let Ok(block_id) = block_id.into_string() else {
        return Ok(ExitCode::SUCCESS);
    };
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_INPUT)
        .read_to_string(&mut input)
        .with_context(|| "reading the hook's input")?;
    let input: Value = if input.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&input).with_context(|| "reading the hook's input as JSON")?
    };
    let Some(conversation) = reported(agent, &input) else {
        return Ok(ExitCode::SUCCESS);
    };
    let state = StateDir::from_env()?;
    state.make_private()?;
    AgentFiles::new(&state).write(&block_id, conversation.as_ref())?;
    Ok(ExitCode::SUCCESS)
}

/// What the hook input reports for the block: `Some(Some(_))` a
/// conversation, `Some(None)` that the block has none, `None` nothing to
/// record (another hook event, a subagent, an ID that can't be one).
fn reported(agent: Agent, input: &Value) -> Option<Option<AgentConversation>> {
    let text = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    match agent {
        Agent::Pi => {
            Some(text("session_file").and_then(|file| AgentConversation::new(agent, file)))
        }
        Agent::Claude | Agent::Codex => {
            if text("hook_event_name").is_some_and(|event| event != "SessionStart") {
                return None;
            }
            // A Claude Code subagent reports its own session.
            if text("agent_id").is_some() {
                return None;
            }
            let session_id = text("session_id")?;
            if agent == Agent::Codex
                && settings::env_non_empty(CODEX_THREAD_VAR)
                    .is_some_and(|thread| thread.to_str() != Some(session_id))
            {
                return None;
            }
            AgentConversation::new(agent, session_id).map(Some)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_reports_its_session_on_session_start_only() {
        let start =
            json!({"hook_event_name": "SessionStart", "session_id": "abc-1", "source": "clear"});
        assert_eq!(
            reported(Agent::Claude, &start),
            Some(AgentConversation::new(Agent::Claude, "abc-1"))
        );
        let stop = json!({"hook_event_name": "Stop", "session_id": "abc-1"});
        assert_eq!(reported(Agent::Claude, &stop), None);
    }

    #[test]
    fn a_claude_subagent_reports_nothing() {
        let input =
            json!({"hook_event_name": "SessionStart", "session_id": "abc-1", "agent_id": "sub"});
        assert_eq!(reported(Agent::Claude, &input), None);
    }

    #[test]
    fn pi_without_a_session_file_has_no_conversation() {
        assert_eq!(reported(Agent::Pi, &json!({})), Some(None));
        let input = json!({"session_file": "/s/a.jsonl"});
        assert_eq!(
            reported(Agent::Pi, &input),
            Some(AgentConversation::new(Agent::Pi, "/s/a.jsonl"))
        );
    }
}
