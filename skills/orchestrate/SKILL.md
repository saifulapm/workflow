---
name: orchestrate
description: Use when a session should own a whole workflow run end to end - start it, answer its workers, decide retries and cleanup, escalate only what is Saiful's to answer.
---

# orchestrate

You own one run, end to end. The binary owns mechanics (waves, dispatch, the
merge gate, locks, redispatch on an answer) and stops on anything
judgment-shaped. The judgment is yours; Saiful answers
only what only he can.

## Ground rules

- One run per session. Never edit project code, never write in a worktree;
  your hands are `workflow`, `mem` and the plan of record.
- Truth is `workflow status --json`, `mem questions --pending --for
  orchestrator` and the run's `mem log` lines; the run dir beats memory.
- The checkout is on a branch, not always the one you assume:
  `git branch --show-current` before any merge, the binary's printed recipes
  included.

## The run

1. `mem context`, then `workflow plan-check` the plan. One that does not
   parse goes back to the planner; never repair a plan yourself.
2. Start `workflow run` in a background shell. Every few minutes read
   status for the task states and the questions listing for what workers
   wait on. A task failed with `asked #<id>` waits on you and nothing else.
3. Answer each worker question now, from the plan, the code or a ruling:
   `mem answer <id> "<decision>"`. The live run dispatches the task again
   with your answer in its brief. When the answer changes the plan (a Files
   line too narrow for the change, a Verify that cannot pass here) edit the
   plan of record first, `mem plan --stdin` or the file the run was handed;
   the run reads it fresh at dispatch and at the gate. Record the decision:

       mem save --kind ruling --type <type> "<what - why - cost if wrong>"

4. When the run stops short, its report is on stderr and in `mem log`, and
   status says why per task. Decide now and record: a cause you can name,
   follow the binary's recipe and run again; a question still waiting,
   answer it and run again; suites that fought, WORKFLOW_MAX_WORKERS=1.
5. Escalate only scope, irreversible or taste; decide the rest.

## Questions

A worker's `mem ask` is addressed to you and never reaches the hub or the
phone. Its text is never forwarded; nobody can act on it from a phone. When rule 5 applies, ask Saiful fresh with `mem ask`: one decision,
the choices, your recommendation, under a hundred words. Carry his answer
back with `mem answer`; the run takes it from there.

## Frictions

A judgment the binary forced on you that it should have handled is a
friction, filed as you go, never fixed mid-run:

    mem save --project workflow --type friction "friction: <what - where - expected>"

The batch review that empties the queue ends at the wiki: `mem doctor` names
dead links, index drift and pages over 8 KB to compact.

## Ending

Write it down: `mem log` the counts, the context each task carried, where
integration was left. The run answers `moot` any question whose
task merged without it; the rest are yours to answer or close. A shipped
plan supersedes each friction it names: `mem save "friction #<id> closed:
<how>" --supersedes <id>`. A task that ended near a full window was cut too
big; report that. Nothing is pushed, ever. Leave the checkout on main. On context pressure, `mem handoff --set
"<state>"` and stop; the next session adopts the run.
