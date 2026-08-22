---
name: mem
description: Use at the start of a session to read this project's memory, and whenever a decision, a gotcha, a finished task, a handoff or a blocking question is worth keeping.
---

# mem

The store is markdown files on disk, synced between machines. None of it lives
in a project repo, and none of it belongs in CLAUDE.md.

## Read first

    mem context            # this project's memory, budgeted for a session start
    mem search "<query>"   # when you need something older than the context
    mem show <id>          # the item behind a search line

A hook runs `mem context` at session start. Read it. If it opens with a
staleness line, the sync unit is behind — say so before trusting what follows.

## Write triggers

Six. Each is one command and none of them blocks.

**A task finished** → `mem log "<what changed, and why>"`. One line, the voice
of a commit message. Not a diff summary.

**A decision or a gotcha** → `mem save "<text>" --title "<short>"`. Worth
saving means you would want it three weeks from now and it is not recoverable
from the code or the git log: a constraint someone imposed, a version that must
not move, an approach that failed and the reason.

**Deciding instead of asking** → `mem save "<text>" --kind ruling`. When the
work needs an answer that is not yours to invent, but stopping costs more than
being wrong: take the decision, record it as a ruling, carry on. A ruling
promises that it was written down, not that it was right — it is what Saiful
reads to overturn you cheaply.

**Session end with work unfinished** → `mem handoff --set "<state and the exact
next action>"`. Write the next action as a command someone could run.

**A stop condition** → a question, on the right channel. Interactive sessions
ask in the conversation. A background session asks with its own question tool
while the machine is watched — `curl -fsS http://127.0.0.1:8787/api/presence`
says `"watching":true` — and with `mem ask "<question>"` when it is not; the
phone is the locked-screen path, and `ask` returns an id without waiting.
Never resolve your own stop condition.

**The workflow itself got in the way** → `mem save --project workflow
--type friction "friction: <what bit you - where - what you expected>"`.
File it and move on;
the workflow is improved in reviewed batches from these, never mid-task.

## Superseding

`mem save "<new text>" --supersedes <id>`, not a second item that contradicts
the first. Two live items that disagree is the failure this store has.

## What not to write

Secrets, keys, tokens — `mem doctor` greps for them, but that is a floor, not a
filter. Not file contents. Not what the code already says. Not one item per
thought: a memory that records everything is one nobody reads.

## Waiting

Only an orchestrator waits: `mem questions --wait <id> --timeout 5m` (exit 4 is
a timeout — park the work). Any machine can answer: `mem answer <id> "<text>"`.
