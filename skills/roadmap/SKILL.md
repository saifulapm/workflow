---
name: roadmap
description: Use when the work is bigger than one plan, to cut a roadmap of milestones with a full plan for each, and later to pick the next milestone up and run it.
---

# roadmap

A project too big for one plan: a short list of milestones, each a slice of
the product Saiful opens and uses, the first one's plan cut now and every
later one cut when it is picked up. Anything one plan holds stays in the
plan lane.

## 1. The shape

Milestone one is the walking skeleton: every layer the product will have,
in its thinnest form, joined end to end, and the product opens. A Shopify
app installs on the dev store and opens in the admin with its navigation. A
SaaS admin signs in, shows an empty dashboard, and is served from the
machine it will ship from. Nothing horizontal comes before it: a crate, an
ABI, a database layer, an event bus, a UI kit is folded into the first
milestone that needs it and built only as thick as that milestone requires,
since a layer-first roadmap gives Saiful nothing to open for weeks and
nothing to correct until the layers meet.

Every milestone after it is a vertical slice, one thing a user of the
product does, from the screen down to the store, and its line carries a
`Show:` -- the click-path Saiful walks in the running product once it lands:
what is opened, what is done, what appears. A milestone whose Show cannot
be written is a layer, not a milestone. `Done:` stays the tested claim;
`Show:` is what a human checks without a diff.

Four to ten milestones for a v1, never thirty. A milestone is a week of
sessions, and the count is kept low by making each one wide rather than by
leaving anything out. Order them by what kills the biggest unknown next,
core before peripheral.

Inside a milestone the plan is cut foundation, wide, join: one foundation
task (the schema, the shared types, the mount points) the rest wait on;
then up to five tasks that share no Files; then one join task that wires
the pieces into the Show and carries the Verify that renders or replays
it. A chain of four tasks each waiting on the last is a milestone cut
in layers; recut it.

## 2. Cutting it

Everything `plan` says about questions, research and detail holds for the
plan a milestone gets. Read the project's pages first (`mem wiki`) and write
the spec as wiki pages before cutting a milestone -- a page per subsystem
the product touches, so a plan reads one with `Read: wiki:<slug>` instead of
restating it. Two pages that disagree are settled on the wiki first, one
ruling, before either sentence reaches a plan. Then write the roadmap in a
scratch dir, never the checkout: `d=$(mktemp -d)`

    # roadmap: shop

    - [ ] m1-skeleton The app opens
          Show: Saiful installs the app on the dev store, it opens in the admin, the three nav items open empty pages
          Done: install writes a shop row, the three routes render, compose up serves it on the VPS
    - [ ] m2-orders Orders from the desk  [after: m1-skeleton]
          Show: Saiful opens Orders, creates one for a customer, and sees it in the list with its number
          Done: every state transition is tested; a cancelled order releases its stock

A milestone id is the slug of its plan: a-z, 0-9 and dashes, up to 64
characters. `[after:]` orders milestones as it orders tasks. Files and Verify
belong to the tasks inside a milestone, never to the milestone line.

Present the milestone list with its Show lines before cutting any plan, and
stop for Saiful's word on it: every plan cut against a wrong list is a
session lost. Then cut the first open milestone's plan only, `# plan:
m1-skeleton` in m1-skeleton.md beside the roadmap, as if it were the only
plan: a task saying "as in m1" sends a worker hunting through a document it
will never see. Plans for the milestones after it are cut at pickup (§4),
against the tree the earlier ones left and what Saiful said after walking
their Shows; a plan cut for a milestone three away describes a tree that
does not exist and cannot take the change Saiful asks for after the next
demo.

## 3. Checking it, reviewing it, then storing it

    workflow plan-check "$d/roadmap.md"

reads the roadmap and, for each open milestone, the plan at `"$d/<id>.md"`
beside it, in wave order, judging each against the tree plus what the
milestones it waits on write and Give. The first open milestone with no
plan, a plan headed under another slug, and a plan that does not parse each
refuse the check; a later milestone with no plan and a milestone without a
Show line warn.

A green check is the grammar's opinion, not a review. Before presenting,
three readers with one lens each go over the scratch dir, as forks or amx
agents, findings only:

- coverage: every sentence a wiki page tags with a milestone, to the
  milestone that delivers it; a sentence with no milestone is a gap; and
  every line of the plan's Show, to the task that delivers it;
- consistency: one spelling for every symbol, path, key and number in the
  plan; every Uses Given; every file a Done or a ruling edits claimed by
  that task's Files; every ruling a worker needs restated in the plan;
- feasibility: every crate, API parameter and Verify held against the docs
  or this machine, and every ruling read for a reading that produces the
  wrong code.

Fix what they find, check again, then present. After the check passes and
Saiful approves, store them:

    mem roadmap --stdin < "$d/roadmap.md"
    mem plan <slug> --set-file "$d/<slug>.md"    # the first open milestone
    mem plan --list                              # what is stored and waiting

A stored plan's first line has to be `# plan: <slug>` under the slug it is
filed as, so a run can never be pointed at the wrong milestone.

## 4. Running the next one

One milestone per session, and nothing else in that session.

    mem roadmap                # the milestones; the first unticked is next
    mem plan --list            # whether its plan is stored yet

A milestone without a stored plan is cut now, with `plan`: the Show line is
the spine of its spec, the pages are read again, and so are the questions
and friction Saiful filed after walking the last Show (`mem log`, `mem
questions`), since those are the changes this plan takes. The same check
and the three readers as in §3, on this one plan, then `mem plan <slug>
--set-file`. Then:

    mem plan --from <slug>     # make its plan the plan of record
    workflow plan-check <(mem plan)

`--from` is refused while the plan of record still holds an unchecked task,
because that plan is a run in flight; `mem plan --clear` abandons it. Check
the plan again here even when it was cut this session: the check reads the
plan of record itself, because `--from` writes it into mem's store and a
`plan.md` in the checkout is some earlier session's leftover.

Then this session builds it with `implement`, task by task on main: `mem
plan --tick <task>` after each commit, `mem roadmap --tick <slug>` when the
last one lands. One strong session lands milestones faster than a run
spends on one task's fix rounds (review-2026-09-15). Hand a plan to
`orchestrate` only when its wide wave holds three or more tasks that share
no Files and the machine carries that many workers, up to five
(`WORKFLOW_MAX_WORKERS`), on the strongest model with `review-model none`;
a run that merges every task ticks the milestone itself. A milestone a
review sends back goes to unchecked with `mem roadmap --untick <slug>`
while the fixes are made.

The milestone is not done when the last task merges. Walk the Show: start
the product (`run`), do what the line says, screenshot what appears, and
put the walk in the handoff (`mem handoff`) with what to open, so Saiful
walks it next. What Saiful says after walking it is the next plan's input,
never a reason to reopen this one.

## 5. Drift

A plan meets a tree that moved, or a milestone list meets what Saiful saw.
Which way the difference goes is settled by size, not by taste.

- A moved path, a renamed symbol, a Verify that no longer spells the same
  command: the orchestrator edits the plan of record, then records what it
  did and why with `mem save --kind ruling`.
- A change Saiful asks for after walking a Show goes into the next
  milestone's plan as it is cut, or through `route` as a change of its own;
  a landed milestone is not reopened.
- A changed scope or interface -- the milestone wants work nobody planned,
  or the milestones after it no longer make sense -- goes back to planning:
  `mem ask`, one decision with your recommendation, and stop. Re-cutting a
  roadmap is Saiful's, never a session's.

Nothing in a roadmap or a plan names a model. Which model the workers run on
and who reads at the gate are the run's business, per project.
