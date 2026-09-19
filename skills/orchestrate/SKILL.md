---
name: orchestrate
description: Use when a session should own a whole workflow run end to end - start it, answer its workers, decide retries and cleanup, escalate only what is Saiful's to answer.
---

# orchestrate

You own one run, end to end. The binary owns mechanics (waves, dispatch, the
gate, locks, redispatch) and stops on anything judgment-shaped.

## Ground rules

- A run is the exception. A milestone is one strong session with
  `implement` unless its wide wave holds three or more tasks that share no
  files, on a machine that carries that many workers (up to five,
  `WORKFLOW_MAX_WORKERS`); then workers on the strongest model
  and `review-model none`, since fix rounds cost more than the defects they
  catch (review-2026-09-15 has the numbers).
- The run's worker, reader and fixer are project keys: set them, never
  look them up (`mem project set model|review-model|fix-model <m>`, or the
  matching `WORKFLOW_*` variable for one run). The name is opaque, a model
  or an amx role file (`.amx/agents/<m>.md`, or `~/.config/amx/agents`);
  `amx new` resolves it, and nothing prints a catalog.
- One run per session: one plan, or one milestone of a `roadmap`.
  Never edit project code or write in a worktree; your hands are
  `workflow`, `mem` and the plan of record.
- Truth is `workflow status --json`, `mem questions --pending --for
  orchestrator` and `mem log`: the run dir beats memory.
- Never sleep on a clock. `workflow wait` blocks until the run needs you
  and its exit code says what for; a `sleep N; workflow status` loop does not.
- `git branch --show-current` before any merge, including the merge recipes
  the binary prints.

## The run

1. `mem context`, then `mem plan` -- a named milestone with an empty record
   is `mem plan --from <slug>` first. `workflow plan-check <(mem plan)`; one
   that does not parse goes back to the planner, never to you.
2. Start `workflow run` in a background shell, then `workflow wait` in
   another (backgrounded, so its exit wakes you); act on what it printed
   and call it again. Exit 2 is a question, 1 a task failed for good, 0
   the end, 4 a merge under `--merges`. Refused for no reader: `mem
   project set review-model none` if the brief says nobody reads, else
   ask Saiful. Refused for a red trunk: fix main first, never the run.
3. Answer each worker question now, from the plan, the code or a ruling:
   `mem answer <id> "<decision>"`; the run redispatches with it in the
   brief, which clips an answer past 600 characters, so the decision and
   the file or symbol it turns on come first. Read the code yourself to
   answer; the run's workers and its reader are the delegation, so no
   subagent is spawned to answer a question or to check a worker's work. The plan of record is mem's: when it changes (a Files line too
   narrow, a Verify that cannot pass here), `mem plan > tmp`, edit, `mem
   plan --stdin < tmp` carries the ticks -- unless the run was started
   with `--plan-file`, only ever a file outside the checkout; edit that
   file, since the run rereads it at dispatch and the gate. Record it:

       mem save --kind ruling --type <type> "<what - why - cost if wrong>"

4. When the run stops short, its report is on stderr and in `mem log`;
   status says why per task. Decide, record, rerun: a cause you can
   name, follow the binary's recipe; a question waiting, answer it; suites
   that fought, WORKFLOW_MAX_WORKERS=1. Failed on a reading: two fix
   rounds go by themselves (the first back into the worker's session,
   the second a fresh one on the fix model), and the third reading may
   block only on an earlier finding or a regression. A task failed on
   its third reading is yours: read `<task>.review.3`, then `workflow
   accept <task>` (lands it as it stands, findings filed as follow-ups)
   or edit the plan and `workflow redispatch <task>`. Never rule a
   `[later]` finding in: it costs a round and is already a follow-up.
   `no verdict`: `<task>.review` is empty; `amx logs <session>` and
   `<task>.review-err` say what happened. A task that consulted the
   advisor left `<task>.advice.<n>` in the run dir and an `advised` line
   in `mem log --type run`; read both before deciding.
5. Escalate only scope, irreversible or taste; decide the rest.

## Questions

A worker's `mem ask` reaches you, not the phone. When rule 5 applies,
ask Saiful fresh with `mem ask`: one decision, the choices, your
recommendation, under 100 words; carry the answer with `mem answer`.

## Frictions

A judgment the binary should have made is a friction, filed as you go, never
fixed mid-run:

    mem save --project workflow --type friction "friction: <what - where - expected>"

The batch review that empties the queue ends with `mem doctor`: pages to
compact.

## Ending

`mem log` the counts, the context each task carried, and where the work is:
an all-merged run fast-forwards the checkout onto integration, or
prints the `git merge --ff-only` that will; run it. Leave the checkout on main.
Open questions are yours to answer or `moot`. A shipped plan supersedes
each friction it names: `mem save "friction #<id> closed: <how>"
--supersedes <id>`. A merged task that changed workflow or mem is followed
by `cargo install --path <crate>` and `workflow doctor --fix`, so the gate,
stubs and roles match the binary. A task that ended near a full window was
cut too big; say so. Nothing is pushed. On context pressure, `mem handoff --set
"<state>"` and stop; the next session adopts the run.
