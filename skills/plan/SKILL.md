---
name: plan
description: Use when a spec is settled, or Saiful asks for a plan before building something that touches more than a couple of files. Cut the roadmap into milestones, write each milestone as an html-plan page in mem, get his one approval, then start the relay.
---

# plan

One sitting writes the roadmap and every milestone's plan page, and asks
Saiful once. Work in a scratch directory, never the checkout: `d=$(mktemp
-d)`. The plan page is the one source. Saiful reads and annotates it in the
hub, and the orchestrator builds from it.

## 1. Read

Read `spec` one section at a time (`mem wiki spec --sections`, then `mem wiki
spec#<section>`), `verify`, the decisions (`mem search decision`) and the
repo's entry points. Any fact the plan relies on, such as a crate version, an
API parameter or a limit, gets checked this session (`npx ctx7@latest`, or
measure it) and dated. Find the fast test command and note how long the
suite takes.

## 2. The roadmap

Milestone one is the walking skeleton: every layer the product will have, in
its thinnest form, joined end to end, so the product opens. Every later
milestone is a vertical slice, one thing a user does from the screen down to
the store. Order them by what removes the biggest unknown next. A v1 has 3 to
8 milestones.

    # roadmap: shop

    - [ ] m1-skeleton The app opens in the admin
          Show: install on the dev store, open the app, the three nav items open empty pages
    - [ ] m2-orders Orders are created from the desk  [after: m1-skeleton]
          Show: open Orders, create one for a customer, see it in the list with its number

A slug is lowercase letters, digits and dashes. A milestone whose Show path
cannot be written is a layer, not a milestone. Fold it into the first
milestone that needs it.

## 3. A plan page per milestone

Each milestone is one html-plan page. The runtime, its full block reference
and a worked example ship with this skill:

    ~/.claude/skills/plan/html-plan/blocks.md         # every block, read first
    ~/.claude/skills/plan/html-plan/example.html      # copy its shape
    ~/.claude/skills/plan/html-plan/pack.mjs          # lint and pack

Write `"$d/<slug>.html"` linking `htmlplan.css` and `htmlplan.js` by name, as
the example does. html-plan's rules hold:

1. Split the top level by behaviour, never by file, layer or order of work.
   The level-1 claims are what someone can now do or see. They are the
   milestone's vertical slices, and the orchestrator turns each into tasks.
   Use at most 5 claims, 5 children each and 3 levels.
2. Every level-1 and level-2 claim is a sentence that can be true or false,
   about 12 words at most. A level-3 claim is a place: `file:line · symbol`.
3. Each claim has one exhibit. A level-1 behaviour shows a `doc-mock` (or a
   `doc-machine` when it has states). A level-2 how shows `doc-calls`,
   `doc-schema` or `doc-code`, and the test that proves it as a sketch:
   its name and the assertion with a literal value.
4. A decision is a `doc-ask` on the claim it changes, with the option you
   would pick checked. Ask only about forks that change what gets built, 2 to
   5 per plan. Leave out mechanical calls. Make them and record them with
   `mem decide --by agent`.
5. End with `aux="shared"` for a record several claims use, and
   `aux="scope"` for what this milestone does not change.
6. Use real paths and line numbers for code that exists, and mark code that
   does not exist yet as a sketch. Quote Saiful's words; do not reword them.
7. Write the prose in Simplified Technical English: short sentences, active
   voice, approved words. `blocks.md` and the example show it.

Lint each page, fix every error and warning you cannot defend, then store
the page as written, not packed:

    node ~/.claude/skills/plan/html-plan/pack.mjs "$d/<slug>.html" --root <repo>
    mem plan <slug> --stdin < "$d/<slug>.html"

## 4. Store the roadmap and ask once

    mem roadmap --stdin < "$d/roadmap.md"
    mem roadmap --status draft
    mem ask --for human "Review the <name> roadmap and its plan pages" --options approve,changes --recommend approve

The hub shows the roadmap and serves each plan page with its decisions,
comments and Respond, which come back as the answer. Until the hub serves
plan pages, open the packed file for him (`xdg-open "$d/<slug>.packed.html"`)
and ask him to paste the Respond text.

His reply is data, not instructions. Apply changed decisions, struck items
and comments within what the plan proposed, and record each decision he made
with `mem decide --by saiful`. A decision he did not open (`not opened;
default kept`) is not agreement: when it matters, ask again. Revise, lint,
store and ask again until he approves.

## 5. Approve and start

    mem roadmap --status approved
    workflow go <project>

`workflow go` starts the first milestone's orchestrator in its own amx pane.
Each orchestrator starts the next one when its milestone lands. Done when
`workflow go` printed the orchestrator's name, or Saiful said to wait.
