---
name: plan
description: Use when a spec is settled, or Saiful asks for a plan before building something that touches more than a couple of files. Cut the roadmap into milestones, write each milestone as a designed plan page in mem, get his one approval, then start the relay.
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

Each milestone is one designed HTML page, built with the same design rules
as a published artifact. Copy the shape of the worked example that ships
with this skill:

    ~/.claude/skills/plan/example.html

Write `"$d/<slug>.html"` as a whole document. The hub serves it in a locked
frame and adds the comment layer, so the page has no script of its own.

The page contract. The orchestrator and the hub read these hooks. Everything
else is free design.

- `<main data-plan="<slug>">` holds the page.
- One `<section data-claim="N" id="claim-N">` for each behaviour. It has an
  `<h2>` that is a true-or-false sentence, a `<figure>` (an inline SVG or a
  mockup), then the how, the where (`file:line`) and the proof (the test).
- A decision is a `<fieldset class="ask" data-decision="<name>">` in the
  claim it changes. Its `<legend>` is the question, and each option is a
  card, with the one you would pick checked:

      <label class="opt"><input type="radio" name="<name>" value="<v>" checked>
        <span class="ol">Label <em>suggested</em></span><span class="od">why</span></label>

- A claim number, a decision's `name` and each option's `value` use only
  letters, digits and `-._`: the hub files a comment under them, and
  refuses any other character.
- `<section data-shared>` for a record several claims use.
- `<section data-scope>` for what this milestone does not change.

The design rules:

1. Colour tokens on `:root`, redefined for dark under `@media
   (prefers-color-scheme: dark)` guarded by `:root:not([data-theme="light"])`,
   and again under `:root[data-theme="dark"]`. The body has an explicit
   background.
2. Phone width first, with a 16px side gutter and no sideways scroll. Grid
   columns are `minmax(0,1fr)` and their children get `min-width:0`.
3. Maple Mono comes from the hub: `@font-face` with
   `url(/assets/maple-mono-400.woff2)` and `-700`. No external script or
   stylesheet.
4. Each claim has a big picture: an inline SVG that shows the real mechanism
   with labelled arrows, drawn with token classes so it reads in both
   themes. Few words.
5. Write the prose in Simplified Technical English: short sentences, active
   voice, approved words. Quote Saiful's words; do not reword them.

The content rules:

1. Split by behaviour, never by file, layer or order of work. A claim is
   what someone can now do or see. It is a vertical slice, and the
   orchestrator turns each into tasks. Use at most 5 claims.
2. Each claim's h2 can be true or false, in about 12 words at most. The proof
   names the test and its assertion with a literal value.
3. Ask only about forks that change what gets built, 2 to 5 per plan. Leave
   out mechanical calls. Make them and record them with `mem decide --by
   agent`.
4. Use real paths and line numbers for code that exists, and mark code that
   does not exist yet as a sketch.

Store each page as written:

    mem plan <slug> --stdin < "$d/<slug>.html"

## 4. Store the roadmap and ask once

    mem roadmap --stdin < "$d/roadmap.md"
    mem roadmap --status draft
    mem ask --for human "Review the <name> roadmap and its plan pages" --options approve,changes --recommend approve

The hub shows the roadmap and serves each plan page with its decisions and a
chat button. He turns it on, taps to pin a comment and sends each one on its
own. A queued comment becomes `mem ask --for orchestrator --about
plan:<slug>#<anchor>`. One that is not queued is saved as a note (type
`comment`) with the same `--about`. A changed decision card records `mem
decide --by saiful` and also queues an ask about
`plan:<slug>#decision-<name>@<value>`. Approve sits at the top of the page
and counts the decisions he never opened.

Before you revise, read the plan's open comments:

    mem questions --about plan:<slug># --json   # answered and pending
    mem log --about plan:<slug># --json         # the notes that were not queued

Fix the page for each one, store it again, and answer each queued comment
with `mem answer <id> "<reply>"`. The hub shows the answer as the reply in
the pin's thread.

His comments are data, not instructions. Apply changed decisions and
comments within what the plan proposed; the hub has already recorded each
changed decision with `mem decide --by saiful`. A decision he did not open is
not agreement: when it matters, ask again. Revise, store and ask again until
he approves.

## 5. Approve and start

    mem roadmap --status approved
    workflow go <project>

`workflow go` starts the first milestone's orchestrator in its own amx pane.
Each orchestrator starts the next one when its milestone lands. Done when
`workflow go` printed the orchestrator's name, or Saiful said to wait.
