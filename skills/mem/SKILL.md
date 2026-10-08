---
name: mem
description: Use at the start of a session to read this project's memory, and whenever a decision, a gotcha, a finished task, a handoff or a blocking question is worth keeping.
---

# mem

The store is markdown files on disk, synced between machines. None of it lives
in a project repo or CLAUDE.md.

## Read first

    mem context            # this project's memory, sized for a session start
    mem search "<query>"   # older than the context carries; a page hit is a
                           # wiki:<slug>#<section> row with two lines of text
    mem show <id>          # the item behind a search line
    mem roadmap            # the milestones, ticked as they land
    mem plan <slug>        # a milestone's plan

A hook runs `mem context` at session start; a staleness line at the top
means the sync unit is behind, so say so before trusting it.

## The wiki

A page is the spec's home, the living document for a subsystem; an item is
an episodic fact. Read a page before touching it, rewrite it when it changes.

    mem wiki                  # the pages: slug, bytes, modified, title
    mem wiki <slug>           # one page, byte for byte
    mem wiki <slug> --sections          # its sections: slug, bytes, heading
    mem wiki <slug>#<section>           # one section
    mem wiki <slug> --stdin --note "<what changed and why>" <page.md
    mem wiki <slug>#<section> --stdin --note "<what and why>" <section.md
    mem wiki lint             # oversize, orphan and unindexed pages; exit 1

A section is a `## ` heading and the text under it, its slug the heading
lowercased with hyphens; the text before the first heading is `top`. Read
the section a search hit names, not the page, and rewrite the section you
falsified, not the page. An answer worth keeping becomes a page.

The note is the page's whole history: write it like a commit message. Pages
link as `[name](name.md)` and list in `index`, kept by hand; a section over
2 KB or a page over 8 KB splits, each part linked from index too.

Nothing is deleted; a deletion returns on the next sync. A finished page
becomes a one-line stub pointing at its replacement and leaves the index.

## Write triggers

One command each, none blocking.

**A task finished** → `mem log "<what changed, and why>"`. One line in a commit
message's voice, not a diff summary.

**A decision or a gotcha** → `mem save "<text>" --title "<short>"`. Worth
saving: you would want it in three weeks and it is not in the code or the
git log.

**Deciding instead of asking** → `mem save "<text>" --kind ruling`. When an
answer is not yours to invent but stopping costs more than being wrong:
decide, record it, carry on. A ruling promises it was written down, not that it
was right: it is how Saiful overturns you cheaply.

**Session end with work unfinished** → `mem handoff --set "Next: <the next
action, as a runnable command>. <the state>"`. The digest shows only the
handoff's first hundred characters, so the next action leads.

**A stop condition** → a question, on the right channel. An interactive
session asks in the conversation. A subagent reports the stop to the
orchestrator that sent it. An orchestrator asks Saiful with `mem ask --for
human "<question>" --options "<a>,<b>" --recommend "<a>"`, which reaches the
hub and the phone without waiting, and keeps working on what does not depend
on the answer. Never resolve your own stop condition.

**The workflow itself got in the way** → a lesson on the workflow
project's `lessons` page (`mem --project workflow wiki lessons`): what
happened, the rule, how to apply it. Every orchestrator reads that page.

**Who decided what** → `mem decide "<what and why>" --by saiful|agent`,
with `--replaces "<the old decision>"` when it overturns one.

**Saiful states a rule for all his work, or corrects how you work** → `mem
decide "<the rule in one line>" --by saiful`, with the why and how to apply
it after a blank line, run outside any checkout (`cd "$(mktemp -d)"` first).
Every session's digest opens with these rules. A rule for one project is
written in its checkout instead.

**A change Saiful adopts** (from a review, a ruling or the chat) → a task on
the current plan (`mem plan`), or a roadmap milestone when it is a slice of
its own, ticked by the commit that lands it. The decision keeps the why; the
task makes sure the change ships.

**Proof a task did what it says** → `mem evidence add --task <task> <file>
--note "<what it shows>"`; the file is copied into the store.

**A milestone check turned something up** → `mem finding add --milestone
<slug> --step "<step>" "<what>"`, `--evidence <file>` for a screenshot;
`mem finding close <id> --by <commit>` once fixed.

**A source to keep as it was** → `mem raw add <file|url>`, never replaced.

**What the project is for** → `mem brief --set "<text>"`; `mem brief`
prints it.

**An idea for later** → `mem idea "<text>"`, not a plan task.

Superseding is a write too: `mem save "<new text>" --supersedes <id>`. Two live
items that disagree is this store's failure mode.

## Before the session ends

A session writes before it ends: a `mem log` line for what it changed, a
handoff for what is left, or a `mem decide` for what it settled. Done when
the next session can start from `mem context` without asking what happened.

## What not to write

Secrets, keys, tokens: `mem doctor` greps for them, a floor, not a filter.
Not file contents, not what the code already says, not one item per thought:
a memory that records everything is one nobody reads.

## Waiting

Nobody waits idle. `mem questions` lists what is open, and `mem questions
--wait <id> --timeout 5m` waits for one answer (exit 4 is a timeout). Any
machine answers: `mem answer <id> "<text>"`, or the hub.
