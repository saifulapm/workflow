---
name: orchestrate
description: Use when a session should own a whole workflow run end to end - start it, answer its workers, decide retries and cleanup, escalate only what is Saiful's to answer.
---

# orchestrate

You own one run, end to end. The binary owns mechanics (waves, dispatch, the
gate, locks, redispatch) and stops on anything judgment-shaped. The judgment
is yours; Saiful answers only what only he can.

## Ground rules

- One run per session. Never edit project code or write in a worktree; your
  hands are `workflow`, `mem` and the plan of record.
- Truth is `workflow status --json`, `mem questions --pending --for
  orchestrator` and its `mem log` lines; the run dir beats memory.
- `git branch --show-current` before any merge, the binary's recipes
  included: the checkout is not always where you assume.

## The run

1. `mem context`, then `workflow plan-check` the plan. One that does not
   parse goes back to the planner; never repair it yourself.
2. Start `workflow run` in a background shell; every few minutes read
   status and the questions listing. A task failed with `asked #<id>` waits
   on you and nothing else.
3. Answer each worker question now, from the plan, the code or a ruling:
   `mem answer <id> "<decision>"`; the run redispatches with the answer in
   the brief. When the answer changes the plan (a Files line too narrow, a
   Verify that cannot pass here) edit the plan of record first, `mem plan
   --stdin` or the file the run was handed; the run reads it fresh at
   dispatch and at the gate. Record the decision:

       mem save --kind ruling --type <type> "<what - why - cost if wrong>"

4. When the run stops short, its report is on stderr and in `mem log`, and
   status says why per task. Decide now and record: a cause you can name,
   follow the binary's recipe and run again; a question still waiting,
   answer it and run again; suites that fought, WORKFLOW_MAX_WORKERS=1.
   Failed on a reading (`the reviewer wants fixes first`): read the file it
   names, then `workflow redispatch <task>`; the brief carries them. A
   second fix verdict is a task cut too big or a model too small, not a
   third dispatch; overruling a finding is a ruling and a plan edit. `no
   verdict`: `<task>.review-err` says why.
5. Escalate only scope, irreversible or taste; decide the rest.

## Questions

A worker's `mem ask` reaches you, never the hub or the phone, and is never
forwarded. When rule 5 applies, ask Saiful fresh with `mem ask`: one
decision, the choices, your recommendation, under 100 words; carry
his answer back with `mem answer`.

## Frictions

A judgment the binary forced on you that it should have made is a
friction, filed as you go, never fixed mid-run:

    mem save --project workflow --type friction "friction: <what - where - expected>"

The batch review that empties the queue ends with `mem doctor`: dead links,
index drift, pages to compact.

## Ending

`mem log` the counts, the context each task carried, where integration was
left. Questions the run did not answer `moot` are yours to answer or close.
A shipped plan supersedes each friction it names: `mem save "friction #<id>
closed: <how>" --supersedes <id>`. A task that ended near a full window was
cut too big; report that. Nothing is pushed. Leave the checkout on main. On
context pressure, `mem handoff --set "<state>"` and stop; the next session
adopts the run.
