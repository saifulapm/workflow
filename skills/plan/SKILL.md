---
name: plan
description: Use when route sent a change to the plan lane, to turn it into a task list with dependencies that workflow run can execute.
---

# plan

Work bigger than one plan is a roadmap of milestones: `roadmap`.

## 1. Questions, once

One numbered round, batched at the frontier of what you cannot work out. Each
carries a recommended answer, so silence answers it, for small trade-offs;
a scope question (what the product is, what stays, what goes) is never
resolved by silence. Facts are researched, not asked; ask about intent,
priorities and trade-offs. A question that leaves the session (`mem ask`,
read on a phone) is one decision, the choices, a recommendation, under 100
words.

Read the project's pages before cutting tasks: `mem wiki`, then each page the
change touches; they hold decisions code cannot show. A rewrite inventories
the predecessor's whole surface into pages first; cuts are Saiful's.

## 2. The spec

Prose above the tasks is mandatory: a `## Spec` section and a numbered
`## Rulings` section, since a worker sees only its task block and this prose.

Store it in mem, never the checkout:

    mem plan --stdin < plan.md

Aim for 1,500–3,500 tokens (bytes ÷ 4): under that the tasks are wishes, over
it nobody reads them. Reference file paths, classes, the introducing commit. A
plan cut from the friction queue names the ids it answers, so shipping closes
them. UI work produces a mockup first. Write it plain.

## 3. The grammar

`workflow run` parses this:

    # plan: cart-pricing-v2

    - [ ] t1 Extract cart pricing into a service  [after: t0]
          Files: app/Services/Cart*.php tests/Unit/Cart*
          Read: app/Cart/Total.php
          Uses: Basket::fixture(): Basket
          Gives: CartPricing::price(Basket $b): Cents
          Pattern: app/Services/Shipping.php:40-88
          Verify: bin/php artisan test --filter=Cart
          Done: cart totals identical for the fixture basket

- `# plan: <slug>` opens it, naming the run's branches and worktrees; a task
  line is `- [ ] <id> <title>` with an optional `[after: a, b]` (ids:
  lowercase, up to 16 chars). Continuation lines indent two-plus spaces as
  `Key: value`, split at the first colon-space.
- `Files:` is whitespace-separated globs; double-quote one with a space. `*`
  stops at a slash, `**` crosses, patterns are anchored at the repo root.
  Both it and `Verify:` are mandatory; `workflow verify` is what the gate
  runs. This is the ownership boundary: a write outside them is refused at
  the merge gate, and a task owning more than eight patterns is two tasks.
  The sweep names four classes: the registry or mount file, the lockfile,
  the barrel or index file, the test file Verify runs. Each one missed
  stops the task to ask.
- `Read:`, `Uses:`, `Gives:` and `Pattern:` carry the middle tier: files to
  open before editing (`Read:` may name `wiki:<slug>`, a page of the
  project's wiki), interfaces consumed and produced across task boundaries
  (exact signatures, items joined with ` · `), and one analog to copy the
  shape of. A worker sees its block and the plan's prose, not a sibling's
  block: restate every symbol another task defines, or it hunts.
- An unknown dependency id or a cycle is a hard error.

Check it from the checkout before approval:

    workflow plan-check plan.md    # exit 1 means the plan, not the code

It reads the tree, runs nothing: an unpassable Verify or deferral language
("for now", "TBD") is refused; ungrounded lines warn. A block over budget
is a task to split, never a line to trim.

## 4. Shape

Tasks that run at once must not share files. A wide refactor expands and
contracts: add the new beside the old, move callers, remove it last, each
its own task.

Write `Done:` checkable and demanding, one sentence under forty words a human
can check without the diff: "every caller migrated" forces the sweep that
"callers updated" lets slide. Self-review: every requirement points at a
task, and a name two tasks share is spelled identically in both.

## 5. One approval checkpoint

Present the plan once, whole; after approval `implement` or `workflow run`
takes it.
