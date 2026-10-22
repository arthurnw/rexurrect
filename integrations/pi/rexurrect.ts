// rexurrect pi extension: reports the conversation pi has open in a Rex
// block, so `rexurrect restore` resumes it there with `pi --session <file>`.
//
// Install: copy or symlink this file into ~/.pi/agent/extensions/.
// It runs `rexurrect agent-hook pi` (from PATH, or $REXURRECT if set) on
// startup, on every new, resumed or forked session, and whenever pi starts
// working, with {"session_file": "<path>"} on stdin. It does nothing outside
// Rex (no REX_BLOCK) and in pi's headless modes (print, JSON, RPC), which
// subagents run in.

import { execFile } from "node:child_process";
import path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  if (!process.env.REX_BLOCK) {
    return;
  }
  const rexurrect = process.env.REXURRECT || "rexurrect";
  // The session file last reported; null before the first report.
  let reported: string | undefined | null = null;

  const report = (ctx: any) => {
    if (ctx?.mode !== "tui") {
      return;
    }
    let file: unknown;
    try {
      file = ctx.sessionManager?.getSessionFile?.();
    } catch {
      file = undefined;
    }
    const sessionFile = typeof file === "string" && path.isAbsolute(file) ? file : undefined;
    if (sessionFile === reported) {
      return;
    }
    reported = sessionFile;
    try {
      const child = execFile(rexurrect, ["agent-hook", "pi"], { timeout: 5000 }, () => {});
      child.on("error", () => {});
      child.stdin?.on("error", () => {});
      child.stdin?.end(JSON.stringify(sessionFile ? { session_file: sessionFile } : {}));
    } catch {
      // rexurrect isn't installed: nothing to report to.
    }
  };

  pi.on("session_start", (_event, ctx) => report(ctx));
  pi.on("agent_start", (_event, ctx) => report(ctx));
}
