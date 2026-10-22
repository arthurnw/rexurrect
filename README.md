# rexurrect

Save your Rex sessions and recreate them after the Rex server stops: after a
crash, a beta update that needs a restart, or a reboot. Like tmux-resurrect, for Rex.

rexurrect is a single self-contained binary. It talks to Rex only through the `rex` CLI and
needs nothing else at runtime.

> **Status:** early. `save` and `restore` bring back each session's label, its windows in order,
> each window's split layout and ratios, window and block labels you set, each block's working
> directory, each block's earlier output above its prompt, and each block's last command typed
> at that prompt: the one the zsh recorder recorded or, without one, the program running in
> the block or last seen running there. Autosave runs `save` every 15 minutes, and auto-restore
> runs `restore` when the Rex server starts.

## Install

With a [Rust toolchain](https://rustup.rs) installed:

```sh
cargo install --git https://github.com/cemcatik/rexurrect
```

That builds `rexurrect` and puts it in `~/.cargo/bin`, which rustup adds to your `PATH`.
rexurrect talks to Rex through the `rex` CLI that ships with Rex: the one on your `PATH` (it is
inside Rex blocks), or else the one a running Rex server runs.

To get last commands back, also load the zsh recorder (next section).

### The zsh recorder

[`rexurrect.plugin.zsh`](rexurrect.plugin.zsh) records each command a block's shell
starts as that block's last command. Load it from `~/.zshrc` in either of these ways.

**Plain `source`:**

```sh
mkdir -p ~/.zsh
curl -fsSL -o ~/.zsh/rexurrect.plugin.zsh \
  https://raw.githubusercontent.com/cemcatik/rexurrect/main/rexurrect.plugin.zsh
echo 'source ~/.zsh/rexurrect.plugin.zsh' >> ~/.zshrc
```

**As an oh-my-zsh custom plugin:**

```sh
dir="${ZSH_CUSTOM:-$HOME/.oh-my-zsh/custom}/plugins/rexurrect"
mkdir -p "$dir"
curl -fsSL -o "$dir/rexurrect.plugin.zsh" \
  https://raw.githubusercontent.com/cemcatik/rexurrect/main/rexurrect.plugin.zsh
```

Then add `rexurrect` to `plugins=(...)` in `~/.zshrc`. Cloning this repository into that
directory works too.

New shells pick it up. The recorder:

- does nothing outside Rex (no `REX_BLOCK`), so other terminals and tmux never write anything;
- records only in a block's outermost shell: commands you run in a nested shell (`nix develop`,
  `poetry shell`, a `zsh` started by hand) don't replace the command that started it, and after
  `exec zsh` the shell keeps recording;
- records a command as it starts, so a program still running at save time (`nvim`, a dev
  server, `claude`) counts too;
- ignores multi-line commands, keeping the previous one;
- ignores trivial commands on its ignore list (below), keeping the previous one, so the restored
  prompt shows your last meaningful command;
- tracks the last command itself rather than reading your history, so `SHARE_HISTORY` and
  per-directory history don't confuse it.

#### The ignore list

By default the recorder ignores `ls`, `ll`, `la`, `l`, `cd`, `pushd`, `popd`, `pwd`, `clear`,
`reset`, `exit` and `rexurrect` itself. A command line is ignored when it runs one of these
on its own, with any arguments: `ls -la` and `cd src` are ignored, but `cd src && make` is
recorded. Commands are matched by name as you typed them, so an alias counts under its own
name, or by the last part of the path you typed: `/bin/ls` and `target/debug/rexurrect save`
are ignored too.

The list is the zsh array `REXURRECT_IGNORE`. You don't have to set it. To change it, set it
in `~/.zshrc`:

```sh
# After loading the recorder: add to the defaults.
REXURRECT_IGNORE+=(htop tig)

# Anywhere in ~/.zshrc: replace the defaults (before or after loading the recorder).
REXURRECT_IGNORE=(ls cd clear)

# Ignore nothing but multi-line commands.
REXURRECT_IGNORE=()
```

The recorder fills in the defaults when it loads, unless `REXURRECT_IGNORE` is already set.
So adding to the list only works after loading the recorder; with oh-my-zsh, that means after
`source $ZSH/oh-my-zsh.sh`. The list is read at every command, so a change applies straight
away.

## Usage

