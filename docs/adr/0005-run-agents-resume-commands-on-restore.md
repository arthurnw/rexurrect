# Run agents' resume commands on restore

A block that was running pi, Claude Code or Codex at save time comes back running the agent's resume command (`pi --session <file>`, `claude --resume <id>`, `codex resume <id>`), not with its last command typed. This is the first thing restore runs rather than types. Typing `claude --resume <id>` would bring the conversation back one Enter away, but every agent block would need that Enter after every restart, which is what issue #35 asks rexurrect to remove. herdr runs the same commands, and rexurrect follows its rules for when not to: only for an agent still running at save time, never twice for one conversation, not in a directory that's gone. It settles #35's open questions this way.

## Considered Options

- **Type the resume command at the prompt**: keeps "typed, never run", and the agent starts at the block's real size once the user presses Enter. But the user has to visit and press Enter in every agent block.
- **Find the conversation without a hook**: Claude Code writes `~/.claude/sessions/<pid>.json`, but it's undocumented, and pi and Codex have nothing like it. A command line (`codex resume <id>`) names the conversation only if it was started that way.
- **Report through hooks and run the resume command** (chosen).

## Consequences

- Auto-restore starts agents before the Rex app attaches, at whatever size Rex gives a new block, so they redraw once the app shows them.
- A hook has to be installed per agent. Without one, an agent block comes back as before: its last command typed.
- Only the conversation comes back. Flags the agent was first started with don't (`claude --model opus` comes back as `claude --resume <id>`).
- A resumed block gets no scrollback.
