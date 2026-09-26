---
name: plan
description: Use when route sent a change to the plan lane, to turn it into a task list with dependencies that workflow run can execute.
---

# plan

Work bigger than one plan is a roadmap of milestones: `roadmap`.

## 1. Questions, once

One numbered round of what you cannot work out. Each carries a recommended
answer, so silence answers small trade-offs; a scope question (what the product
is, what stays, what goes) is never resolved by silence. Facts are researched,
not asked. A question that leaves the session (`mem ask`, read on a phone) is
one decision, the choices, a recommendation, under 100 words.

A ruling that names a crate, a package, an API parameter, a header, a feature
flag or a number the plan will be held to (a package count, a byte budget, a
latency) is a fact, not a preference: fetch the docs this session (the
provider's skill, `npx ctx7@latest`) or measure it on this machine, and write
the date beside it: written from memory, such lines are wrong often enough
that each one costs the session that discovers it.

Read the project's pages before cutting tasks: `mem wiki`, then each page the
change touches; they hold decisions code cannot show. A rewrite inventories the
predecessor's whole surface into pages first; cuts are Saiful's.

## 2. The spec

Prose above the tasks is mandatory: a `## Spec` section and a numbered
`## Rulings` section, since a worker sees only its task block and this prose.

Write it in a scratch dir, never the checkout:

    d=$(mktemp -d)

The prose is what every worker reads before its block: keep the Spec and the
Rulings under 1,500 tokens (bytes ÷ 4), and move what a page can hold to the
wiki. A plan's length is its task count; a block has its own budget. Reference
paths, classes, commits. A plan cut from the friction queue names the ids it
answers, so shipping closes them. UI work produces a mockup first.

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
  line is `- [ ] <id> <title>` with an optional `[after: a, b]` (ids: lowercase,
  up to 16 chars). Continuation lines indent 2+ spaces as `Key: value`, split
  at the first colon-space.
- `Files:` is whitespace-separated globs; double-quote one with a space. `*`
  stops at a slash, `**` crosses, patterns are anchored at the repo root. Both
  it and `Verify:` are mandatory. `Verify:` is the worker's evidence; the gate
  runs the project's whole suite on integration, so a task that changes what
  any test outside its Verify asserts fixes that test in the same task and
  claims it in Files: the suite is green after every task. A Done line about
  what a user sees or touches -- a gesture, a layout, a rendered page -- wants
  a Verify that renders or replays it: a fixture replay, a screenshot compared
  to a stored one, a playwright script; a unit test that cannot reach the
  pixels leaves the defect to the reader. Files is the ownership boundary; a
  task owning more than eight patterns is two tasks. Before closing a Files
  line, sweep the six classes of file a change drags in: the registry or
  mount file, the lockfile, the barrel or index file, the test file Verify
  runs, the arm an exhaustive match demands, the dead-code guard on a
  sibling's symbol this task first calls. One the sweep missed is what stops
  the worker to ask.
- `Read:`, `Uses:`, `Gives:` and `Pattern:` carry the middle tier: files to open
  before editing (`Read:` may name `wiki:<slug>`), interfaces consumed and
  produced across task boundaries (exact signatures, items joined with ` · `),
  and one analog to copy. A worker sees its block and the plan's prose, not a
  sibling's: restate every symbol another task defines, spelled as that task
  Gives it, or it hunts. A type inside a signature is a symbol too: `ctx: &mut
  ToolCtx` obliges some task to Give `ToolCtx { .. }` as an item of its own.
- An unknown dependency id or a cycle is a hard error.

Check it from the checkout:

    workflow plan-check "$d/plan.md"

It reads the tree, runs nothing: an unpassable Verify or deferral language
("for now", "TBD") is refused; ungrounded lines warn, and so do a Uses spelled
unlike its Gives, a type no item defines, a Uses some task Gives with no
`[after:]` between the two, a block that points at a sibling ("as in t1"),
a Read or Pattern path git does not track, a Verify grepping a file Files does
not claim, and prose past its budget. A block over budget is a task to split, never a
line to trim.

## 4. Shape

Tasks that run at once must not share files. A wide refactor expands and
contracts: add the new beside the old, move callers, remove it last, each its
own task. A signature is the same: a task changing one that files outside its
Files call adds the new shape beside the old and leaves the old working, and
the task that moves the last caller removes the old, since the suite is green
after every task.
Several one-line edits of one kind across files are one task, not one per file.
Write `Done:` checkable and demanding, one sentence under forty words a human
can check without the diff: "every caller migrated" forces the sweep that
"callers updated" lets slide. A worker reads its block literally, so a rule
across files or callers says so ("each of the three handlers", "every test that
asserts the old literal"), and a ruling is literal -- a file, a symbol, a
value, what breaks if it is wrong -- never a metaphor. Self-review: every
requirement points at a task, and a name two tasks share is spelled
identically in both.

## 5. One approval checkpoint

Present it once, whole. After approval, and not before, store it as the plan
of record; `implement` takes it from there:

    mem plan --stdin < "$d/plan.md"