```sh
rexurrect save      # snapshot every session on the Rex server
rexurrect restore   # recreate the latest snapshot's sessions that aren't live
rexurrect list      # list the snapshots, newest first
```

- **`save`** records every session: its label, its windows in order, each window's tiled split
  layout (directions and ratios), and each block's working directory and last command. Window
  and block labels are recorded only if you set them; windows and blocks with Rex's default
  labels get fresh default labels on restore, not stale copies like `Window 1a2b3c4d`. Each
  save is a new timestamped snapshot; `latest.json` points at the newest. The newest 10
  snapshots are kept (set `REXURRECT_KEEP` to keep a different number).
  - Untouched sessions are left out: one window holding one shell, with Rex's default window
    and block labels, still in your home directory, and no last command. Sessions that Rex or
    you create and then abandon therefore don't pile up across restarts. A session's own label
    doesn't count, because Rex's generated ones (`amber-ridge`) look like any other, so a
    session you only renamed is still untouched. A restored session keeps its last command, so
    it doesn't turn untouched just because you haven't run anything in it since.
  - If there's nothing to save (no sessions, or only untouched sessions or sessions with no
    windows left), `save` writes nothing, leaves `latest.json` alone and exits 0, so a server
    with nothing on it never replaces your latest snapshot.
  - If Rex's output doesn't have the shape rexurrect expects (Rex is in beta and its API
    can change) or the snapshot fails validation (every session needs a window, every split
    ratio must be between 0 and 1), `save` writes nothing, exits non-zero and says what didn't
    match.
- **`restore`** recreates each saved session from the latest snapshot, unless that session is
  already live, and prints what it `created`, `skipped` and why anything `failed`. One session
  failing doesn't stop the others; `restore` exits non-zero if any failed. Running it twice
  creates no duplicates. Options, which combine:
  - `--snapshot <id>` restores from an older snapshot, using an ID from `list`;
  - `--session <label>` restores only the saved sessions with that label (all of them, if
    several share it), and fails if the snapshot has none;
  - `--dry-run` (`-n`) prints what would be created and skipped, and creates nothing.
- **`list`** shows each snapshot's ID, when it was taken (UTC) and its session labels, newest
  first.

Whatever the snapshot, restore skips a saved session that's still live, even if it has changed
since, so restoring an older snapshot never brings back an older copy of a session you still
have. Kill the live one first if that's what you want.

A block that can't come back as it was still comes back as a shell, so the layout around it
stays intact. A block whose directory no longer exists opens in your home directory. A block
that isn't a shell block, a block on another host, and a window whose blocks have all floated
come back as a plain shell in your home directory. Floating blocks aren't saved.

Each restored block's last command is typed at its prompt but **never run**: press Enter to
resume, edit it, or clear it. Only single-line commands are typed; a block with no last
command gets an empty prompt. A restored block keeps its last command through later saves and
restores until the recorder records something new in it or, without the recorder, until a save
sees a program running in it.

