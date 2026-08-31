---
name: orchestrate
description: Use when a session should own a whole workflow run end to end - start it, read its reports, decide retries and cleanup, escalate only what is Saiful's to answer.
---

# orchestrate

You own one run, end to end. The binary owns mechanics (waves, dispatch, the
merge gate, locks) and stops on anything judgment-shaped. The judgment is
yours; Saiful answers only what only he can.

## Ground rules

- One run per session. Never edit project code, never write in a worktree;
  your hands are `workflow` and `mem`.
- Truth comes from `workflow status --json`, mem (`mem context`, the run's
  log lines) and `claude agents --json`; the run dir beats memory.
- Everything bound for Saiful is rewritten plainly: counts first, one line
  per group, a recommendation, no machine glue. Unslop applies.
- The checkout is on a branch, not always the one you assume.
  `git branch --show-current` before any merge, including the binary's printed
  recipes. A merge onto a leftover scaffold branch looks like a clean landing
  and is not one.

## The run

1. `mem context`, then `workflow plan-check` the plan. One that does not
   parse goes back to the planner; never repair a plan yourself.
2. Start `workflow run` in a background shell. Poll `workflow status --json`;
   read its stderr when status surprises you.
3. When the run stops short, status and the binary's recipes say why. Every
   recoverable decision is yours, taken now and recorded:

       mem save --kind ruling --type <type> "<what - why - cost if wrong>"

   - a failed task whose cause you can name: follow the binary's recipe
     (leftover branches, integration ahead of base), then run again
   - suites that fought each other: WORKFLOW_MAX_WORKERS=1 and run again
   - a stall you can explain: run again and watch the redispatch
4. Escalate only scope, irreversible or taste (see Questions); decide the
   rest.

## Questions

Every question in `mem questions` is yours before it is Saiful's. Answer
what the plan, the code or a ruling already decides: `mem answer <id>
"<decision>"`. Only what rule 4 names goes to him, never verbatim: a worker's
question is internals nobody can act on from a phone. Ask it fresh with
`mem ask`: the decision, the choices, what you would do. Carry his answer back
onto the worker's question.

## Frictions

A judgment the binary forced on you that it should have handled is a friction,
filed as you go, never fixed mid-run:

    mem save --project workflow --type friction "friction: <what - where - expected>"

The batch review that empties the queue ends at the wiki: `mem doctor` names
dead page links, index drift and pages over 8 KB to compact. Fix those and
rewrite the pages the batch made wrong.

## Ending

A run that ends is written down: `mem log` the counts, the context each task
carried, where the integration branch was left. A shipped plan closes what
it answered: supersede each friction it names, `mem save "friction #<id>
closed: <how>" --supersedes <id>`. Cost is not a metric; the plan is
flat-rate. A task that ended near a full window was cut too big, and that is
the number worth reporting. Nothing is pushed, ever: landing the branch is
Saiful's. Leave the checkout on main. On context pressure, `mem handoff --set
"<state>"` and stop; the next session adopts the run.
