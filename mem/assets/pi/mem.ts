// installed by mem
//
// mem writes this file and `mem doctor --fix` puts it back the way it ships, so
// an edit here is an edit that goes. It does for pi what mem's Claude hooks do
// for Claude Code: brief the first prompt of a session, steer every fifth tool
// result with mem's context, hand a compaction summarizer mem's instruction,
// and nudge a session that settled without writing anything down.
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
// batch or a settled turn.
function steer(pi, content: string): void {
  if (!content) return;
  pi.sendMessage({ customType: "mem", content, display: false }, { deliverAs: "steer" });
}

export default function (pi: ExtensionAPI) {
  let firstPrompt = true;
  let batches = 0;
  let compacting = false;

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

  // pi 0.85.1 reads no instructions off this event -- only off ctx.compact --
  // so a threshold compaction is cancelled and rerun through it instead. A
  // manual or overflow compaction, or one already in flight, runs as pi's
  // own default.
  pi.on("session_before_compact", async (event, ctx) => {
    if (event.reason !== "threshold" || compacting) return;
    compacting = true;
    const clear = () => {
      compacting = false;
    };
    const customInstructions = await run(["precompact"]);
    ctx.compact({ customInstructions, onComplete: clear, onError: clear });
    return { cancel: true };
  });

  pi.on("agent_settled", async (_event, ctx) => {
    const id = sessionId(ctx);
    const args = id ? ["session-check", "--session-id", id] : ["session-check"];
    steer(pi, await run(args));
  });
}
