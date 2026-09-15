// installed by mem
//
// mem writes this file and `mem doctor --fix` puts it back the way it ships, so
// an edit here is an edit that goes. It does for pi what mem's Claude hooks do
// for Claude Code: brief the first prompt of a session, steer every fifth tool
// result with mem's context, and nudge a session that settled without writing
// anything down.
//
// The fourth Claude hook, PreCompact, has no counterpart here: in pi 0.85.1 a
// session_before_compact handler can only cancel the compaction or hand back a
// whole summary of its own, and ctx.compact aborts the turn it was called
// from, so there is no way to give the summarizer mem's one instruction
// without stopping the work. The seam stays open until pi grows one.
//
// Every command is fire-and-forget where nothing waits on it, and nothing
// here throws.
// @ts-nocheck
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// Batches between two steers of the brief (mirrors mem's PostToolBatch cadence).
const BATCH_EVERY = 5;

// mem off the PATH, or MEM_BIN when set.
function memBinary(): string {
  const named = process.env.MEM_BIN;
  if (named) return named;
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    if (!dir) continue;
    const candidate = join(dir, "mem");
    if (existsSync(candidate)) return candidate;
  }
  return "mem";
}

// Runs a mem command and hands back its stdout, trimmed. A mem that is not
// there, or exits nonzero, is empty output rather than a throw.
function run(args: string[]): Promise<string> {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(memBinary(), args, { stdio: ["ignore", "pipe", "ignore"] });
    } catch {
      resolve("");
      return;
    }
    let out = "";
    child.on("error", () => resolve(""));
    // Decoding on the stream keeps a UTF-8 sequence split across two chunks
    // whole; an unlistened stream `error` would be rethrown out of an IO
    // callback and take pi down with it.
    child.stdout.setEncoding("utf8");
    child.stdout.on("error", () => {});
    child.stdout.on("data", (chunk) => {
      out += chunk;
    });
    child.on("close", () => resolve(out.trim()));
  });
}

function sessionId(ctx): string | undefined {
  try {
    const id = ctx?.sessionManager?.getSessionId?.();
    return typeof id === "string" && id ? id : undefined;
  } catch {
    return undefined;
  }
}

// A steer that reaches the model the same way, whether it came off a tool
// batch or a settled turn. `triggerTurn` is what carries the settled one: the
// run is over by then, and without it pi appends the message to the branch and
// starts nothing.
function steer(pi, content: string): void {
  if (!content) return;
  pi.sendMessage(
    { customType: "mem", content, display: false },
    { deliverAs: "steer", triggerTurn: true },
  );
}

export default function (pi: ExtensionAPI) {
  let firstPrompt = true;
  let batches = 0;

  pi.on("session_start", () => {
    firstPrompt = true;
    batches = 0;
  });

  pi.on("before_agent_start", async (_event, _ctx) => {
    if (!firstPrompt) return;
    firstPrompt = false;
    const content = await run(["context"]);
    if (!content) return;
    return { message: { customType: "mem", content, display: false } };
  });

  pi.on("tool_result", async (_event, ctx) => {
    batches += 1;
    if (batches % BATCH_EVERY !== 0) return;
    const id = sessionId(ctx);
    if (!id) return;
    steer(pi, await run(["context", "--brief", "--session-id", id]));
  });

  // Without an id there is nothing to check: a bare `session-check` would read
  // MEM_SESSION_ID out of the environment and spend some other session's nudge.
  pi.on("agent_settled", async (_event, ctx) => {
    const id = sessionId(ctx);
    if (!id) return;
    steer(pi, await run(["session-check", "--session-id", id]));
  });
}
