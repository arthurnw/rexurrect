# Close the app session after auto-restore

Once auto-restore has created sessions in a server instance, it closes the app session: the earliest live session that restore didn't create and that's untouched, at most one per server instance. It looks for one until 30 seconds after restore finishes, and closes it with `session.destroy` through `rex api call`, like every other call. The Rex app creates a session of its own when it finds the server with no sessions, and on Rex 1282 it did so even while auto-restore was creating sessions in the same second, so every restart left one untouched session beside the restored ones, to be killed by hand. Rex doesn't say who created a session, so rexurrect recognises the app session only by what it isn't: restore didn't create it, and nothing in it is worth restoring. This reverses #1's out-of-scope "Detecting or closing a session the Rex app creates on its own", and settles how issue #36 closes it.

## Considered Options

- **Leave it alone** (what #1 decided): untouched sessions aren't saved, so app sessions don't pile up across restarts, but every restart leaves one live.
- **Close every untouched session restore didn't create**: a session the user opens by hand while auto-restore looks is indistinguishable from the app's. The app creates one, so a cap of one limits what a wrong guess costs, and the earliest is the better guess: the app's arrives within seconds of the server being ready.
- **Close only when there's exactly one candidate**: safest, but it keeps the app session in exactly the case where it's hard to tell apart.
- **Also close it from `restore` and from save's restore-first** (#15): a by-hand restore usually runs from a shell in the app session, which has to stay, and either can run long after the server started, when an untouched session is more likely the user's own.
- **Look for longer**: on builds before 1282 the app created its session up to 28 seconds after the server was ready, and once almost 4 minutes later, but each time while the server was still empty. The one app session seen after restore had created sessions came in the same second. Looking for longer widens the time in which a session the user just opened gets closed.
- **`rex kill`, or following `rex events`**: `rex kill` was proposed in the belief that `session.destroy` fails unless the caller attached to the session first. On Rex 1282 it doesn't, even while another client is attached and owns the session's block. Both would be a second way of reaching Rex beside `rex api call`, and polling `session.list` once a second is soon enough.
- **Close the app session after auto-restore** (chosen).

## Consequences

- A session the user opens by hand in the 30 seconds after auto-restore finishes can be closed before they've used it, when no app session turned up first. Nothing worth restoring is lost, since it's untouched, but it disappears from the app.
- Untouched isn't one-way: a session that's untouched again before the 30 seconds are up (its shell `cd`ed away and back, say) is closed, and so is one that only looked touched while its shell was starting.
- An app session created after the 30 seconds survives, as before #36. `rexurrect.log` names every session closed, so an app session left live with no line for it means the window is too short.
- There's no setting to keep the app session. Leaving auto-restore out of init.lua is the only way.
- Auto-restore's entry in `rexurrect.log` is written when it stops looking (as soon as it has closed the app session, and at the latest 30 seconds after the restore), rather than as the restore finishes.
