---
name: mem
description: Use at the start of a session to read this project's memory, and whenever a decision, a gotcha, a finished task, a handoff or a blocking question is worth keeping.
---

# mem

The store is markdown files on disk, synced between machines. None of it lives
in a project repo or CLAUDE.md.

## Read first

    mem context            # this project's memory, sized for a session start
    mem search "<query>"   # older than the context carries
    mem show <id>          # the item behind a search line

A hook runs `mem context` at session start. Read it: a staleness line at the
top means the sync unit is behind, so say so before trusting it.

## The wiki

An item is an episodic fact; a page is the living document for a subsystem.
Read it before you touch that subsystem, rewrite it when you change one.

    mem wiki                  # the pages: slug, bytes, modified, title
    mem wiki <slug>           # one page, byte for byte
    mem wiki <slug> --stdin --note "<what changed and why>" <page.md

The note is the page's whole history, so write it like a commit message. Pages
link as `[name](name.md)` and each has a line in the `index` page you keep by
hand.

Nothing is deleted; a deletion returns on the next sync. A finished page
becomes a one-line stub pointing at its replacement and leaves the index.
`mem doctor` reports dead links, index drift and pages over 8 KB to compact.

## Write triggers

Six, one command each, none of them blocking.

**A task finished** → `mem log "<what changed, and why>"`. One line in a commit
message's voice, not a diff summary.

**A decision or a gotcha** → `mem save "<text>" --title "<short>"`. Worth
saving means you would want it in three weeks and it is not in the code or the
git log: a constraint someone imposed, a version that must not move, an
approach that failed.

**Deciding instead of asking** → `mem save "<text>" --kind ruling`. When an
answer is not yours to invent but stopping costs more than being wrong:
decide, record it, carry on. A ruling promises it was written down, not that it
was right: it is how Saiful overturns you cheaply.

**Session end with work unfinished** → `mem handoff --set "<state and the next
action>"`, the action as a command someone could run.

**A stop condition** → a question, on the right channel. Interactive sessions
ask in the conversation. A background session uses its own question tool while
the machine is watched (the local hub's `/api/presence` says so) and
`mem ask "<question>"` when it is not, which reaches the phone and returns
without waiting. Never resolve your own stop condition.

**The workflow itself got in the way** → `mem save --project workflow
--type friction "friction: <what bit you - where - expected>"`. File it and
move on; never fix the workflow mid-task.

Superseding is a write too: `mem save "<new text>" --supersedes <id>`. Two live
items that disagree is this store's failure mode.

## What not to write

Secrets, keys, tokens: `mem doctor` greps for them, but that is a floor, not a
filter. Not file contents, not what the code already says, not one item per
thought: a memory that records everything is one nobody reads.

## Waiting

Only an orchestrator waits: `mem questions --wait <id> --timeout 5m`, where
exit 4 is a timeout to park on. Any machine answers: `mem answer <id> "<text>"`.
