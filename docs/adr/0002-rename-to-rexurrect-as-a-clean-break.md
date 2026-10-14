# Rename to rexurrect as a clean break

The tool is renamed from rex-resurrect to rexurrect with no compatibility code. rexurrect never reads the `REX_RESURRECT_*` settings, never looks in or moves the old state directory (`$XDG_STATE_HOME/rex-resurrect` or `~/.local/state/rex-resurrect`), and never removes the old autosave agent, `io.github.cemcatik.rex-resurrect.autosave`. Its own agent has a different label, so `autosave install` can't replace the old one either. Existing users move over by hand, following the README's "Upgrading from rex-resurrect". The project is at 0.1.0 with no releases, and the few installs are built from source. Compatibility code would keep two names in the code, tests and docs indefinitely, while the guide is a one-time cost per install. This settles how the rename in issue #19 treats existing installs.

## Considered Options

- **Adopt the old state directory on first run**: when the new state directory doesn't exist and the old one does, move it over. This keeps snapshots and the lineage map for users who skip the guide, but rexurrect would have to resolve the old location (and the recorder too, in zsh) for as long as the code exists, with tests for it. It still leaves the old agent, recorder and `init.lua` line running, so users need the guide anyway, and while they run they write into the old directory after it has moved.
- **Adopt the old state directory, and fall back to the old settings**: also read each `REX_RESURRECT_*` setting whose `REXURRECT_*` counterpart is unset. Every setting gets two names in the binary, the recorder, the help, the README and the tests.
- **A clean break** (chosen).

## Consequences

- A user who upgrades without the guide starts with an empty state directory: no snapshots to restore, and no recorded last commands. The snapshots taken before the upgrade stay in the old directory, where rexurrect never looks, so it can't restore them at all. A user who moves `snapshots/` but not `lineage.json` loses the lineage map, so per ADR-0001 the live sessions a restore created look unrelated to every snapshot taken before the upgrade, and restoring one of those recreates them beside the live ones.
- Until the old autosave agent and `init.lua` line are removed, an installed rex-resurrect keeps saving into its own state directory beside rexurrect, and both restore on every server start, so sessions can come back twice. Once its binary is gone, the old agent fails silently every 15 minutes instead. The guide's steps remove both before anything else.
- The README's upgrade section and this ADR are the only places the old name remains on purpose.
