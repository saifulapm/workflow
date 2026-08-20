---
name: plan
description: Use when route sent a change to the plan lane, to turn it into a task list with dependencies that workflow run can execute.
---

# plan

## 1. Questions, once

One numbered round, batched at the frontier of what you cannot work out for
yourself. Each question carries a recommended answer, so silence is an answer
— for small trade-offs only. Scope questions — what the product is, what
stays, what goes — are never resolved by silence: park and keep waiting.
Facts are researched, never asked: versions, file layouts, existing names,
current behaviour. Ask about intent, priorities and trade-offs — and ask
enough. A rewrite inventories the predecessor's whole surface (docs,
backlog, screenshots) into mem first; cuts are Saiful's, never defaulted.

## 2. The spec

Write it to mem:

    mem plan --stdin < plan.md

Aim for 900–1,600 tokens (bytes ÷ 4). Under that and the tasks are wishes; over
and nobody reads it. Reference generously — file paths, existing classes, the
commit that introduced the thing. UI work produces a mockup before tasks.
Write it plain; the unslop skill's rules apply to specs too.

## 3. The grammar

`workflow run` parses exactly this:

    # plan: cart-pricing-v2

    - [ ] t1 Extract cart pricing into a service  [after: t0]
          Files: app/Services/Cart*.php tests/Unit/Cart*
          Verify: bin/php artisan test --filter=Cart
          Done: cart totals identical for the fixture basket

- The first line is `# plan: <slug>`. The slug is the plan id, and it names the
  branches and worktrees the run creates.
- A task line is `- [ ] <id> <title>` with an optional `[after: a, b]`. Ids are
  lowercase, up to 16 characters.
- Continuation lines are indented two spaces or more and read `Key: value`,
  split at the first colon-space — so `Verify: pnpm run test:unit` is fine.
  Known keys are `Files`, `Verify` and `Done`. A repeated key is an error; an
  unknown one is ignored with a warning.
- `Files:` is whitespace-separated globs; double-quote one that contains a
  space. `*` stops at a slash, `**` crosses, patterns are anchored at the repo
  root. This is the ownership boundary: anything a worker writes outside its
  patterns is refused at the merge gate and the task parks.
- `Files:` and `Verify:` are mandatory here. `Verify:` is the worker's evidence
  command; `workflow verify` is what the gate runs.
- An unknown dependency id or a cycle stops the run before anything is
  dispatched.

Check it parses before you ask for approval:

    workflow run --plan-file plan.md    # exit 2 means the plan, not the code

## 4. Shape

Tasks that can run at the same time should not touch the same files — the
ownership patterns are what make that checkable. For a wide refactor, expand
and contract: add the new thing beside the old one, move callers over, remove
the old one last. Each of those is a task; the last one is its own task.

Keep tasks small enough that `Done:` is a sentence a human can check without
reading the diff.

## 5. One approval checkpoint

Present the plan once, whole. Not a task at a time, not a running commentary.
After approval, `implement` or `workflow run` takes it from here.
