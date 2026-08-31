---
name: implement
description: Use when working through an approved plan's tasks, or any task with a Verify command, to keep each one tested, committed and recorded.
---

# implement

First action of the session:

    mem log "session start"

## The loop, one task at a time

1. Write the failing test. Watch it fail for the right reason.
2. Write the smallest code that passes it.
3. Run `workflow verify` — that is your evidence, and it is authoritative at
   the gate. In a task worktree it runs the task's own `Verify:` command,
   under the machine's suite lock; a raw suite run beside the merge gate's is
   how a timing-sensitive test goes red (friction #DKMYMTDH). A single
   targeted test while iterating is fine — the full suite is not. Exit codes:
   0 green · 1 failed · 2 no verifier · 3 test removal.
4. Commit. Ordinary engineering voice, present tense, says what changed and
   why. Plain words, no puffery; the unslop skill is the standard for any
   longer prose. Stage only the files this task touched; never `git add -A`.
5. `mem log "<what landed>"`.
6. `/clear`, then the next task. (Interactive sessions only.)

If the task changed how a subsystem works, the page that describes it is now
wrong. Read it before you start and rewrite it before you log:

    mem wiki <slug> > page.md
    mem wiki <slug> --stdin --note "<what changed and why>" <page.md

The note is the page's history, so it says what changed, not that something
did. A page nobody corrected is worse than no page: the next session believes
it. New page, new line in `index`.

Stay inside the task's `Files:` patterns. In an orchestrated run, anything
outside them is refused at the merge gate and the task fails.

**A background session works exactly one task, then ends.** A task is sized to
one fresh context window; a session that rolls into the next task is working
from a degraded context and paying for the whole history again. Finish, hand
off, exit — the orchestrator relaunches. This bounds interactive sessions too:
that is what the `/clear` above is.

## Rulings

When you decide something a later session would otherwise re-litigate, record
it as *what — why — cost if wrong*:

    mem save --kind ruling --type <type> "<what - why - cost if wrong>"

Types that other tools read: `no-verifier`, `test-removal`, `verify-optout`,
`lint-exception`. At the end of a session, show what you ruled on:

    mem log --kind ruling --since 8h --json

## Friction

When this workflow itself gets in the way — a gate misfires, an instruction
reads two ways, a verb is missing — record it and carry on:

    mem save --project workflow --type friction "friction: <what bit you - where - expected>"

Never fix the workflow mid-task; it is improved in reviewed batches.

## The five stops

Never resolve these yourself. On any of them: raise the question on your
channel — in the conversation when interactive; a background session uses its
own question tool while the machine is watched (the local hub's
`/api/presence` says so) and `mem ask` when it is not — then
`mem handoff --set "<where you are>"`, and stop.

1. **Irreversible** — fails the git-revert test.
2. **Security-sensitive** — auth, permissions, secrets handling, crypto.
3. **Outside the worktree** — push, publish, deploy, any external write.
4. **The plan is broken** beyond guessing what was meant.
5. **Credentials or secrets** in play.

These bind you and every worker. The orchestrator's local merge onto its own
integration branch is exempt; pushing is forbidden to all of us. With a human
present who has said yes, a push is `WORKFLOW_ALLOW_PUSH=1 git push` — a
visible, deliberate override of the pre-push stub, never a habit.

## Context

`mem handoff --set` at the end of a session and any time context runs short.
Keep sessions short and the model fixed. Anything over about a kilobyte belongs
in a file the next step reads, not in the conversation.

## mem exit codes

4 park · 5 CAS conflict, re-read and retry · 6 accepted but over budget, which
is not a failure · 7 ambiguous id. A filtered read that matches nothing prints
its document with an empty array and exits 1 — test the array, not the code.
