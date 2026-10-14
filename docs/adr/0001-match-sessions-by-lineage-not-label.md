# Match sessions by lineage, not label

Restore decides whether a saved session is already live by its lineage. rexurrect records which live session it restored from which saved session, and carries that identity forward across any number of restarts. It never compares labels: Rex lets two live sessions share a label, and Rex assigns session IDs itself (`session.create` accepts only a label and initial windows), so a restored session can't keep its old ID. This refines the label check in issue #1.

## Considered Options

- **Match by label** (what #1 first specified, and the tmux habit): skip a saved session if any live session has its label. With two saved sessions sharing a label, restore either drops one or can't tell them apart. Renaming a restored session also makes restore recreate it.
- **Group by label**: treat every saved session sharing a label as one unit. Still breaks on renames, and one live namesake blocks all of them.
- **Match by count**: create as many sessions with a label as are missing. Restore has to guess which namesake is the missing one.
- **Track what restore created** (chosen).

## Consequences

- A session the user creates by hand before running restore belongs to no lineage, so restore creates the saved session next to it even when they share a label. This deliberately drops story 24's "after recreating a session by hand" promise. The user resolves the duplicate if they care.
- Restoring an older snapshot skips every session whose lineage is live. Getting an old version back means killing the live session first.
- Rex has nowhere to store a lineage, so the record lives in rexurrect's own state. If that state is lost, live sessions look unrelated to every snapshot, and restore recreates them all.
