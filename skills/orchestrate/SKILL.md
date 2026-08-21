---
name: orchestrate
description: Use when a session should own a whole workflow run end to end - start it, read its reports, decide retries and cleanup, escalate only what is Saiful's to answer.
---

# orchestrate

You own one run, end to end, and nothing else. The binary owns mechanics —
waves, dispatch, ownership, the merge gate, locks, refusals — and stops,
machine-readably, on anything judgment-shaped. The judgment is yours. Saiful
answers only what only he can.

## Ground rules

- One run per session. Never edit project code; never write in a worktree.
  Your hands are `workflow` and `mem`, nothing else.
- Truth is reconciled from three places: `workflow status --json`, mem
  (`mem context`, the run's own log lines) and `claude agents --json`. The
  run dir's word beats your memory of it.
- Everything bound for Saiful is rewritten plainly: counts first, one line
  per group, a recommendation, no machine glue. The unslop rules apply.
- The checkout is on a branch, and it is not always the one you assume.
  `git branch --show-current` before any merge, including the recipes the
  binary prints, and land on main. A merge onto a leftover scaffold branch
  looks exactly like a clean landing and is not one.

## The run

1. `mem context`, then `workflow plan-check` the plan. One that does not
   parse goes back to the planner; you never repair a plan yourself.
2. Start `workflow run` in a background shell. Poll `workflow status --json`
   at an unhurried cadence; read the run's stderr when status surprises you.
3. When the run stops short, status plus the binary's printed recipes say
   why. Every recoverable decision is yours, taken now and recorded:

       mem save --kind ruling --type <type> "<what - why - cost if wrong>"

   - a parked task whose cause you can name: follow the recipe the binary
     printed (leftover branches, integration ahead of base), then run again
   - suites that fought each other: WORKFLOW_MAX_WORKERS=1 and run again
   - a stall you can explain: run again and watch the redispatch
   - work parked on another machine: `workflow resume` its bundle
4. Escalate only scope, irreversible or taste — one `mem ask`, carrying what
   you would do and why. Everything else you decide.

## Frictions

Every judgment the binary forced on you that it should have handled is a
friction, filed as you go and never fixed mid-run:

    mem save --project workflow "friction: <what bit you - where - expected>"

## Ending

A run that ends is written down: `mem log` the counts, the context each task
carried and where the integration branch was left. Cost is not a metric here
— the plan is flat-rate — so a task that ended near a full window is the
number worth reporting: it says the plan was cut too big. Nothing is pushed,
ever — landing the branch is Saiful's. Leave the checkout on main, whatever
it was on while you worked. On context pressure: `mem handoff --set
"<state>"` and stop; the next session adopts the run with nothing lost.
