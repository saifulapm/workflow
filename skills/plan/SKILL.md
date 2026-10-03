---
name: plan
description: Use when the spec is settled or route sent a change to the plan lane, to cut the roadmap and every milestone plan, check them, and ask for the one approval.
---

# plan

One sitting cuts the roadmap and every milestone's plan, checks them, and
asks Saiful once. Work in a scratch dir, never the checkout: `d=$(mktemp -d)`.

## 1. Read

Read `spec` a section at a time (`mem wiki spec --sections`, then `mem wiki
spec#<section>`), then `verify`, `research-resources` and the repo. A line
that names a crate, an API parameter, a header or a number the plan is held
to is a fact: fetch the docs this session (`npx ctx7@latest`) or measure it
here, and write the date beside it. Done when every section is read and
every such fact carries its date.

## 2. The roadmap

Milestone one is the walking skeleton: every layer the product will have, in
its thinnest form, joined end to end, and the product opens. A crate, a
database layer, a UI kit is folded into the first milestone that needs it,
built only as thick as that milestone requires.

Every later milestone is a vertical slice, one thing a user does, from the
screen down to the store. Each carries `Show:`, the click-path Saiful walks
in the running product; `Done:`, the tested claim; and `Surface:` (web,
mobile, cli, emacs or lib), which tells the engine how to drive it. A
milestone whose Show cannot be written is a layer. Four to ten milestones
for a v1, each wide; order them by what kills the biggest unknown next.

    # roadmap: shop

    - [ ] m1-skeleton The app opens
          Show: Saiful installs the app on the dev store, it opens in the admin, the three nav items open empty pages
          Done: install writes a shop row, the three routes render, compose up serves it on the VPS
          Surface: web
    - [ ] m2-orders Orders from the desk  [after: m1-skeleton]
          Show: Saiful opens Orders, creates one for a customer, and sees it in the list with its number
          Done: every state transition is tested; a cancelled order releases its stock
          Surface: web

A milestone id is its plan's slug: a-z, 0-9 and dashes, up to 64 characters.
Done when every milestone has Show, Done and Surface lines and every spec
section names the milestone that delivers it.

## 3. Every milestone plan

Each milestone gets `"$d/<id>.md"` headed `# plan: <id>`, cut as if it were
the only plan: a task saying "as in m1" sends a worker hunting. The prose
is a `## Spec` section and a
`## Decisions` section of sentences, each with its reason, never numbered:
a number is a thing to cite, and the reason is what the worker writes down
instead. Keep the two under 1,500 tokens (bytes / 4) and point at the spec
with `Read: wiki:spec#<section>` rather than restating it.

Cut it foundation, wide, join: one foundation task the rest wait on, up to
five tasks that share no Files, one join task that wires them into the Show
and carries the Verify that renders or replays it. Every task block is under
2,000 bytes with a Verify that proves its Done; a task that changes a surface
carries `Show:`, the evidence the worker captures; `Effort: xhigh` marks the
few hard tasks. Done when every step of the milestone's Show points at the
task that delivers it.

## 4. Check

    workflow plan-check "$d/roadmap.md"

reads the roadmap and every plan beside it, in wave order. Fix what it
refuses and what it warns, then check again. Done when it refuses nothing
and every warning left is one you can say why it stands.

## 5. Store and ask

    mem roadmap --set-file "$d/roadmap.md"
    mem plan <slug> --set-file "$d/<slug>.md"    # each milestone
    mem plan <slug> --status draft

Write `plan-summary` with `mem wiki plan-summary --stdin --note`: the
milestones in one screen with their Show paths, and what each plan assumes.
Then ask once:

    mem ask "Approve roadmap <slug>?" --options approve,changes --recommend approve

and wait with `mem questions --wait <id>`. On changes, revise, check, store
and ask again. Done when the answer is approve and `mem roadmap --status
approved` is recorded.

## The grammar

`workflow run` parses this:

    - [ ] t1 Extract cart pricing into a service  [after: t0]
          Files: app/Services/Cart*.php tests/Unit/Cart*
          Read: app/Cart/Total.php wiki:spec#pricing
          Uses: Basket::fixture(): Basket
          Gives: CartPricing::price(Basket $b): Cents
          Pattern: app/Services/Shipping.php:40-88
          Verify: bin/php artisan test --filter=Cart
          Done: cart totals identical for the fixture basket

- A task line is `- [ ] <id> <title>` with an optional `[after: a, b]` (ids:
  lowercase, up to 16 chars). Continuation lines indent 2+ spaces as `Key:
  value`, split at the first colon-space.
- `Files:` is whitespace-separated globs; double-quote one with a space. `*`
  stops at a slash, `**` crosses, patterns are anchored at the repo root. Both
  it and `Verify:` are mandatory. `Verify:` is the worker's evidence; the gate
  runs the project's whole suite on integration, so a task that changes what
  any test outside its Verify asserts fixes that test in the same task and
  claims it in Files: the suite is green after every task. A Done about what
  a user sees or touches wants a Verify that renders or replays it: a fixture
  replay, a stored screenshot compared, a playwright script. Files is the
  ownership boundary; a task owning more than eight patterns is two tasks.
  Before closing a Files line, sweep the six classes of file a change drags
  in: the registry or mount file, the lockfile, the barrel or index file, the
  test file Verify runs, the arm an exhaustive match demands, the dead-code
  guard on a sibling's symbol this task first calls.
- `Read:`, `Uses:`, `Gives:` and `Pattern:` carry the middle tier: files to
  open before editing (`Read:` may name `wiki:<slug>` or `wiki:<slug>#<section>`),
  interfaces consumed and produced across task boundaries (exact signatures,
  items joined with ` · `), and one analog to copy. A worker sees its block
  and the plan's prose, not a sibling's: restate every symbol another task
  defines, spelled as that task Gives it. A type inside a signature is a
  symbol too: `ctx: &mut ToolCtx` obliges some task to Give `ToolCtx { .. }`.
- An unknown dependency id or a cycle is a hard error.

plan-check reads the tree and runs nothing. An unpassable Verify, deferral
language ("for now", "TBD") and a numbered Decisions list are refused;
ungrounded lines warn, and so do a Uses spelled unlike its Gives, a type no
item defines, a Uses some
task Gives with no `[after:]` between the two, a block that points at a
sibling, a Read or Pattern path git does not track, a Verify grepping a file
Files does not claim, and prose past its budget. A block over budget is a
task to split, never a line to trim.

## Shape

Tasks that run at once share no files. A wide refactor expands and
contracts: add the new beside the old, move callers, remove the old last,
each its own task. A signature is the same: a task changing one that files
outside its Files call adds the new shape beside the old, and the task that
moves the last caller removes the old.
Several one-line edits of one kind across files are one task, not one per file.
Write `Done:` checkable and demanding, one sentence under forty words a human
can check without the diff: "every caller migrated" forces the sweep that
"callers updated" lets slide. A worker reads its block literally, so a rule
across files says so ("each of the three handlers"), and a decision is
literal: a file, a symbol, a value, what breaks if it is wrong. Every
requirement points at a task, and a name two tasks share is spelled
identically in both.
