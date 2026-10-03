---
name: grill
description: Use after research to interview the owner in rounds until no decision is open, recording each answer as it settles, then write the spec and verify pages.
---

# grill

The topic is a tree of decisions. The frontier is every open decision whose
prerequisites are settled. A round asks the whole frontier. Decisions are the
owner's; facts are yours to find. The grilling ends when the frontier is
empty and the owner confirms `grilling-summary`; the spec is then written
from the record with no second approval.

## 1. Read

Read `mem wiki research-summary`, `mem brief`, `mem wiki glossary` and the
decisions already recorded (`mem context --full`, `mem search "<topic>"`).
Open a research section only when a question needs it:
`mem wiki <slug>#<section>`. Build the decision tree: each node a decision,
each edge a decision it depends on.

Done when every owner question in `research-summary` is a node and you can
name the frontier.

## 2. Ask the round

Ask the whole frontier in one message, numbered. Each question carries its
options, your recommendation and the reason in one line. A fact is never a
question: send a subagent to find it in the repo, the docs or the web, and
state it in the round as found, with its source.

Done when every frontier decision is in the round with options and a
recommendation, and no question asks the owner a fact.

## 3. Record each answer

The moment an answer settles, record it:

    mem decide "<decision and why>" --by saiful --replaces "<your recommendation>"

`--replaces` names the recommendation the owner overrode; leave it off when
the owner took it. A default you take without asking, because it sits below
the owner's level or the owner waved it through, is `--by agent`, so the
owner can revisit it from the hub.

When a term's meaning is agreed, write it into `glossary` in the same turn:

    mem wiki glossary --stdin --note "<term>: agreed in round <n>" <glossary.md

Done when every settled answer has a decision item and every agreed term is
in `glossary`.

## 4. Repeat

Recompute the frontier from the answers and ask the next round. A decision
you made without asking is a `--by agent` item, never only a sentence on a
page; that is what keeps nothing silently assumed.

When the owner leaves (no reply for ten minutes and hub `/api/presence`
says away), post the open round as one question per decision and wait:

    mem ask --for human "<question>" --options "<a>,<b>" --recommend "<a>"
    mem questions --wait <id> --timeout 60m

The hub shows each with its options and your recommendation, the owner
answers from the phone, and an answer is recorded as in step 3.

Done when the frontier is empty and every decision taken is a decision item.

## 5. Confirm the summary

Write `grilling-summary`: the shared understanding in plain words, each point
naming the decision ids behind it. Ask the owner to confirm it. A change the
owner asks for is a new `mem decide` and a revised page.

Done when the owner confirms the page as it stands.

## 6. Write the spec

Write from the decisions, the glossary and the research sections. Every
section is under 2 KB; a page that would pass 8 KB splits by subsystem.

- `spec`: Problem, Users and stories, Solution shape, Decisions (one line
  each, linking its decision id rather than restating it), Surfaces and Show
  paths (what the owner opens and clicks for each feature), Testing decisions
  (the seams, which fakes are allowed and where), Out of scope, Open
  questions, and an index section naming every `spec-<subsystem>` page.
- `spec-<subsystem>`: the detail of one subsystem.
- `verify`: the feature map skeleton. Launch (how to start the product),
  Doctor (one read-only check that the instance is worth driving), Drive
  (which tool drives which surface), Evidence (what counts as proof),
  Cleanup.

Write each page with `mem wiki <slug> --stdin --note "<what and why>"` and
list it in `index`.

Done when every decision item appears in the Decisions section, every
feature has a Show path, every section is under 2 KB, and `mem wiki lint`
exits 0.
