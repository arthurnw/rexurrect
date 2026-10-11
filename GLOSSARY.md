# rex-resurrect

A standalone tool that saves Rex sessions and recreates them after the Rex server stops. Rex's API and UI name things differently; this glossary uses the API's words.

## Language

### Rex

**Server**:
The Rex process that owns every session on a machine. When it stops, all of its sessions are gone.

**Server instance**:
One run of the server, from start to stop. Rex gives each one its own ID; no session outlives the instance it was created in.
_Avoid_: boot, server session, lifetime

**Host**:
Another Rex server the user connects to by label.
_Avoid_: remote server, tailnet machine

**Session**:
A group of windows on a server, and the unit rex-resurrect saves and restores. Its label is only a display name: two sessions can share one, so rex-resurrect tells sessions apart by lineage, never by label.

**Window**:
One tab in a session, holding one layout.
_Avoid_: tab

**Block**:
One leaf of a layout. Today every block is a shell block.
_Avoid_: pane

**Shell block**:
A block running an interactive terminal shell.
_Avoid_: terminal block

**Remote block**:
A block that lives on a host rather than on the local server.

**Layout**:
A window's tree of splits and blocks: how the window's space is divided. Floating layers aren't part of it.

**Split**:
A layout node that divides its space between two child nodes in one direction, at a ratio.

**Label**:
The display name Rex shows for a session, window or block. Labels aren't unique.
_Avoid_: name, title

**Default label**:
The label Rex generates for a window or block nobody labeled: `Window` plus the last 8 hex characters of the window's ID, or `shell` for a shell block. Any other label is user-set. Rex also generates labels for sessions (random word pairs such as `amber-ridge`), but those can't be told apart from user-set ones, so a session never has a default label.

**Working directory**:
The directory a shell block's shell was last in.
_Avoid_: cwd, pwd, last directory

### Saving

**Snapshot**:
A saved description of every live session at one point in time.
_Avoid_: backup, save file

**Latest snapshot**:
The most recently written snapshot, and the one restore uses unless told otherwise.

**Empty server**:
A server with no live sessions worth saving: none at all, or only untouched ones. Saving from one never replaces the latest snapshot.

**Untouched session**:
A session with nothing worth restoring: one window holding one shell block, default window and block labels, its working directory still home, and no last command. Its label doesn't count, so a session that was only renamed is still untouched. Untouched sessions aren't saved.

**Autosave**:
A save that runs on a timer without the user asking.
_Avoid_: scheduled save, periodic save

**Recorder**:
The hook in a shell block's outermost shell that records each command as it starts. Shells nested inside it (`nix develop`, `poetry shell`) don't record.

**Ignored command**:
A command the recorder doesn't record, leaving the previous last command in place: anything on the ignore list, and any multi-line command.
_Avoid_: skip list, skipped command

**Last command**:
The command a restored shell block gets typed at its prompt, never run: the most recent command the block's outermost shell recorded. A restored block starts out with the last command it was restored with and keeps it, across any number of server instances, until its shell records a new one. A block with neither falls back to the program in its foreground.
_Avoid_: last history entry

### Restoring

**Live session**:
A session that exists on the server right now.
_Avoid_: existing session, current session

**Saved session**:
A session as recorded in a snapshot.

**Restored session**:
A live session that restore created from a saved session. It belongs to the saved session's lineage, whatever it's labeled.

**Lineage**:
A session plus every session restore has created from it, across any number of restarts. Only one session of a lineage is ever live.
_Avoid_: version (easily confused with a snapshot)

**Skipped session**:
A saved session that restore doesn't create because a session of its lineage is already live. A live session with the same label but a different lineage doesn't count.

**Unrestored server instance**:
A server instance rex-resurrect hasn't restored into yet. Save restores before saving from one, so a freshly started server never replaces the latest snapshot.

**Auto-restore**:
A restore that runs by itself once per server instance, as soon as the server is ready.
