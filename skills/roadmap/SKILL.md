---
name: roadmap
description: Use when the work is bigger than one plan, to cut a roadmap of milestones with a full plan for each, and later to pick the next milestone up and run it.
---

# roadmap

A project planned whole: milestones cut in one sitting, each with a complete
plan in the grammar `plan` teaches, picked up one at a time by later
sessions. Anything one plan holds stays in the plan lane.

## 1. Cutting it

Everything `plan` says about questions, research and detail holds for every
milestone plan. The difference is that they are cut together, while the whole
shape is still in front of you. Read the project's pages first (`mem wiki`),
then write the roadmap:

    # roadmap: shop

    - [ ] m1-auth Sign-in and sessions
          Done: users sign in, sessions survive a restart
    - [ ] m2-billing Billing  [after: m1-auth]
          Done: an order is paid before it ships

A milestone id is the slug of its plan: a-z, 0-9 and dashes, up to 64
characters. `[after:]` orders milestones as it orders tasks. Files and Verify
belong to the tasks inside a milestone, never to the milestone line.

Then one plan per milestone, `# plan: m1-auth` in m1-auth.md beside the
roadmap, each cut as if it were the only plan: a task saying "as in m1" sends
a worker hunting through a document it will never see. A milestone may name
files an earlier one creates and symbols an earlier one Gives.

## 2. Checking it, then storing it

    workflow plan-check roadmap.md

reads the roadmap and every milestone plan from `<id>.md` beside it, in wave
order, judging each against the tree plus what the milestones it waits on
write and Give -- so a path a later milestone reads and an earlier one
creates is not a finding. A milestone with no plan, a plan headed under
another slug, and a plan that does not parse each refuse the check.

After the check passes and Saiful approves, store them:

    mem roadmap --stdin < roadmap.md
    mem plan <slug> --set-file <slug>.md    # once per milestone
    mem plan --list                         # what is stored and waiting

A stored plan's first line has to be `# plan: <slug>` under the slug it is
filed as, so a run can never be pointed at the wrong milestone.

## 3. Running the next one

One milestone per session, and nothing else in that session.

    mem roadmap                # the milestones; the first unticked is next
    mem plan --from <slug>     # make its plan the plan of record
    workflow plan-check <(mem plan)

`--from` is refused while the plan of record still holds an unchecked task,
because that plan is a run in flight; `mem plan --clear` abandons it. Check
the plan again here even though it was checked when it was cut: the
milestones before it have landed and the tree has moved under it. The check
reads the plan of record itself, because `--from` writes it into mem's store
and a `plan.md` in the checkout is some earlier session's leftover.

Then `orchestrate` owns the run. A run that merges every task ticks its
milestone off in the roadmap by itself.

## 4. Drift

A plan cut weeks ago meets a tree that moved. Which way the difference goes
is settled by size, not by taste.

- A moved path, a renamed symbol, a Verify that no longer spells the same
  command: the orchestrator edits the plan of record, then records what it
  did and why with `mem save --kind ruling`.
- A changed scope or interface -- the milestone wants work nobody planned,
  or the milestones after it no longer make sense -- goes back to planning:
  `mem ask`, one decision with your recommendation, and stop. Re-cutting a
  roadmap is Saiful's, never a session's.

Nothing in a roadmap or a plan names a model. Which model the workers run on
and who reads at the gate are the run's business, per project.
