---
name: orchestrate
description: Use when a session should own a whole workflow run end to end - start it, answer its workers, decide retries and cleanup, escalate only what is Saiful's to answer.
---

# orchestrate

You own one run, end to end. The binary owns mechanics (waves, dispatch, the
gate, locks, redispatch) and stops on anything judgment-shaped.

## Ground rules

- One run per session: one plan, or one milestone of a `roadmap`.
  Never edit project code or write in a worktree; your hands are
  `workflow`, `mem` and the plan of record.
- Truth is `workflow status --json`, `mem questions --pending --for
  orchestrator` and `mem log`: the run dir beats memory.
- `git branch --show-current` before any merge, the binary's recipes
  too.

## The run

1. `mem context`, then `workflow plan-check` the plan. One that does not
   parse goes back to the planner, never to you.
2. Start `workflow run` in a background shell; every few minutes read
   status and the questions; a task failed `asked #<id>` waits on you. Refused for no reader: `mem project set review-model none` if the
   brief says nobody reads, else ask Saiful.
3. Answer each worker question now, from the plan, the code or a ruling:
   `mem answer <id> "<decision>"`; the run redispatches with it in the
   brief. When it changes the plan (a Files line too narrow, a Verify that
   cannot pass here) edit the plan of record first (`mem plan > tmp`, edit,
   `mem plan --stdin < tmp`: mem's copy carries the ticks) or the file the
   run was handed; the run rereads it at dispatch and the gate. Record it:

       mem save --kind ruling --type <type> "<what - why - cost if wrong>"

4. When the run stops short, its report is on stderr and in `mem log`;
   status says why per task. Decide, record, then run again: a cause you can
   name, follow the binary's recipe; a question waiting, answer it;
   suites that fought, WORKFLOW_MAX_WORKERS=1; failed on a reading (`the
   reviewer wants fixes first`), read the file it names and `workflow
   redispatch <task>`. A second fix verdict is a task cut too big or a model
   too small, not a third dispatch; overruling a finding is a ruling and a
   plan edit. `no verdict`: `<task>.review-err` says why.
5. Escalate only scope, irreversible or taste; decide the rest.

## Questions

A worker's `mem ask` reaches you, not the phone. When rule 5 applies,
ask Saiful fresh with `mem ask`: one decision, the choices, your
recommendation, under 100 words; carry the answer back with `mem answer`.

## Frictions

A judgment the binary should have made is a friction, filed as you go, never
fixed mid-run:

    mem save --project workflow --type friction "friction: <what - where - expected>"

The batch review that empties the queue ends with `mem doctor`: pages to
compact.

## Ending

`mem log` the counts, the context each task carried, where integration is.
Open questions are yours to answer or `moot`. A shipped plan
supersedes each friction it names: `mem save "friction #<id>
closed: <how>" --supersedes <id>`. A merged task that changed the workflow
or mem crate is followed by `cargo install --path <crate>`: the gate and
the next run use the installed binary. A task that ended near a full window
was cut too big; say so. Nothing is pushed. Leave the checkout on main. On
context pressure, `mem handoff --set "<state>"` and stop; the next session
adopts the run.
