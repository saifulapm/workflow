---
name: plan
description: Use when a spec is settled, or Saiful asks for a plan before building something that touches more than a couple of files: cut milestones, write their plans, get one approval.
---

# plan

One sitting writes the roadmap and every milestone's plan, and asks Saiful
once. Work in a scratch directory, never the checkout: `d=$(mktemp -d)`.

A plan is markdown for the agents that build the milestone. The
orchestrator cuts its checklist from it, and the reviewer reads the landed
work against it. Saiful reviews the roadmap, not the plans. When he wants
to see something, he makes a claude.ai artifact, and the plan links it.

## 1. Read

Read `spec` one section at a time (`mem wiki spec --sections`, then `mem wiki
spec#<section>`), `verify`, the decisions (`mem search decision`), the
artifacts Saiful made for this work (`mem search artifact`, and the links in
the spec), the lessons past runs left (`mem wiki lessons` and `mem --project
workflow wiki lessons`), which the acceptance lines and the review focus
answer, and the repo's entry points. Any fact the plan relies on, such as a
crate version, an API parameter or a limit, gets checked this session (`npx
ctx7@latest`, or measure it) and dated. Find the fast test command and note
how long the suite takes.

## 2. Settle the forks

A fork is an open choice that changes what gets built. A plan holds none.
Ask Saiful each fork the spec leaves open before you write a plan: in the
chat, as one numbered round with your recommended answer for each (the
grill skill's format), or, when he is not in the chat, with `mem ask --for
human "<question>" --options "<a>,<b>" --recommend "<a>"`, which the hub
shows with a button per option. Record each answer with `mem decide
"<decision and why>" --by saiful`.

A mechanical call, one with a right answer, is yours. Make it and record it
with `mem decide --by agent`.

Done when every fork has a recorded answer.

## 3. The roadmap

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

## 4. A plan per milestone

Write `"$d/<slug>.md"` in this shape:

    # plan: m2-orders

    Goal: a shop owner creates an order from the desk and finds it by its number.
    Show: open Orders, create one for a customer, see it in the list with its number
    Spec: spec#orders, spec-orders#numbering
    Artifacts: [order desk mockup](https://claude.ai/artifact/<id>)

    ## Decisions
    - Order numbers count per shop from 1001. (saiful, #4KQ2M9TX)

    ## Behaviours

    ### 1. A new order shows first in the list, with its number
    The owner picks a customer and products, then saves. The list shows the
    order with its number, customer and total.
    Acceptance:
    - [ ] the first order of a new shop is number 1001
    - [ ] an order saved after another one is listed above it
    After: none

    ## Shared
    - Order: number, customer, lines, total, created.

    ## Out of scope
    - Refunds, which m4-refunds takes.

    ## Review focus
    - Saving twice while the first save is in flight makes one order, not two.

The rules:

1. Split by behaviour, never by file, layer or order of work. A behaviour is
   what someone can now do or see, a vertical slice the orchestrator turns
   into tasks. Use at most 5.
2. Each behaviour's heading is a sentence that is true or false once the
   milestone lands, in about 12 words.
3. Describe behaviour, not code. Name modules, records and interfaces by
   their domain names. File paths, line numbers and code go stale between
   the plan and the build, and the orchestrator reads the code itself.
4. Each acceptance line is an outcome someone can observe, with a literal
   value, so it becomes a failing test as written. A line with no value
   ("errors are handled", "works on mobile") gets one or goes.
5. `Decisions` lists the settled answers this milestone builds on, each with
   who took it and its id. `After` names the behaviours that land first. A
   fact you checked keeps its date.
6. `Artifacts` links each claude.ai artifact this milestone builds on, as
   `[what it shows](<url>)`. The hub shows the plan with that link, which is
   where Saiful opens it again. Write what the artifact settled into the
   behaviours in words, since the agents build from the plan. Leave the line
   out when there is none.
7. `Review focus` lists up to five inputs or states the spec implies and a
   person will meet, most likely first, each with what they would expect.
   Pin each with an acceptance line on the behaviour it belongs to. The
   orchestrator hands this list to the reviewer.
8. Write short sentences in the active voice. Quote Saiful's words; do not
   reword them.

Store each plan as written. mem refuses one whose first line is not
`# plan: <slug>`.

    mem plan <slug> --stdin < "$d/<slug>.md"

Done when every milestone on the roadmap has a stored plan with a review
focus, and every behaviour has an acceptance line with a literal value.

## 5. Store the roadmap and ask once

    mem roadmap --stdin < "$d/roadmap.md"
    mem roadmap --status draft

Saiful reviews the roadmap once. In the chat, show it as a numbered list of
each milestone's slug, title, Show path and what it waits on, and ask him to
approve it or say what to change. When he is not in the chat, ask through
the hub:

    mem ask --for human "Review the <name> roadmap" --options approve,changes --recommend approve

The hub's roadmap page answers that question with Approve or Request
changes, and opens each milestone's plan as text.

Apply each change he asks for, store the roadmap and the plans it touches
again, and ask again. Done when he approved.

## 6. Approve and start

    mem roadmap --status approved
    workflow go <project>

The hub's Approve sets the status itself. `workflow go` starts the first
milestone's orchestrator in its own amx pane. Each orchestrator starts the
next one when its milestone lands. Done when `workflow go` printed the
orchestrator's name, or Saiful said to wait.
