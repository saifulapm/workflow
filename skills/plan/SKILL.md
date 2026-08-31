---
name: plan
description: Use when route sent a change to the plan lane, to turn it into a task list with dependencies that workflow run can execute.
---

# plan

## 1. Questions, once

One numbered round, batched at the frontier of what you cannot work out. Each
question carries a recommended answer, so silence answers it, for small
trade-offs only. Scope questions (what the product is, what stays, what goes)
are never resolved by silence: park and keep waiting. Facts are researched,
never asked: versions, file layouts, existing names, current behaviour. Ask
about intent, priorities and trade-offs. A rewrite inventories
the predecessor's whole surface (docs, backlog, screenshots) into mem first;
cuts are Saiful's.

Read the project's pages before you cut tasks: `mem wiki`, then each page the
change touches; they hold decisions code cannot show.

## 2. The spec

Write it to mem:

    mem plan --stdin < plan.md

Aim for 900–1,600 tokens (bytes ÷ 4): under that the tasks are wishes, over it
nobody reads them. Reference generously: file paths, existing classes, the
commit that introduced the thing. A plan cut from the friction queue names the
ids it answers, so shipping closes them. UI work produces a mockup first.
Write it plain; the unslop rules apply.

## 3. The grammar

`workflow run` parses exactly this:

    # plan: cart-pricing-v2

    - [ ] t1 Extract cart pricing into a service  [after: t0]
          Files: app/Services/Cart*.php tests/Unit/Cart*
          Read: app/Cart/Total.php
          Uses: Basket::fixture(): Basket
          Gives: CartPricing::price(Basket $b): Cents
          Pattern: app/Services/Shipping.php:40-88
          Verify: bin/php artisan test --filter=Cart
          Done: cart totals identical for the fixture basket

- The first line is `# plan: <slug>`; the slug names the run's branches and
  worktrees. A task line is `- [ ] <id> <title>` with an optional
  `[after: a, b]`. Ids are lowercase, up to 16 characters.
- Continuation lines are indented two or more spaces and read `Key: value`,
  split at the first colon-space.
- `Files:` is whitespace-separated globs; double-quote one that contains a
  space. `*` stops at a slash, `**` crosses, patterns are anchored at the repo
  root. This is the ownership boundary: what a worker writes outside its
  patterns is refused at the merge gate and the task fails. Grep for every name the
  task changes; a file asserting it belongs here too.
- `Files:` and `Verify:` are mandatory. `Verify:` is the worker's evidence
  command; `workflow verify` is what the gate runs.
- `Read:`, `Uses:`, `Gives:` and `Pattern:` carry the middle tier: files to
  open before editing, interfaces consumed and produced across task
  boundaries (exact signatures, items joined with ` · `), and one analog to
  copy the shape of. A worker sees only its own block: restate every symbol
  a neighbouring task defines, or it hunts.
- An unknown dependency id or a cycle is a hard error.

Check it from the project checkout before asking approval:

    workflow plan-check plan.md    # exit 1 means the plan, not the code

It reads the tree, runs nothing: a Verify that cannot pass here is refused,
and so is deferral language ("for now", "TBD", "wired later"). Ungrounded
Files, Read, Pattern and Uses lines are warned about. `workflow run` dispatches real workers, never from
here.

## 4. Shape

Tasks that run at the same time must not touch the same files; the ownership
patterns make that checkable. For a wide refactor, expand and contract: add
the new beside the old, move callers over, remove the old last. Each is a
task; the removal is its own.

Write `Done:` checkable and demanding, a sentence a human can check without
the diff: "every caller migrated" forces the sweep that "callers updated"
lets slide. Then self-review before approval: every requirement points at a
task, and a name two tasks share is spelled identically in both — a
`clear_layers` in t3 that t7 calls `clear_full_layers` ships a plan bug.

## 5. One approval checkpoint

Present the plan once, whole. After approval,
`implement` or `workflow run` takes it from here.
