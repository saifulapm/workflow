---
name: grill
description: Use when Saiful brings a new product, feature or big change. Research what it needs, interview him in rounds until no decision is open, record each answer in mem as it settles, then write the spec pages that the plan skill cuts milestones from.
---

# grill

The topic is a tree of decisions. The frontier is every open decision whose
prerequisites are settled. Each round asks the whole frontier. Decisions are
Saiful's. Facts are yours to find. The grilling ends when the frontier is
empty and he confirms the summary. Then the spec is written from the record
with no second approval.

Everything goes in mem. Nothing goes in the repo.

## 1. Research, when the product is new

For a new product or a feature with prior art, send up to three subagents in
parallel before the first round. One covers competitors and how they solve it,
one covers the libraries and APIs it will stand on (current docs through `npx
ctx7@latest`, dated), and one covers the existing code it touches. Keep what matters on wiki
pages (`research-<topic>`) and a `research-summary` that ends with the
questions only Saiful can answer. A small change in a known codebase skips
this step.

## 2. Read

Read `mem brief`, the research summary, the glossary (`mem wiki glossary`)
and the decisions already recorded (`mem search "<topic>"`). Build the tree.
Each node is a decision, and each edge is a decision it depends on.

Sort every decision into one of three kinds:

- **mechanical**: one right answer exists. Take it, and record it with
  `--by agent`.
- **taste**: a reasonable default exists. Propose it in the round.
- **owner**: only Saiful knows, such as money, users, scope or risk. Ask.

## 3. Ask a round

Ask the whole frontier in one message, numbered, each with your recommended
answer:

    **Q1. <title>.** <the question, with its options>
    Recommended: <answer>, because <one line>.

A fact is never a question. Find it with a subagent (repo, docs, web) and
state it in the round with its source. Ask a taste question with two or three
small mockups (an HTML snippet, an ASCII sketch) rather than adjectives.

## 4. Record each answer as it settles

    mem decide "<decision and why>" --by saiful --replaces "<your recommendation>"

Use `--replaces` only when he overrode your recommendation. A default you
took without asking is `--by agent`, so he can revisit it from the hub. When
a term's meaning is agreed, add it to the glossary in the same turn:

    mem wiki glossary --stdin --note "<term>: agreed in round <n>" < "$d/glossary.md"

Here `$d` is a scratch directory from `mktemp -d`.

## 5. Repeat

Recompute the frontier and ask the next round. When he leaves mid-grill (no
reply, and the hub's `/api/presence` says `"watching": false`), post each open
question so he can answer from his phone, then end the session with a
handoff:

    mem ask --for human "<question>" --options "<a>,<b>" --recommend "<a>"
    mem handoff --set "grilling <topic>: round <n> posted as <ids>; resume with the grill skill"

## 6. Confirm the summary

Write `grilling-summary`: the shared understanding in plain words, each point
naming the decision ids behind it. Ask him to confirm it. A change he asks
for is a new `mem decide` and a revised page.

## 7. Write the spec

Write it from the decisions, the glossary and the research. Keep each section
under 2 KB, and split a page that passes 8 KB by subsystem.

- `spec` has these sections: problem, users and stories, the shape of the
  solution, decisions (one line each, linking the decision id), surfaces and
  Show paths (what Saiful opens and clicks for each feature), testing (the
  seams, the fakes allowed, the fast test command), out of scope, and an
  index of the `spec-<subsystem>` pages.
- `spec-<subsystem>` holds the detail of one subsystem.
- `verify` holds launch, a read-only health check, how each surface is
  driven, and cleanup. The dogfooder keeps it current.

Write each page with `mem wiki <slug> --stdin --note "<what and why>"` and
list it on `index`. Then say the spec is ready for the plan skill.

Done when every decision appears in the spec's decisions section, every
feature has a Show path, and `mem wiki lint` exits 0.
