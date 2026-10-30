# Fill in sessions Rex restored itself

Since Rex 0.1.0-preview.23 (Mac) and Linux server preview.11, the Rex server saves its sessions and brings them back when it starts: windows, splits, labels, and the same session and block IDs, each block a new shell in its saved directory. Restore then found every saved session live under its own lineage and skipped it, so agents weren't resumed and last commands weren't typed.

Restore now fills such a session in instead of skipping it, but only on a server instance it hasn't restored into yet, and only within the startup window (two minutes) after the server started. For each block that came back, it writes what it would have given a block it created: the agent's resume command followed by Enter, or else the last command, typed and not run. Blocks are found by the block ID the snapshot saved for them; in snapshots from before blocks saved their IDs, by position, in windows whose live layout has as many blocks as the saved one.

## Considered Options

- **Close Rex's sessions and create them again**: keeps scrollback replay, but throws away the IDs Rex kept for scripts, and floating panes Rex restores but rexurrect doesn't save.
- **Start the server with `--restore=false`**: the Rex app runs its own embedded server, with no setting for it.
- **Write into the blocks Rex restored** (chosen).

## Consequences

- Filled-in blocks get no scrollback: the replay works by starting the block's shell itself, and Rex already started it. Rex says it will restore scrollback itself.
- Restore writes into blocks it didn't create. The instance and age checks keep it to fresh shells: a later `restore`, or a save's restore after the startup window, skips them as before.
- Only sessions Rex restored are filled in. A saved session Rex didn't bring back (a server started with `--restore=false`, or one whose saved state was discarded) is created as before.
