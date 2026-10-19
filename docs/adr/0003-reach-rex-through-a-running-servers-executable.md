# Reach Rex through a running server's executable when `rex` isn't on `PATH`

`save` and `restore` run the `rex` on `PATH` when there is one, and otherwise the executable of a `rex server run` the user is running: the one whose `--listen` is the endpoint they call, if any, else any of them. Autosave's job has no `rex` on its `PATH`, so it always takes the second route. Rex has no stable path to its CLI. Build 1203 moved it from inside the app bundle to `RexServer/Versions/1203/`, under `~/Library/Application Support/com.superlogical.rex`, and removed the old directory, so the directory `autosave install` had pinned was gone and every autosave failed. Pinning the new one would only last until the CLI moves again. While there's a server to save, its executable exists, and auto-restore already reaches its server the same way. The endpoint is still passed with `-S`, even when it's rex's default, so the executable only has to be a working `rex`; it never decides which server is called. This settles how #32's autosave failure is fixed.

## Considered Options

- **Pin the directory of the `rex` on `PATH` at install** (what `autosave install` did): broke when Rex 1203 moved its CLI.
- **The newest `RexServer/Versions/*/`**: depends on Rex's private install layout, and right after an update, before the server restarts, picks a CLI newer than the running server.
- **The `rex` bundled in the app** (`Rex Beta.app/Contents/Helpers/Rex Server.rex/...`): its path changed in 1203 too (`Rex Server.app` became `Rex Server.rex`), it names the app, and right after an update it's a newer build than the running server too.
- **Wait for a stable CLI path from Rex**: `RexServer/CLI/` exists but is empty, and autosave fails until Rex fills it.
- **Always a running server's executable, never `PATH`**: one way to reach a server, but the tests that put a stub `rex` first on `PATH` would need another way in, and it would ship in the product.
- **Only the endpoint's own server, checked by asking `server.status` for its pid**, as auto-restore's readiness check does: it only matters when two different Rex builds run at once, which an ordinary Mac doesn't (two app servers would want the same socket).
- **`PATH` first, then a running server's executable** (chosen).

## Consequences

- With no `rex` on `PATH` and no `rex server run` running, there's nothing to save: the command fails as it does when the server isn't running, and logs it.
- Autosave depends on what `ps` shows of a running server: its executable's path and the `--listen` on its command line.
- Only the user's own servers count. Another user's `rex server run` runs an executable that user can replace, and running it would run their code as this user.
- With two Rex builds running at once and the endpoint's server started without `--listen`, autosave may call it through the other build's CLI.
- An agent installed before this pinned a `rex` directory on its `PATH`. One that still exists after a Rex update is used ahead of the lookup, so the README asks users to run `autosave install` once after upgrading rexurrect.