Each restored block also shows its scrollback, the output it held at save time, above the
`Last login:` line and the new prompt, styles included. It wraps to the block's current width
and reflows when you resize the block. A block that was showing a full-screen program (`nvim`,
`less`, `top`, anything on the terminal's alternate screen) gets no scrollback, so you never see
a frozen picture of the program; its command still comes back typed at the prompt.

Some programs draw their interface on the main screen instead, where Rex can't tell it from
earlier output. A block running one of the programs listed in `REXURRECT_NO_SCROLLBACK` gets
no scrollback either. The list holds program names separated by spaces, matched against the name
the program was run by (`claude`) and the name of its executable. It defaults to `claude` (Claude
Code); set it to replace the default, or to an empty string for no programs:

```sh
export REXURRECT_NO_SCROLLBACK="claude aider"
```

Set it where `save` sees it, and run `autosave install` again afterwards, so autosave uses it too.

A block whose scrollback is too large for Rex to hand over (Rex caps one response at 1 MB, which a
few thousand lines of coloured output can fill) is saved without it, and the save carries on: it
comes back in its directory with its command typed, but nothing above the prompt. The first
save to find a block's scrollback too large adds a line to `rexurrect.log` naming its session
and block; later saves don't repeat it while it stays too large. Clearing the block's scrollback
(Cmd-K) lets the next save keep it again.

Without the recorder (you don't use zsh, or skipped that step), or in a block whose shell
started before you loaded it, rexurrect falls back to the program running in the block at
save time: `nvim`, a dev server or a nested shell comes back with its full command line typed at
the prompt. Arguments come from `ps`, which drops their quoting. The save also remembers that
program as the block's last command, so it still comes back after the program exits and the
block sits at a prompt. A block sitting at a prompt that no save has seen a program in comes back
with the command restore typed there, if restore created it, or else an empty prompt. Running
`rexurrect` itself (a `save` you ran there) never counts as a program. Restored blocks fall
back like any other: what restore typed is only their last command until a save sees a program
in them. (This replaces the first design, #6, in which a restored block never fell back, so
without the recorder it kept what restore typed forever.) Whatever the recorder recorded beats
all of this.

Restore recognises sessions by where they came from, not by label: two saved sessions with the
same label both come back, a restored session you rename still counts as live, and a session
you created by hand under a saved label doesn't (see
[ADR-0001](docs/adr/0001-match-sessions-by-lineage-not-label.md)).

For two minutes after a restore (any restore: `restore`, auto-restore, or the one a `save` runs
first) creates a session, a save saves that session's blocks as restore created them: working
directory, last command and scrollback. Their shells may still be starting then: a slow shell
config may still be running, and the scrollback may still be printing, so reading them as they
are would save the startup instead. A block the recorder records a command in during those two
minutes is saved as it is. A session whose new blocks restore couldn't tell apart is saved as it
was in the snapshot it came from. The snapshot's scrollback is kept for those two minutes even if
retention removes the snapshot itself.

After a server restart, the first `save` restores before it saves, unless a `restore` already
ran on that server. A save that runs on a freshly started server (an autosave, say) therefore
can't replace your latest snapshot with a near-empty one. A save on a server already restored
into never restores again, even if you've killed restored sessions since. A `--dry-run` or
`--session` restore doesn't count, because it leaves sessions out: a save after it would replace
your latest snapshot without the other sessions. Restoring a whole older snapshot does count.
Only one restore runs at a time, and a `save` that starts during a restore waits for it to
finish.

A `restore` or auto-restore in which every session it tried to create failed (a server that
refuses them, say) restored nothing, so it doesn't count either, and the next save restores
first. If every session fails in the restore a `save` runs first too, that save saves nothing:
it leaves `latest.json` alone, exits non-zero, and logs which snapshot failed to restore. It
does count the server as restored, though, so the next save goes ahead and saves what's live
instead of failing the same way forever. To try that snapshot again, run the command the
message gives, `rexurrect restore --snapshot <id>`, before the next save (an autosave
within 15 minutes) replaces `latest.json`; the snapshot itself stays in `list` until retention
removes it. If only some sessions fail, the save goes ahead and still exits non-zero.

rexurrect never starts a Rex server. If Rex isn't running, `save` and `restore` fail and
say so.

Every failure of `save` or `restore`, including each session that fails to restore, is also
appended to `rexurrect.log` in the state directory, one line per failure starting with a
UTC timestamp. Successful runs write nothing there, with two exceptions: auto-restore logs every
outcome (see [Auto-restore](#auto-restore)), and save logs each block whose scrollback it
starts saving without because it's too large. Once the log passes 1 MB (an autosave failing
every 15 minutes while Rex is quit gets it there in a few months), it keeps only its newest half.

### Which server

`REX_SERVER` if set (Rex sets it in every block's shell), otherwise the `rex` CLI's default.
Autosave saves the server `autosave install` names (see [Autosave](#autosave)).

rexurrect calls that server through the `rex` on your `PATH`. With no `rex` there, it runs the
`rex` a Rex server of yours runs: the server listening at that endpoint, if its command line
says so, otherwise any (never another user's). It names the endpoint in every call, so
whichever `rex` it runs reaches the same server. Rex moves its CLI when it updates, so this is
how autosave finds one (see
[ADR-0003](docs/adr/0003-reach-rex-through-a-running-servers-executable.md)).

### Where state lives

Snapshots and rexurrect's own bookkeeping live in its state directory:

1. `$REXURRECT_STATE_DIR`, if set;
2. otherwise `$XDG_STATE_HOME/rexurrect`;
3. otherwise `~/.local/state/rexurrect`.

It holds `snapshots/`, `latest.json` and `lineage.json`, the lineage map, which remembers which
live session restore created from which saved one. Deleting `lineage.json` makes the next
`restore` recreate sessions that are already live. `restored-instances` names the server
instances restore has run in (the 32 most recent), and `restore.lock` makes restores take turns.
`starting-sessions.json` holds what restore created each session with, for saves in its first
two minutes.
`recorder/` holds one file per block with the last command the recorder recorded; only
recorders write there. `default-commands.json` holds each block's last command when there's no
recorder file: the last program a save saw running in it, else the command restore typed there.
`too-large-scrollback.json` names the blocks the last save saved without scrollback because it
was too large, so each is logged once rather than at every save. Each save that writes a snapshot then removes the `recorder/` files and `default-commands.json`
entries of blocks that no longer exist, and the `lineage.json` and `starting-sessions.json`
entries of sessions that no longer exist, so the directory doesn't grow forever. A save that
fails or writes nothing removes nothing. `scrollback/` holds each snapshot's block scrollback,
which goes when its snapshot goes (or two minutes after a restore from it, if that's later); a
block with 50,000 lines of output takes about 340 KB per snapshot. `rexurrect.log` is the log of
failures, auto-restore outcomes and blocks saved without scrollback because it was too large.
The recorder finds the state directory the same way, so set `REXURRECT_STATE_DIR` (if you
use it) where both your shells and rexurrect see it.
Autosave and auto-restore run outside your shells, so `autosave install` and
`auto-restore --print-hook` pin the state directory they resolve.

Snapshots, scrollback, recorder files, `default-commands.json` and `starting-sessions.json` hold
commands, and commands can hold secrets, so the state directory is readable by you only: `save` and `restore` make it `0700` if it isn't.
rexurrect writes its own files with mode `0600`, and the recorder keeps its files in a
`0700` directory. Every file except the log is written atomically (a temporary file, then a
rename), so a crash never leaves a partial snapshot or record behind; the log is appended to.
A temporary file that a crash does leave behind is removed by the next save that writes a
snapshot, once it has gone unchanged for an hour.

## Autosave

```sh
rexurrect autosave install                 # save every 15 minutes
rexurrect autosave install --interval 5    # or every 5
rexurrect autosave uninstall
```

`autosave install` puts a launchd user agent in `~/Library/LaunchAgents`
(`io.github.cemcatik.rexurrect.autosave.plist`) and loads it, so `rexurrect save` runs
every 15 minutes, or every `--interval` minutes, from now on and after every login. Running it
again replaces the agent, so there's only ever one. macOS may show a one-time "Background Items
Added" notification.

Autosave works on a Mac nobody is logged in to at the console, too: a headless Mac you reach over
SSH, running a Rex server there. launchd loads agents into the GUI domain (`gui/<uid>`), which
exists only while you're logged in at the console or over Screen Sharing. Without it,
`autosave install` loads the agent into your user domain (`user/<uid>`) instead, which exists
while you have any process running, an SSH login included. Install again after logging in at the
console to move the agent to the GUI domain, which loads it at every later login. launchd
doesn't document reloading agents into the user domain after a restart, so on a headless Mac
check that autosave is loaded after restarting (below), and run `autosave install` again if not.

Autosave is silent: its output is discarded, and a failure (Rex not running, an API error, a
snapshot that fails validation) only adds a line to `rexurrect.log` in the state directory.
While Rex isn't running, each autosave fails and logs it; it never starts a server. On a freshly
started server, autosave restores before it saves, like any `save`.

launchd starts the agent with almost no environment: no `REX_SERVER`, a bare `PATH`, nothing
from your shell config. So `autosave install` writes into the agent what `save` needs, as it
finds it when you run it, and prints the server and state directory it chose:

- **the server**: `REX_SERVER` if set, so run it from a Rex block to save the server that block
  is on. Otherwise the Rex app's server,
  `unix://$HOME/Library/Application Support/rex/server.sock`, even if `XDG_RUNTIME_DIR` would
  point the `rex` CLI elsewhere. Autosave always names that server explicitly, so it can't end
  up on another one;
- **the state directory**, resolved as above, so autosave shares snapshots with your manual
  runs and the recorder;
- `REXURRECT_KEEP` and `REXURRECT_NO_SCROLLBACK`, if set.

Run `autosave install` again after changing any of these, or after moving `rexurrect`.

The agent's `PATH` is only the system directories, so it has no `rex` on it: Rex moves its CLI
when it updates, and a directory pinned at install would be gone after the next update. Each
autosave runs the `rex` a running Rex server runs instead (see [Which server](#which-server)).
While Rex isn't running there's no server to save, and the autosave fails and logs it as above.

**After upgrading rexurrect**, run `autosave install` once. An agent installed by an earlier
rexurrect has the directory of the `rex` that was on your `PATH` back then pinned on its
`PATH`, and keeps using that `rex` for as long as the directory exists, even after Rex has
updated past it.

`autosave uninstall` unloads the agent and removes its plist, then tells you how to remove the
zsh recorder and the auto-restore line in `init.lua`. Your snapshots stay in the state
directory.

To check that autosave works, run `launchctl print gui/$(id -u)/io.github.cemcatik.rexurrect.autosave`
(or `user/$(id -u)/…` without a GUI login; its `last exit code` should be 0 after the first run) and `rexurrect list` (a new snapshot
every interval), and look in `rexurrect.log` for failures.

## Auto-restore

Auto-restore runs `restore` by itself once each time the Rex server starts, so your sessions come
back after a crash, an update or a reboot without you doing anything. It's one line in Rex's
`init.lua`, which is `~/.config/rex/init.lua` unless you've moved it (`rex config check` shows
where yours is).

**To turn it on**, add the line. `auto-restore --print-hook` prints it; run this from a shell
that sees your usual environment (a Rex block, say):

```sh
mkdir -p ~/.config/rex &&
  rexurrect auto-restore --print-hook >> ~/.config/rex/init.lua
```

The line reads

```lua
do local r = rex.exec({ [[/Users/you/.cargo/bin/rexurrect]], "auto-restore" }, { env = { REXURRECT_STATE_DIR = [[/Users/you/.local/state/rexurrect]] } }) if r then io.stderr:write(r.stderr) end end
```

The server runs it in its own environment, not your shell's: the Rex app starts its server with
a bare `PATH` and no `XDG_*` variables. So the line names `rexurrect` by its full path, and
pins the state directory as it was resolved when you printed it (see
[Where state lives](#where-state-lives)), so auto-restore uses the same snapshots as your shells
and their recorder. Print the line again, replacing the old one, after moving `rexurrect` or
changing `REXURRECT_STATE_DIR` or `XDG_STATE_HOME`.

**If your `init.lua` has an `os.execute(...)` line** that runs `rexurrect auto-restore`, the
line `--print-hook` printed before Rex build 1203, replace it with the line it prints now. Rex
1203 made `os.execute` an error, so the server stops running `init.lua` at that line: nothing is
restored, and nothing after it in `init.lua` runs either.

**To turn it off**, delete that line. `autosave uninstall` reminds you to.

The server runs `init.lua` when it starts, and again on every `rex config reload`. The line starts
`rexurrect auto-restore`, which returns at once, because the server can't answer anything
until `init.lua` has finished, and carries on in the background:

- It waits for the server to be ready, then restores the latest snapshot into it the way
  `restore` does. Saved sessions whose lineage is live are skipped, so nothing is duplicated.
- It restores once per server instance. On a config reload, or on a server that `restore` or a
  `save` already restored into (see [Usage](#usage)), it restores nothing, so sessions you killed
  stay gone. Adding the line and running `rex config reload` restores into the running server
  only if nothing has restored into it yet.
- It restores into the server that ran `init.lua`, even one started from inside another server's
  block, whose `REX_SERVER` that server inherits. It reads the server's endpoint from the server
  process's command line, uses it only once the server there reports the right process ID, and
  then names it explicitly in every call. It runs the `rex` that the server itself runs.
- It's silent. Its output is discarded, and each run adds one line to `rexurrect.log` saying
  which server it restored into and what it created and skipped, and the app session it closed
  (next point), that there was nothing to do, or why it gave up, plus a line for each failure.
  A run that created sessions adds its line once it stops looking for the app session, up to
  30 seconds after restoring. If the server isn't ready within 60 seconds, it gives up and
  logs that. Set `REXURRECT_READY_TIMEOUT` (seconds) in the
  server's environment, or in the line's `env`, to wait longer. A failure before it carries on
  in the background (a `REXURRECT_READY_TIMEOUT` that isn't a number, say) goes to the server's
  own log too.
- The Rex app creates a session of its own when it finds the server with no sessions, even
  while auto-restore is creating yours, so every restart would leave this app session, untouched,
  beside them. Once auto-restore has created sessions, it closes that app session: it looks for 30
  seconds after restoring, and closes the first live session it finds that restore didn't
  create and that's untouched (see [Usage](#usage)), the earliest if there are several. It
  closes at most one per server instance, and names it in `rexurrect.log`. A session you open
  yourself in those 30 seconds and haven't used yet looks just like the app's, so if the app's
  doesn't turn up first, yours is closed instead; nothing in it was worth restoring. An app
  session that turns up after the 30 seconds is left alone, and while it's untouched `save`
  never saves it. Only auto-restore closes it: `restore` and the restore a `save` runs first
  never close a session. See
  [ADR-0004](docs/adr/0004-close-the-app-session-after-auto-restore.md).

`rex config check` runs `init.lua` too. There auto-restore says it wasn't started by a Rex server
and does nothing. Any other Rex server that reads the same `init.lua` (one you start by hand with
`rex server run` and no `--config`, say) gets your sessions restored into it too. That doesn't
make your own server look unrestored: rexurrect remembers every recent server instance it
restored into, not just the last.

If auto-restore can't tell which endpoint is the server's (one containing a space that isn't
percent-encoded; Rex's own endpoints are encoded), it gives up and logs that rather than restore
into any other server.

## Upgrading from rex-resurrect

rexurrect used to be called rex-resurrect. The rename is a clean break: rexurrect ignores every
old name. It never reads the `REX_RESURRECT_*` settings, never looks in the old state directory
and never removes the old autosave agent, so it picks up none of your old settings or state by
itself (see [ADR-0002](docs/adr/0002-rename-to-rexurrect-as-a-clean-break.md)). These steps move
an install over without losing your snapshots or lineage map. Do them in order: until the old
agent and `init.lua` line are gone, rex-resurrect keeps saving and restoring beside rexurrect,
from its own state directory, and a server restart can bring your sessions back twice.

1. **Remove the old autosave agent**, while the old binary still exists:

   ```sh
   rex-resurrect autosave uninstall
   ```

   Its last line names the old state directory (`snapshots: they stay in ...`). Don't delete
   it: step 6 moves it. If you've already uninstalled rex-resurrect, remove the agent by hand:

   ```sh
   launchctl bootout gui/$(id -u)/io.github.cemcatik.rex-resurrect.autosave
   rm ~/Library/LaunchAgents/io.github.cemcatik.rex-resurrect.autosave.plist
   ```

2. **Remove the old auto-restore line** from Rex's `init.lua` (`~/.config/rex/init.lua` unless
   you've moved it; `rex config check` shows where). It runs `rex-resurrect` with
   `auto-restore`, like
   `os.execute([[REX_RESURRECT_STATE_DIR='/Users/you/.local/state/rex-resurrect' '/Users/you/.cargo/bin/rex-resurrect' auto-restore]])`.

3. **Remove the old recorder** by deleting the line in `~/.zshrc` that sources
   `rex-resurrect.plugin.zsh`, and the file itself. With oh-my-zsh, take `rex-resurrect` out of
   `plugins=(...)` and delete its directory:

   ```sh
   rm -r "${ZSH_CUSTOM:-$HOME/.oh-my-zsh/custom}/plugins/rex-resurrect"
   ```

   Shells already running keep the old recorder until step 8.

4. **Uninstall the old binary**:

   ```sh
   cargo uninstall rex-resurrect
   ```

5. **Rename your settings** in `~/.zshrc`, and wherever else you set them. A setting left under
   its old name silently stops applying. Values stay the same.

   | rex-resurrect                 | rexurrect                 |
   | ----------------------------- | ------------------------- |
   | `REX_RESURRECT_STATE_DIR`     | `REXURRECT_STATE_DIR`     |
   | `REX_RESURRECT_KEEP`          | `REXURRECT_KEEP`          |
   | `REX_RESURRECT_NO_SCROLLBACK` | `REXURRECT_NO_SCROLLBACK` |
   | `REX_RESURRECT_READY_TIMEOUT` | `REXURRECT_READY_TIMEOUT` |
   | `REX_RESURRECT_IGNORE`        | `REXURRECT_IGNORE`        |

   If your ignore list names `rex-resurrect`, change that entry to `rexurrect`. If you set
   `REX_RESURRECT_READY_TIMEOUT` in the auto-restore line you removed in step 2, you'll add it
   back in step 7. The test-only `REX_RESURRECT_LAUNCH_AGENTS_DIR` and
   `REX_RESURRECT_AUTOSAVE_LABEL` (see [Development](#development)) are renamed the same way.

6. **Move the state directory** to its new default location:

   ```sh
   mv ~/.local/state/rex-resurrect ~/.local/state/rexurrect
   ```

   If `XDG_STATE_HOME` is set to an absolute path, move `$XDG_STATE_HOME/rex-resurrect` to
   `$XDG_STATE_HOME/rexurrect` instead (`rex-resurrect autosave uninstall` printed which).
   Skip this step if you set `REX_RESURRECT_STATE_DIR`: the `REXURRECT_STATE_DIR` you renamed
   it to names the same directory. Move it before step 7: once rexurrect runs, the new
   directory exists, and `mv` would put the old one inside it.

   Everything in it carries over as it is: snapshots, `lineage.json`, recorded last commands and
   the rest. Only the log changes name. rexurrect starts `rexurrect.log` fresh, and you can keep,
   rename or delete the old `rex-resurrect.log`.

   Skip this step and rexurrect starts with an empty state directory. It can't see the snapshots
   you had, so a server restart before rexurrect's first save brings nothing back. Move the whole
   directory: if `snapshots/` moves without `lineage.json`, there's no lineage map, so the live
   sessions a restore created look unrelated to every snapshot you had, and restoring one of
   those recreates them beside the live ones
   ([ADR-0001](docs/adr/0001-match-sessions-by-lineage-not-label.md)).

7. **Set up rexurrect as a new user would**, starting from a new shell (a new Rex block, or
   `exec zsh`), so it sees the settings you renamed in step 5: `autosave install` and
   `auto-restore --print-hook` pin the state directory and settings they find. Then:
   - [install](#install) rexurrect;
   - load [the zsh recorder](#the-zsh-recorder);
   - turn on [autosave](#autosave) with `rexurrect autosave install`;
   - turn on [auto-restore](#auto-restore) by appending `rexurrect auto-restore --print-hook`
     to `init.lua`, adding `REXURRECT_READY_TIMEOUT` to the line if you had it in the old one.

8. **Move running shells onto the new recorder**, since shells that started before step 3 still
   run the old one, which records into the old state directory. Either run `exec zsh` in each
   block's outermost shell, or save and then restart the Rex server, so auto-restore brings your
   sessions back from the moved state with new shells. To save before restarting, run:

   ```sh
   rexurrect save
   ```

   If you moved the state directory in step 6, an old recorder that recorded a command in between
   may have recreated the old one, holding only `recorder/`. Nothing reads it, so it's safe to
   delete once every shell is on the new recorder.

To check the upgrade worked:

```sh
launchctl list | grep cemcatik
ls ~/Library/LaunchAgents | grep cemcatik
rexurrect list
```

Both `launchctl` and `~/Library/LaunchAgents` should show exactly one autosave agent,
`io.github.cemcatik.rexurrect.autosave`, and nothing under `io.github.cemcatik.rex-resurrect`.
`rexurrect list` should show the snapshots you had before the upgrade. If you restarted the Rex
server in step 8, each session should have come back once.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

The integration tests run the built binary against throwaway Rex servers, so they need the
`rex` CLI on `PATH` (or its path in `REX_BIN`). Each test starts its own server under
`/tmp/rxt.*` with its own state directory and a minimal zsh config, and stops it afterwards.
They never touch your real Rex server, sessions or snapshots. The shared harness is in
`tests/common/`.

Autosave's tests never touch the real launchd either: `launchctl` is found on `PATH`, so they
put a stand-in first, and `REXURRECT_LAUNCH_AGENTS_DIR` replaces `~/Library/LaunchAgents`.
To try autosave on the real launchd without touching your own agent, also set
`REXURRECT_AUTOSAVE_LABEL` to a scratch label (for both `install` and `uninstall`).

`REXURRECT_APP_SESSION_WATCH` sets how long auto-restore looks for the app session after
restoring: a whole number of seconds, at least 1, 30 by default. The tests give every
throwaway server 1, so a test waiting for auto-restore's entry in the log doesn't wait half a
minute.

Terms (session, window, block, label, lineage, ...) follow [GLOSSARY.md](GLOSSARY.md).
